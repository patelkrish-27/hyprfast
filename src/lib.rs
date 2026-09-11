#![allow(dead_code,clippy::all,clippy::pedantic)]
//! Library surface for integration tests.
//!
//! The `hyprfast` binary owns `main.rs`; integration tests under `tests/`
//! link against this crate target. Phase 1 exposes only `browser_runtime`;
//! later phases add their modules here as they land.
pub mod browser_runtime;
pub mod devtools_mcp;
