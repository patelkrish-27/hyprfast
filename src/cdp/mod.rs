//! CDP: discovery + WebSocket JSON-RPC surface
//! v0.8: Chrome DevTools MCP (stdio) is the preferred browser backend;
//! this module retains its public helper API for compatibility, but every
//! call now routes through `devtools_mcp::proxy` when available, then
//! `browser_runtime::client` (daemon when alive, ephemeral fallback otherwise).
//! No per-call WebSocket is created here — the only WebSocket connect lives in
//! `browser_runtime/connection.rs` (invariant I2). DevTools MCP owns its own
//! browser connection over stdio.
//!
//! The old direct HTTP discovery (`GET /json` → `webSocketDebuggerUrl`) is
//! superseded by DevTools target caching (2s TTL in proxy) and by the
//! daemon's `Target.getTargets` over the persistent socket. HTTP remains only
//! as a degraded fallback when both proxy and daemon are unavailable.

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::time::Duration;

const DEFAULT_PORT: u16 = 9222;
const DEFAULT_HOST: &str = "127.0.0.1";

fn cdp_base_url() -> String {
    let host = std::env::var("HYPRFAST_CDP_HOST").unwrap_or_else(|_| DEFAULT_HOST.to_string());
    let port = std::env::var("HYPRFAST_CDP_PORT").unwrap_or_else(|_| DEFAULT_PORT.to_string());
    format!("http://{}:{}", host, port)
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct Target {
    pub id: String,
    pub title: String,
    pub url: String,
    #[serde(rename="webSocketDebuggerUrl")]
    pub web_socket_debugger_url: String,
    #[serde(rename="type")]
    pub typ: String,
}

pub async fn list_targets_async() -> Result<Vec<Target>> {
    let base = cdp_base_url();
    let url = format!("{}/json", base);
    let client = reqwest::Client::builder().timeout(Duration::from_secs(2)).build()?;
    let resp = client.get(&url).send().await.map_err(|e| {
        anyhow::anyhow!("CDP unreachable at {}/json ({}). Launch browser with --remote-debugging-port={} e.g. `hyprfast browser open https://example.com`", base, e, DEFAULT_PORT)
    })?;
    if !resp.status().is_success() {
        bail!("CDP GET /json failed: {}", resp.status());
    }
    let targets: Vec<Target> = resp.json().await.context("parse targets")?;
    Ok(targets)
}

fn tokio_block_on<F, T>(f: F) -> T
where
    F: std::future::Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    // Use a fresh OS thread + current_thread runtime so we never panic with
    // "there is no reactor running" (futures::executor) or "cannot start a runtime from within a runtime".
    std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime for blocking cdp")
            .block_on(f)
    })
    .join()
    .expect("tokio_block_on thread join")
}

pub fn list_targets() -> Result<Vec<Target>> {
    // v0.8: prefer DevTools MCP proxy (2s target cache) when available.
    if crate::devtools_mcp::process::devtools_proxy_enabled() {
        let proxy_res = crate::devtools_mcp::proxy::proxy_block_on(async {
            crate::devtools_mcp::proxy::global_proxy().tabs().await
        });
        if let Ok(v) = proxy_res {
            if let Some(arr) = v.get("targets").and_then(|x| x.as_array()) {
                let out: Vec<Target> = arr.iter().filter_map(|o| {
                    Some(Target {
                        id: o.get("id").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                        title: o.get("title").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                        url: o.get("url").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                        web_socket_debugger_url: o.get("ws").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                        typ: o.get("type").and_then(|x| x.as_str()).unwrap_or("page").to_string(),
                    })
                }).collect();
                if !out.is_empty() { return Ok(out); }
            }
        }
    }
    let v = crate::browser_runtime::client::cdp_call_sync(
        "Target.getTargets",
        json!({}),
        None,
        None,
        crate::browser_runtime::server::CapabilityClass::None,
    );
    if let Ok(val) = v {
        if let Some(infos) = val.get("targetInfos").and_then(|x| x.as_array()) {
            let mut out = Vec::new();
            for info in infos {
                let id = info.get("targetId").and_then(|x| x.as_str()).unwrap_or("").to_string();
                let title = info.get("title").and_then(|x| x.as_str()).unwrap_or("").to_string();
                let url = info.get("url").and_then(|x| x.as_str()).unwrap_or("").to_string();
                let typ = info.get("type").and_then(|x| x.as_str()).unwrap_or("page").to_string();
                out.push(Target { id, title, url, web_socket_debugger_url: String::new(), typ });
            }
            if !out.is_empty() { return Ok(out); }
        }
    }
    tokio_block_on(list_targets_async()).or_else(|_| {
        Err(anyhow::anyhow!("list_targets: proxy, daemon and HTTP discovery all failed"))
    })
}

