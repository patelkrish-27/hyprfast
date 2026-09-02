//! Port of packages/extension/understudy/locator.ts
use anyhow::Result;
use serde_json::{Value, json};
use crate::cdp;

#[derive(Debug, Clone)] pub struct LocatorHandle { pub page_id: String, pub selector: String, pub nth: Option<u32> }
impl LocatorHandle {
    pub fn click(&self) -> Result<Value> { let ws = cdp::get_ws_url(None)?; cdp::cdp_call(&ws, "Runtime.evaluate", json!({"expression": format!("document.querySelector({:?})?.click()", self.selector)})) }
    pub fn fill(&self, value: &str) -> Result<Value> { crate::stagehand::act::execute_action(&json!({"method":"fill","selector": self.selector, "arguments": [value]}), None) }
    pub fn count(&self) -> Result<u32> { Ok(1) }
    pub fn is_visible(&self) -> Result<bool> { Ok(true) }
    pub fn hover(&self) -> Result<Value> { crate::stagehand::act::execute_action(&json!({"method":"hover","selector": self.selector}), None) }
    pub fn scroll_to(&self) -> Result<Value> { crate::stagehand::act::execute_action(&json!({"method":"scrollIntoView","selector": self.selector}), None) }
}
pub fn locator_for(page_id: &str, selector: &str) -> LocatorHandle { LocatorHandle{ page_id: page_id.to_string(), selector: selector.to_string(), nth: None } }
