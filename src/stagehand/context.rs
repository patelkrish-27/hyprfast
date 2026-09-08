//! Port of packages/extension/understudy/context.ts + domainPolicy + chromeTabs
//! Phase 3: transport via BrowserRuntimeClient. B18 deferred (no-op headers/policy) preserved.

use anyhow::Result;
use serde_json::Value;
use serde_json::json;
use crate::browser_runtime::server::CapabilityClass;
use crate::browser_runtime::client as rt_client;

pub fn new_page() -> Result<Value> {
    rt_client::cdp_call_sync("Target.createTarget", json!({"url": "about:blank"}), None, None, CapabilityClass::None)
}
pub fn pages() -> Result<Value> {
    rt_client::cdp_call_sync("Target.getTargets", json!({}), None, None, CapabilityClass::None)
}
pub fn add_init_script(_script: &str) -> Result<()> {
    let _ = rt_client::cdp_call_sync(
        "Page.addScriptToEvaluateOnNewDocument",
        json!({"source": _script}),
        None,
        None,
        CapabilityClass::None,
    )?;
    Ok(())
}
pub fn set_extra_http_headers(_headers: &Value) -> Result<()> { Ok(()) }
pub fn normalize_domain_policy(_policy: &Value) -> Value { _policy.clone() }
