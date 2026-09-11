//! Browser automation via CDP / Chrome DevTools MCP — hyprfast 0.8
//! Mirrors browsermcp tools but pure Rust. v0.8 routes through the
//! Chrome DevTools MCP proxy (stdio) when available; falls back to
//! the persistent BrowserRuntime daemon (capability-tagged) otherwise.
//! No per-call WebSocket is created here — the only WebSocket connect
//! lives in `browser_runtime/connection.rs` (invariant I2). The proxy's
//! child owns its own browser connection over stdio.

use anyhow::{Result, bail, Context};
use serde_json::{Value, json};
use crate::browser_runtime::server::CapabilityClass;
use crate::browser_runtime::client as rt_client;

fn cdp_call(method: &str, params: Value, capability: CapabilityClass) -> Result<Value> {
    rt_client::cdp_call_sync(method, params, None, None, capability)
}

fn proxy_enabled() -> bool {
    crate::devtools_mcp::process::devtools_proxy_enabled()
}

// ---- Navigate ----
pub fn navigate(url: &str, _target: Option<&str>) -> Result<Value> {
    if url.is_empty() { bail!("url required"); }
    if proxy_enabled() {
        let res = crate::devtools_mcp::proxy::proxy_block_on(async {
            crate::devtools_mcp::proxy::global_proxy().navigate(url).await
        });
        if let Ok(v) = res {
            let _ = rt_client::wait_for_lifecycle_sync("load", std::time::Duration::from_secs(10));
            return Ok(v);
        }
    }
    let _ = cdp_call("Page.enable", json!({}), CapabilityClass::None);
    let res = cdp_call("Page.navigate", json!({"url": url}), CapabilityClass::Navigation)?;
    let _ = rt_client::wait_for_lifecycle_sync("load", std::time::Duration::from_secs(10));
    Ok(json!({"result": res, "url": url}))
}

pub fn go_back() -> Result<Value> {
    if proxy_enabled() {
        let res = crate::devtools_mcp::proxy::proxy_block_on(async {
            crate::devtools_mcp::proxy::global_proxy().go_back().await
        });
        if let Ok(v) = res { return Ok(v); }
    }
    let expr = "history.back(); location.href";
    let v = rt_client::evaluate_sync(expr, false)?;
    Ok(json!({"result": v}))
}
pub fn go_forward() -> Result<Value> {
    if proxy_enabled() {
        let res = crate::devtools_mcp::proxy::proxy_block_on(async {
            crate::devtools_mcp::proxy::global_proxy().go_forward().await
        });
        if let Ok(v) = res { return Ok(v); }
    }
    let expr = "history.forward(); location.href";
    let v = rt_client::evaluate_sync(expr, false)?;
    Ok(json!({"result": v}))
}

// ---- Snapshot (AX tree) ----
pub fn snapshot(max_nodes: usize) -> Result<Value> {
    if proxy_enabled() {
        let res = crate::devtools_mcp::proxy::proxy_block_on(async {
            crate::devtools_mcp::proxy::global_proxy().snapshot(max_nodes).await
        });
        if let Ok(v) = res { return Ok(v); }
    }
    let ax = cdp_call("Accessibility.getFullAXTree", json!({}), CapabilityClass::None);
    if let Ok(val) = ax {
        if let Some(nodes) = val.get("nodes").and_then(|v| v.as_array()) {
            if !nodes.is_empty() {
                let limited: Vec<&Value> = nodes.iter().take(max_nodes).collect();
                let refs = build_snapshot_from_ax(&limited)?;
                return Ok(json!({"snapshot": refs, "raw_nodes": limited.len(), "via": "Accessibility"}));
            }
        }
    }
    snapshot_via_js()
}

