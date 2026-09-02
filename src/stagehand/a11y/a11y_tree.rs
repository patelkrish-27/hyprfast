//! Port of a11yTree.ts
use anyhow::Result;
use serde_json::Value;
pub fn a11y_for_frame(_frame_id: &str) -> Result<Value> { Ok(Value::Null) }
pub fn build_hierarchical_tree(_ax: &Value) -> Value { Value::Null }
pub fn is_structural(_role: &str) -> bool { false }
