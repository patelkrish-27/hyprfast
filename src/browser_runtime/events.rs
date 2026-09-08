//! Phase 4 — Single ordered EventDispatcher.
//!
//! All CDP events flow through ONE `EventDispatcher`, itself downstream of
//! Phase 1's ordering pipeline. It does NOT re-sort or re-buffer — it trusts
//! the `sequence` ordering Phase 1 already guarantees, and treats any
//! out-of-order arrival as a Phase 1 bug (rule 28 / I9).
//!
//! Feature modules (`DomState`, `TargetManager`, `FrameManager` in later
//! phases) are mutated ONLY via `BrowserRuntime`'s state-owner API (rule 26).
//! They never independently consume raw WebSocket messages (rule 27).
//!
//! `EventDispatcher` is the owned module that bridges the ordered broadcast
//! into state mutation. It owns the single task that drains the broadcast in
//! order and calls `DomState::on_event`.

use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;

use crate::browser_runtime::connection::{BrowserRuntime, CdpEvent};
use crate::browser_runtime::dom_diff::DomDiffEngine;
use crate::browser_runtime::dom_state::DomState;
use crate::browser_runtime::error::{RuntimeError, RuntimeResult};
use crate::browser_runtime::frames::FrameManager;
use crate::browser_runtime::targets::BrowserTargetManager;

/// The single event dispatcher for a `BrowserRuntime`.
///
/// Created once per runtime after `Connected`. Exactly one instance owns the
/// draining task; cloning the runtime does not clone the dispatcher (I24).
pub struct EventDispatcher {
    runtime: BrowserRuntime,
    dom_state: Arc<DomState>,
    target_manager: Option<Arc<BrowserTargetManager>>,
    frame_manager: Option<Arc<FrameManager>>,
    dom_diff: Option<Arc<DomDiffEngine>>,
    task: Option<JoinHandle<()>>,
}

impl EventDispatcher {
    /// Create and spawn the single dispatcher task. The task subscribes to
    /// `runtime.subscribe()` (the ordered output of Phase 1's reorder buffer)
    /// and forwards each event to `dom_state.on_event` in arrival order.
    pub fn new(runtime: BrowserRuntime, dom_state: Arc<DomState>) -> Self {
        Self::new_with_targets(runtime, dom_state, None)
    }

    /// Phase 5: same as `new`, but also forwards every ordered event to
    /// `target_manager.on_event` in the same wire order (I9). The dispatcher
    /// still trusts Phase 1's ordering — it does not re-sort (rule 28).
    pub fn new_with_targets(
        runtime: BrowserRuntime,
        dom_state: Arc<DomState>,
        target_manager: Option<Arc<BrowserTargetManager>>,
    ) -> Self {
        Self::new_with_all(runtime, dom_state, target_manager, None)
    }

    /// Phase 6: also forwards to `FrameManager` (frame_tree_version per ANY frame).
    pub fn new_with_all(
        runtime: BrowserRuntime,
        dom_state: Arc<DomState>,
        target_manager: Option<Arc<BrowserTargetManager>>,
        frame_manager: Option<Arc<FrameManager>>,
    ) -> Self {
        Self::new_with_diff(runtime, dom_state, target_manager, frame_manager, None)
    }

    /// Phase 12: also forwards to `DomDiffEngine` for incremental DOM updates.
    pub fn new_with_diff(
        runtime: BrowserRuntime,
        dom_state: Arc<DomState>,
        target_manager: Option<Arc<BrowserTargetManager>>,
        frame_manager: Option<Arc<FrameManager>>,
        dom_diff: Option<Arc<DomDiffEngine>>,
    ) -> Self {
        let rx = runtime.subscribe();
        let ds = dom_state.clone();
        let tm = target_manager.clone();
        let fm = frame_manager.clone();
        let dd = dom_diff.clone();
        let rt = runtime.clone();
        let task = tokio::spawn(dispatch_loop(rx, ds, tm, fm, dd, rt));
        Self {
            runtime,
            dom_state,
            target_manager,
            frame_manager,
            dom_diff: dom_diff.clone(),
            task: Some(task),
        }
    }

