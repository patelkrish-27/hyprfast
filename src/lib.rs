#![allow(dead_code,clippy::all,clippy::pedantic)]
//! Library surface for integration tests.
//!
//! The `hyprfast` binary owns `main.rs`; integration tests under `tests/`
//! link against this crate target. Phase 1 exposes only `browser_runtime`;
//! later phases add their modules here as they land.
pub mod browser_runtime;
pub mod decider;
pub mod devtools_mcp;
pub mod perception;

// Stubs so `decider::image` (which reuses `screenshot::capture` + `browser::screenshot_cdp`
// in the binary) still compiles when this crate is built as a library for `tests/`.
#[doc(hidden)]
pub mod screenshot {
    use anyhow::Result;
    use serde_json::Value;
    pub fn capture(_window: &str, _region: &str, _scale: f64) -> Result<(Vec<u8>, Value)> {
        anyhow::bail!("screenshot::capture unavailable in library context (binary only)")
    }
}
#[doc(hidden)]
pub mod browser {
    use anyhow::Result;
    use serde_json::Value;
    pub fn screenshot_cdp() -> Result<(Vec<u8>, Value)> {
        anyhow::bail!("browser::screenshot_cdp unavailable in library context")
    }
    pub fn click_by_selector(_selector: &str) -> Result<Value> { anyhow::bail!("browser::click_by_selector unavailable in library context") }
    pub fn type_text(_r: &str, _text: &str, _submit: bool, _sel: Option<&str>) -> Result<Value> { anyhow::bail!("browser::type_text unavailable") }
    pub fn click_by_ref(_r: &str, _desc: &str) -> Result<Value> { anyhow::bail!("browser::click_by_ref unavailable") }
}
#[doc(hidden)]
pub mod hint {
    use anyhow::Result;
    use serde_json::Value;
    pub fn hint_snapshot() -> Result<Value> { anyhow::bail!("hint unavailable in library context") }
    pub fn hint_snapshot_with_target(_target: Option<&str>) -> Result<Value> { anyhow::bail!("hint unavailable in library context") }
    pub fn hint_click(_label: &str) -> Result<Value> { anyhow::bail!("hint unavailable in library context") }
    pub fn hint_click_with_target(_label: &str, _target: Option<&str>) -> Result<Value> { anyhow::bail!("hint unavailable in library context") }
    pub fn hint_type(_label: &str, _text: &str) -> Result<Value> { anyhow::bail!("hint unavailable in library context") }
    pub fn hint_type_with_target(_label: &str, _text: &str, _target: Option<&str>) -> Result<Value> { anyhow::bail!("hint unavailable in library context") }
    pub fn heuristic_hint_match(_instr: &str, _hints: &Value) -> Option<String> { None }
    pub fn resolve_hint_for_instruction(_instr: &str, _hints: &Value, _target: Option<&str>) -> Option<String> { None }
}
#[doc(hidden)]
pub mod ground {
    use anyhow::Result;
    use serde_json::Value;
    #[derive(Debug, Clone, Copy)] pub enum GroundingBackend { Gemini, DeciderCandidate, Auto }
    pub fn ground(_a: &str, _b: &str, _c: &str) -> Result<Value> { anyhow::bail!("ground unavailable in library context") }
    pub fn ground_candidate(_a: &str, _b: Option<&str>) -> Result<Value> { anyhow::bail!("ground unavailable in library context") }
    pub fn ground_with_backend(_a: &str, _b: &str, _c: &str, _d: GroundingBackend) -> Result<Value> { anyhow::bail!("ground unavailable in library context") }
}
#[doc(hidden)]
pub mod input {
    use anyhow::Result;
    pub fn click(_x: Option<f64>, _y: Option<f64>, _b: &str, _d: bool) -> Result<()> { anyhow::bail!("input unavailable in library context") }
    pub fn type_text(_text: &str) -> Result<()> { anyhow::bail!("input unavailable") }
    pub fn key_combo(_keys: &str) -> Result<()> { anyhow::bail!("input unavailable") }
    pub fn move_cursor(_x: f64, _y: f64) -> Result<()> { anyhow::bail!("input unavailable") }
    pub fn drag(_x: f64, _y: f64, _tx: f64, _ty: f64, _b: &str) -> Result<()> { anyhow::bail!("input unavailable") }
    pub fn scroll(_dy: f64, _dx: f64, _x: Option<f64>, _y: Option<f64>) -> Result<()> { anyhow::bail!("input unavailable") }
}
