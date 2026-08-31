//! Browser automation via CDP - hyprfast 0.5
//! Mirrors browsermcp tools but pure Rust via cdp:: module
//! Covers navigate/snapshot/click/hover/type/press/evaluate/screenshot/tabs/console

use anyhow::{Result, bail, Context};
use serde_json::{Value, json};
use crate::cdp;

fn ws_for_target(target: Option<&str>) -> Result<String> {
    cdp::get_ws_url(target)
}

// ---- Navigate ----
pub fn navigate(url: &str, target: Option<&str>) -> Result<Value> {
    if url.is_empty() { bail!("url required"); }
    let ws = ws_for_target(target)?;
    // ensure Page enabled
    let _ = cdp::cdp_call(&ws, "Page.enable", json!({}));
    let res = cdp::cdp_call(&ws, "Page.navigate", json!({"url": url}))?;
    // wait a bit for load (network idle best-effort)
    std::thread::sleep(std::time::Duration::from_millis(400));
    Ok(json!({"result": res, "url": url}))
}

pub fn go_back() -> Result<Value> {
    let expr = "history.back(); location.href";
    let v = cdp::evaluate(expr, false)?;
    Ok(json!({"result": v}))
}
pub fn go_forward() -> Result<Value> {
    let expr = "history.forward(); location.href";
    let v = cdp::evaluate(expr, false)?;
    Ok(json!({"result": v}))
}

// ---- Snapshot (AX tree) ----
// Uses Accessibility.getFullAXTree when available, fallback to JS-built snapshot
pub fn snapshot(max_nodes: usize) -> Result<Value> {
    let ws = ws_for_target(None)?;
    // try Accessibility domain first
    let ax = cdp::cdp_call(&ws, "Accessibility.getFullAXTree", json!({}));
    if let Ok(val) = ax {
        if let Some(nodes) = val.get("nodes").and_then(|v| v.as_array()) {
            if !nodes.is_empty() {
                let limited: Vec<&Value> = nodes.iter().take(max_nodes).collect();
                let refs = build_snapshot_from_ax(&limited, &ws)?;
                return Ok(json!({"snapshot": refs, "raw_nodes": limited.len(), "via": "Accessibility"}));
            }
        }
    }
    // fallback: JS snapshot builder
    snapshot_via_js(&ws)
}

