//! Port of packages/extension/understudy/clipboard.ts
use anyhow::Result;
pub fn write_text(text: &str) -> Result<()> { let ws = crate::cdp::get_ws_url(None)?; let _ = crate::cdp::cdp_call(&ws, "Runtime.evaluate", serde_json::json!({"expression": format!("navigator.clipboard.writeText({:?})", text)}))?; Ok(()) }
pub fn read_text() -> Result<String> { let ws = crate::cdp::get_ws_url(None)?; let v = crate::cdp::cdp_call(&ws, "Runtime.evaluate", serde_json::json!({"expression": "navigator.clipboard.readText()", "returnByValue": true}))?; Ok(v.get("result").and_then(|r| r.get("value")).and_then(|v| v.as_str()).unwrap_or("").to_string()) }
pub fn clear() -> Result<()> { write_text("") }
