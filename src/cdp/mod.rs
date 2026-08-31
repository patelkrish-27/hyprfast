//! CDP: Chrome DevTools Protocol via HTTP discovery + WebSocket JSON-RPC
//! Minimal client for hyprfast 0.5 browser automation.
//! No persistent daemon yet; per-call connects are ~5-15ms. Reuses tokio runtime.

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::collections::HashMap;
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
        anyhow::anyhow!("CDP unreachable at {}/json ({}). Launch browser with --remote-debugging-port={} e.g. `hyprfast browser open https://example.com` or `hyprfast launch \"brave --remote-debugging-port=9222 --force-renderer-accessibility --new-window https://example.com\"`", base, e, DEFAULT_PORT)
    })?;
    if !resp.status().is_success() {
        bail!("CDP GET /json failed: {}", resp.status());
    }
    let targets: Vec<Target> = resp.json().await.context("parse targets")?;
    Ok(targets)
}

pub fn list_targets() -> Result<Vec<Target>> {
    rt().block_on(list_targets_async())
}

pub async fn list_targets_filtered_async(typ: Option<&str>) -> Result<Vec<Target>> {
    let all = list_targets_async().await?;
    if let Some(t) = typ {
        Ok(all.into_iter().filter(|x| x.typ==t).collect())
    } else { Ok(all) }
}

pub fn version() -> Result<Value> {
    rt().block_on(async {
        let base = cdp_base_url();
        let url = format!("{}/json/version", base);
        let client = reqwest::Client::builder().timeout(Duration::from_secs(2)).build()?;
        let v: Value = client.get(&url).send().await?.json().await?;
        Ok(v)
    })
}

pub async fn get_ws_url_async(target_url_match: Option<&str>) -> Result<String> {
    let targets = list_targets_async().await?;
    if targets.is_empty() {
        bail!("no debuggable targets at {} — launch browser with --remote-debugging-port={} (e.g. brave --remote-debugging-port=9222 --force-renderer-accessibility)", cdp_base_url(), DEFAULT_PORT);
    }
    // Prefer pages
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
    // Prefer last (most recent) page
    for t in pages.iter().rev() {
        if !t.web_socket_debugger_url.is_empty() { return Ok(t.web_socket_debugger_url.clone()); }
    }
    bail!("no page with webSocketDebuggerUrl")
}

pub fn get_ws_url(target_match: Option<&str>) -> Result<String> {
    rt().block_on(get_ws_url_async(target_match))
}

pub async fn new_page_async(url: &str) -> Result<Target> {
    let base = cdp_base_url();
    let client = reqwest::Client::builder().timeout(Duration::from_secs(3)).build()?;
    let v: Value = client.put(format!("{}/json/new", base)).query(&[("url", url)]).send().await?.json().await?;
    let t: Target = serde_json::from_value(v)?;
    Ok(t)
}

// ---- WebSocket JSON-RPC ----

use futures::{SinkExt, StreamExt};
use tokio_tungstenite::{connect_async, tungstenite::Message};

static RT: std::sync::OnceLock<tokio::runtime::Runtime> = std::sync::OnceLock::new();
fn rt() -> &'static tokio::runtime::Runtime {
    RT.get_or_init(|| tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("rt"))
}

pub async fn cdp_call_async(ws_url: &str, method: &str, params: Value) -> Result<Value> {
    let (mut ws, _) = connect_async(ws_url).await.context("ws connect")?;
    let id = 1;
    let req = json!({"id": id, "method": method, "params": params});
    ws.send(Message::Text(req.to_string().into())).await?;
    // Read until id matches or error
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    loop {
        if tokio::time::Instant::now() > deadline { bail!("CDP timeout for {}", method); }
        let msg = tokio::time::timeout(Duration::from_secs(5), ws.next()).await.context("timeout")?;
        let Some(Ok(Message::Text(txt))) = msg else { continue; };
        let v: Value = serde_json::from_str(&txt).unwrap_or(json!({}));
        if v.get("id").and_then(|x| x.as_i64()) == Some(id as i64) {
            if let Some(err) = v.get("error") { bail!("CDP {} error: {}", method, err); }
            // close gracefully
            let _ = ws.close(None).await;
            return Ok(v.get("result").cloned().unwrap_or(Value::Null));
        }
        // ignore events without id
    }
}

pub fn cdp_call(ws_url: &str, method: &str, params: Value) -> Result<Value> {
    rt().block_on(cdp_call_async(ws_url, method, params))
}

/// Send multiple calls over single WS connection (faster)
pub async fn cdp_batch_async(ws_url: &str, calls: Vec<(&str, Value)>) -> Result<Vec<Value>> {
    let (mut ws, _) = connect_async(ws_url).await.context("ws connect")?;
    let mut out = Vec::new();
    for (i, (method, params)) in calls.into_iter().enumerate() {
        let id = (i+1) as i64;
        let req = json!({"id": id, "method": method, "params": params});
        ws.send(Message::Text(req.to_string().into())).await?;
        loop {
            let msg = ws.next().await.ok_or_else(|| anyhow::anyhow!("ws closed"))??;
            if let Message::Text(txt) = msg {
                let v: Value = serde_json::from_str(&txt).unwrap_or(json!({}));
                if v.get("id").and_then(|x| x.as_i64()) == Some(id) {
                    if let Some(err) = v.get("error") { bail!("CDP {} error: {}", method, err); }
                    out.push(v.get("result").cloned().unwrap_or(Value::Null));
                    break;
                }
            }
        }
    }
    let _ = ws.close(None).await;
    Ok(out)
}

pub fn cdp_batch(ws_url: &str, calls: Vec<(&str, Value)>) -> Result<Vec<Value>> {
    rt().block_on(cdp_batch_async(ws_url, calls))
}

// ---- Helpers that combine discovery + ws ----

pub fn call(method: &str, params: Value) -> Result<Value> {
    let ws = get_ws_url(None)?;
    cdp_call(&ws, method, params)
}
pub fn call_on(match_str: Option<&str>, method: &str, params: Value) -> Result<Value> {
    let ws = get_ws_url(match_str)?;
    cdp_call(&ws, method, params)
}

/// Evaluate JS in main frame: Runtime.evaluate
pub fn evaluate(expression: &str, await_promise: bool) -> Result<Value> {
    let ws = get_ws_url(None)?;
    let params = json!({"expression": expression, "returnByValue": true, "awaitPromise": await_promise, "userGesture": true});
    let res = cdp_call(&ws, "Runtime.evaluate", params)?;
    if let Some(exc) = res.get("exceptionDetails") { bail!("evaluate exception: {}", exc); }
    Ok(res.get("result").and_then(|r| r.get("value")).cloned().unwrap_or(res))
}

/// DOM snapshot via Runtime.evaluate -> outerHTML + AX built in JS (fallback when Accessibility domain not ready)
pub fn ensure_enabled() -> Result<()> {
    let ws = get_ws_url(None)?;
    let _ = cdp_call(&ws, "Page.enable", json!({}));
    let _ = cdp_call(&ws, "DOM.enable", json!({}));
    let _ = cdp_call(&ws, "Runtime.enable", json!({}));
    Ok(())
}