pub async fn list_targets_filtered_async(typ: Option<&str>) -> Result<Vec<Target>> {
    let all = list_targets_async().await?;
    if let Some(t) = typ {
        Ok(all.into_iter().filter(|x| x.typ==t).collect())
    } else { Ok(all) }
}

pub fn version() -> Result<Value> {
    // Prefer DevTools MCP path via proxy's evaluate? But Browser.getVersion is not a proxy tool.
    // Try daemon's Browser.getVersion first.
    let v = crate::browser_runtime::client::cdp_call_sync(
        "Browser.getVersion",
        json!({}),
        None,
        None,
        crate::browser_runtime::server::CapabilityClass::None,
    );
    if let Ok(val) = v { return Ok(val); }
    tokio_block_on(async {
        let base = cdp_base_url();
        let url = format!("{}/json/version", base);
        let client = reqwest::Client::builder().timeout(Duration::from_secs(2)).build()?;
        let v: Value = client.get(&url).send().await?.json().await?;
        Ok::<Value, anyhow::Error>(v)
    })
}

// WS discovery is superseded by DevTools proxy target caching + daemon's flat session.
// These helpers remain for fallback callers but now prefer the managed paths.
pub async fn get_ws_url_async(target_url_match: Option<&str>) -> Result<String> {
    // Deprecated: WS URLs are owned by the daemon/proxy. Fall back to HTTP list only when needed.
    let targets = list_targets_async().await?;
    if targets.is_empty() {
        bail!("no debuggable targets at {} — launch browser with --remote-debugging-port={} (e.g. brave --remote-debugging-port=9222 --force-renderer-accessibility)", cdp_base_url(), DEFAULT_PORT);
    }
    let mut pages: Vec<&Target> = targets.iter().filter(|t| t.typ=="page").collect();
    if pages.is_empty() { pages = targets.iter().collect(); }
    if let Some(needle) = target_url_match {
        if !needle.is_empty() {
            let lower = needle.to_lowercase();
            if let Some(m) = pages.iter().find(|t| t.url.to_lowercase().contains(&lower) || t.title.to_lowercase().contains(&lower)) {
                if !m.web_socket_debugger_url.is_empty() { return Ok(m.web_socket_debugger_url.clone()); }
            }
        }
    }
    for t in pages.iter().rev() {
        if !t.web_socket_debugger_url.is_empty() { return Ok(t.web_socket_debugger_url.clone()); }
    }
    bail!("no page with webSocketDebuggerUrl (WS discovery superseded by devtools-mcp/daemon)")
}

pub fn get_ws_url(target_match: Option<&str>) -> Result<String> {
    let m = target_match.map(|s| s.to_owned());
    std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime")
            .block_on(get_ws_url_async(m.as_deref()))
    })
    .join()
    .expect("thread join")
}

pub async fn new_page_async(url: &str) -> Result<Target> {
    // Prefer proxy new_page when available (no HTTP /json/new needed).
    if crate::devtools_mcp::process::devtools_proxy_enabled() {
        let proxy_res = crate::devtools_mcp::proxy::proxy_block_on(async {
            crate::devtools_mcp::proxy::global_proxy().open(url).await
        });
        // If proxy opened, synthesize Target from its result and return.
        if proxy_res.is_ok() {
            // Fall back to list to get canonical Target shape
            if let Ok(targets) = list_targets_async().await {
                if let Some(t) = targets.into_iter().find(|t| t.url.contains(url)) { return Ok(t); }
            }
        }
    }
    let base = cdp_base_url();
    let client = reqwest::Client::builder().timeout(Duration::from_secs(3)).build()?;
    let v: Value = client.put(format!("{}/json/new", base)).query(&[("url", url)]).send().await?.json().await?;
    let t: Target = serde_json::from_value(v)?;
    Ok(t)
}

