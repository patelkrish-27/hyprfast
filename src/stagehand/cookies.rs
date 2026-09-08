//! Port of packages/extension/understudy/cookies.ts
//! Phase 3: transport via BrowserRuntimeClient (capability cookies, B19 deferred preserved).

use anyhow::Result;
use serde_json::Value;
use crate::browser_runtime::server::CapabilityClass;
use crate::browser_runtime::client as rt_client;

pub fn filter_cookies(cookies: Vec<Value>, _filter: &Value) -> Vec<Value> { cookies }
pub fn normalize_cookie_params(v: Value) -> Result<Value> { Ok(v) }
pub fn to_cdp_cookie_param(v: &Value) -> Value { v.clone() }
pub fn cookie_matches_filter(_cookie: &Value, _filter: &Value) -> bool { true }
pub fn get_cookies() -> Result<Vec<Value>> {
    let r = rt_client::cdp_call_sync("Storage.getCookies", serde_json::json!({}), None, None, CapabilityClass::Cookies)?;
    Ok(r.get("cookies").and_then(|v| v.as_array()).cloned().unwrap_or_default())
}
pub fn set_cookies(cookies: Vec<Value>) -> Result<()> {
    // B19 deferred: per-cookie {"cookies":[c]} instead of one setCookies — preserved, fixed in Phase 8
    for c in cookies {
        let _ = rt_client::cdp_call_sync("Storage.setCookies", serde_json::json!({"cookies": [c]}), None, None, CapabilityClass::Cookies)?;
    }
    Ok(())
}
