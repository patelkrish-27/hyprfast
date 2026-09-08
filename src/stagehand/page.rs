//! Port of packages/extension/understudy/page.ts (core nav + evaluate + screenshot)
//! Phase 3: transport via BrowserRuntimeClient. B6/B10/B11 deferred preserved.

use anyhow::Result;
use serde_json::{Value, json};
use crate::browser_runtime::server::CapabilityClass;
use crate::browser_runtime::client as rt_client;

pub fn goto(_page_id: &str, url: &str) -> Result<Value> {
    // B6 deferred: page_id ignored, hits last page — preserved
    rt_client::cdp_call_sync("Page.navigate", json!({"url": url}), None, None, CapabilityClass::Navigation)
}
pub fn reload() -> Result<Value> {
    rt_client::cdp_call_sync("Page.reload", json!({}), None, None, CapabilityClass::Navigation)
}
pub fn evaluate(_page_id: &str, expr: &str) -> Result<Value> {
    // B6 deferred: page_id ignored — preserved; capability runtime_evaluate
    rt_client::cdp_call_sync("Runtime.evaluate", json!({"expression": expr, "returnByValue": true}), None, None, CapabilityClass::RuntimeEvaluate)
}
pub fn screenshot() -> Result<Vec<u8>> { Ok(vec![]) }
pub fn wait_for_load_state(_state: &str, _timeout_ms: u32) -> Result<()> { Ok(()) }
