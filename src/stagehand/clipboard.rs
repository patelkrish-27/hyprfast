//! Port of packages/extension/understudy/clipboard.ts
//! Phase 3: transport via BrowserRuntimeClient (capability clipboard, B2 preserved).

use anyhow::Result;
use crate::browser_runtime::server::CapabilityClass;
use crate::browser_runtime::client as rt_client;

pub fn write_text(text: &str) -> Result<()> {
    // B2 deferred: string-concatenated JS — preserved
    let expr = format!("navigator.clipboard.writeText({:?})", text);
    let _ = rt_client::cdp_call_sync(
        "Runtime.evaluate",
        serde_json::json!({"expression": expr}),
        None,
        None,
        CapabilityClass::Clipboard,
    )?;
    Ok(())
}
pub fn read_text() -> Result<String> {
    let v = rt_client::cdp_call_sync(
        "Runtime.evaluate",
        serde_json::json!({"expression": "navigator.clipboard.readText()", "returnByValue": true}),
        None,
        None,
        CapabilityClass::Clipboard,
    )?;
    Ok(v.get("result").and_then(|r| r.get("value")).and_then(|v| v.as_str()).unwrap_or("").to_string())
}
pub fn clear() -> Result<()> { write_text("") }
