//! Phase 5 — Target and tab manager.
//!
//! `BrowserTargetManager` is a module **owned by** `BrowserRuntime` (rule 26).
//! It may **read** the runtime's current `target_generation` / `connection_generation`
//! (via `DomState`) but **never** bumps its own separate counter — every
//! increment goes through `BrowserRuntime`'s state-owner API
//! (`DomState::bump_target_generation` / `bump_connection_generation` as
//! delegated by `BrowserRuntimeServer`). This guarantees a single
//! authoritative generation source (I12).
//!
//! Only `BrowserRuntime`'s transport (`connection.rs`) talks to Chromium
//! (rule 27). Switching target only changes active-session state — it never
//! creates a WebSocket (I1/I24). Popup discovery is event-driven via
//! `Target.setAutoAttach {flatten:true}` (Phase 1) + `Target.targetCreated` /
//! `attachedToTarget` events, never `/json` polling.
//!
//! Generations (§3 Identity Model):
//! ```text
//! TargetRef  { target_id,  target_generation }
//! SessionRef { session_id, connection_generation }
//! ```
//! `target_generation` increments on every reconnect-rediscovery.
//! `connection_generation` increments on every WebSocket re-establish
//! (`DomState::runtime_generation` is its authoritative store).
//! A ref whose generation doesn't match current is fully invalid, even if the
//! raw ID string collides.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::browser_runtime::connection::{BrowserRuntime, CdpEvent};
use crate::browser_runtime::dom_state::DomState;
use crate::browser_runtime::error::{RuntimeError, RuntimeResult};

// ---------------------------------------------------------------------------
// Identity refs (§3)
// ---------------------------------------------------------------------------

/// Stable reference to a target, bound to the generation it was observed at.
/// Using a `TargetRef` whose `target_generation` is stale must be rejected,
/// regardless of whether the raw `target_id` string still exists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetRef {
    pub target_id: String,
    pub target_generation: u64,
}

impl TargetRef {
    pub fn new(target_id: impl Into<String>, target_generation: u64) -> Self {
        Self {
            target_id: target_id.into(),
            target_generation,
        }
    }
    /// `true` iff `current_generation` matches the generation this ref was
    /// captured at. A `false` result means the ref is fully invalid (I12).
    pub fn is_valid(&self, current_generation: u64) -> bool {
        self.target_generation == current_generation
    }
}

/// Stable reference to a flat session, bound to the connection generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionRef {
    pub session_id: String,
    pub connection_generation: u64,
}

impl SessionRef {
    pub fn new(session_id: impl Into<String>, connection_generation: u64) -> Self {
        Self {
            session_id: session_id.into(),
            connection_generation,
        }
    }
    pub fn is_valid(&self, current_generation: u64) -> bool {
        self.connection_generation == current_generation
    }
}

// ---------------------------------------------------------------------------
// Target lifecycle + record
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TargetLifecycle {
    Created,
    Attached,
    Detached,
    Destroyed,
    Crashed,
    Navigated,
}

impl TargetLifecycle {
    pub fn as_str(&self) -> &'static str {
        match self {
            TargetLifecycle::Created => "created",
            TargetLifecycle::Attached => "attached",
            TargetLifecycle::Detached => "detached",
            TargetLifecycle::Destroyed => "destroyed",
            TargetLifecycle::Crashed => "crashed",
            TargetLifecycle::Navigated => "navigated",
        }
    }
}

/// Authoritative per-target state (Phase 5 DoD shape).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TargetRecord {
    pub target_id: String,
    pub target_generation: u64,
    pub session_id: Option<String>,
    pub connection_generation: u64,
    pub url: String,
    pub title: String,
    pub target_type: String,
    pub active: bool,
    pub lifecycle: TargetLifecycle,
    pub created_at: SystemTime,
    pub crashed: bool,
}

impl TargetRecord {
    pub fn target_ref(&self) -> TargetRef {
        TargetRef::new(self.target_id.clone(), self.target_generation)
    }
    pub fn session_ref(&self) -> Option<SessionRef> {
        self.session_id
            .as_ref()
            .map(|s| SessionRef::new(s.clone(), self.connection_generation))
    }
}

// ---------------------------------------------------------------------------
// BrowserTargetManager
// ---------------------------------------------------------------------------

/// Owned target manager (rule 26). Single owner is `BrowserRuntimeServer` which
/// holds `Arc<BrowserTargetManager>` and forwards ordered `CdpEvent`s in wire
/// order (downstream of Phase 1's reorder buffer, I9).
pub struct BrowserTargetManager {
    targets: RwLock<HashMap<String, TargetRecord>>,
    session_to_target: RwLock<HashMap<String, String>>,
    active_target_id: RwLock<Option<String>>,
    /// Read-only view of authoritative generations. The manager **reads**
    /// `target_generation` / `connection_generation` from here but **never**
    /// increments them directly — increments are delegated to the
    /// `BrowserRuntime`-owned `DomState` via `BrowserRuntimeServer`'s
    /// `bump_*` API (rule 26).
    dom_state: Arc<DomState>,
    /// Optional runtime handle for `attach_target`/`detach_target` CDP calls.
    /// `None` in unit tests that only exercise event-driven state.
    runtime: RwLock<Option<BrowserRuntime>>,
}

impl std::fmt::Debug for BrowserTargetManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BrowserTargetManager")
            .field("targets", &self.targets.read().map(|g| g.len()).unwrap_or(0))
            .field("active", &self.active_target_id.read().ok().and_then(|g| g.clone()))
            .finish_non_exhaustive()
    }
}

