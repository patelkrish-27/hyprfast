//! Port of domTree.ts
use anyhow::Result;
use serde_json::Value;
pub fn should_expand_node(_node: &Value) -> bool { false }
pub fn get_dom_tree_with_fallback(_session: &str) -> Result<Value> { Ok(Value::Null) }
pub fn build_session_dom_index(_session: &str) -> Result<Value> { Ok(Value::Null) }
pub fn find_node_by_backend_id(_tree: &Value, _id: u32) -> Option<Value> { None }