    /// Ordered event subscription (passthrough to the runtime's broadcast).
    /// Liftable waiters use this; they must implement `check → subscribe →
    /// re-check → await` themselves — the dispatcher never re-orders.
    pub fn subscribe(&self) -> broadcast::Receiver<CdpEvent> {
        self.runtime.subscribe()
    }

    pub fn dom_state(&self) -> &Arc<DomState> {
        &self.dom_state
    }

    pub fn target_manager(&self) -> Option<&Arc<BrowserTargetManager>> {
        self.target_manager.as_ref()
    }

    pub fn frame_manager(&self) -> Option<&Arc<FrameManager>> {
        self.frame_manager.as_ref()
    }

    pub fn dom_diff(&self) -> Option<&Arc<DomDiffEngine>> {
        self.dom_diff.as_ref()
    }

    pub fn runtime(&self) -> &BrowserRuntime {
        &self.runtime
    }

    /// Enable required CDP domains on every attached session. Called on
    /// connect and reconnect (plan §"On connect and reconnect: Page.enable,
    /// DOM.enable, Runtime.enable per attached session").
    ///
    /// Best-effort per session: a single-vanishing-target failure does not
    /// fail the whole batch (same philosophy as `initialize`'s attach loop).
    ///
    /// NOTE: this only covers sessions known at call time. Sessions attached
    /// later are covered by the `Target.attachedToTarget` hook in
    /// `dispatch_loop` → `enable_new_session` (Phase 16.1 Task 2).
    pub async fn enable_domains(&self) -> RuntimeResult<()> {
        use std::collections::HashSet;
        // Include the browser session itself (None) once, then each flat session.
        // Page/DOM/Runtime enable are idempotent; duplicate enables are harmless.
        // DOM.getDocument is included per flat session (not the browser session):
        // without it the browser emits no DOM.childNodeInserted/Removed/
        // attributeModified for the session, so dom_version never advances on
        // real mutations (Phase 16 Known Issue #1). Sessions attached later
        // are covered by enable_new_session via the attachedToTarget hook.
        //
        // Reconcile loop (Phase 16.1 Task 2): the initial attach burst is read
        // off the socket asynchronously, so sessions may still be arriving while
        // this runs. Re-snapshot until no unhandled session remains (bounded).
        let empty = || Value::Object(serde_json::Map::new());
        let mut handled: HashSet<String> = HashSet::new();
        // Browser-level session (None): domains only, no document.
        for (domain, method) in [
            ("Page", "Page.enable"),
            ("DOM", "DOM.enable"),
            ("Runtime", "Runtime.enable"),
        ] {
            let res = self.runtime.call(None, method, empty()).await;
            match res {
                Ok(_) => tracing::debug!(domain, "browser-session domain enabled"),
                Err(e) => tracing::warn!(domain, error = %e, "browser-session domain enable failed; continuing"),
            }
        }
        for _ in 0..5 {
            let sessions = self.runtime.diagnostics().attached_session_ids;
            let fresh: Vec<String> = sessions
                .into_iter()
                .filter(|s| !handled.contains(s))
                .collect();
            if fresh.is_empty() {
                break;
            }
            for sid in fresh {
                self.enable_session(&sid).await;
                handled.insert(sid);
            }
        }
        Ok(())
    }

    /// Enable domains + fetch the document for one newly attached session.
    /// Thin wrapper over the free [`enable_new_session`] used by the
    /// `Target.attachedToTarget` hook below.
    pub async fn enable_session(&self, session_id: &str) {
        enable_new_session(&self.runtime, session_id).await;
    }

