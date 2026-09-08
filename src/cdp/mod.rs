//! CDP: Chrome DevTools Protocol via HTTP discovery + WebSocket JSON-RPC
//! Phase 3: transport migrated to BrowserRuntime (persistent daemon).
//! This module retains its public helper API for compatibility, but every
//! call now routes through `browser_runtime::client` (daemon when alive,
//! ephemeral BrowserRuntime fallback otherwise). No per-call WebSocket is
//! created here — the only WebSocket connect lives in
//! `browser_runtime/connection.rs` (invariant I2).

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
        anyhow::anyhow!("CDP unreachable at {}/json ({}). Launch browser with --remote-debugging-port={} e.g. `hyprfast browser open https://example.com` or `hyprfast launch \"brave --remote-debugging-port=9222 --force-renderer-accessibility --new-window https://example.com\"`", base, e, DEFAULT_PORT)
    })?;
    if !resp.status().is_success() {
        bail!("CDP GET /json failed: {}", resp.status());
    }
    let targets: Vec<Target> = resp.json().await.context("parse targets")?;
    Ok(targets)
}

pub fn list_targets() -> Result<Vec<Target>> {
    // Phase 3: try daemon's tabs path first (still Target.getTargets via persistent WS);
    // fall back to HTTP discovery when daemon not alive.
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
                // Map CDP TargetInfo to our Target shape (best-effort)
                let id = info.get("targetId").and_then(|x| x.as_str()).unwrap_or("").to_string();
                let title = info.get("title").and_then(|x| x.as_str()).unwrap_or("").to_string();
                let url = info.get("url").and_then(|x| x.as_str()).unwrap_or("").to_string();
                let typ = info.get("type").and_then(|x| x.as_str()).unwrap_or("page").to_string();
                out.push(Target { id, title, url, web_socket_debugger_url: String::new(), typ });
            }
            if !out.is_empty() {
                return Ok(out);
            }
        }
    }
    // Fallback to HTTP /json (works even without daemon)
    futures::executor::block_on(list_targets_async()).or_else(|_| {
        // If even CDP via daemon failed and HTTP failed, propagate HTTP error
        Err(anyhow::anyhow!("list_targets: daemon and HTTP discovery both failed"))
    })
}

pub async fn list_targets_filtered_async(typ: Option<&str>) -> Result<Vec<Target>> {
    let all = list_targets_async().await?;
    if let Some(t) = typ {
        Ok(all.into_iter().filter(|x| x.typ==t).collect())
    } else { Ok(all) }
}

pub fn version() -> Result<Value> {
    // Browser.getVersion via persistent transport when possible.
    let v = crate::browser_runtime::client::cdp_call_sync(
        "Browser.getVersion",
        json!({}),
        None,
        None,
        crate::browser_runtime::server::CapabilityClass::None,
    );
    if let Ok(val) = v {
        return Ok(val);
    }
    // Fallback to HTTP /json/version
    futures::executor::block_on(async {
        let base = cdp_base_url();
        let url = format!("{}/json/version", base);
        let client = reqwest::Client::builder().timeout(Duration::from_secs(2)).build()?;
        let v: Value = client.get(&url).send().await?.json().await?;
        Ok(v)
    })
}

// WS discovery is now owned by BrowserRuntime/targets (Phase 5); these wrappers
// are retained for compat but route through the same discovery as the daemon.
pub async fn get_ws_url_async(target_url_match: Option<&str>) -> Result<String> {
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
    bail!("no page with webSocketDebuggerUrl")
}

pub fn get_ws_url(target_match: Option<&str>) -> Result<String> {
    futures::executor::block_on(get_ws_url_async(target_match))
}

pub async fn new_page_async(url: &str) -> Result<Target> {
    let base = cdp_base_url();
    let client = reqwest::Client::builder().timeout(Duration::from_secs(3)).build()?;
    let v: Value = client.put(format!("{}/json/new", base)).query(&[("url", url)]).send().await?.json().await?;
    let t: Target = serde_json::from_value(v)?;
    Ok(t)
}

// ---- WebSocket JSON-RPC via BrowserRuntime (no direct WS connect here) ----

pub async fn cdp_call_async(_ws_url: &str, method: &str, params: Value) -> Result<Value> {
    // ws_url is ignored: call goes via persistent daemon (or ephemeral fallback).
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
    futures::executor::block_on(cdp_call_async(ws_url, method, params))
}

/// Send multiple calls via persistent transport (multiplexed, not serial throwaway WS).
pub async fn cdp_batch_async(ws_url: &str, calls: Vec<(&str, Value)>) -> Result<Vec<Value>> {
    let mut out = Vec::new();
    for (method, params) in calls {
        out.push(cdp_call_async(ws_url, method, params).await?);
    }
    Ok(out)
}

pub fn cdp_batch(ws_url: &str, calls: Vec<(&str, Value)>) -> Result<Vec<Value>> {
    futures::executor::block_on(cdp_batch_async(ws_url, calls))
}

// ---- Helpers that combine discovery + ws ----

pub fn call(method: &str, params: Value) -> Result<Value> {
    cdp_call("", method, params)
}
pub fn call_on(_match_str: Option<&str>, method: &str, params: Value) -> Result<Value> {
    cdp_call("", method, params)
}

/// Evaluate JS in main frame: Runtime.evaluate
pub fn evaluate(expression: &str, await_promise: bool) -> Result<Value> {
    crate::browser_runtime::client::evaluate_sync(expression, await_promise)
}

/// DOM snapshot via Runtime.evaluate -> outerHTML + AX built in JS (fallback when Accessibility domain not ready)
pub fn ensure_enabled() -> Result<()> {
    // Page/DOM/Runtime.enable — best-effort, capability none (read/setup)
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
