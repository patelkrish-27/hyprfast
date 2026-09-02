//! Port of sessions.ts
use std::collections::HashMap;
pub fn owner_session(_page_id: &str, _frame_id: &str) -> Option<String> { None }
pub fn parent_session(_page_id: &str, _parent_by_frame: &HashMap<String,String>, _frame_id: &str) -> Option<String> { None }
