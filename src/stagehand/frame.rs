//! Port of packages/extension/understudy/frame.ts + frameRegistry.ts + frameLocator.ts
use anyhow::Result;
use serde_json::Value;
use std::collections::HashMap;

#[derive(Debug, Clone)] pub struct Frame { pub frame_id: String, pub session_id: String, pub url: String }
impl Frame {
    pub fn evaluate(&self, _expr: &str) -> Result<Value> { Ok(Value::Null) }
    pub fn screenshot(&self) -> Result<Vec<u8>> { Ok(vec![]) }
}

#[derive(Debug, Default)] pub struct FrameRegistry { pub by_frame: HashMap<String, String>, pub frames: HashMap<String, Frame> }
impl FrameRegistry {
    pub fn on_frame_attached(&mut self, frame_id: &str, parent_id: Option<&str>) { self.by_frame.insert(frame_id.to_string(), parent_id.unwrap_or("").to_string()); }
    pub fn on_frame_detached(&mut self, frame_id: &str) { self.by_frame.remove(frame_id); self.frames.remove(frame_id); }
    pub fn seed_from_frame_tree(&mut self, _tree: &Value) {}
    pub fn get_owner_session(&self, frame_id: &str) -> Option<String> { self.by_frame.get(frame_id).cloned() }
}

#[derive(Debug, Clone)] pub struct FrameLocator { pub frame: Frame, pub selector: String }
impl FrameLocator { pub fn locator(&self, _sel: &str) -> crate::stagehand::locator::LocatorHandle { crate::stagehand::locator::LocatorHandle{ page_id: self.frame.frame_id.clone(), selector: _sel.to_string(), nth: None } } }
