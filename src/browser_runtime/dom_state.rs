//! Phase 4 — Live DOM state.
//!
//! `DomState` is an owned module mutated ONLY via `BrowserRuntime`'s
//! state-owner API (rule 26). Feature code must never independently consume
//! raw WebSocket messages (rule 27) — all events flow through the single
//! `EventDispatcher` downstream of Phase 1's ordering pipeline, which calls
//! `DomState::on_event` in strict wire order (rule 28 / I9).
//!
//! Generations (rule 12, §4):
//! - `runtime_generation` — bumps on connect/reconnect (mirrors connection_generation in §3)
//! - `navigation_generation` — bumps on main-frame `Page.frameNavigated`
//! - `dom_version` — bumps on DOM invalidation/mutation
//! - `element_index_version` — bumps on full invalidation (DOM.documentUpdated)
//! - `frame_tree_version` — authoritative on `FrameManager` (Phase 5); exposed
//!   here for resolver use (Phase 6). Held as shared `AtomicU64` until then.

use std::collections::HashSet;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    RwLock,
};
use std::time::Duration;

use serde_json::Value;
use tokio::sync::broadcast;

use crate::browser_runtime::connection::CdpEvent;
use crate::browser_runtime::error::RuntimeResult;

/// Live DOM/navigation/lifecycle state owned by `BrowserRuntime`.
///
/// All counters use `SeqCst` because they participate in the snapshot-
/// consistency contract (§6) — a stale check must observe a total order.
pub struct DomState {
    /// Bumped on every browser-level WebSocket (re)connect.
    pub runtime_generation: AtomicU64,
    /// Bumped on main-frame `Page.frameNavigated`.
    pub navigation_generation: AtomicU64,
    /// Bumped on any DOM mutation / full invalidation.
    pub dom_version: AtomicU64,
    /// Bumped on full invalidation (`DOM.documentUpdated`).
    pub element_index_version: AtomicU64,
    /// Frame-tree version — Phase 5 owns authority, DomState exposes read access
    /// for Phase 6 resolver. Shared with `FrameManager` later; for now local.
    pub frame_tree_version: AtomicU64,
    /// Target generation (§3): authoritative counter for `TargetRef` validity.
    /// Owned by `BrowserRuntime` (rule 26); `BrowserTargetManager` reads it
    /// but increments only via `BrowserRuntime`'s state-owner API
    /// (`bump_target_generation`), never its own counter.
    pub target_generation: AtomicU64,

    /// `document.readyState` style string (e.g. "loading"/"interactive"/"complete").
    ready_state: RwLock<String>,
    /// Last `Page.lifecycleEvent` name (e.g. "init", "DOMContentLoaded", "load", "networkIdle").
    lifecycle_state: RwLock<String>,
    /// Parent node ids touched by recent insert/remove/attributeModified.
    /// Bounded by pruning to keep memory finite.
    affected_parents: RwLock<HashSet<i64>>,
    /// Last wire `sequence` applied — out-of-order arrival is a Phase 1 bug, not masked here.
    last_sequence: AtomicU64,
}

impl Default for DomState {
    fn default() -> Self {
        Self::new()
    }
}

impl DomState {
    pub fn new() -> Self {
        Self {
            runtime_generation: AtomicU64::new(0),
            navigation_generation: AtomicU64::new(0),
            dom_version: AtomicU64::new(0),
            element_index_version: AtomicU64::new(0),
            frame_tree_version: AtomicU64::new(0),
            target_generation: AtomicU64::new(0),
            ready_state: RwLock::new("unknown".to_string()),
            lifecycle_state: RwLock::new("init".to_string()),
            affected_parents: RwLock::new(HashSet::new()),
            last_sequence: AtomicU64::new(0),
        }
    }

