//! Port of packages/sdk-ts/src/webmcp.ts
use anyhow::Result;
use serde_json::Value;
pub fn list_tools(page_id: &str) -> Result<Vec<Value>> {
    let ws = crate::cdp::get_ws_url(None)?;
    let v = crate::cdp::cdp_call(&ws, "Runtime.evaluate", serde_json::json!({"expression": "(() => globalThis.__webmcpTools || [])()", "returnByValue": true}))?;
    Ok(v.get("result").and_then(|r| r.get("value")).and_then(|v| v.as_array()).cloned().unwrap_or_default())
}
pub fn invoke_tool(page_id: &str, name: &str, input: Value) -> Result<Value> {
    let ws = crate::cdp::get_ws_url(None)?;
    crate::cdp::cdp_call(&ws, "Runtime.evaluate", serde_json::json!({"expression": format!("globalThis.__invokeWebMCPTool({:?}, {})", name, input), "returnByValue": true}))
}
