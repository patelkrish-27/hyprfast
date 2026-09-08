//! Phase 6 — Frame tree manager.
//!
//! `FrameManager` is an owned module (rule 26). Only `BrowserRuntime` /
//! `BrowserRuntimeServer` may mutate authoritative frame state — it never
//! bumps its own detached counter. It bumps `DomState::frame_tree_version`
//! (the authoritative `frame_tree_version` store) via the shared
//! `Arc<DomState>` on every `Page.frameAttached` / `frameNavigated` /
//! `frameDetached` for **ANY** frame, not just the main frame
//! (plan §3, §4). It never assumes the main frame execution context
//! (stores per-frame `execution_context_id` from `Runtime.executionContextCreated`).
//!
//! Granularity: the global `frame_tree_version` always bumps, but
//! staleness for an `ElementRef` is **per-frame** — a ref scoped to
//! frame A stays valid when frame B navigates. `ElementRef.frame_tree_version`
//! stores the version of **its own frame** at creation, not the global max.
//! An additional `frame_versions: HashMap<frame_id, u64>` tracks this.
//! `status.frame_tree_version` reports the global max (via `DomState`).

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::sync::atomic::Ordering;

use serde_json::Value;

use crate::browser_runtime::connection::CdpEvent;
use crate::browser_runtime::dom_state::DomState;

/// One frame in the CDP frame tree.
#[derive(Debug, Clone)]
pub struct FrameRecord {
    pub frame_id: String,
    pub parent_frame: Option<String>,
    pub execution_context_id: Option<i64>,
    /// Owning target (when known via session mapping).
    pub target_id: Option<String>,
    pub session_id: Option<String>,
    pub url: String,
    pub name: String,
    pub is_main: bool,
}

/// Owned frame manager (rule 26). Single owner is `BrowserRuntimeServer`
/// which forwards ordered `CdpEvent`s in wire order (downstream of Phase 1).
pub struct FrameManager {
    frames: RwLock<HashMap<String, FrameRecord>>,
    /// Per-frame version: last global `frame_tree_version` assigned to that frame.
    frame_versions: RwLock<HashMap<String, u64>>,
    dom_state: Arc<DomState>,
}

impl std::fmt::Debug for FrameManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FrameManager")
            .field("frames", &self.frames.read().map(|g| g.len()).unwrap_or(0))
            .field("global", &self.dom_state.frame_tree_version())
            .finish_non_exhaustive()
    }
}

impl FrameManager {
    pub fn new(dom_state: Arc<DomState>) -> Arc<Self> {
        Arc::new(Self {
            frames: RwLock::new(HashMap::new()),
            frame_versions: RwLock::new(HashMap::new()),
            dom_state,
        })
    }

    /// Current global `frame_tree_version` (max across all frames).
    pub fn frame_tree_version(&self) -> u64 {
        self.dom_state.frame_tree_version()
    }

    /// Per-frame version for `frame_id`. 0 if frame never seen (treated as missing).
    pub fn frame_version(&self, frame_id: &str) -> u64 {
        self.frame_versions
            .read()
            .ok()
            .and_then(|m| m.get(frame_id).cloned())
            .unwrap_or(0)
    }

    pub fn get_frame(&self, frame_id: &str) -> Option<FrameRecord> {
        self.frames.read().ok().and_then(|m| m.get(frame_id).cloned())
    }

    pub fn list_frames(&self) -> Vec<FrameRecord> {
        self.frames.read().map(|m| m.values().cloned().collect()).unwrap_or_default()
    }

    pub fn frame_count(&self) -> usize {
        self.frames.read().map(|m| m.len()).unwrap_or(0)
    }

    /// Ordered event entry point. Must be called in wire `sequence` order.
    pub fn on_event(&self, ev: &CdpEvent) {
        match ev.method.as_str() {
            "Page.frameAttached" => self.on_frame_attached(ev),
            "Page.frameNavigated" => self.on_frame_navigated(ev),
            "Page.frameDetached" => self.on_frame_detached(ev),
            "Runtime.executionContextCreated" => self.on_execution_context_created(ev),
            "Runtime.executionContextDestroyed" => self.on_execution_context_destroyed(ev),
            "Runtime.executionContextsCleared" => self.on_execution_contexts_cleared(ev),
            _ => {}
        }
    }

    fn bump_for_frame(&self, frame_id: &str) -> u64 {
        // Global bump via authoritative DomState store (rule 26).
        let new_global = self.dom_state.frame_tree_version.fetch_add(1, Ordering::SeqCst) + 1;
        if let Ok(mut m) = self.frame_versions.write() {
            m.insert(frame_id.to_string(), new_global);
        }
        new_global
    }

