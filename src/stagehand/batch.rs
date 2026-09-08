//! Port of packages/sdk-ts/src/batch.ts — experimentalBatch
//! Phase 3: route via BrowserRuntimeClient (capability runtime_evaluate, deferred B8 timeout/input handling).

use anyhow::Result;
use serde_json::Value;
use crate::browser_runtime::server::CapabilityClass;
use crate::browser_runtime::client as rt_client;

pub type BatchCallback<Input, Output> = Box<dyn Fn(Input) -> Output + Send>;
pub fn experimental_batch(callback_source: &str, _input: Option<Value>, _timeout_ms: u32) -> Result<Value> {
    // B8 deferred: _timeout_ms and _input ignored — preserved, fixed in Phase 7/8
    // Transport now via persistent daemon; capability runtime_evaluate
    let res = rt_client::cdp_call_sync(
        "Runtime.evaluate",
        serde_json::json!({"expression": callback_source, "returnByValue": true}),
        None,
        None,
        CapabilityClass::RuntimeEvaluate,
    )?;
    // Preserve old extraction: unwrap result.value or return raw
    let val = res.get("result").and_then(|r| r.get("value")).cloned().unwrap_or(res);
    Ok(val)
}