    // -- generation accessors -------------------------------------------------
    pub fn runtime_generation(&self) -> u64 {
        self.runtime_generation.load(Ordering::SeqCst)
    }
    pub fn navigation_generation(&self) -> u64 {
        self.navigation_generation.load(Ordering::SeqCst)
    }
    pub fn dom_version(&self) -> u64 {
        self.dom_version.load(Ordering::SeqCst)
    }
    pub fn element_index_version(&self) -> u64 {
        self.element_index_version.load(Ordering::SeqCst)
    }
    pub fn frame_tree_version(&self) -> u64 {
        self.frame_tree_version.load(Ordering::SeqCst)
    }
    pub fn target_generation(&self) -> u64 {
        self.target_generation.load(Ordering::SeqCst)
    }
    /// State-owner API: bump `target_generation` (only `BrowserRuntime` / `BrowserRuntimeServer` may call).
    pub fn bump_target_generation(&self) -> u64 {
        self.target_generation.fetch_add(1, Ordering::SeqCst) + 1
    }
    /// State-owner API: bump `connection_generation` (`runtime_generation` is its store).
    pub fn bump_connection_generation(&self) -> u64 {
        self.runtime_generation.fetch_add(1, Ordering::SeqCst) + 1
    }

    pub fn ready_state(&self) -> String {
        self.ready_state.read().map(|g| g.clone()).unwrap_or_default()
    }
    pub fn lifecycle_state(&self) -> String {
        self.lifecycle_state.read().map(|g| g.clone()).unwrap_or_default()
    }
    pub fn affected_parents(&self) -> Vec<i64> {
        self.affected_parents
            .read()
            .map(|g| g.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Snapshot staleness predicate (§6): `true` iff the ref's versions are behind current.
    pub fn is_stale(&self, dom_version_created: u64, frame_tree_version_created: u64) -> bool {
        dom_version_created != self.dom_version() || frame_tree_version_created != self.frame_tree_version()
    }

    // -- lifecycle hooks ------------------------------------------------------
    /// Called once on first connect. Bumps `runtime_generation`.
    pub fn on_connected(&self) {
        self.runtime_generation.fetch_add(1, Ordering::SeqCst);
    }

    /// Called on every WebSocket reconnect. Bumps **both** generations via the
    /// state-owner `bump_*` APIs (single bump per generation, no double-bump —
    /// see `BrowserRuntimeServer::simulate_reconnect` fix for I12) and
    /// invalidates DOM state (conservative: new connection may have new document).
    /// Returns `(new_target_generation, new_connection_generation)`.
    pub fn on_reconnected(&self) -> (u64, u64) {
        let new_tg = self.bump_target_generation();
        let new_cg = self.bump_connection_generation();
        self.dom_version.fetch_add(1, Ordering::SeqCst);
        self.element_index_version.fetch_add(1, Ordering::SeqCst);
        if let Ok(mut g) = self.affected_parents.write() {
            g.clear();
        }
        (new_tg, new_cg)
    }

    // -- event application (strict wire order, no re-sort) -------------------
    /// Apply one ordered `CdpEvent` to live state. Must be called in wire
    /// `sequence` order (guaranteed by Phase 1's reorder buffer). An
    /// out-of-order arrival logs a warning and is treated as a Phase 1 bug,
    /// not worked around locally.
    pub fn on_event(&self, event: &CdpEvent) {
        let seq = event.sequence;
        let last = self.last_sequence.load(Ordering::SeqCst);
        if seq != 0 && seq < last {
            tracing::warn!(
                method = %event.method,
                sequence = seq,
                last_sequence = last,
                "out-of-order CdpEvent at DomState — Phase 1 ordering bug, not masking"
            );
        }
        if seq >= last {
            self.last_sequence.store(seq + 1, Ordering::SeqCst);
        }

        match event.method.as_str() {
            "DOM.documentUpdated" => {
                self.dom_version.fetch_add(1, Ordering::SeqCst);
                self.element_index_version.fetch_add(1, Ordering::SeqCst);
                if let Ok(mut g) = self.affected_parents.write() {
                    g.clear();
                }
                tracing::debug!(sequence = seq, dom_version = self.dom_version(), "DOM.documentUpdated: full invalidation");
            }
            "DOM.childNodeInserted" => {
                self.dom_version.fetch_add(1, Ordering::SeqCst);
                if let Some(pid) = event.params.get("parentNodeId").and_then(Value::as_i64) {
                    if let Ok(mut g) = self.affected_parents.write() {
                        g.insert(pid);
                        Self::prune_affected(&mut g);
                    }
                }
            }
            "DOM.childNodeRemoved" => {
                self.dom_version.fetch_add(1, Ordering::SeqCst);
                if let Some(pid) = event.params.get("parentNodeId").and_then(Value::as_i64) {
                    if let Ok(mut g) = self.affected_parents.write() {
                        g.insert(pid);
                        Self::prune_affected(&mut g);
                    }
                }
            }
            "DOM.attributeModified" => {
                self.dom_version.fetch_add(1, Ordering::SeqCst);
                if let Some(nid) = event.params.get("nodeId").and_then(Value::as_i64) {
                    if let Ok(mut g) = self.affected_parents.write() {
                        g.insert(nid);
                        Self::prune_affected(&mut g);
                    }
                }
            }
            "Page.frameNavigated" => {
                // `frameNavigated` fires for every frame; navigation_generation
                // only bumps for the main frame (no parentId). This matches
                // the plan's "for main frame" wording.
                let is_main = event
                    .params
                    .get("frame")
                    .and_then(|f| f.get("parentId"))
                    .is_none();
                if is_main {
                    self.navigation_generation.fetch_add(1, Ordering::SeqCst);
                    tracing::debug!(sequence = seq, navigation_generation = self.navigation_generation(), "Page.frameNavigated (main frame)");
                }
                // Any frame navigation invalidates DOM identity for frame-scoped refs,
                // but Phase 5's frame_tree_version bump covers that precisely; for
                // Phase 4 we at least bump dom_version on main-frame navigation so
                // snapshot checks catch it.
                if is_main {
                    self.dom_version.fetch_add(1, Ordering::SeqCst);
                }
            }
            "Page.lifecycleEvent" => {
                if let Some(name) = event.params.get("name").and_then(Value::as_str) {
                    if let Ok(mut g) = self.lifecycle_state.write() {
                        *g = name.to_string();
                    }
                    // Map to readyState approximation for waiters that check it.
                    let ready = match name {
                        "init" => "loading",
                        "DOMContentLoaded" => "interactive",
                        "load" => "complete",
                        "networkAlmostIdle" | "networkIdle" => "complete",
                        _ => name,
                    };
                    if let Ok(mut g) = self.ready_state.write() {
                        *g = ready.to_string();
                    }
                    tracing::debug!(sequence = seq, lifecycle = %name, ready_state = %ready, "Page.lifecycleEvent");
                }
            }
            _ => {}
        }
    }

    fn prune_affected(set: &mut HashSet<i64>) {
        const CAP: usize = 512;
        if set.len() > CAP {
            // Drain an arbitrary subset down to CAP/2 — exact eviction policy
            // is not load-bearing; bound is.
            let to_remove = set.len() - CAP / 2;
            let keys: Vec<i64> = set.iter().cloned().take(to_remove).collect();
            for k in keys {
                set.remove(&k);
            }
        }
    }

    // -- liftable wait primitive — now delegated to the single engine in wait.rs (Phase 9) --
    /// Generic liftable waiter: `check → subscribe → re-check → await`.
    /// Thin wrapper over [`crate::browser_runtime::wait::wait_for`] — the single
    /// authoritative primitive (exactly one `check/subscribe/re-check/await` loop
    /// exists codebase-wide; this wrapper just adapts the sync `check` to the
    /// engine's async-check signature).
    pub async fn wait_for<F, P>(
        &self,
        check: F,
        subscribe: impl Fn() -> broadcast::Receiver<CdpEvent>,
        predicate: P,
        timeout: Duration,
    ) -> RuntimeResult<Option<CdpEvent>>
    where
        F: Fn() -> bool,
        P: Fn(&CdpEvent) -> bool,
    {
        crate::browser_runtime::wait::wait_for(
            move || {
                let v = check();
                async move { v }
            },
            subscribe,
            predicate,
            timeout,
        )
        .await
    }

    /// Wait until `Page.lifecycleEvent.name == desired` is observed.
    ///
    /// Phase 9 refactor: reimplemented on top of the unified condition engine's
    /// `Lifecycle` / `NavigationComplete` conditions — no duplicate loop.
    pub async fn wait_for_lifecycle(
        &self,
        subscribe: impl Fn() -> broadcast::Receiver<CdpEvent>,
        desired: &str,
        timeout: Duration,
    ) -> RuntimeResult<()> {
        let desired_owned = desired.to_string();
        let desired_check = desired_owned.clone();
        let desired_pred = desired_owned;
        crate::browser_runtime::wait::wait_for(
            {
                let desired = desired_check.clone();
                // Capture `self` as `&DomState` (Copy) — each `check()` call borrows
                // `self` for the duration of the future, which is safe because `self`
                // outlives the entire `wait_for` await.
                move || {
                    let desired = desired.clone();
                    let ds: &DomState = self;
                    async move {
                        let ls = ds.lifecycle_state();
                        if ls == desired {
                            return true;
                        }
                        if desired == "complete" {
                            let rs = ds.ready_state();
                            if ls == "load" || rs == "complete" {
                                return true;
                            }
                        }
                        false
                    }
                }
            },
            subscribe,
            move |ev: &CdpEvent| {
                if ev.method != "Page.lifecycleEvent" {
                    return false;
                }
                let got = ev.params.get("name").and_then(|v| v.as_str()).unwrap_or("");
                if got == desired_pred {
                    return true;
                }
                if desired_pred == "complete" && got == "load" {
                    return true;
                }
                false
            },
            timeout,
        )
        .await
        .map(|_| ())
    }

    /// Convenience waiter for any in-flight navigation to reach `load`/`complete`.
    pub async fn wait_for_navigation(
        &self,
        subscribe: impl Fn() -> broadcast::Receiver<CdpEvent>,
        timeout: Duration,
    ) -> RuntimeResult<()> {
        self.wait_for_lifecycle(subscribe, "load", timeout).await
    }
}

 #[cfg(test)]
mod tests {
    use super::*;
    use crate::browser_runtime::connection::CdpEvent;
    use std::time::Instant;

    fn ev(method: &str, params: serde_json::Value, seq: u64) -> CdpEvent {
        CdpEvent {
            method: method.to_string(),
            params,
            session_id: None,
            sequence: seq,
            timestamp: Instant::now(),
        }
    }

    #[test]
    fn document_updated_invalidates() {
        let ds = DomState::new();
        let before = ds.dom_version();
        ds.on_event(&ev("DOM.documentUpdated", serde_json::json!({}), 0));
        assert_eq!(ds.dom_version(), before + 1);
        assert_eq!(ds.element_index_version(), 1);
        assert!(ds.affected_parents().is_empty());
    }

    #[test]
    fn child_insert_records_parent() {
        let ds = DomState::new();
        ds.on_event(&ev("DOM.childNodeInserted", serde_json::json!({"parentNodeId": 42}), 1));
        assert_eq!(ds.dom_version(), 1);
        assert!(ds.affected_parents().contains(&42));
    }

    #[test]
    fn frame_navigated_main_bumps_navigation() {
        let ds = DomState::new();
        ds.on_event(&ev(
            "Page.frameNavigated",
            serde_json::json!({"frame": {"id": "main", "url": "https://example.com"}}),
            2,
        ));
        assert_eq!(ds.navigation_generation(), 1);
        // Non-main frame must not bump navigation_generation
        ds.on_event(&ev(
            "Page.frameNavigated",
            serde_json::json!({"frame": {"id": "child", "parentId": "main"}}),
            3,
        ));
        assert_eq!(ds.navigation_generation(), 1);
    }

    #[test]
    fn lifecycle_event_updates_state() {
        let ds = DomState::new();
        ds.on_event(&ev(
            "Page.lifecycleEvent",
            serde_json::json!({"name": "DOMContentLoaded", "frameId": "main"}),
            4,
        ));
        assert_eq!(ds.lifecycle_state(), "DOMContentLoaded");
        assert_eq!(ds.ready_state(), "interactive");
        ds.on_event(&ev(
            "Page.lifecycleEvent",
            serde_json::json!({"name": "load", "frameId": "main"}),
            5,
        ));
        assert_eq!(ds.lifecycle_state(), "load");
        assert_eq!(ds.ready_state(), "complete");
    }
}
