//! Port of packages/sdk-ts/src/webmcp.ts
//! Phase 3: transport via BrowserRuntimeClient (capability runtime_evaluate, B6 preserved).

use anyhow::Result;
use serde_json::Value;
use crate::browser_runtime::server::CapabilityClass;
use crate::browser_runtime::client as rt_client;

pub fn list_tools(_page_id: &str) -> Result<Vec<Value>> {
    // B6 deferred: page_id ignored — preserved
    let v = rt_client::cdp_call_sync(
        "Runtime.evaluate",
        serde_json::json!({"expression": "(() => globalThis.__webmcpTools || [])()", "returnByValue": true}),
        None,
        None,
        CapabilityClass::RuntimeEvaluate,
    )?;
    Ok(v.get("result").and_then(|r| r.get("value")).and_then(|v| v.as_array()).cloned().unwrap_or_default())
}
pub fn invoke_tool(_page_id: &str, name: &str, input: Value) -> Result<Value> {
    // B2/B6 deferred: string-concatenated JS, page_id ignored — preserved
    rt_client::cdp_call_sync(
        "Runtime.evaluate",
        serde_json::json!({"expression": format!("globalThis.__invokeWebMCPTool({:?}, {})", name, input), "returnByValue": true}),
        None,
        None,
        CapabilityClass::RuntimeEvaluate,
    )
}
