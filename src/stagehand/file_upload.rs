//! Port of packages/extension/understudy/fileUploadUtils.ts + packages/sdk-ts/src/fileUpload.ts
//! Phase 3: transport via BrowserRuntimeClient (capability file_upload, B7 deferred preserved).

use anyhow::Result;
use base64::{Engine as _, engine::general_purpose::STANDARD};
pub fn normalize_input_files(files: Vec<serde_json::Value>) -> Result<Vec<serde_json::Value>> { Ok(files) }
pub fn to_bytes(s: &str) -> Vec<u8> { s.as_bytes().to_vec() }
pub fn bytes_to_base64(b: &[u8]) -> String { STANDARD.encode(b) }
pub fn set_input_files(selector: &str, files: Vec<serde_json::Value>) -> Result<()> {
    // B7 deferred: only dispatches change, never DOM.setFileInputFiles — preserved
    let expr = format!("document.querySelector({:?})?.dispatchEvent(new Event('change',{{bubbles:true}}))", selector);
    let _ = crate::browser_runtime::client::cdp_call_sync(
        "Runtime.evaluate",
        serde_json::json!({"expression": expr}),
        None,
        None,
        crate::browser_runtime::server::CapabilityClass::FileUpload,
    )?;
    let _ = files;
    Ok(())
}