    fn on_frame_attached(&self, ev: &CdpEvent) {
        // CDP: { frameId, parentFrameId }
        let frame_id = ev
            .params
            .get("frameId")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if frame_id.is_empty() {
            return;
        }
        let parent = ev
            .params
            .get("parentFrameId")
            .and_then(Value::as_str)
            .map(|s| s.to_string());
        // Store / update
        if let Ok(mut g) = self.frames.write() {
            g.entry(frame_id.clone())
                .and_modify(|r| {
                    r.parent_frame = parent.clone();
                    r.is_main = parent.is_none();
                })
                .or_insert_with(|| FrameRecord {
                    frame_id: frame_id.clone(),
                    parent_frame: parent.clone(),
                    execution_context_id: None,
                    target_id: None,
                    session_id: ev.session_id.clone(),
                    url: String::new(),
                    name: String::new(),
                    is_main: parent.is_none(),
                });
            // Also update session if not set
            if let Some(rec) = g.get_mut(&frame_id) {
                if rec.session_id.is_none() {
                    rec.session_id = ev.session_id.clone();
                }
            }
        }
        let v = self.bump_for_frame(&frame_id);
        tracing::debug!(frame_id = %frame_id, parent = ?parent, frame_tree_version = v, "Page.frameAttached");
    }

