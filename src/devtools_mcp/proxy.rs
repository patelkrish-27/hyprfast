//! Proxy: maps hyprfast `browser_*` tools to Chrome DevTools MCP tools.
//!
//! The proxy talks stdio to `chrome-devtools-mcp` (which owns the browser
//! WebSocket). It never calls `connect_async` itself (I2). When the MCP
//! child is unavailable, every method returns `Err` so callers fall back to
//! the persistent `BrowserRuntime` daemon path.

use anyhow::{Result, bail};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

use super::process::global;

const PAGE_CACHE_TTL: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Default)]
struct PageCache {
    pages: Vec<Value>,
    at: Option<Instant>,
    selected: Option<u64>,
}

impl PageCache {
    fn fresh(&self) -> bool { self.at.map(|t| t.elapsed() < PAGE_CACHE_TTL).unwrap_or(false) }
}

pub struct DevToolsMcpProxy {
    cache: Mutex<PageCache>,
    // DevTools session/target caching mirrors ws_url 2s cache semantics via
    // page list + selected page id caching.
    next_page_fallback: AtomicU64,
}

impl DevToolsMcpProxy {
    pub fn new() -> Self {
        Self { cache: Mutex::new(PageCache::default()), next_page_fallback: AtomicU64::new(1) }
    }

    pub async fn is_available(&self) -> bool {
        global().is_available().await
    }

    async fn cached_pages(&self, force: bool) -> Result<Vec<Value>> {
        {
            let c = self.cache.lock().await;
            if !force && c.fresh() { return Ok(c.pages.clone()); }
        }
        let v = global().call_tool("list_pages", json!({})).await?;
        let pages = if let Some(arr) = v.as_array() { arr.clone() } else if let Some(arr) = v.get("pages").and_then(|x| x.as_array()) { arr.clone() } else { vec![] };
        let mut c = self.cache.lock().await;
        c.pages = pages.clone();
        c.at = Some(Instant::now());
        if c.selected.is_none() {
            if let Some(first) = pages.first().and_then(|p| p.get("id").or_else(|| p.get("pageId"))).and_then(|v| v.as_u64()) {
                c.selected = Some(first);
            }
        }
        Ok(pages)
    }

    async fn page_id(&self) -> Result<u64> {
        let pages = self.cached_pages(false).await?;
        {
            let c = self.cache.lock().await;
            if let Some(id) = c.selected { return Ok(id); }
        }
        if let Some(p) = pages.first() {
            if let Some(id) = p.get("id").or_else(|| p.get("pageId")).and_then(|v| v.as_u64()) {
                self.cache.lock().await.selected = Some(id);
                return Ok(id);
            }
        }
        bail!("no pages available")
    }

    async fn ensure_page_for_url(&self, _url: &str) -> Result<u64> {
        Ok(self.page_id().await.unwrap_or_else(|_| self.next_page_fallback.fetch_add(1, Ordering::SeqCst)))
    }

    // ---- Navigation ----

    pub async fn navigate(&self, url: &str) -> Result<Value> {
        if url.is_empty() { bail!("url required"); }
        let pid = self.ensure_page_for_url(url).await.unwrap_or(1);
        // Try navigate_page url type, fallback to new_page
        let res = global().call_tool("navigate_page", json!({"pageId": pid, "url": url, "type":"url"})).await;
        match res {
            Ok(v) => Ok(json!({"result": v, "url": url, "via":"devtools_mcp", "pageId": pid})),
            Err(_) => {
                let v = global().call_tool("new_page", json!({"url": url})).await?;
                let nid = v.get("pageId").or_else(|| v.get("id")).and_then(|x| x.as_u64()).unwrap_or(pid);
                self.cache.lock().await.selected = Some(nid);
                self.cache.lock().await.at = None; // invalidate to refetch
                Ok(json!({"result": v, "url": url, "via":"devtools_mcp", "pageId": nid}))
            }
        }
    }

    pub async fn go_back(&self) -> Result<Value> {
        let pid = self.page_id().await?;
        let v = global().call_tool("navigate_page", json!({"pageId": pid, "type":"back"})).await?;
        Ok(json!({"result": v, "via":"devtools_mcp"}))
    }

