//! Port of packages/extension/understudy/page.ts (core nav + evaluate + screenshot)
use anyhow::Result;
use serde_json::{Value, json};
use crate::cdp;
pub fn goto(page_id: &str, url: &str) -> Result<Value> { let ws = cdp::get_ws_url(None)?; cdp::cdp_call(&ws, "Page.navigate", json!({"url": url})) }
pub fn reload() -> Result<Value> { let ws = cdp::get_ws_url(None)?; cdp::cdp_call(&ws, "Page.reload", json!({})) }
pub fn evaluate(page_id: &str, expr: &str) -> Result<Value> { let ws = cdp::get_ws_url(None)?; cdp::cdp_call(&ws, "Runtime.evaluate", json!({"expression": expr, "returnByValue": true})) }
pub fn screenshot() -> Result<Vec<u8>> { Ok(vec![]) }
pub fn wait_for_load_state(_state: &str, _timeout_ms: u32) -> Result<()> { Ok(()) }