impl BrowserTargetManager {
    /// Create a manager owned by `BrowserRuntimeServer`. `dom_state` is the
    /// single authoritative generation store (rule 26). `runtime` may be `None`
    /// in tests.
    pub fn new(dom_state: Arc<DomState>, runtime: Option<BrowserRuntime>) -> Arc<Self> {
        Arc::new(Self {
            targets: RwLock::new(HashMap::new()),
            session_to_target: RwLock::new(HashMap::new()),
            active_target_id: RwLock::new(None),
            dom_state,
            runtime: RwLock::new(runtime),
        })
    }

    /// Attach a live `BrowserRuntime` after construction (lets `server::serve`
    /// create the manager before the WebSocket exists).
    pub fn set_runtime(&self, rt: BrowserRuntime) {
        if let Ok(mut g) = self.runtime.write() {
            *g = Some(rt);
        }
    }

    fn current_target_generation(&self) -> u64 {
        self.dom_state.target_generation()
    }
    fn current_connection_generation(&self) -> u64 {
        self.dom_state.runtime_generation()
    }

    // -- event application (strict wire order caller guarantees I9) ----------

    /// Apply one ordered `CdpEvent`. Must be called in `sequence` order —
    /// caller is the ordered pipeline (Phase 1 reorder buffer → dispatcher).
    /// Out-of-order arrival is a Phase 1 bug, not masked here.
    pub fn on_event(&self, ev: &CdpEvent) {
        match ev.method.as_str() {
            "Target.targetCreated" => self.on_target_created(ev),
            "Target.targetDestroyed" => self.on_target_destroyed(ev),
            "Target.targetCrashed" => self.on_target_crashed(ev),
            "Target.attachedToTarget" => self.on_attached(ev),
            "Target.detachedFromTarget" => self.on_detached(ev),
            "Page.frameNavigated" => self.on_frame_navigated(ev),
            _ => {}
        }
    }

    fn on_target_created(&self, ev: &CdpEvent) {
        // CDP shape: { targetInfo: { targetId, type, url, title, ... } }
        let info = ev
            .params
            .get("targetInfo")
            .cloned()
            .unwrap_or(Value::Null);
        let target_id = info
            .get("targetId")
            .and_then(Value::as_str)
            .or_else(|| ev.params.get("targetId").and_then(Value::as_str))
            .unwrap_or("")
            .to_string();
        if target_id.is_empty() {
            return;
        }
        let target_type = info
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("page")
            .to_string();
        let url = info.get("url").and_then(Value::as_str).unwrap_or("").to_string();
        let title = info
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let cur_tg = self.current_target_generation();
        let cur_cg = self.current_connection_generation();
        let mut g = match self.targets.write() {
            Ok(g) => g,
            Err(_) => return,
        };
        g.entry(target_id.clone())
            .and_modify(|rec| {
                // Rediscovery (e.g. after reconnect): refresh url/title/type but
                // keep generation pinned until `handle_reconnect` bumps it — the
                // bump is owned by BrowserRuntime, not by this event.
                rec.url = url.clone();
                rec.title = title.clone();
                rec.target_type = target_type.clone();
                // If the target was previously destroyed, resurrect as created.
                if rec.lifecycle == TargetLifecycle::Destroyed {
                    rec.lifecycle = TargetLifecycle::Created;
                    rec.crashed = false;
                    rec.target_generation = cur_tg;
                    rec.connection_generation = cur_cg;
                }
            })
            .or_insert_with(|| TargetRecord {
                target_id: target_id.clone(),
                target_generation: cur_tg,
                session_id: None,
                connection_generation: cur_cg,
                url,
                title,
                target_type,
                active: false,
                lifecycle: TargetLifecycle::Created,
                created_at: SystemTime::now(),
                crashed: false,
            });
        // First target becomes active by default if none active.
        if let Ok(mut active) = self.active_target_id.write() {
            if active.is_none() {
                *active = Some(target_id.clone());
                // Mark active flag
                if let Some(rec) = g.get_mut(active.as_ref().unwrap()) {
                    rec.active = true;
                }
            }
        }
        tracing::debug!(target_id = %target_id, sequence = ev.sequence, "Target.targetCreated");
    }

    fn on_target_destroyed(&self, ev: &CdpEvent) {
        let target_id = ev
            .params
            .get("targetId")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if target_id.is_empty() {
            return;
        }
        // If the event carries a nested targetInfo, prefer it (some versions do).
        // Remove session mapping for any session that was attached to this target.
        let mut targets = match self.targets.write() {
            Ok(g) => g,
            Err(_) => return,
        };
        if let Some(rec) = targets.get_mut(&target_id) {
            rec.lifecycle = TargetLifecycle::Destroyed;
            rec.active = false;
            if let Some(sid) = rec.session_id.take() {
                if let Ok(mut m) = self.session_to_target.write() {
                    m.remove(&sid);
                }
            }
        }
        // Drop the record entirely after marking — list_targets reflects live set.
        // Keep the removal here so popup destruction is observable as count going
        // from 2 → 1 (DoD: popup.html two-target test, then close).
        targets.remove(&target_id);
        if let Ok(mut active) = self.active_target_id.write() {
            if active.as_deref() == Some(&target_id) {
                *active = None;
                // Promote another target to active if any remain.
                if let Some(next) = targets.keys().next().cloned() {
                    *active = Some(next.clone());
                    if let Some(rec) = targets.get_mut(&next) {
                        rec.active = true;
                    }
                }
            }
        }
        // Also clear any session mapping that pointed at this target (already handled via record's session_id above,
        // but handle detached-then-destroyed case where session mapping outlives record).
        if let Ok(mut m) = self.session_to_target.write() {
            m.retain(|_, tid| tid != &target_id);
        }
        tracing::debug!(target_id = %target_id, sequence = ev.sequence, "Target.targetDestroyed");
    }