    /// Liftable wait primitive — now delegated to the single engine in `wait.rs`.
    /// Exactly one `check/subscribe/re-check/await` loop exists (in `wait::wait_for`);
    /// this is a thin adapter that forwards the runtime's ordered broadcast.
    pub async fn wait_for<F, P>(
        &self,
        check: F,
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
            || self.runtime.subscribe(),
            predicate,
            timeout,
        )
        .await
    }

    /// Wait until `Page.lifecycleEvent` for `desired` (e.g. "load") is observed.
    /// Phase 9: reimplemented on top of the condition engine's `Lifecycle` — no duplicate loop.
    pub async fn wait_for_lifecycle(&self, desired: &str, timeout: Duration) -> RuntimeResult<()> {
        let ds = self.dom_state.clone();
        let runtime = self.runtime.clone();
        // Build a temporary WaitEngine for the lifecycle condition without needing
        // TargetManager/FrameManager/ElementIndex — lifecycle only needs DomState.
        // We still route through `wait::wait_for_lifecycle_state` which uses the
        // single primitive.
        crate::browser_runtime::wait::wait_for_lifecycle_state(
            ds,
            move || runtime.subscribe(),
            desired,
            timeout,
        )
        .await
    }

    /// Wait until any navigation reaches `load` (alias for `wait_for_lifecycle("load")`).
    pub async fn wait_for_navigation(&self, timeout: Duration) -> RuntimeResult<()> {
        self.wait_for_lifecycle("load", timeout).await
    }

    pub fn abort(&mut self) {
        if let Some(h) = self.task.take() {
            h.abort();
        }
    }
}

impl Drop for EventDispatcher {
    fn drop(&mut self) {
        if let Some(h) = self.task.take() {
            h.abort();
        }
    }
}

