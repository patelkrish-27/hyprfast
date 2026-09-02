//! Port of capture.ts — delegates to snapshot::capture_hybrid for now
use anyhow::Result;
use crate::stagehand::snapshot::{HybridSnapshot, capture_hybrid};
pub fn capture_hybrid_snapshot() -> Result<HybridSnapshot> { capture_hybrid() }