fn build_snapshot_from_ax(nodes: &[&Value]) -> Result<Value> {
    let mut out = Vec::new();
    for n in nodes.iter().take(80) {
        let role = n.get("role").and_then(|r| r.get("value")).and_then(|v| v.as_str()).unwrap_or("");
        let name = n.get("name").and_then(|r| r.get("value")).and_then(|v| v.as_str()).unwrap_or("");
        let node_id = n.get("nodeId").and_then(|v| v.as_str()).unwrap_or("");
        let backend = n.get("backendDOMNodeId").and_then(|v| v.as_i64()).unwrap_or(0);
        if role.is_empty() && name.is_empty() { continue; }
        let ignored = n.get("ignored").and_then(|v| v.as_bool()).unwrap_or(false);
        if ignored { continue; }
        let mut item = json!({
            "role": role,
            "name": name,
            "ref": backend.to_string(),
            "nodeId": node_id,
        });
        if let Some(v) = n.get("properties") { item["properties"] = v.clone(); }
        out.push(item);
        if out.len()>=60 { break; }
    }
    if out.is_empty() { bail!("empty AX tree"); }
    Ok(Value::Array(out))
}

fn snapshot_via_js() -> Result<Value> {
    let js = r#"
(() => {
  const MAX=80;
  const out=[];
  const walker=document.createTreeWalker(document.body, NodeFilter.SHOW_ELEMENT);
  let n=walker.currentNode;
  let count=0;
  while(n && count<MAX){
    const el=n;
    const tag=el.tagName.toLowerCase();
    const role=el.getAttribute('role')||({'a':'link','button':'button','input':'textbox','select':'combobox','textarea':'textbox'}[tag]||tag);
    const name=(el.getAttribute('aria-label')||el.innerText||el.value||el.placeholder||'').trim().slice(0,120);
    const rect=el.getBoundingClientRect();
    const visible=rect.width>0 && rect.height>0 && getComputedStyle(el).visibility!=='hidden';
    const clickable= visible && (['a','button','input','select','textarea'].includes(tag) || el.onclick || el.getAttribute('role')==='button');
    if(visible || name){
      out.push({role, name: name||tag, ref: el.tagName+':'+count, tag, x: Math.round(rect.x), y: Math.round(rect.y), width: Math.round(rect.width), height: Math.round(rect.height), clickable});
      count++;
    }
    n=walker.nextNode();
  }
  return out;
})()
"#;
    let params = json!({"expression": js, "returnByValue": true, "awaitPromise": false});
    let res = cdp_call("Runtime.evaluate", params, CapabilityClass::None)?;
    let val = res.get("result").and_then(|r| r.get("value")).cloned().unwrap_or(Value::Null);
    Ok(json!({"snapshot": val, "via": "Runtime.evaluate"}))
}