    fn on_frame_navigated(&self, ev: &CdpEvent) {
        // CDP: { frame: { id, parentId, url, name, ... } }  OR  { frame: {...} } with sessionId
        let frame = ev.params.get("frame");
        let frame_id = frame
            .and_then(|f| f.get("id"))
            .and_then(Value::as_str)
            .or_else(|| ev.params.get("frameId").and_then(Value::as_str))
            .unwrap_or("")
            .to_string();
        if frame_id.is_empty() {
            return;
        }
        let parent = frame
            .and_then(|f| f.get("parentId"))
            .and_then(Value::as_str)
            .map(|s| s.to_string());
        let url = frame
            .and_then(|f| f.get("url"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let name = frame
            .and_then(|f| f.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();

        if let Ok(mut g) = self.frames.write() {
            let rec = g.entry(frame_id.clone()).or_insert_with(|| FrameRecord {
                frame_id: frame_id.clone(),
                parent_frame: parent.clone(),
                execution_context_id: None,
                target_id: None,
                session_id: ev.session_id.clone(),
                url: url.clone(),
                name: name.clone(),
                is_main: parent.is_none(),
            });
            rec.parent_frame = parent.clone();
            rec.url = url.clone();
            if !name.is_empty() {
                rec.name = name.clone();
            }
            rec.is_main = parent.is_none();
            if rec.session_id.is_none() {
                rec.session_id = ev.session_id.clone();
            }
        }
        let v = self.bump_for_frame(&frame_id);
        tracing::debug!(frame_id = %frame_id, url = %url, frame_tree_version = v, "Page.frameNavigated");
    }

    fn on_frame_detached(&self, ev: &CdpEvent) {
        // CDP: { frameId, reason }
        let frame_id = ev
            .params
            .get("frameId")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if frame_id.is_empty() {
            return;
        }
        // Bump BEFORE removing so version history is preserved for staleness checks
        // (detached frame's version advances to global, refs to it become stale).
        let v = self.bump_for_frame(&frame_id);
        if let Ok(mut g) = self.frames.write() {
            g.remove(&frame_id);
        }
        tracing::debug!(frame_id = %frame_id, frame_tree_version = v, "Page.frameDetached");
    }

    fn on_execution_context_created(&self, ev: &CdpEvent) {
        // CDP: { context: { id, origin, name, auxData: { isDefault, type, frameId } } }
        let ctx = ev.params.get("context");
        let ctx_id = ctx.and_then(|c| c.get("id")).and_then(Value::as_i64);
        let frame_id = ctx
            .and_then(|c| c.get("auxData"))
            .and_then(|a| a.get("frameId"))
            .and_then(Value::as_str)
            .map(|s| s.to_string());
        let Some(frame_id) = frame_id else { return; };
        let Some(ctx_id) = ctx_id else { return; };
        if let Ok(mut g) = self.frames.write() {
            if let Some(rec) = g.get_mut(&frame_id) {
                rec.execution_context_id = Some(ctx_id);
            } else {
                // Frame not yet seen via Page.* — create placeholder.
                g.insert(
                    frame_id.clone(),
                    FrameRecord {
                        frame_id: frame_id.clone(),
                        parent_frame: None,
                        execution_context_id: Some(ctx_id),
                        target_id: None,
                        session_id: ev.session_id.clone(),
                        url: String::new(),
                        name: String::new(),
                        is_main: false,
                    },
                );
            }
        }
        tracing::debug!(frame_id = %frame_id, execution_context_id = ctx_id, "Runtime.executionContextCreated");
        // Do NOT bump frame_tree_version for execution context creation alone —
        // only Page.frame* events bump per spec. Tests assert exactly that.
    }

    fn on_execution_context_destroyed(&self, ev: &CdpEvent) {
        let ctx_id = ev
            .params
            .get("executionContextId")
            .and_then(Value::as_i64);
        if let Some(ctx_id) = ctx_id {
            if let Ok(mut g) = self.frames.write() {
                for rec in g.values_mut() {
                    if rec.execution_context_id == Some(ctx_id) {
                        rec.execution_context_id = None;
                    }
                }
            }
        }
        tracing::debug!(execution_context_id = ?ctx_id, "Runtime.executionContextDestroyed");
    }

    fn on_execution_contexts_cleared(&self, _ev: &CdpEvent) {
        if let Ok(mut g) = self.frames.write() {
            for rec in g.values_mut() {
                rec.execution_context_id = None;
            }
        }
        tracing::debug!("Runtime.executionContextsCleared");
    }

    /// Check if an ElementRef scoped to `frame_id` with `stored_version` is stale.
    /// True if frame's current version != stored, or frame is detached (still stale
    /// because its last bump made version != stored), or global advanced for that frame.
    pub fn is_stale_for_frame(&self, frame_id: &str, stored_version: u64) -> bool {
        let cur = self.frame_version(frame_id);
        // If frame never existed, stored 0 is not stale; otherwise any mismatch is stale.
        // Detached frames keep their last bumped version in frame_versions, so a stored
        // ref to a detached frame will mismatch (global bump on detach) and be stale.
        if cur == 0 && stored_version == 0 {
            return false;
        }
        cur != stored_version
    }

    /// Full staleness including dom_version (delegates dom check to DomState).
    pub fn is_element_stale(&self, element: &crate::browser_runtime::element_index::ElementRef) -> bool {
        if self.is_stale_for_frame(&element.frame_id, element.frame_tree_version) {
            return true;
        }
        // dom_version check is global; but we delegate to DomState::is_stale helper shape
        // ElementRef carries dom_version_created; compare to current.
        element.dom_version_created != self.dom_state.dom_version()
    }

    /// Handle reconnect: bump global is already done via DomState::on_reconnected;
    /// frame state is invalidated conservatively — clear all frames and set
    /// per-frame versions to new global so old refs are stale.
    pub fn handle_reconnect(&self, new_global: u64) {
        if let Ok(mut g) = self.frames.write() {
            g.clear();
        }
        // Keep frame_versions entries but update to new_global for staleness?
        // Simpler: clear them — any prior frame_id will have cur=0, but old refs stored
        // non-zero global, so is_stale_for_frame returns cur != stored => stale.
        if let Ok(mut m) = self.frame_versions.write() {
            // Preserve keys with new_global to mark detached?
            // Actually we want every old ref to be stale, so clearing achieves that
            // because cur becomes 0 vs old stored !=0. Keep empty.
            m.clear();
            let _ = new_global;
        }
        tracing::info!(new_global, "FrameManager reconnect: cleared frames");
    }

    pub fn trace_snapshot(&self) -> Value {
        let frames: Vec<Value> = self
            .list_frames()
            .iter()
            .map(|r| {
                serde_json::json!({
                    "frameId": r.frame_id,
                    "parentFrame": r.parent_frame,
                    "executionContextId": r.execution_context_id,
                    "sessionId": r.session_id,
                    "url": r.url,
                    "name": r.name,
                    "isMain": r.is_main,
                    "frameVersion": self.frame_version(&r.frame_id),
                })
            })
            .collect();
        serde_json::json!({
            "frame_count": frames.len(),
            "frames": frames,
            "frame_tree_version_global": self.frame_tree_version(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser_runtime::connection::CdpEvent;
    use std::time::Instant;

    fn ds() -> Arc<DomState> {
        Arc::new(DomState::new())
    }

    fn ev(method: &str, params: Value, session: Option<String>, seq: u64) -> CdpEvent {
        CdpEvent {
            method: method.to_string(),
            params,
            session_id: session,
            sequence: seq,
            timestamp: Instant::now(),
        }
    }

    #[test]
    fn frame_attached_bumps_any_frame() {
        let dom = ds();
        let fm = FrameManager::new(dom.clone());
        assert_eq!(fm.frame_tree_version(), 0);
        fm.on_event(&ev(
            "Page.frameAttached",
            serde_json::json!({"frameId": "child1", "parentFrameId": "main"}),
            None,
            0,
        ));
        assert_eq!(fm.frame_tree_version(), 1);
        assert_eq!(fm.frame_version("child1"), 1);
        assert_eq!(fm.frame_count(), 1);
        // main frame navigation also bumps
        fm.on_event(&ev(
            "Page.frameNavigated",
            serde_json::json!({"frame": {"id": "main", "url": "https://example.com"}}),
            None,
            1,
        ));
        assert_eq!(fm.frame_tree_version(), 2);
        assert_eq!(fm.frame_version("main"), 2);
        assert_eq!(fm.frame_version("child1"), 1); // untouched
    }

    #[test]
    fn iframe_navigation_does_not_over_invalidate_unrelated_frame() {
        let dom = ds();
        let fm = FrameManager::new(dom.clone());
        // main and iframe
        fm.on_event(&ev(
            "Page.frameNavigated",
            serde_json::json!({"frame": {"id": "main", "url": "https://example.com"}}),
            None,
            0,
        ));
        fm.on_event(&ev(
            "Page.frameAttached",
            serde_json::json!({"frameId": "iframe1", "parentFrameId": "main"}),
            None,
            1,
        ));
        fm.on_event(&ev(
            "Page.frameNavigated",
            serde_json::json!({"frame": {"id": "iframe1", "parentId": "main", "url": "https://inner.example"}}),
            None,
            2,
        ));
        let main_ver = fm.frame_version("main");
        let iframe_ver = fm.frame_version("iframe1");
        assert_eq!(fm.frame_tree_version(), 3);
        // Simulate ElementRef scoped to main with its frame version
        let stale_main = fm.is_stale_for_frame("main", main_ver);
        assert!(!stale_main, "main ref should still be valid after iframe nav");
        // Now navigate main itself -> main ref goes stale
        fm.on_event(&ev(
            "Page.frameNavigated",
            serde_json::json!({"frame": {"id": "main", "url": "https://example.com/new"}}),
            None,
            3,
        ));
        assert!(fm.is_stale_for_frame("main", main_ver), "main ref must be stale after its own frame navigated");
        assert!(!fm.is_stale_for_frame("iframe1", iframe_ver) || true); // iframe untouched by main nav? Actually main nav is separate; iframe version unchanged
        // iframe still valid if not touched
        let still_valid = !fm.is_stale_for_frame("iframe1", iframe_ver);
        assert!(still_valid, "iframe ref should still be valid after main navigated — per-frame granularity");
        // Now navigate iframe itself -> its ref goes stale
        fm.on_event(&ev(
            "Page.frameNavigated",
            serde_json::json!({"frame": {"id": "iframe1", "parentId": "main", "url": "https://inner.example/2"}}),
            None,
            4,
        ));
        assert!(fm.is_stale_for_frame("iframe1", iframe_ver));
    }

    #[test]
    fn frame_detached_bumps_and_removes() {
        let dom = ds();
        let fm = FrameManager::new(dom.clone());
        fm.on_event(&ev(
            "Page.frameAttached",
            serde_json::json!({"frameId": "f1", "parentFrameId": "main"}),
            None,
            0,
        ));
        let ver_before = fm.frame_version("f1");
        fm.on_event(&ev(
            "Page.frameDetached",
            serde_json::json!({"frameId": "f1", "reason": "remove"}),
            None,
            1,
        ));
        assert_eq!(fm.frame_tree_version(), 2);
        // detached frame's version was bumped on detach, so old ver is stale
        assert!(fm.is_stale_for_frame("f1", ver_before));
        assert!(fm.get_frame("f1").is_none());
    }

    #[test]
    fn execution_context_tracked_without_bump() {
        let dom = ds();
        let fm = FrameManager::new(dom);
        fm.on_event(&ev(
            "Page.frameAttached",
            serde_json::json!({"frameId": "main"}),
            None,
            0,
        ));
        let v_before = fm.frame_tree_version();
        fm.on_event(&ev(
            "Runtime.executionContextCreated",
            serde_json::json!({"context": {"id": 42, "origin": "https://example.com", "name": "", "auxData": {"isDefault": true, "type": "default", "frameId": "main"}}}),
            None,
            1,
        ));
        assert_eq!(fm.frame_tree_version(), v_before, "execution context creation must not bump frame_tree_version");
        assert_eq!(fm.get_frame("main").unwrap().execution_context_id, Some(42));
    }

    #[test]
    fn never_assume_main_frame_context() {
        let dom = ds();
        let fm = FrameManager::new(dom);
        // Create iframe without main frame existing
        fm.on_event(&ev(
            "Page.frameAttached",
            serde_json::json!({"frameId": "iframe-only", "parentFrameId": "unknown-parent"}),
            Some("s-1".to_string()),
            0,
        ));
        fm.on_event(&ev(
            "Runtime.executionContextCreated",
            serde_json::json!({"context": {"id": 99, "auxData": {"frameId": "iframe-only"}}}),
            Some("s-1".to_string()),
            1,
        ));
        let rec = fm.get_frame("iframe-only").unwrap();
        assert_eq!(rec.execution_context_id, Some(99));
        assert_eq!(rec.session_id.as_deref(), Some("s-1"));
    }
}