    fn on_target_crashed(&self, ev: &CdpEvent) {
        let target_id = ev
            .params
            .get("targetId")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if target_id.is_empty() {
            return;
        }
        if let Ok(mut g) = self.targets.write() {
            if let Some(rec) = g.get_mut(&target_id) {
                rec.crashed = true;
                rec.lifecycle = TargetLifecycle::Crashed;
                rec.active = false;
            } else {
                // Unknown target crashed: insert a tombstone so callers can observe it.
                let cur_tg = self.current_target_generation();
                let cur_cg = self.current_connection_generation();
                g.insert(
                    target_id.clone(),
                    TargetRecord {
                        target_id: target_id.clone(),
                        target_generation: cur_tg,
                        session_id: None,
                        connection_generation: cur_cg,
                        url: String::new(),
                        title: String::new(),
                        target_type: "page".to_string(),
                        active: false,
                        lifecycle: TargetLifecycle::Crashed,
                        created_at: SystemTime::now(),
                        crashed: true,
                    },
                );
            }
        }
        if let Ok(mut active) = self.active_target_id.write() {
            if active.as_deref() == Some(&target_id) {
                *active = None;
            }
        }
        tracing::warn!(target_id = %target_id, sequence = ev.sequence, "Target.targetCrashed");
    }

    fn on_attached(&self, ev: &CdpEvent) {
        let session_id = ev
            .params
            .get("sessionId")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if session_id.is_empty() {
            return;
        }
        let target_id = ev
            .params
            .get("targetInfo")
            .and_then(|v| v.get("targetId"))
            .and_then(Value::as_str)
            .or_else(|| ev.params.get("targetId").and_then(Value::as_str))
            .unwrap_or("")
            .to_string();
        let cur_cg = self.current_connection_generation();
        if !target_id.is_empty() {
            // Ensure record exists (targetCreated may have raced ahead or been missed pre-attach).
            {
                let mut g = match self.targets.write() {
                    Ok(g) => g,
                    Err(_) => return,
                };
                let rec = g.entry(target_id.clone()).or_insert_with(|| TargetRecord {
                    target_id: target_id.clone(),
                    target_generation: self.current_target_generation(),
                    session_id: None,
                    connection_generation: cur_cg,
                    url: ev
                        .params
                        .get("targetInfo")
                        .and_then(|v| v.get("url"))
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    title: ev
                        .params
                        .get("targetInfo")
                        .and_then(|v| v.get("title"))
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    target_type: ev
                        .params
                        .get("targetInfo")
                        .and_then(|v| v.get("type"))
                        .and_then(Value::as_str)
                        .unwrap_or("page")
                        .to_string(),
                    active: false,
                    lifecycle: TargetLifecycle::Created,
                    created_at: SystemTime::now(),
                    crashed: false,
                });
                rec.session_id = Some(session_id.clone());
                rec.connection_generation = cur_cg;
                rec.lifecycle = TargetLifecycle::Attached;
            }
            if let Ok(mut m) = self.session_to_target.write() {
                m.insert(session_id.clone(), target_id.clone());
            }
            // Auto-attach implies active if no active yet, or if this is a new popup
            // it becomes discoverable via list_targets; don't force-switch active —
            // switching is explicit via switch_target (DoD: "switching target only
            // changes active session state").
            if let Ok(active) = self.active_target_id.read() {
                if active.is_none() {
                    drop(active);
                    if let Ok(mut a) = self.active_target_id.write() {
                        *a = Some(target_id.clone());
                    }
                    if let Ok(mut g) = self.targets.write() {
                        if let Some(rec) = g.get_mut(&target_id) {
                            rec.active = true;
                        }
                    }
                }
            }
        } else {
            // Session without targetInfo — still track mapping if we can infer later via frameNavigated.
            // Store session but don't create record without target_id.
            tracing::debug!(session_id = %session_id, "Target.attachedToTarget without targetInfo");
        }
        tracing::debug!(session_id = %session_id, target_id = %target_id, sequence = ev.sequence, "Target.attachedToTarget");
    }

    fn on_detached(&self, ev: &CdpEvent) {
        let session_id = ev
            .params
            .get("sessionId")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if session_id.is_empty() {
            return;
        }
        let target_id_opt = self
            .session_to_target
            .read()
            .ok()
            .and_then(|m| m.get(&session_id).cloned())
            .or_else(|| {
                ev.params
                    .get("targetId")
                    .and_then(Value::as_str)
                    .map(|s| s.to_string())
            });
        if let Some(target_id) = target_id_opt {
            if let Ok(mut g) = self.targets.write() {
                if let Some(rec) = g.get_mut(&target_id) {
                    // Only clear if this session matches the record's session
                    if rec.session_id.as_deref() == Some(&session_id) {
                        rec.session_id = None;
                        rec.lifecycle = TargetLifecycle::Detached;
                        rec.active = false;
                    }
                }
            }
            if let Ok(mut m) = self.session_to_target.write() {
                m.remove(&session_id);
            }
            // If detached target was active, clear active and promote another.
            let should_promote = self
                .active_target_id
                .read()
                .ok()
                .and_then(|a| a.clone())
                .as_deref()
                == Some(target_id.as_str());
            if should_promote {
                if let Ok(mut active) = self.active_target_id.write() {
                    *active = None;
                    // Promote first remaining attached target
                    if let Ok(g) = self.targets.read() {
                        if let Some((tid, _)) = g.iter().find(|(_, r)| r.session_id.is_some()) {
                            *active = Some(tid.clone());
                        } else if let Some(tid) = g.keys().next().cloned() {
                            *active = Some(tid.clone());
                        }
                    }
                }
                // Mark new active's flag
                if let Ok(active) = self.active_target_id.read() {
                    if let Some(tid) = active.clone() {
                        if let Ok(mut g) = self.targets.write() {
                            if let Some(rec) = g.get_mut(&tid) {
                                rec.active = true;
                            }
                        }
                    }
                }
            }
        } else {
            // No mapping — just remove session key if present
            if let Ok(mut m) = self.session_to_target.write() {
                m.remove(&session_id);
            }
        }
        tracing::debug!(session_id = %session_id, sequence = ev.sequence, "Target.detachedFromTarget");
    }

