//! BrowserRuntime — persistent browser automation runtime.
//!
//! Phase 1: browser-level persistent CDP transport (`connection`).
//! Phase 2: daemon + lifecycle state machine + IPC protocol
//! (`state`, `server`, `client`).
//!
//! [`BrowserRuntime`](connection::BrowserRuntime) owns exactly one
//! browser-level WebSocket (opened from `/json/version`) and multiplexes all
//! targets over it in CDP flat-session mode. [`server::BrowserRuntimeServer`]
//! owns the single `BrowserRuntime`; CLI processes are clients via
//! [`client::ClientConn`] and never hold persistent CDP connections.

pub mod action;
pub mod action_journal;
pub mod client;
pub mod connection;
pub mod crash_recovery;
pub mod dom_diff;
pub mod dom_state;
pub mod element_index;
pub mod error;
pub mod events;
pub mod executor;
pub mod frames;
pub mod plan;
pub mod recovery;
pub mod server;
pub mod state;
pub mod targets;
pub mod metrics;
pub mod trace;
pub mod wait;
pub mod vision;

pub use client::{ClientConn, daemon_available, evaluate_once, status_once, stop_once};
pub use connection::{
    BrowserInfo, BrowserRuntime, BrowserRuntimeDiagnostics, CdpError, CdpEvent, CdpRequest,
    CdpResponse, RuntimeConfig, DEFAULT_CALL_TIMEOUT, DEFAULT_DECODE_OFFLOAD_THRESHOLD_BYTES,
    DEFAULT_MAX_MESSAGE_SIZE_BYTES,
};
pub use error::{RuntimeError, RuntimeResult};
pub use server::{
    BrowserRuntimeServer, CapabilityClass, IPC_PROTOCOL_VERSION, MAX_IPC_FRAME_BYTES,
    RUNTIME_VERSION, RuntimeStatus, ServeOptions, bind_socket_exclusive, browser_socket_path,
    probe_live, restart_on_crash_from_env, serve, socket_mode_octal, start_daemon_detached,
    stop_daemon_sync,
};
pub use dom_state::DomState;
pub use element_index::{BoundingBox, DomNodeInfo, AxNodeInfo, ElementIndex, ElementRef, ResolveRequest};
pub use events::EventDispatcher;
pub use frames::{FrameManager, FrameRecord};
pub use recovery::{RecoveryEngine, RecoveryOptions, RecoveryOutcome, RecoveryResult, RecoveryTier, TierStatus, LADDER};
pub use crash_recovery::{CrashPolicy, CrashRecoveryOutcome, CrashRecoveryResult, handle_cdp_disconnect};
pub use action::{ActionRecord, ActionState, StepOutcome as ActionStepOutcome, VerificationResult};
pub use action_journal::{ActionJournal, journal_tags_for};
pub use executor::{ActionExecutor, ExecutorConfig};
pub use plan::{ExecutionPlan, PlanStep, PlanExecutionResult, StepOutcome};
pub use state::{GenerationCounters, LifecycleState, RequestKind};
pub use targets::{BrowserTargetManager, SessionRef, TargetLifecycle, TargetRecord, TargetRef};
pub use vision::*;
pub use wait::{WaitCondition, WaitEngine, wait_for, wait_for_lifecycle_state, wait_for_navigation_complete};
pub use dom_diff::{DiffBenchmark, DiffKind, DiffMetrics, DomDiff, DomDiffEngine, apply_batch_incremental, benchmark_incremental_vs_rebuild};
pub use trace::{CdpTrace, TraceCollector, TraceEntry, now_ms};
pub use metrics::{
    BenchmarkResult, MetricsSnapshot, RuntimeMetrics, LATENCY_SAMPLE_CAP,
    benchmark_handshake_elimination, benchmark_handshake_elimination_with_iterations,
    benchmark_incremental_vs_rebuild_wrapper, benchmark_latency_samples, benchmark_persistent_calls,
};