fn build_snapshot_from_ax(nodes: &[&Value], _ws: &str) -> Result<Value> {
    // Convert AX nodes to browsermcp-like tree with refs
    // AX node: nodeId, role {value}, name {value}, properties, childIds, backendDOMNodeId
    let mut out = Vec::new();
    for n in nodes.iter().take(80) {
        let role = n.get("role").and_then(|r| r.get("value")).and_then(|v| v.as_str()).unwrap_or("");
        let name = n.get("name").and_then(|r| r.get("value")).and_then(|v| v.as_str()).unwrap_or("");
        let node_id = n.get("nodeId").and_then(|v| v.as_str()).unwrap_or("");
        let backend = n.get("backendDOMNodeId").and_then(|v| v.as_i64()).unwrap_or(0);
        if role.is_empty() && name.is_empty() { continue; }
        // Filter non-visible?
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

fn snapshot_via_js(ws: &str) -> Result<Value> {
    // JS that walks DOM and emits role/name/ref/bounds
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
    let res = cdp::cdp_call(ws, "Runtime.evaluate", params)?;
    let val = res.get("result").and_then(|r| r.get("value")).cloned().unwrap_or(Value::Null);
    Ok(json!({"snapshot": val, "via": "Runtime.evaluate"}))
}

// ---- Click / Hover ----
// Accepts ref (backendDOMNodeId string from snapshot) or selector string, or element description
pub fn click_by_ref(r#ref: &str, element_desc: &str) -> Result<Value> {
    let selector = if !r#ref.is_empty() && r#ref.chars().all(|c| c.is_ascii_digit()) {
        // backend id -> resolve
        let backend: i64 = r#ref.parse().unwrap_or(0);
        if backend!=0 {
            click_by_backend(backend)?
        } else { 0 };
        return Ok(json!({"clicked": true, "ref": r#ref, "via": "backend"}))
    } else if !r#ref.is_empty() && (r#ref.contains(':') || r#ref.contains('[')) {
        // selector-ish
        r#ref.to_string()
    } else if !element_desc.is_empty() {
        // try to guess selector from description
        format!("*[aria-label*=\"{}\"], button:contains(\"{}\")", element_desc, element_desc)
    } else { bail!("click needs ref or element"); };
    // For selector path, use JS click
    click_by_selector(&selector)
}

fn click_by_backend(backend_id: i64) -> Result<i32> {
    let ws = ws_for_target(None)?;
    // DOM.resolveNode -> objectId -> DOM.getBoxModel -> click via Input
    let resolved = cdp::cdp_call(&ws, "DOM.resolveNode", json!({"backendNodeId": backend_id}))?;
    let object_id = resolved.get("object").and_then(|o| o.get("objectId")).and_then(|v| v.as_str()).ok_or_else(|| anyhow::anyhow!("resolve failed"))?;
    // Use Runtime callFunctionOn to click
    let clicked = cdp::cdp_call(&ws, "Runtime.callFunctionOn", json!({
        "objectId": object_id,
        "functionDeclaration": "function(){ this.click(); return this.tagName; }",
        "returnByValue": true
    }))?;
    Ok(1)
}

pub fn click_by_selector(selector: &str) -> Result<Value> {
    let js = format!("(() => {{ const el=document.querySelector({:?}); if(!el) return {{error:'not found'}}; el.click(); const r=el.getBoundingClientRect(); return {{clicked:true, tag: el.tagName, x: r.x, y: r.y}}; }})()", selector);
    let v = cdp::evaluate(&js, false)?;
    Ok(v)
}

pub fn hover_by_ref(r#ref: &str, selector: Option<&str>) -> Result<Value> {
    let sel = selector.unwrap_or(r#ref);
    let js = format!("(() => {{ const el=document.querySelector({:?}); if(!el) return {{error:'not found'}}; el.dispatchEvent(new MouseEvent('mouseover',{{bubbles:true}})); const r=el.getBoundingClientRect(); return {{hovered:true, x:r.x, y:r.y}}; }})()", sel);
    let v = cdp::evaluate(&js, false)?;
    Ok(v)
}

// ---- Type ----
// browser_type element ref text submit
pub fn type_text(r#ref: &str, text: &str, submit: bool, selector: Option<&str>) -> Result<Value> {
    let sel = if let Some(s) = selector { s.to_string() } else if !r#ref.is_empty() { format!("[data-ref=\"{}\"]", r#ref) } else { "input, textarea, [contenteditable]".to_string() };
    // Try selector first, fallback to activeElement or first input
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
    let v = cdp::evaluate(&js, false)?;
    Ok(v)
}

// For typing by selector explicitly
pub fn fill(selector: &str, text: &str) -> Result<Value> {
    let js = format!(r#"(() => {{ const el=document.querySelector({:?}); if(!el) return {{error:'not found'}}; el.focus(); el.value={:?}; el.dispatchEvent(new Event('input',{{bubbles:true}})); return {{filled:true}}; }})()"#, selector, text);
    let v = cdp::evaluate(&js, false)?;
    Ok(v)
}

// ---- Select option ----
pub fn select_option(r#ref: &str, values: &[String]) -> Result<Value> {
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
    let v = cdp::evaluate(&js, false)?;
    Ok(v)
}

// ---- Press key ----
pub fn press_key(key: &str) -> Result<Value> {
    // Use Input.dispatchKeyEvent for fidelity, fallback to JS
    let ws = ws_for_target(None)?;
    // Map friendly names to CDP key
    let (cdp_key, code) = map_key(key);
    // keyDown + keyUp
    let _ = cdp::cdp_call(&ws, "Input.dispatchKeyEvent", json!({"type":"keyDown","key": cdp_key, "code": code, "windowsVirtualKeyCode": key_code(&cdp_key)}));
    let _ = cdp::cdp_call(&ws, "Input.dispatchKeyEvent", json!({"type":"keyUp","key": cdp_key, "code": code}));
    // also JS fallback for Enter etc to trigger form submit
    if key.to_lowercase()=="enter" {
        let _v = cdp::evaluate("(() => { const ae=document.activeElement; if(ae&&ae.form) ae.form.dispatchEvent(new Event('submit',{bubbles:true,cancelable:true})); return true; })()", false)?;
        return Ok(json!({"pressed": key, "via":"Input"}));
    }
    let _ = cdp::evaluate(&format!("document.dispatchEvent(new KeyboardEvent('keydown',{{key:{:?},bubbles:true}}))", cdp_key), false)?;
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
    let v = cdp::evaluate(expr, true)?;
    Ok(v)
}

// ---- Screenshot via CDP ----
pub fn screenshot_cdp() -> Result<(Vec<u8>, Value)> {
    let ws = ws_for_target(None)?;
    let res = cdp::cdp_call(&ws, "Page.captureScreenshot", json!({"format":"png", "captureBeyondViewport": false}))?;
    let data = res.get("data").and_then(|v| v.as_str()).ok_or_else(|| anyhow::anyhow!("no screenshot data"))?;
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD.decode(data).context("base64 decode")?;
    let meta = json!({"format":"png","via":"CDP","bytes": bytes.len()});
    Ok((bytes, meta))
}

// ---- Tabs / Targets ----
pub fn tabs() -> Result<Value> {
    let list = cdp::list_targets()?;
    let pages: Vec<Value> = list.iter().map(|t| json!({"id": t.id, "title": t.title, "url": t.url, "type": t.typ, "ws": t.web_socket_debugger_url})).collect();
    Ok(json!({"targets": pages}))
}

// ---- Console logs ----
// Enable Runtime/Console and pull messages; simple: Runtime.getProperties(Console) not persistent.
// We'll fetch via Runtime.evaluate reading console buffer? Instead call Log.enable then collect.
// Minimal: return last console via evaluate of window._hyprfast_logs if instrumented, otherwise note.
pub fn console_logs() -> Result<Value> {
    let ws = ws_for_target(None)?;
    let _ = cdp::cdp_call(&ws, "Console.enable", json!({}));
    // There's no getLogs; we return via evaluate of performance.getEntries or just explain
    let js = r#"(() => { if(window._hyprfast_console) return window._hyprfast_console; return {note:'console via Log domain - use Page.enable + Log.enable and subscribe; for now returning empty, check browser devtools'}; })()"#;
    let v = cdp::evaluate(js, false)?;
    Ok(json!({"logs": v}))
}

// ---- Wait ----
pub fn wait(seconds: f64) -> Result<Value> {
    std::thread::sleep(std::time::Duration::from_secs_f64(seconds));
    Ok(json!({"waited": seconds}))
}

// ---- Network / misc ----
// For now expose evaluate as generic; network inspection via Network.enable + getResponseBody not stored.
// Provide helper to enable domains.
pub fn enable_domains() -> Result<Value> {
    let ws = ws_for_target(None)?;
    let _ = cdp::cdp_call(&ws, "Page.enable", json!({}));
    let _ = cdp::cdp_call(&ws, "Network.enable", json!({}));
    let _ = cdp::cdp_call(&ws, "Runtime.enable", json!({}));
    Ok(json!({"enabled": ["Page","Network","Runtime"]}))
}