    fn on_frame_navigated(&self, ev: &CdpEvent) {
        // Session-scoped: find target via session_id, update url/title on main frame.
        let session_id = ev.session_id.clone();
        let frame = ev.params.get("frame");
        let is_main = frame
            .and_then(|f| f.get("parentId"))
            .is_none();
        if !is_main {
            return;
        }
        let url = frame
            .and_then(|f| f.get("url"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let title_like = frame
            .and_then(|f| f.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        // Prefer explicit title fallback: the event doesn't always carry title; keep existing.
        let target_id_opt = session_id
            .as_deref()
            .and_then(|sid| {
                self.session_to_target
                    .read()
                    .ok()
                    .and_then(|m| m.get(sid).cloned())
            });
        if let Some(target_id) = target_id_opt {
            if let Ok(mut g) = self.targets.write() {
                if let Some(rec) = g.get_mut(&target_id) {
                    if !url.is_empty() {
                        rec.url = url;
                    }
                    if !title_like.is_empty() {
                        rec.title = title_like;
                    }
                    // Lifecycle navigated — not Drawn as attached/detached state, but track for diagnostics.
                    // Keep active/active logic unchanged.
                    if rec.lifecycle != TargetLifecycle::Crashed
                        && rec.lifecycle != TargetLifecycle::Destroyed
                    {
                        rec.lifecycle = TargetLifecycle::Navigated;
                    }
                }
            }
            tracing::debug!(target_id = %target_id, sequence = ev.sequence, "Page.frameNavigated");
        } else if ev.session_id.is_none() {
            // Browser-level frameNavigated without session — apply to active target if single.
            if let Ok(active) = self.active_target_id.read() {
                if let Some(tid) = active.clone() {
                    if let Ok(mut g) = self.targets.write() {
                        if let Some(rec) = g.get_mut(&tid) {
                            if !url.is_empty() {
                                rec.url = url;
                            }
                        }
                    }
                }
            }
        }
    }

    // -- public API -----------------------------------------------------------

    /// All live targets (excludes `Destroyed`; includes `Crashed` as tombstones
    /// so callers can observe `crashed=true`).
    pub fn list_targets(&self) -> Vec<TargetRecord> {
        self.targets
            .read()
            .map(|g| g.values().cloned().collect())
            .unwrap_or_default()
    }

    /// Number of live targets (same set as `list_targets`).
    pub fn target_count(&self) -> usize {
        self.targets.read().map(|g| g.len()).unwrap_or(0)
    }

    pub fn active_target(&self) -> Option<TargetRecord> {
        let tid = self.active_target_id.read().ok().and_then(|g| g.clone())?;
        self.targets.read().ok().and_then(|g| g.get(&tid).cloned())
    }

    pub fn get_target(&self, target_id: &str) -> Option<TargetRecord> {
        self.targets.read().ok().and_then(|g| g.get(target_id).cloned())
    }

    /// Validate a `TargetRef` against current `target_generation` (I12).
    /// `true` iff the ref's generation matches the runtime's current generation
    /// AND the target still exists.
    pub fn is_target_ref_valid(&self, r: &TargetRef) -> bool {
        if !r.is_valid(self.current_target_generation()) {
            return false;
        }
        self.targets
            .read()
            .map(|g| g.contains_key(&r.target_id))
            .unwrap_or(false)
    }

    /// Validate a `SessionRef` against current `connection_generation` (I12).
    pub fn is_session_ref_valid(&self, r: &SessionRef) -> bool {
        if !r.is_valid(self.current_connection_generation()) {
            return false;
        }
        self.session_to_target
            .read()
            .map(|g| g.contains_key(&r.session_id))
            .unwrap_or(false)
    }

    /// Check a `SessionRef` and return a structured error if stale (I12).
    /// This exercises `SessionRef` validation and removes the dead-code warning
    /// by routing raw session strings through `SessionRef` + `is_session_ref_valid`.
    /// Phase 5 enforces minimal validation; full capability enforcement is Phase 11.
    pub fn check_session_ref(&self, r: &SessionRef) -> RuntimeResult<()> {
        if !r.is_valid(self.current_connection_generation()) {
            return Err(RuntimeError::InvalidResponse(format!(
                "stale SessionRef: session {} generation {} != current {}",
                r.session_id, r.connection_generation, self.current_connection_generation()
            )));
        }
        if self
            .session_to_target
            .read()
            .map(|g| !g.contains_key(&r.session_id))
            .unwrap_or(true)
        {
            return Err(RuntimeError::InvalidResponse(format!(
                "unknown session {}",
                r.session_id
            )));
        }
        Ok(())
    }

    /// Validate a raw `session_id` string via `SessionRef` path (I12).
    /// Looks up the stored generation for the session (via `TargetRecord` or
    /// `session_to_target` mapping) and validates through `check_session_ref`.
    /// This is the IPC-facing helper: any path accepting a raw `sessionId`
    /// must call this before dispatching.
    pub fn check_session_id(&self, session_id: &str) -> RuntimeResult<()> {
        // Resolve target_id for the session, then stored generation.
        let tid_opt = self
            .session_to_target
            .read()
            .ok()
            .and_then(|m| m.get(session_id).cloned());
        if let Some(tid) = tid_opt {
            if let Ok(g) = self.targets.read() {
                if let Some(rec) = g.get(&tid) {
                    if let Some(sid) = rec.session_id.as_deref() {
                        if sid == session_id {
                            let r = SessionRef::new(session_id.to_string(), rec.connection_generation);
                            return self.check_session_ref(&r);
                        }
                    }
                }
            }
        }
        // Fallback scan: handle edge where mapping not yet cleared but record holds session
        if let Ok(g) = self.targets.read() {
            for rec in g.values() {
                if rec.session_id.as_deref() == Some(session_id) {
                    let r = SessionRef::new(session_id.to_string(), rec.connection_generation);
                    return self.check_session_ref(&r);
                }
            }
        }
        // No mapping nor record — treat as stale/unknown. Construct a SessionRef
        // with current generation to exercise the validation path, then fail as unknown.
        let probe = SessionRef::new(session_id.to_string(), self.current_connection_generation());
        if !self.is_session_ref_valid(&probe) {
            return Err(RuntimeError::InvalidResponse(format!(
                "unknown session {}",
                session_id
            )));
        }
        // Should not reach here; probe valid means mapping exists but earlier branch missed (race)
        Err(RuntimeError::InvalidResponse(format!(
            "unknown session {}",
            session_id
        )))
    }

    /// Convenience bool wrapper for `check_session_id`.
    pub fn is_session_id_valid(&self, session_id: &str) -> bool {
        self.check_session_id(session_id).is_ok()
    }

    /// Check a `TargetRef` and return a structured error if stale (for APIs
    /// that accept a captured ref). Callers should prefer this over manual
    /// `is_valid` checks so the error shape is consistent.
    pub fn check_target_ref(&self, r: &TargetRef) -> RuntimeResult<()> {
        if !r.is_valid(self.current_target_generation()) {
            return Err(RuntimeError::InvalidResponse(format!(
                "stale TargetRef: target {} generation {} != current {}",
                r.target_id, r.target_generation, self.current_target_generation()
            )));
        }
        if self
            .targets
            .read()
            .map(|g| !g.contains_key(&r.target_id))
            .unwrap_or(true)
        {
            return Err(RuntimeError::InvalidResponse(format!(
                "unknown target {}",
                r.target_id
            )));
        }
        Ok(())
    }

    /// Switch active target. Only changes active-session state — never creates
    /// a WebSocket (I1/I24). Validates that the target exists and is not
    /// crashed/destroyed; generation check is via `check_target_ref` if caller
    /// supplies a ref, otherwise checks liveness directly.
    pub fn switch_target(&self, target_id: &str) -> RuntimeResult<TargetRecord> {
        let rec = {
            let g = self
                .targets
                .read()
                .map_err(|_| RuntimeError::InvalidResponse("target lock poisoned".to_string()))?;
            g.get(target_id).cloned().ok_or_else(|| {
                RuntimeError::InvalidResponse(format!("unknown target {target_id}"))
            })?
        };
        if rec.crashed {
            return Err(RuntimeError::InvalidResponse(format!(
                "target {target_id} is crashed"
            )));
        }
        if rec.lifecycle == TargetLifecycle::Destroyed {
            return Err(RuntimeError::InvalidResponse(format!(
                "target {target_id} is destroyed"
            )));
        }
        // Validate generation matches current (stale ref protection even when caller passes bare id:
        // if the record's own generation is behind current, it survived a reconnect without bump — bug).
        if rec.target_generation != self.current_target_generation() {
            return Err(RuntimeError::InvalidResponse(format!(
                "stale target generation: target {} gen {} != current {}",
                target_id, rec.target_generation, self.current_target_generation()
            )));
        }
        // Perform switch: clear old active flag, set new.
        {
            let mut active = self
                .active_target_id
                .write()
                .map_err(|_| RuntimeError::InvalidResponse("active lock poisoned".to_string()))?;
            let old = active.clone();
            *active = Some(target_id.to_string());
            // Update flags
            if let Ok(mut g) = self.targets.write() {
                if let Some(old_id) = old {
                    if let Some(r) = g.get_mut(&old_id) {
                        r.active = false;
                    }
                }
                if let Some(r) = g.get_mut(target_id) {
                    r.active = true;
                }
            }
        }
        self.get_target(target_id)
            .ok_or_else(|| RuntimeError::InvalidResponse(format!("target {target_id} vanished during switch")))
    }

    /// Switch via an explicit `TargetRef` — validates the ref's generation
    /// before switching (DoD: ref captured before reconnect is rejected).
    pub fn switch_target_ref(&self, r: &TargetRef) -> RuntimeResult<TargetRecord> {
        self.check_target_ref(r)?;
        self.switch_target(&r.target_id)
    }

    /// Attach to a target via the single browser-level WebSocket (flat session).
    /// Issues `Target.attachToTarget {flatten:true}` over the existing
    /// transport — never opens another WebSocket. The resulting
    /// `Target.attachedToTarget` event will populate `session_id`.
    pub async fn attach_target(&self, target_id: &str) -> RuntimeResult<Value> {
        let rt = self
            .runtime
            .read()
            .map_err(|_| RuntimeError::InvalidResponse("runtime lock poisoned".to_string()))?
            .clone()
            .ok_or_else(|| RuntimeError::RuntimeDead("no browser connection for attach".to_string()))?;
        if !rt.is_alive() {
            return Err(RuntimeError::RuntimeDead("browser connection is dead".to_string()));
        }
        rt.call(
            None,
            "Target.attachToTarget",
            serde_json::json!({ "targetId": target_id, "flatten": true }),
        )
        .await
    }

    /// Detach a session. Issues `Target.detachFromTarget`.
    /// Enforces `SessionRef` validation (I12) before CDP dispatch — stale or
    /// unknown sessions are rejected locally, never sent to the browser.
    pub async fn detach_target(&self, session_id: &str) -> RuntimeResult<Value> {
        self.check_session_id(session_id)?;
        let rt = self
            .runtime
            .read()
            .map_err(|_| RuntimeError::InvalidResponse("runtime lock poisoned".to_string()))?
            .clone()
            .ok_or_else(|| RuntimeError::RuntimeDead("no browser connection for detach".to_string()))?;
        if !rt.is_alive() {
            return Err(RuntimeError::RuntimeDead("browser connection is dead".to_string()));
        }
        rt.call(
            None,
            "Target.detachFromTarget",
            serde_json::json!({ "sessionId": session_id }),
        )
        .await
    }

    /// Seed or refresh from `Target.getTargets` result (handles existing
    /// targets at startup without relying on polling). Called once after
    /// connect, and after reconnect rediscovery.
    pub fn sync_from_target_infos(&self, target_infos: &[Value]) {
        let cur_tg = self.current_target_generation();
        let cur_cg = self.current_connection_generation();
        let mut g = match self.targets.write() {
            Ok(g) => g,
            Err(_) => return,
        };
        for info in target_infos {
            let tid = match info.get("targetId").and_then(Value::as_str) {
                Some(s) => s.to_string(),
                None => continue,
            };
            let ttype = info.get("type").and_then(Value::as_str).unwrap_or("page").to_string();
            let url = info.get("url").and_then(Value::as_str).unwrap_or("").to_string();
            let title = info.get("title").and_then(Value::as_str).unwrap_or("").to_string();
            g.entry(tid.clone())
                .and_modify(|rec| {
                    rec.url = url.clone();
                    rec.title = title.clone();
                    rec.target_type = ttype.clone();
                })
                .or_insert_with(|| TargetRecord {
                    target_id: tid.clone(),
                    target_generation: cur_tg,
                    session_id: None,
                    connection_generation: cur_cg,
                    url,
                    title,
                    target_type: ttype,
                    active: false,
                    lifecycle: TargetLifecycle::Created,
                    created_at: SystemTime::now(),
                    crashed: false,
                });
        }
        // Ensure one active if none
        if self.active_target_id.read().map(|a| a.is_none()).unwrap_or(false) {
            if let Some(first) = g.keys().next().cloned() {
                if let Ok(mut a) = self.active_target_id.write() {
                    *a = Some(first.clone());
                }
                if let Some(rec) = g.get_mut(&first) {
                    rec.active = true;
                }
            }
        }
    }

    /// Handle browser restart / reconnect: every surviving record must adopt
    /// the new generations (none may silently keep the old). Sessions from the
    /// previous connection are invalidated (cleared) — they will be
    /// re-established via new `attachedToTarget` events in flat mode.
    ///
    /// Caller (BrowserRuntimeServer) is responsible for bumping the
    /// authoritative counters via `DomState::bump_*` (rule 26) and passing the
    /// new values here. This method **never** bumps counters itself.
    pub fn handle_reconnect(&self, new_target_generation: u64, new_connection_generation: u64) {
        if let Ok(mut g) = self.targets.write() {
            for rec in g.values_mut() {
                if rec.lifecycle == TargetLifecycle::Destroyed {
                    continue;
                }
                rec.target_generation = new_target_generation;
                rec.connection_generation = new_connection_generation;
                // Sessions from prior connection are dead (I12) — clear to force re-attach.
                rec.session_id = None;
                // Crashed targets stay crashed but with new generation (still invalid for use until rediscovered).
            }
        }
        if let Ok(mut m) = self.session_to_target.write() {
            m.clear();
        }
        // Active target cleared — caller will re-attach and re-select.
        if let Ok(mut a) = self.active_target_id.write() {
            *a = None;
        }
        // Re-promote first surviving target as active candidate (will be re-attached next).
        if let Ok(mut g) = self.targets.write() {
            if let Some(first) = g.keys().next().cloned() {
                if let Ok(mut a) = self.active_target_id.write() {
                    *a = Some(first.clone());
                }
                if let Some(rec) = g.get_mut(&first) {
                    rec.active = true;
                }
            }
        }
        tracing::info!(
            new_target_generation,
            new_connection_generation,
            "BrowserTargetManager reconnect: all surviving records bumped"
        );
    }

    /// Test/simulation helper: verify every surviving record has the expected
    /// `target_generation` — none silently kept the old one. Returns the count
    /// of mismatches (0 means pass).
    pub fn count_generation_mismatches(&self, expected: u64) -> usize {
        self.targets
            .read()
            .map(|g| {
                g.values()
                    .filter(|r| {
                        r.lifecycle != TargetLifecycle::Destroyed
                            && r.target_generation != expected
                    })
                    .count()
            })
            .unwrap_or(0)
    }

    /// Return trace snapshot for diagnostics / DoD demonstration.
    pub fn trace_snapshot(&self) -> Value {
        let targets = self.list_targets();
        serde_json::json!({
            "target_count": targets.len(),
            "targets": targets.iter().map(|r| serde_json::json!({
                "targetId": r.target_id,
                "sessionId": r.session_id,
                "target_generation": r.target_generation,
                "connection_generation": r.connection_generation,
                "url": r.url,
                "title": r.title,
                "type": r.target_type,
                "lifecycle": r.lifecycle.as_str(),
                "active": r.active,
                "crashed": r.crashed
            })).collect::<Vec<_>>(),
            "active_target": self.active_target().map(|r| r.target_id),
            "current_target_generation": self.current_target_generation(),
            "current_connection_generation": self.current_connection_generation(),
        })
    }
}

// ---------------------------------------------------------------------------
// Tests — generation bump + stale ref rejection (Phase 5 DoD)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser_runtime::connection::CdpEvent;
    use serde_json::json;
    use std::time::Instant;

    fn ev(method: &str, params: Value, session_id: Option<String>, sequence: u64) -> CdpEvent {
        CdpEvent {
            method: method.to_string(),
            params,
            session_id,
            sequence,
            timestamp: Instant::now(),
        }
    }

    fn ds_with_generations(tg: u64, cg: u64) -> Arc<DomState> {
        let ds = Arc::new(DomState::new());
        // Initialize generations to desired values
        // DomState starts at 0; bump to reach tg/cg
        for _ in 0..tg {
            ds.bump_target_generation();
        }
        for _ in 0..cg {
            ds.bump_connection_generation();
        }
        ds
    }

    #[test]
    fn target_created_and_attached_and_list() {
        let ds = ds_with_generations(1, 1);
        let mgr = BrowserTargetManager::new(ds.clone(), None);
        mgr.on_event(&ev(
            "Target.targetCreated",
            json!({"targetInfo": {"targetId": "t1", "type": "page", "url": "https://example.com", "title": "Example"}}),
            None,
            0,
        ));
        assert_eq!(mgr.target_count(), 1);
        mgr.on_event(&ev(
            "Target.attachedToTarget",
            json!({"sessionId": "s1", "targetInfo": {"targetId": "t1", "type": "page"}}),
            None,
            1,
        ));
        let rec = mgr.get_target("t1").unwrap();
        assert_eq!(rec.session_id.as_deref(), Some("s1"));
        assert_eq!(rec.target_generation, 1);
        assert_eq!(rec.connection_generation, 1);
        assert_eq!(mgr.list_targets().len(), 1);
        assert!(mgr.active_target().is_some());
    }

    #[test]
    fn popup_discovery_two_targets() {
        let ds = ds_with_generations(1, 1);
        let mgr = BrowserTargetManager::new(ds.clone(), None);
        // Existing target
        mgr.on_event(&ev(
            "Target.targetCreated",
            json!({"targetInfo": {"targetId": "main", "type": "page", "url": "https://example.com/popup.html", "title": "popup fixture"}}),
            None,
            0,
        ));
        mgr.on_event(&ev(
            "Target.attachedToTarget",
            json!({"sessionId": "s-main", "targetInfo": {"targetId": "main"}}),
            None,
            1,
        ));
        // Popup
        mgr.on_event(&ev(
            "Target.targetCreated",
            json!({"targetInfo": {"targetId": "popup", "type": "page", "url": "https://example.com/basic.html", "title": "basic"}}),
            None,
            2,
        ));
        mgr.on_event(&ev(
            "Target.attachedToTarget",
            json!({"sessionId": "s-popup", "targetInfo": {"targetId": "popup"}}),
            None,
            3,
        ));
        assert_eq!(mgr.target_count(), 2);
        // Trace shows targetCreated/attachedToTarget consumed — verified by count
        let snap = mgr.trace_snapshot();
        assert_eq!(snap["target_count"], 2);
    }

    #[test]
    fn popup_destruction_reduces_count() {
        let ds = ds_with_generations(1, 1);
        let mgr = BrowserTargetManager::new(ds, None);
        for tid in ["main", "popup"] {
            mgr.on_event(&ev(
                "Target.targetCreated",
                json!({"targetInfo": {"targetId": tid, "type": "page", "url": "", "title": ""}}),
                None,
                0,
            ));
            mgr.on_event(&ev(
                "Target.attachedToTarget",
                json!({"sessionId": format!("s-{tid}"), "targetInfo": {"targetId": tid}}),
                None,
                1,
            ));
        }
        assert_eq!(mgr.target_count(), 2);
        mgr.on_event(&ev("Target.targetDestroyed", json!({"targetId": "popup"}), None, 2));
        assert_eq!(mgr.target_count(), 1);
        assert!(mgr.get_target("popup").is_none());
    }

    #[test]
    fn crashed_target_marked() {
        let ds = ds_with_generations(1, 1);
        let mgr = BrowserTargetManager::new(ds, None);
        mgr.on_event(&ev(
            "Target.targetCreated",
            json!({"targetInfo": {"targetId": "t1", "type": "page"}}),
            None,
            0,
        ));
        mgr.on_event(&ev("Target.targetCrashed", json!({"targetId": "t1", "errorCode": 5}), None, 1));
        let rec = mgr.get_target("t1").unwrap();
        assert!(rec.crashed);
        assert_eq!(rec.lifecycle, TargetLifecycle::Crashed);
    }

    #[test]
    fn detached_session_clears() {
        let ds = ds_with_generations(1, 1);
        let mgr = BrowserTargetManager::new(ds, None);
        mgr.on_event(&ev(
            "Target.targetCreated",
            json!({"targetInfo": {"targetId": "t1", "type": "page"}}),
            None,
            0,
        ));
        mgr.on_event(&ev(
            "Target.attachedToTarget",
            json!({"sessionId": "s1", "targetInfo": {"targetId": "t1"}}),
            None,
            1,
        ));
        assert_eq!(mgr.get_target("t1").unwrap().session_id.as_deref(), Some("s1"));
        mgr.on_event(&ev("Target.detachedFromTarget", json!({"sessionId": "s1"}), None, 2));
        assert!(mgr.get_target("t1").unwrap().session_id.is_none());
    }

    #[test]
    fn frame_navigated_updates_url() {
        let ds = ds_with_generations(1, 1);
        let mgr = BrowserTargetManager::new(ds, None);
        mgr.on_event(&ev(
            "Target.targetCreated",
            json!({"targetInfo": {"targetId": "t1", "type": "page", "url": "https://old.example"}}),
            None,
            0,
        ));
        mgr.on_event(&ev(
            "Target.attachedToTarget",
            json!({"sessionId": "s1", "targetInfo": {"targetId": "t1"}}),
            None,
            1,
        ));
        mgr.on_event(&ev(
            "Page.frameNavigated",
            json!({"frame": {"id": "main", "url": "https://new.example", "name": ""}}),
            Some("s1".to_string()),
            2,
        ));
        assert_eq!(mgr.get_target("t1").unwrap().url, "https://new.example");
    }

    #[test]
    fn switch_target_only_changes_active_state() {
        let ds = ds_with_generations(1, 1);
        let mgr = BrowserTargetManager::new(ds, None);
        for tid in ["t1", "t2"] {
            mgr.on_event(&ev(
                "Target.targetCreated",
                json!({"targetInfo": {"targetId": tid, "type": "page"}}),
                None,
                0,
            ));
            mgr.on_event(&ev(
                "Target.attachedToTarget",
                json!({"sessionId": format!("s-{tid}"), "targetInfo": {"targetId": tid}}),
                None,
                1,
            ));
        }
        assert_eq!(mgr.active_target().unwrap().target_id, "t1");
        mgr.switch_target("t2").unwrap();
        assert_eq!(mgr.active_target().unwrap().target_id, "t2");
        // No WebSocket creation — verified by the fact that manager holds no WS handle in test (None) and switch succeeded without calling runtime.
    }

    #[test]
    fn generation_bump_across_reconnect_and_stale_ref_rejected() {
        let ds = ds_with_generations(5, 10);
        let mgr = BrowserTargetManager::new(ds.clone(), None);
        mgr.on_event(&ev(
            "Target.targetCreated",
            json!({"targetInfo": {"targetId": "t1", "type": "page"}}),
            None,
            0,
        ));
        mgr.on_event(&ev(
            "Target.attachedToTarget",
            json!({"sessionId": "s1", "targetInfo": {"targetId": "t1"}}),
            None,
            1,
        ));
        let before = mgr.get_target("t1").unwrap();
        let captured_ref = before.target_ref();
        assert_eq!(captured_ref.target_generation, 5);
        assert!(mgr.is_target_ref_valid(&captured_ref));
        assert_eq!(mgr.count_generation_mismatches(5), 0);

        // Simulate reconnect: bump via BrowserRuntime API (DomState), then tell manager.
        let new_tg = ds.bump_target_generation(); // 5 -> 6
        let new_cg = ds.bump_connection_generation(); // 10 -> 11
        mgr.handle_reconnect(new_tg, new_cg);

        // Every surviving record must have new generation, none keep old
        assert_eq!(mgr.count_generation_mismatches(new_tg), 0);
        let after = mgr.get_target("t1").unwrap();
        assert_eq!(after.target_generation, new_tg);
        assert_eq!(after.connection_generation, new_cg);
        assert!(after.session_id.is_none(), "session from prior connection must be cleared (I12)");

        // Captured ref before reconnect must be rejected
        assert!(!mgr.is_target_ref_valid(&captured_ref));
        assert!(mgr.check_target_ref(&captured_ref).is_err());
        assert!(mgr.switch_target_ref(&captured_ref).is_err());

        // New ref from after reconnect must be valid
        let new_ref = after.target_ref();
        assert!(mgr.is_target_ref_valid(&new_ref));
        assert!(mgr.check_target_ref(&new_ref).is_ok());

        // Even if raw ID collides, generation check rejects old ref — explicitly prove I12:
        // Create a new target with same ID string but new generation (reused ID simulation)
        // Old ref still rejected because generation differs, even though ID string matches live entry.
        assert_eq!(captured_ref.target_id, new_ref.target_id);
        assert_ne!(captured_ref.target_generation, new_ref.target_generation);
        // Old ref invalid, new ref valid — generation is the discriminator, not ID.
    }

    #[test]
    fn stale_generation_switch_rejected() {
        let ds = ds_with_generations(1, 1);
        let mgr = BrowserTargetManager::new(ds.clone(), None);
        mgr.on_event(&ev(
            "Target.targetCreated",
            json!({"targetInfo": {"targetId": "t1", "type": "page"}}),
            None,
            0,
        ));
        // Bump behind the record's back (simulating a missed reconnect bump as a bug detector):
        ds.bump_target_generation(); // now 2, record still 1
        // Switch must reject stale generation even for bare id (proves we don't silently accept)
        assert!(mgr.switch_target("t1").is_err());
    }
}
