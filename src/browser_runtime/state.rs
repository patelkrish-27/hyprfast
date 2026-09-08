//! Phase 2 — BrowserRuntime lifecycle state machine (plan §1).
//!
//! Every state defines exactly what is permitted. [`LifecycleState::check`]
//! is the explicit gate enforced at the top of every server-side handler —
//! never an implicit consequence of connection liveness (invariant I11).
//!
//! ```text
//! Starting → Connecting → Connected → Degraded → Disconnected
//!                                         ↑            │
//!                                         └─ Reconnecting
//! Any state → Stopping → Stopped
//! ```
//!
//! `Failed(String)` is a terminal latch carrying the failure reason; for
//! permission purposes it behaves like `Disconnected` (reads serve
//! last-known state, new actions are `RuntimeDead`).

use super::error::{RuntimeError, RuntimeResult};

/// What a request wants to do, for permission gating.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestKind {
    /// Status/diagnostics reads. Served in every state except `Stopped`
    /// (which has no last-known state worth serving over a removed socket).
    Read,
    /// Any state-changing browser dispatch. Serialized per target by the
    /// server (rule 29); gated here by lifecycle state first.
    StateChanging,
}

/// Authoritative lifecycle state (§1 table).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LifecycleState {
    Starting,
    Connecting,
    Connected,
    Degraded,
    Reconnecting,
    Disconnected,
    Stopping,
    Stopped,
    Failed(String),
}