// ---- Click / Hover ----
pub fn click_by_ref(r#ref: &str, element_desc: &str) -> Result<Value> {
    if proxy_enabled() && !r#ref.is_empty() {
        let proxy_res = crate::devtools_mcp::proxy::proxy_block_on(async {
            crate::devtools_mcp::proxy::global_proxy().click(r#ref, element_desc).await
        });
        if let Ok(v) = proxy_res { return Ok(v); }
    }
    let selector = if !r#ref.is_empty() && r#ref.chars().all(|c| c.is_ascii_digit()) {
        let backend: i64 = r#ref.parse().unwrap_or(0);
        if backend!=0 {
            click_by_backend(backend)?
        } else { 0 };
        return Ok(json!({"clicked": true, "ref": r#ref, "via": "backend"}))
    } else if !r#ref.is_empty() && (r#ref.contains(':') || r#ref.contains('[')) {
        r#ref.to_string()
    } else if !element_desc.is_empty() {
        format!("*[aria-label*=\"{}\"], button:contains(\"{}\")", element_desc, element_desc)
    } else { bail!("click needs ref or element"); };
    click_by_selector(&selector)
}

fn click_by_backend(backend_id: i64) -> Result<i32> {
    let resolved = cdp_call("DOM.resolveNode", json!({"backendNodeId": backend_id}), CapabilityClass::None)?;
    let object_id = resolved.get("object").and_then(|o| o.get("objectId")).and_then(|v| v.as_str()).ok_or_else(|| anyhow::anyhow!("resolve failed"))?;
    let _clicked = cdp_call("Runtime.callFunctionOn", json!({
        "objectId": object_id,
        "functionDeclaration": "function(){ this.click(); return this.tagName; }",
        "returnByValue": true
    }), CapabilityClass::RuntimeEvaluate)?;
    let _ = cdp_call("Runtime.releaseObject", json!({"objectId": object_id}), CapabilityClass::None);
    Ok(1)
}

pub fn click_by_selector(selector: &str) -> Result<Value> {
    if proxy_enabled() {
        let s = selector.to_string();
        let res = crate::devtools_mcp::proxy::proxy_block_on(async {
            crate::devtools_mcp::proxy::global_proxy().click(&s, &s).await
        });
        if let Ok(v) = res { return Ok(v); }
    }
    let js = format!("(() => {{ const el=document.querySelector({:?}); if(!el) return {{error:'not found'}}; el.click(); const r=el.getBoundingClientRect(); return {{clicked:true, tag: el.tagName, x: r.x, y: r.y}}; }})()", selector);
    let v = rt_client::evaluate_sync(&js, false)?;
    Ok(v)
}

pub fn hover_by_ref(r#ref: &str, selector: Option<&str>) -> Result<Value> {
    if proxy_enabled() {
        let r = r#ref.to_string();
        let e = selector.unwrap_or(r#ref).to_string();
        let res = crate::devtools_mcp::proxy::proxy_block_on(async {
            crate::devtools_mcp::proxy::global_proxy().hover(&r, &e).await
        });
        if let Ok(v) = res { return Ok(v); }
    }
    let sel = selector.unwrap_or(r#ref);
    let js = format!("(() => {{ const el=document.querySelector({:?}); if(!el) return {{error:'not found'}}; el.dispatchEvent(new MouseEvent('mouseover',{{bubbles:true}})); const r=el.getBoundingClientRect(); return {{hovered:true, x:r.x, y:r.y}}; }})()", sel);
    let v = rt_client::evaluate_sync(&js, false)?;
    Ok(v)
}

// ---- Type ----
pub fn type_text(r#ref: &str, text: &str, submit: bool, selector: Option<&str>) -> Result<Value> {
    if proxy_enabled() {
        let rf = r#ref.to_string();
        let txt = text.to_string();
        let sel = selector.map(|s| s.to_string());
        let res = crate::devtools_mcp::proxy::proxy_block_on(async {
            crate::devtools_mcp::proxy::global_proxy().type_text(&rf, &txt, submit, sel.as_deref()).await
        });
        if let Ok(v) = res { return Ok(v); }
    }
    let sel = if let Some(s) = selector { s.to_string() } else if !r#ref.is_empty() { format!("[data-ref=\"{}\"]", r#ref) } else { "input, textarea, [contenteditable]".to_string() };
    let js = format!(r#"
(() => {{
  let el=document.querySelector({:?});
  if(!el) el=document.activeElement;
  if(!el || (el.tagName!=='INPUT' && el.tagName!=='TEXTAREA' && !el.isContentEditable)) {{
    el=document.querySelector('input, textarea, [contenteditable=true]');
  }}
  if(!el) return {{error:'no editable element'}};
  el.focus();
  if(el.isContentEditable) {{
    document.execCommand('selectAll', false, null);
    document.execCommand('insertText', false, {:?});
  }} else {{
    el.value={:?};
    el.dispatchEvent(new Event('input',{{bubbles:true}}));
    el.dispatchEvent(new Event('change',{{bubbles:true}}));
  }}
  if({}) {{ el.dispatchEvent(new KeyboardEvent('keydown',{{key:'Enter',code:'Enter',keyCode:13,bubbles:true}})); el.dispatchEvent(new KeyboardEvent('keyup',{{key:'Enter',code:'Enter',keyCode:13,bubbles:true}})); }}
  return {{typed: {:?}.length, tag: el.tagName}};
}})()
"#, sel, text, text, submit, text);
    let v = rt_client::evaluate_sync(&js, false)?;
    Ok(v)
}

pub fn fill(selector: &str, text: &str) -> Result<Value> {
    if proxy_enabled() {
        let s = selector.to_string();
        let t = text.to_string();
        let res = crate::devtools_mcp::proxy::proxy_block_on(async {
            crate::devtools_mcp::proxy::global_proxy().fill(&s, &t).await
        });
        if let Ok(v) = res { return Ok(v); }
    }
    let js = format!(r#"(() => {{ const el=document.querySelector({:?}); if(!el) return {{error:'not found'}}; el.focus(); el.value={:?}; el.dispatchEvent(new Event('input',{{bubbles:true}})); return {{filled:true}}; }})()"#, selector, text);
    let v = rt_client::evaluate_sync(&js, false)?;
    Ok(v)
}

// ---- Select option ----
pub fn select_option(r#ref: &str, values: &[String]) -> Result<Value> {
    if proxy_enabled() {
        let r = r#ref.to_string();
        let vals = values.to_vec();
        let res = crate::devtools_mcp::proxy::proxy_block_on(async {
            crate::devtools_mcp::proxy::global_proxy().select_option(&r, &vals).await
        });
        if let Ok(v) = res { return Ok(v); }
    }
    let js = format!(r#"
(() => {{
  const el=document.querySelector({:?}) || document.querySelector('select');
  if(!el) return {{error:'no select'}};
  const vals={};
  for(const opt of el.options) {{ if(vals.includes(opt.value) || vals.includes(opt.text)) opt.selected=true; }}
  el.dispatchEvent(new Event('change',{{bubbles:true}}));
  return {{selected: vals}};
}})()
"#, r#ref, serde_json::to_string(values).unwrap());
    let v = rt_client::evaluate_sync(&js, false)?;
    Ok(v)
}

// ---- Press key ----
pub fn press_key(key: &str) -> Result<Value> {
    if proxy_enabled() {
        let k = key.to_string();
        let res = crate::devtools_mcp::proxy::proxy_block_on(async {
            crate::devtools_mcp::proxy::global_proxy().press_key(&k).await
        });
        if let Ok(v) = res { return Ok(v); }
    }
    let (cdp_key, code) = map_key(key);
    let _ = cdp_call("Input.dispatchKeyEvent", json!({"type":"keyDown","key": cdp_key, "code": code, "windowsVirtualKeyCode": key_code(&cdp_key)}), CapabilityClass::None);
    let _ = cdp_call("Input.dispatchKeyEvent", json!({"type":"keyUp","key": cdp_key, "code": code}), CapabilityClass::None);
    if key.to_lowercase()=="enter" {
        let _v = rt_client::evaluate_sync("(() => { const ae=document.activeElement; if(ae&&ae.form) ae.form.dispatchEvent(new Event('submit',{bubbles:true,cancelable:true})); return true; })()", false)?;
        return Ok(json!({"pressed": key, "via":"Input"}));
    }
    let _ = rt_client::evaluate_sync(&format!("document.dispatchEvent(new KeyboardEvent('keydown',{{key:{:?},bubbles:true}}))", cdp_key), false)?;
    Ok(json!({"pressed": key, "via":"Input"}))
}

fn map_key(k: &str) -> (String,String) {
    let lower=k.to_lowercase();
    match lower.as_str() {
        "enter" => ("Enter".into(),"Enter".into()),
        "escape" | "esc" => ("Escape".into(),"Escape".into()),
        "tab" => ("Tab".into(),"Tab".into()),
        "arrowleft" => ("ArrowLeft".into(),"ArrowLeft".into()),
        "arrowright" => ("ArrowRight".into(),"ArrowRight".into()),
        "arrowup" => ("ArrowUp".into(),"ArrowUp".into()),
        "arrowdown" => ("ArrowDown".into(),"ArrowDown".into()),
        "backspace" => ("Backspace".into(),"Backspace".into()),
        "delete" | "del" => ("Delete".into(),"Delete".into()),
        _ if k.len()==1 => (k.to_string(), format!("Key{}", k.to_uppercase())),
        _ => (k.to_string(), k.to_string()),
    }
}
fn key_code(k: &str) -> u32 {
    match k { "Enter"=>13, "Escape"=>27, "Tab"=>9, "Backspace"=>8, "Delete"=>46, "ArrowLeft"=>37, "ArrowRight"=>39, "ArrowUp"=>38, "ArrowDown"=>40, _=>0 }
}

// ---- Evaluate ----
pub fn evaluate_js(expr: &str) -> Result<Value> {
    if proxy_enabled() {
        let e = expr.to_string();
        let res = crate::devtools_mcp::proxy::proxy_block_on(async {
            crate::devtools_mcp::proxy::global_proxy().evaluate(&e).await
        });
        if let Ok(v) = res { return Ok(v); }
    }
    let v = rt_client::evaluate_sync(expr, true)?;
    Ok(v)
}

// ---- Screenshot via CDP ----
pub fn screenshot_cdp() -> Result<(Vec<u8>, Value)> {
    if proxy_enabled() {
        let res = crate::devtools_mcp::proxy::proxy_block_on(async {
            crate::devtools_mcp::proxy::global_proxy().screenshot().await
        });
        if let Ok(v) = res { return Ok(v); }
    }
    let res = cdp_call("Page.captureScreenshot", json!({"format":"png", "captureBeyondViewport": false}), CapabilityClass::None)?;
    let data = res.get("data").and_then(|v| v.as_str()).ok_or_else(|| anyhow::anyhow!("no screenshot data"))?;
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD.decode(data).context("base64 decode")?;
    let meta = json!({"format":"png","via":"CDP","bytes": bytes.len()});
    Ok((bytes, meta))
}

// ---- Tabs / Targets ----
pub fn tabs() -> Result<Value> {
    if proxy_enabled() {
        let res = crate::devtools_mcp::proxy::proxy_block_on(async {
            crate::devtools_mcp::proxy::global_proxy().tabs().await
        });
        if let Ok(v) = res { return Ok(v); }
    }
    let list = crate::cdp::list_targets()?;
    let pages: Vec<Value> = list.iter().map(|t| json!({"id": t.id, "title": t.title, "url": t.url, "type": t.typ, "ws": t.web_socket_debugger_url})).collect();
    Ok(json!({"targets": pages}))
}

// ---- Console logs ----
pub fn console_logs() -> Result<Value> {
    if proxy_enabled() {
        let res = crate::devtools_mcp::proxy::proxy_block_on(async {
            crate::devtools_mcp::proxy::global_proxy().console().await
        });
        if let Ok(v) = res { return Ok(v); }
    }
    let _ = cdp_call("Console.enable", json!({}), CapabilityClass::None);
    let js = r#"(() => { if(window._hyprfast_console) return window._hyprfast_console; return {note:'console via Log domain - use Page.enable + Log.enable and subscribe; for now returning empty, check browser devtools'}; })()"#;
    let v = rt_client::evaluate_sync(js, false)?;
    Ok(json!({"logs": v}))
}

// ---- Wait ----
pub fn wait(seconds: f64) -> Result<Value> {
    if proxy_enabled() {
        let res = crate::devtools_mcp::proxy::proxy_block_on(async {
            crate::devtools_mcp::proxy::global_proxy().wait(seconds).await
        });
        if let Ok(v) = res { return Ok(v); }
    }
    std::thread::sleep(std::time::Duration::from_secs_f64(seconds));
    Ok(json!({"waited": seconds}))
}

// ---- Network / misc ----
pub fn enable_domains() -> Result<Value> {
    let _ = cdp_call("Page.enable", json!({}), CapabilityClass::None);
    let _ = cdp_call("Network.enable", json!({}), CapabilityClass::None);
    let _ = cdp_call("Runtime.enable", json!({}), CapabilityClass::None);
    Ok(json!({"enabled": ["Page","Network","Runtime"]}))
}
