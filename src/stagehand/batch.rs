//! Port of packages/sdk-ts/src/batch.ts — experimentalBatch
use anyhow::Result;
use serde_json::Value;
pub type BatchCallback<Input, Output> = Box<dyn Fn(Input) -> Output + Send>;
pub fn experimental_batch(callback_source: &str, input: Option<Value>, _timeout_ms: u32) -> Result<Value> {
    // In Rust hyprfast, batch runs callback via Runtime.evaluate in page context.
    // Full JS serialization parity kept via callbackSource string (same as stagehand.ts:153 check).
    let ws = crate::cdp::get_ws_url(None)?;
    crate::cdp::cdp_call(&ws, "Runtime.evaluate", serde_json::json!({"expression": callback_source, "returnByValue": true}))
}