    pub async fn go_forward(&self) -> Result<Value> {
        let pid = self.page_id().await?;
        let v = global().call_tool("navigate_page", json!({"pageId": pid, "type":"forward"})).await?;
        Ok(json!({"result": v, "via":"devtools_mcp"}))
    }

    // ---- Snapshot ----

    pub async fn snapshot(&self, max_nodes: usize) -> Result<Value> {
        let pid = self.page_id().await?;
        let v = global().call_tool("take_snapshot", json!({"pageId": pid})).await?;
        // Normalize to hyprfast snapshot shape
        let nodes = extract_snapshot_nodes(&v);
        let limited: Vec<Value> = nodes.into_iter().take(max_nodes).collect();
        let refs = build_refs_from_snapshot(&limited)?;
        Ok(json!({"snapshot": refs, "raw_nodes": limited.len(), "via":"devtools_mcp"}))
    }

    // ---- Click / Hover ----

    pub async fn click(&self, r#ref: &str, _element: &str) -> Result<Value> {
        let pid = self.page_id().await?;
        // Numeric uid path
        if !r#ref.is_empty() {
            let by_uid = global().call_tool("click", json!({"pageId": pid, "uid": r#ref})).await;
            if by_uid.is_ok() { return Ok(json!({"clicked": true, "ref": r#ref, "via":"devtools_mcp"})); }
        }
        // Fallback selector via evaluate_script
        let selector = if r#ref.contains(':') || r#ref.contains('[') || r#ref.starts_with('.') || r#ref.starts_with('#') { r#ref } else { _element };
        if selector.is_empty() { bail!("click needs ref or element"); }
        self.eval_via_script(&format!("document.querySelector({:?})?.click(); 'ok'", selector)).await
    }

    pub async fn hover(&self, r#ref: &str, element: &str) -> Result<Value> {
        let pid = self.page_id().await?;
        if !r#ref.is_empty() {
            let by_uid = global().call_tool("hover", json!({"pageId": pid, "uid": r#ref})).await;
            if by_uid.is_ok() { return Ok(json!({"hovered": true, "ref": r#ref, "via":"devtools_mcp"})); }
        }
        let selector = if !element.is_empty() { element } else { r#ref };
        self.eval_via_script(&format!("document.querySelector({:?})?.dispatchEvent(new MouseEvent('mouseover',{{bubbles:true}})); 'ok'", selector)).await
    }

    // ---- Type / Fill ----

    pub async fn type_text(&self, r#ref: &str, text: &str, submit: bool, selector: Option<&str>) -> Result<Value> {
        let pid = self.page_id().await?;
        if !r#ref.is_empty() {
            let fv = global().call_tool("fill", json!({"pageId": pid, "uid": r#ref, "value": text})).await;
            if fv.is_ok() {
                if submit { let _ = global().call_tool("press_key", json!({"pageId": pid, "key":"Enter"})).await; }
                return Ok(json!({"typed": text.len(), "via":"devtools_mcp"}));
            }
            // Try type_text (previously focused)
            let tv = global().call_tool("type_text", json!({"pageId": pid, "text": text})).await;
            if tv.is_ok() {
                if submit { let _ = global().call_tool("press_key", json!({"pageId": pid, "key":"Enter"})).await; }
                return Ok(json!({"typed": text.len(), "via":"devtools_mcp"}));
            }
        }
        let sel = selector.unwrap_or(if r#ref.is_empty() { "input, textarea, [contenteditable]" } else { r#ref });
        self.eval_via_script(&format!(
            "(() => {{ const el=document.querySelector({:?})||document.activeElement; if(!el) return 'no el'; el.focus(); if(el.isContentEditable) document.execCommand('insertText', false, {:?}); else {{ el.value={:?}; el.dispatchEvent(new Event('input',{{bubbles:true}})); el.dispatchEvent(new Event('change',{{bubbles:true}})); }} if({}) el.dispatchEvent(new KeyboardEvent('keydown',{{key:'Enter', keyCode:13, bubbles:true}})); return 'ok'; }})()",
            sel, text, text, submit
        )).await
    }

    pub async fn fill(&self, selector: &str, text: &str) -> Result<Value> {
        self.type_text(selector, text, false, Some(selector)).await
    }

    pub async fn select_option(&self, r#ref: &str, values: &[String]) -> Result<Value> {
        let pid = self.page_id().await?;
        if !r#ref.is_empty() {
            for v in values {
                let _ = global().call_tool("fill", json!({"pageId": pid, "uid": r#ref, "value": v})).await;
            }
            return Ok(json!({"selected": values, "via":"devtools_mcp"}));
        }
        self.eval_via_script(&format!(
            "(() => {{ const el=document.querySelector({:?})||document.querySelector('select'); if(!el) return 'no select'; const vals={}; for(const o of el.options) if(vals.includes(o.value)||vals.includes(o.text)) o.selected=true; el.dispatchEvent(new Event('change',{{bubbles:true}})); return 'ok'; }})()",
            r#ref, serde_json::to_string(values).unwrap()
        )).await
    }

    // ---- Keys ----

    pub async fn press_key(&self, key: &str) -> Result<Value> {
        let pid = self.page_id().await?;
        let v = global().call_tool("press_key", json!({"pageId": pid, "key": key})).await?;
        Ok(json!({"pressed": key, "via":"devtools_mcp", "result": v}))
    }

    // ---- Evaluate ----

    pub async fn evaluate(&self, expression: &str) -> Result<Value> {
        self.eval_via_script(expression).await
    }

    async fn eval_via_script(&self, expression: &str) -> Result<Value> {
        let pid = self.page_id().await?;
        // chrome-devtools-mcp evaluate_script expects a JS function string.
        let func = if expression.trim_start().starts_with("()") || expression.trim_start().starts_with("async") {
            expression.to_string()
        } else {
            format!("() => {{ return ({}); }}", expression)
        };
        let v = global().call_tool("evaluate_script", json!({"pageId": pid, "function": func})).await?;
        Ok(v)
    }

    // ---- Screenshot ----

    pub async fn screenshot(&self) -> Result<(Vec<u8>, Value)> {
        let pid = self.page_id().await?;
        let v = global().call_tool("take_screenshot", json!({"pageId": pid, "format":"png"})).await?;
        // Response may contain base64 data or file path; normalize to bytes.
        if let Some(data) = v.get("data").and_then(|x| x.as_str()) {
            if let Ok(bytes) = base64_decode(data) {
                return Ok((bytes, json!({"format":"png","via":"devtools_mcp"})));
            }
        }
        if let Some(content) = v.as_array() {
            if let Some(txt) = content.first().and_then(|o| o.get("text")).and_then(|x| x.as_str()) {
                if let Ok(bytes) = base64_decode(txt) { return Ok((bytes, json!({"format":"png","via":"devtools_mcp"}))); }
            }
        }
        bail!("no screenshot data from devtools-mcp")
    }

    // ---- Tabs ----

    pub async fn tabs(&self) -> Result<Value> {
        let pages = self.cached_pages(true).await?;
        let mapped: Vec<Value> = pages.iter().map(|p| {
            let id = p.get("id").or_else(|| p.get("pageId")).cloned().unwrap_or(Value::Null);
            let title = p.get("title").or_else(|| p.get("name")).cloned().unwrap_or(Value::String(String::new()));
            let url = p.get("url").cloned().unwrap_or(Value::String(String::new()));
            json!({"id": id, "title": title, "url": url, "type":"page", "ws": ""})
        }).collect();
        Ok(json!({"targets": mapped, "via":"devtools_mcp"}))
    }

    // ---- Console ----

    pub async fn console(&self) -> Result<Value> {
        let pid = self.page_id().await?;
        let v = global().call_tool("list_console_messages", json!({"pageId": pid})).await?;
        Ok(json!({"logs": v, "via":"devtools_mcp"}))
    }

    // ---- Wait ----

    pub async fn wait(&self, secs: f64) -> Result<Value> {
        // Prefer devtools wait_for if we have text wait semantics; for time wait, use thread sleep
        // but keep verify it's not blind sleep for navigation (navigation uses wait_for).
        // For generic browser_wait, devtools has no timed wait, so sleep via tokio.
        tokio::time::sleep(Duration::from_secs_f64(secs)).await;
        Ok(json!({"waited": secs, "via":"devtools_mcp"}))
    }

    pub async fn wait_for_text(&self, texts: &[String], timeout_ms: u64) -> Result<Value> {
        let pid = self.page_id().await?;
        global().call_tool("wait_for", json!({"pageId": pid, "text": texts, "timeout": timeout_ms})).await
    }

    // ---- Open (Hyprland + CDP) ----
    // Hyprland workspace handling stays in src/main.rs ensure_browser_args.
    // Proxy's open just does new_page after launch is handled by caller.

    pub async fn open(&self, url: &str) -> Result<Value> {
        let v = global().call_tool("new_page", json!({"url": url})).await?;
        self.cache.lock().await.at = None;
        Ok(json!({"launched": url, "via":"devtools_mcp", "result": v}))
    }

    // ---- Cache management ----

    pub async fn invalidate_cache(&self) {
        let mut c = self.cache.lock().await;
        c.at = None;
    }
}

fn extract_snapshot_nodes(v: &Value) -> Vec<Value> {
    // take_snapshot may return {snapshot: "..."} text or structured nodes
    if let Some(arr) = v.as_array() { return arr.clone(); }
    if let Some(s) = v.get("snapshot").and_then(|x| x.as_str()) {
        // Snapshot is text tree; synthesize single entry to preserve shape
        return vec![json!({"role":"document","name": s.chars().take(200).collect::<String>(), "ref":"0"})];
    }
    if let Some(nodes) = v.get("nodes").and_then(|x| x.as_array()) { return nodes.clone(); }
    if let Some(c) = v.get("content").and_then(|x| x.as_array()) {
        if let Some(t) = c.first().and_then(|o| o.get("text")).and_then(|x| x.as_str()) {
            if let Ok(val) = serde_json::from_str::<Value>(t) {
                if let Some(arr) = val.as_array() { return arr.clone(); }
                if let Some(n) = val.get("nodes").and_then(|x| x.as_array()) { return n.clone(); }
            }
        }
    }
    vec![]
}

fn build_refs_from_snapshot(nodes: &[Value]) -> Result<Value> {
    let mut out = Vec::new();
    for (i, n) in nodes.iter().enumerate() {
        let role = n.get("role").and_then(|r| r.get("value")).and_then(|v| v.as_str())
            .or_else(|| n.get("role").and_then(|v| v.as_str())).unwrap_or("");
        let name = n.get("name").and_then(|r| r.get("value")).and_then(|v| v.as_str())
            .or_else(|| n.get("name").and_then(|v| v.as_str())).unwrap_or("");
        let uid = n.get("uid").and_then(|v| v.as_str()).or_else(|| n.get("backendDOMNodeId").and_then(|v| v.as_str())).unwrap_or("");
        let r = if uid.is_empty() { i.to_string() } else { uid.to_string() };
        if role.is_empty() && name.is_empty() { continue; }
        out.push(json!({"role": role, "name": name, "ref": r, "nodeId": n.get("nodeId").cloned().unwrap_or(Value::Null)}));
        if out.len() >= 60 { break; }
    }
    if out.is_empty() && !nodes.is_empty() {
        for (i, n) in nodes.iter().take(60).enumerate() {
            out.push(json!({"role": "generic", "name": format!("{:?}", n).chars().take(80).collect::<String>(), "ref": i.to_string()}));
        }
    }
    Ok(Value::Array(out))
}

fn base64_decode(s: &str) -> Result<Vec<u8>> {
    use base64::Engine;
    let clean = s.trim().trim_start_matches("data:image/png;base64,");
    base64::engine::general_purpose::STANDARD.decode(clean).map_err(|e| anyhow::anyhow!("base64 decode: {}", e))
}

static GLOBAL_PROXY: once_cell::sync::Lazy<DevToolsMcpProxy> = once_cell::sync::Lazy::new(DevToolsMcpProxy::new);
pub fn global_proxy() -> &'static DevToolsMcpProxy { &*GLOBAL_PROXY }

/// Sync helper for browser sync wrappers
pub fn proxy_block_on<F: std::future::Future>(f: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread().enable_all().build().expect("proxy runtime").block_on(f)
}

/// Try proxy path, return Some(Ok) on success, Some(Err) on proxy error (fallback), None if unavailable
pub async fn try_proxy<F, T>(f: F) -> Option<Result<T>>
where
    F: FnOnce(&DevToolsMcpProxy) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<T>> + Send>> + Send,
    T: Send + 'static,
{
    if !global_proxy().is_available().await { return None; }
    let v = f(global_proxy()).await;
    Some(v)
}
