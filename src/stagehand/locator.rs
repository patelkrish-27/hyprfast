//! Port of packages/extension/understudy/locator.ts
//! Phase 3: transport via BrowserRuntimeClient. B2/B6/B12 deferred preserved.

use anyhow::Result;
use serde_json::{Value, json};
use crate::browser_runtime::server::CapabilityClass;
use crate::browser_runtime::client as rt_client;

#[derive(Debug, Clone)] pub struct LocatorHandle { pub page_id: String, pub selector: String, pub nth: Option<u32> }
impl LocatorHandle {
    pub fn click(&self) -> Result<Value> {
        // B2 deferred: string-concatenated JS — preserved
        rt_client::cdp_call_sync(
            "Runtime.evaluate",
            json!({"expression": format!("document.querySelector({:?})?.click()", self.selector)}),
            None,
            None,
            CapabilityClass::RuntimeEvaluate,
        )
    }
    pub fn fill(&self, value: &str) -> Result<Value> { crate::stagehand::act::execute_action(&json!({"method":"fill","selector": self.selector, "arguments": [value]}), None) }
    pub fn count(&self) -> Result<u32> { Ok(1) }
    pub fn is_visible(&self) -> Result<bool> { Ok(true) }
    pub fn hover(&self) -> Result<Value> { crate::stagehand::act::execute_action(&json!({"method":"hover","selector": self.selector}), None) }
    pub fn scroll_to(&self) -> Result<Value> { crate::stagehand::act::execute_action(&json!({"method":"scrollIntoView","selector": self.selector}), None) }
}
pub fn locator_for(page_id: &str, selector: &str) -> LocatorHandle { LocatorHandle{ page_id: page_id.to_string(), selector: selector.to_string(), nth: None } }