// ---- WebSocket JSON-RPC via BrowserRuntime / DevTools proxy (no direct WS connect here) ----

pub async fn cdp_call_async(_ws_url: &str, method: &str, params: Value) -> Result<Value> {
    crate::browser_runtime::client::try_cdp_call_via_daemon(
        method,
        params,
        None,
        None,
        capability_for_method(method),
    )
    .await
    .map_err(|e| anyhow::anyhow!(e.to_string()))
}

pub fn cdp_call(ws_url: &str, method: &str, params: Value) -> Result<Value> {
    let ws = ws_url.to_owned();
    let m = method.to_owned();
    std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime")
            .block_on(cdp_call_async(&ws, &m, params))
    })
    .join()
    .expect("thread join")
}

pub async fn cdp_batch_async(ws_url: &str, calls: Vec<(&str, Value)>) -> Result<Vec<Value>> {
    let mut out = Vec::new();
    for (method, params) in calls {
        out.push(cdp_call_async(ws_url, method, params).await?);
    }
    Ok(out)
}

pub fn cdp_batch(ws_url: &str, calls: Vec<(&str, Value)>) -> Result<Vec<Value>> {
    let ws = ws_url.to_owned();
    let owned_calls: Vec<(String, Value)> = calls.into_iter().map(|(m, p)| (m.to_owned(), p)).collect();
    std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime")
            .block_on(async {
                let mut out = Vec::new();
                for (method, params) in owned_calls {
                    out.push(cdp_call_async(&ws, &method, params).await?);
                }
                Ok::<Vec<Value>, anyhow::Error>(out)
            })
    })
    .join()
    .expect("thread join")
}

// ---- Helpers that combine discovery + ws ----

pub fn call(method: &str, params: Value) -> Result<Value> {
    cdp_call("", method, params)
}
pub fn call_on(_match_str: Option<&str>, method: &str, params: Value) -> Result<Value> {
    cdp_call("", method, params)
}

pub fn evaluate(expression: &str, await_promise: bool) -> Result<Value> {
    crate::browser_runtime::client::evaluate_sync(expression, await_promise)
}

pub fn ensure_enabled() -> Result<()> {
    let _ = crate::browser_runtime::client::cdp_call_sync(
        "Page.enable",
        json!({}),
        None,
        None,
        crate::browser_runtime::server::CapabilityClass::None,
    );
    let _ = crate::browser_runtime::client::cdp_call_sync(
        "DOM.enable",
        json!({}),
        None,
        None,
        crate::browser_runtime::server::CapabilityClass::None,
    );
    let _ = crate::browser_runtime::client::cdp_call_sync(
        "Runtime.enable",
        json!({}),
        None,
        None,
        crate::browser_runtime::server::CapabilityClass::None,
    );
    Ok(())
}

fn capability_for_method(method: &str) -> crate::browser_runtime::server::CapabilityClass {
    use crate::browser_runtime::server::CapabilityClass as C;
    match method {
        "Runtime.evaluate" | "Runtime.callFunctionOn" | "Runtime.releaseObject" => C::RuntimeEvaluate,
        "Storage.getCookies" | "Storage.setCookies" | "Storage.clearCookies" => C::Cookies,
        "Page.captureScreenshot" => C::None,
        "Page.navigate" | "Page.reload" => C::Navigation,
        "Input.dispatchKeyEvent" | "Input.insertText" => C::None,
        "DOM.resolveNode" | "DOM.getDocument" | "DOM.querySelector" | "DOM.describeNode" | "Accessibility.getFullAXTree" => C::None,
        "Target.createTarget" | "Target.attachToTarget" | "Target.getTargets" | "Target.setAutoAttach" => C::None,
        _ if method.starts_with("Storage.") => C::Cookies,
        _ => C::None,
    }
}