impl LifecycleState {
    /// Stable name used in `status` output and logs.
    pub fn name(&self) -> &'static str {
        match self {
            LifecycleState::Starting => "Starting",
            LifecycleState::Connecting => "Connecting",
            LifecycleState::Connected => "Connected",
            LifecycleState::Degraded => "Degraded",
            LifecycleState::Reconnecting => "Reconnecting",
            LifecycleState::Disconnected => "Disconnected",
            LifecycleState::Stopping => "Stopping",
            LifecycleState::Stopped => "Stopped",
            LifecycleState::Failed(_) => "Failed",
        }
    }

    /// Enforce the §1 permission table. Returns `Ok` iff `kind` may proceed
    /// in this state; otherwise a structured, state-specific error — never
    /// a generic one.
    pub fn check(&self, kind: RequestKind) -> RuntimeResult<()> {
        match (self, kind) {
            // Connected: everything allowed.
            (LifecycleState::Connected, _) => Ok(()),
            // Starting / Connecting: new actions rejected; reads limited to
            // state-only status (the only read Phase 2 serves).
            (LifecycleState::Starting, RequestKind::Read) => Ok(()),
            (LifecycleState::Connecting, RequestKind::Read) => Ok(()),
            (LifecycleState::Starting, RequestKind::StateChanging) => Err(RuntimeError::NotReady(
                "runtime is starting; browser actions not accepted yet".to_string(),
            )),
            (LifecycleState::Connecting, RequestKind::StateChanging) => Err(RuntimeError::NotReady(
                "runtime is connecting to the browser; try again shortly".to_string(),
            )),
            // Degraded: read-only only, no new state-changing dispatch.
            (LifecycleState::Degraded, RequestKind::Read) => Ok(()),
            (LifecycleState::Degraded, RequestKind::StateChanging) => Err(RuntimeError::Degraded(
                "runtime is degraded; state-changing dispatch paused, reads still served"
                    .to_string(),
            )),
            // Reconnecting: reads show the generation in progress; dispatch
            // rejected with its own variant (never NotReady/RuntimeDead).
            (LifecycleState::Reconnecting, RequestKind::Read) => Ok(()),
            (LifecycleState::Reconnecting, RequestKind::StateChanging) => {
                Err(RuntimeError::Reconnecting(
                    "runtime is reconnecting; retry after it returns to Connected".to_string(),
                ))
            }
            // Disconnected / Failed: reads serve last-known state; new
            // actions are RuntimeDead (I11: a disconnected runtime cannot
            // execute browser actions).
            (LifecycleState::Disconnected, RequestKind::Read) => Ok(()),
            (LifecycleState::Failed(_), RequestKind::Read) => Ok(()),
            (LifecycleState::Disconnected, RequestKind::StateChanging) => {
                Err(RuntimeError::RuntimeDead(
                    "runtime is disconnected; no browser actions possible".to_string(),
                ))
            }
            (LifecycleState::Failed(reason), RequestKind::StateChanging) => {
                Err(RuntimeError::RuntimeDead(format!(
                    "runtime failed and is not usable: {reason}"
                )))
            }
            // Stopping: limited reads; new work refused as ShuttingDown.
            (LifecycleState::Stopping, RequestKind::Read) => Ok(()),
            (LifecycleState::Stopping, RequestKind::StateChanging) => Err(RuntimeError::ShuttingDown(
                "runtime is stopping; no new actions accepted".to_string(),
            )),
            // Stopped: nothing served (socket is gone by now; this covers
            // in-flight handlers racing the final transition).
            (LifecycleState::Stopped, _) => Err(RuntimeError::RuntimeDead(
                "runtime is stopped".to_string(),
            )),
        }
    }

    /// Whether `next` is a legal transition from `self` (§1 diagram).
    /// `Stopping` is reachable from any state except the terminal
    /// `Stopped`; `Stopped` is reachable only from `Stopping`.
    pub fn can_transition_to(&self, next: &LifecycleState) -> bool {
        use LifecycleState as S;
        // Any non-terminal state may begin shutdown.
        if matches!(next, S::Stopping) {
            return !matches!(self, S::Stopped);
        }
        // Stopped is only reachable from Stopping.
        if matches!(next, S::Stopped) {
            return matches!(self, S::Stopping);
        }
        // Failed latches from any non-terminal state.
        if matches!(next, S::Failed(_)) {
            return !matches!(self, S::Stopped | S::Stopping);
        }
        // No transitions out of the terminal states (except to Stopping,
        // handled above — Stopped never even does that).
        if matches!(self, S::Stopped | S::Stopping | S::Failed(_)) {
            return false;
        }
        matches!(
            (self, next),
            (S::Starting, S::Connecting)
                | (S::Connecting, S::Connected)
                | (S::Connecting, S::Disconnected)
                | (S::Connected, S::Degraded)
                | (S::Connected, S::Disconnected)
                | (S::Connected, S::Reconnecting)
                | (S::Degraded, S::Connected)
                | (S::Degraded, S::Reconnecting)
                | (S::Degraded, S::Disconnected)
                | (S::Reconnecting, S::Connected)
                | (S::Reconnecting, S::Disconnected)
                | (S::Disconnected, S::Reconnecting)
        )
    }

    /// Explicit transition with validation. Illegal edges are a caller bug
    /// and surface as `InvalidResponse` (structured, never silent).
    pub fn transition_to(&mut self, next: LifecycleState) -> RuntimeResult<()> {
        if self.can_transition_to(&next) {
            *self = next;
            Ok(())
        } else {
            Err(RuntimeError::InvalidResponse(format!(
                "illegal lifecycle transition {} -> {}",
                self.name(),
                next.name()
            )))
        }
    }
}

/// Runtime-owned generation counters exposed via `status`.
///
/// PROVENANCE (Phase 2): the authoritative counters land in later phases
/// (dom/navigation in Phase 4, frame tree in Phase 5, reconnect generations
/// in Phase 11). Until then these are placeholders pinned at 0 so the
/// status shape is stable from day one — see plan Phase 2 §"Generation
/// counters ... reported as 0/placeholder with clear provenance".
#[derive(Debug, Clone, Default)]
pub struct GenerationCounters {
    pub runtime_generation: u64,
    pub navigation_generation: u64,
    pub dom_version: u64,
    pub frame_tree_version: u64,
}