async fn dispatch_loop(
    mut rx: broadcast::Receiver<CdpEvent>,
    dom_state: Arc<DomState>,
    target_manager: Option<Arc<BrowserTargetManager>>,
    frame_manager: Option<Arc<FrameManager>>,
    dom_diff: Option<Arc<DomDiffEngine>>,
    runtime: BrowserRuntime,
) {
    loop {
        match rx.recv().await {
            Ok(ev) => {
                // Wire order: capture versions before DomState bump (I9), then DomState → Target → Frame → DomDiff.
                let dom_before = dom_state.dom_version();
                let eiv_before = dom_state.element_index_version();
                dom_state.on_event(&ev);
                if let Some(tm) = &target_manager {
                    tm.on_event(&ev);
                }
                if let Some(fm) = &frame_manager {
                    fm.on_event(&ev);
                }
                if let Some(dd) = &dom_diff {
                    dd.on_event_with_pre(&ev, dom_before, eiv_before);
                }
                // Phase 16.1 Task 2: every newly attached flat session needs
                // its own Page/DOM/Runtime enable + DOM.getDocument, otherwise
                // DOM mutation events (childNodeInserted/Removed/...) never
                // fire for that session. Detached task so the ordered loop
                // never blocks on these CDP round-trips (I9/I22); the calls
                // themselves are sequential per session inside
                // `enable_new_session`.
                if ev.method == "Target.attachedToTarget" {
                    if let Some(sid) = ev
                        .params
                        .get("sessionId")
                        .and_then(Value::as_str)
                    {
                        let sid = sid.to_string();
                        let rt = runtime.clone();
                        tokio::spawn(async move {
                            enable_new_session(&rt, &sid).await;
                        });
                    }
                }
                // Phase 16.1 Task 2 (part 2): a navigation replaces the
                // document that DOM.getDocument subscribed the session to
                // (the browser emits DOM.documentUpdated, ×2 per navigation
                // in live probing). Until getDocument is re-issued, descendant
                // mutations on the new document emit NOTHING — verified live:
                // mutate-after-nav with no re-get → 0 events; with re-get →
                // childNodeInserted fires. So re-fetch the full tree on every
                // documentUpdated for that event's own session. getDocument is
                // a pure query (emits no events itself), so this cannot loop.
                // Detached task, same ordering rationale as above.
                if ev.method == "DOM.documentUpdated" {
                    if let Some(sid) = ev.session_id.clone() {
                        let rt = runtime.clone();
                        tokio::spawn(async move {
                            match rt
                                .call(
                                    Some(&sid),
                                    "DOM.getDocument",
                                    serde_json::json!({"depth": -1, "pierce": true}),
                                )
                                .await
                            {
                                Ok(_) => tracing::debug!(session_id = %sid, "documentUpdated: re-fetched document"),
                                Err(e) => tracing::warn!(session_id = %sid, error = %e, "documentUpdated: re-fetch failed; continuing"),
                            }
                        });
                    }
                }
            }
            Err(broadcast::error::RecvError::Lagged(n)) => {
                tracing::warn!(lagged = n, "EventDispatcher lagged — some events dropped; state may be stale; DomState will self-heal on next full invalidation");
            }
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
}

// ---------------------------------------------------------------------------
// Per-session domain enable (Phase 16.1 Task 2)
// ---------------------------------------------------------------------------

/// Enable `Page`/`DOM`/`Runtime` and fetch the document for ONE newly
/// attached flat session, sequentially and best-effort.
///
/// Background: `enable_domains` runs only at daemon connect/reconnect, so
/// every target attached later via `Target.attachedToTarget` (the first
/// page target at startup included) never received these calls — and
/// `DOM.getDocument` was never issued at all. Without them the browser
/// emits no `DOM.childNodeInserted/Removed/attributeModified` events for
/// the session, so `dom_version` never advances on real DOM mutations
/// (Phase 16 Known Issue #1: dom 72→72, events 345→345 after a real
/// node insertion).
///
/// Best-effort: attach races with target teardown (popup closed before we
/// enable, non-page targets that reject Page.enable) must warn, never
/// panic and never fail the dispatcher.
pub async fn enable_new_session(runtime: &BrowserRuntime, session_id: &str) {
    let empty = || Value::Object(serde_json::Map::new());
    for (domain, method) in [
        ("Page", "Page.enable"),
        ("DOM", "DOM.enable"),
        ("Runtime", "Runtime.enable"),
    ] {
        match runtime.call(Some(session_id), method, empty()).await {
            Ok(_) => tracing::debug!(session_id, domain, "new-session domain enabled"),
            Err(e) => tracing::warn!(session_id, domain, error = %e, "new-session domain enable failed; continuing"),
        }
    }
    // Gives the browser a document tree to track mutations against for this
    // session; without it DOM.childNodeInserted never fires here. Must be a
    // FULL-tree request (depth -1): an empty-params getDocument does NOT
    // subscribe the session to descendant mutation events (verified live:
    // empty params → 0 events on real insertion; depth -1 → childNodeInserted
    // fires). Pierce covers open shadow roots, matching element_index.
    match runtime
        .call(
            Some(session_id),
            "DOM.getDocument",
            serde_json::json!({"depth": -1, "pierce": true}),
        )
        .await
    {
        Ok(_) => tracing::debug!(session_id, "new-session DOM.getDocument ok"),
        Err(e) => tracing::warn!(session_id, error = %e, "new-session DOM.getDocument failed; continuing"),
    }
}

// ---------------------------------------------------------------------------
// Free liftable primitive — now delegated to the single engine in wait.rs
// ---------------------------------------------------------------------------

/// Free liftable waiter — thin wrapper over the single `wait::wait_for`
/// primitive. Preserves the original `wait_for_condition` error mapping
/// (already-satisfied → `InvalidResponse`) while ensuring exactly one loop
/// exists codebase-wide.
pub async fn wait_for_condition<F, P>(
    check: F,
    subscribe: impl Fn() -> broadcast::Receiver<CdpEvent>,
    predicate: P,
    timeout: Duration,
) -> RuntimeResult<CdpEvent>
where
    F: Fn() -> bool,
    P: Fn(&CdpEvent) -> bool,
{
    let res = crate::browser_runtime::wait::wait_for(
        move || {
            let v = check();
            async move { v }
        },
        subscribe,
        predicate,
        timeout,
    )
    .await?;
    match res {
        Some(ev) => Ok(ev),
        None => Err(RuntimeError::InvalidResponse(
            "wait_for_condition: already satisfied — caller should handle fast path without waiting".to_string(),
        )),
    }
}
