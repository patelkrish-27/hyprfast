//! Port of packages/extension/understudy/context.ts + domainPolicy + chromeTabs
use anyhow::Result;
use serde_json::Value;
use crate::cdp;
use serde_json::json;
pub fn new_page() -> Result<Value> { let ws = cdp::get_ws_url(None)?; cdp::cdp_call(&ws, "Target.createTarget", json!({"url": "about:blank"})) }
pub fn pages() -> Result<Value> { let ws = cdp::get_ws_url(None)?; cdp::cdp_call(&ws, "Target.getTargets", json!({})) }
pub fn add_init_script(_script: &str) -> Result<()> { let ws = cdp::get_ws_url(None)?; let _ = cdp::cdp_call(&ws, "Page.addScriptToEvaluateOnNewDocument", json!({"source": _script}))?; Ok(()) }
pub fn set_extra_http_headers(_headers: &Value) -> Result<()> { Ok(()) }
pub fn normalize_domain_policy(_policy: &Value) -> Value { _policy.clone() }
