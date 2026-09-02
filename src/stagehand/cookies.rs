//! Port of packages/extension/understudy/cookies.ts
use anyhow::Result;
use serde_json::Value;
pub fn filter_cookies(cookies: Vec<Value>, _filter: &Value) -> Vec<Value> { cookies }
pub fn normalize_cookie_params(v: Value) -> Result<Value> { Ok(v) }
pub fn to_cdp_cookie_param(v: &Value) -> Value { v.clone() }
pub fn cookie_matches_filter(_cookie: &Value, _filter: &Value) -> bool { true }
pub fn get_cookies() -> Result<Vec<Value>> { let ws = crate::cdp::get_ws_url(None)?; let r = crate::cdp::cdp_call(&ws, "Storage.getCookies", serde_json::json!({}))?; Ok(r.get("cookies").and_then(|v| v.as_array()).cloned().unwrap_or_default()) }
pub fn set_cookies(cookies: Vec<Value>) -> Result<()> { let ws = crate::cdp::get_ws_url(None)?; for c in cookies { let _ = crate::cdp::cdp_call(&ws, "Storage.setCookies", serde_json::json!({"cookies": [c]}))?; } Ok(()) }
