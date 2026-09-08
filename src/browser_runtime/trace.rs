//! Phase 14 — Trace collection and ordering-pipeline diagnostics.
//!
//! Every action traceable per rule 19 / I14:
//! `action_id`, `target_id`, `session_id`, `step_index`, `operation`,
//! timestamps, latency, dom versions before/after, runtime generation,
//! resolution method, verification result, outcome.
//!
//! V3 ordering diagnostics surfaced alongside existing fields (Phase 1):
//! `reorder_buffer_depth_current`, `reorder_buffer_depth_max`,
//! `offloaded_decode_count`, `oversize_dropped_count` from
//! `BrowserRuntimeDiagnostics` (connection.rs).
//!
//! `TraceCollector` is owned by `BrowserRuntimeServer` (rule 26): bounded
//! `VecDeque<TraceEntry>` with `max_entries` (default 10000), thread-safe
//! via `RwLock`. Must not create a WebSocket, must not mutate
//! `BrowserRuntime` state directly — only reads `BrowserRuntimeDiagnostics`.
//!
//! Also defines `CdpTrace` for per-CDP-call latency trace (optional), kept
//! bounded in the same collector.

use std::collections::VecDeque;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, RwLock,
};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::action::{ActionRecord, StepOutcome, VerificationResult};
use super::connection::BrowserRuntimeDiagnostics;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Current wall-clock time in milliseconds since UNIX_EPOCH.
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

static TRACE_COUNTER: AtomicU64 = AtomicU64::new(1);

fn next_trace_id() -> String {
    let n = TRACE_COUNTER.fetch_add(1, Ordering::SeqCst);
    let ms = now_ms();
    format!("trace-{n}-{ms}")
}

// ---------------------------------------------------------------------------
// TraceEntry
// ---------------------------------------------------------------------------

/// Per-action trace entry (rule 19 + V3 ordering diagnostics).
///
/// Fields required per spec:
/// action_id, target_id, session_id, step_index, operation, timestamps,
/// latency, dom versions before/after, runtime generation, resolution method,
/// verification result, outcome, plus V3 reorder/offloaded diagnostics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceEntry {
    pub trace_id: String,
    pub timestamp_ms: u64,
    pub action_id: String,
    pub target_id: String,
    pub session_id: Option<String>,
    pub step_index: usize,
    pub operation: String,
    pub capability_class: String,
    pub state: String,
    pub started_at: u64,
    pub completed_at: Option<u64>,
    pub latency_ms: u64,
    pub dom_version_before: u64,
    pub dom_version_after: u64,
    pub frame_tree_version_before: u64,
    pub frame_tree_version_after: u64,
    pub runtime_generation: u64,
    pub target_generation: u64,
    pub resolution_method: String,
    pub verification: Option<VerificationResult>,
    pub outcome: StepOutcome,
    pub production_profile: bool,
    pub error_detail: Option<String>,
    pub reorder_buffer_depth_current: usize,
    pub reorder_buffer_depth_max: usize,
    pub offloaded_decode_count: u64,
    pub oversize_dropped_count: u64,
}

impl TraceEntry {
    /// Build from an `ActionRecord` without ordering diagnostics (defaults 0).
    pub fn from_action(record: &ActionRecord) -> Self {
        Self::from(record)
    }

    /// Enrich an existing entry with ordering diagnostics from `BrowserRuntimeDiagnostics`.
    /// Mutates in place; returns `&mut Self` for chaining.
    pub fn enrich_with_diagnostics(&mut self, diag: &BrowserRuntimeDiagnostics) -> &mut Self {
        self.reorder_buffer_depth_current = diag.reorder_buffer_depth_current;
        self.reorder_buffer_depth_max = diag.reorder_buffer_depth_max;
        self.offloaded_decode_count = diag.offloaded_decode_count;
        self.oversize_dropped_count = diag.oversize_dropped_count;
        self
    }

    /// Consuming enrichment — returns owned entry with diagnostics applied.
    pub fn with_diagnostics(mut self, diag: &BrowserRuntimeDiagnostics) -> Self {
        self.enrich_with_diagnostics(diag);
        self
    }

    /// Direct constructor with all fields (for testing convenience).
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        action_id: String,
        target_id: String,
        session_id: Option<String>,
        step_index: usize,
        operation: String,
        capability_class: String,
        state: String,
        started_at: u64,
        completed_at: Option<u64>,
        latency_ms: u64,
        dom_version_before: u64,
        dom_version_after: u64,
        frame_tree_version_before: u64,
        frame_tree_version_after: u64,
        runtime_generation: u64,
        target_generation: u64,
        resolution_method: String,
        verification: Option<VerificationResult>,
        outcome: StepOutcome,
        production_profile: bool,
        error_detail: Option<String>,
    ) -> Self {
        Self {
            trace_id: next_trace_id(),
            timestamp_ms: now_ms(),
            action_id,
            target_id,
            session_id,
            step_index,
            operation,
            capability_class,
            state,
            started_at,
            completed_at,
            latency_ms,
            dom_version_before,
            dom_version_after,
            frame_tree_version_before,
            frame_tree_version_after,
            runtime_generation,
            target_generation,
            resolution_method,
            verification,
            outcome,
            production_profile,
            error_detail,
            reorder_buffer_depth_current: 0,
            reorder_buffer_depth_max: 0,
            offloaded_decode_count: 0,
            oversize_dropped_count: 0,
        }
    }
}

impl From<&ActionRecord> for TraceEntry {
    fn from(record: &ActionRecord) -> Self {
        Self {
            trace_id: next_trace_id(),
            timestamp_ms: now_ms(),
            action_id: record.action_id.clone(),
            target_id: record.target_id.clone(),
            session_id: record.session_id.clone(),
            step_index: record.step_index,
            operation: record.action.clone(),
            capability_class: record.capability_class.clone(),
            state: record.state.name().to_string(),
            started_at: record.started_at,
            completed_at: record.completed_at,
            latency_ms: record.latency_ms,
            dom_version_before: record.dom_version_before,
            dom_version_after: record.dom_version_after,
            frame_tree_version_before: record.frame_tree_version_before,
            frame_tree_version_after: record.frame_tree_version_after,
            runtime_generation: record.runtime_generation,
            target_generation: record.target_generation,
            resolution_method: record.resolution_method.clone(),
            verification: record.verification.clone(),
            outcome: record.outcome.clone(),
            production_profile: record.production_profile,
            error_detail: record.error_detail.clone(),
            // V3 diagnostics default 0; caller enriches via `with_diagnostics`.
            reorder_buffer_depth_current: 0,
            reorder_buffer_depth_max: 0,
            offloaded_decode_count: 0,
            oversize_dropped_count: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// CdpTrace — per-CDP-call latency trace (optional)
// ---------------------------------------------------------------------------

/// Per-CDP-call latency trace kept bounded alongside `TraceEntry`s.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CdpTrace {
    pub cdp_method: String,
    pub session_id: Option<String>,
    pub latency_ms: u64,
    pub timestamp_ms: u64,
    pub success: bool,
    pub pending_count_at_start: usize,
    pub reorder_depth_at_start: usize,
}

impl CdpTrace {
    pub fn new(
        cdp_method: String,
        session_id: Option<String>,
        latency_ms: u64,
        success: bool,
        pending_count_at_start: usize,
        reorder_depth_at_start: usize,
    ) -> Self {
        Self {
            cdp_method,
            session_id,
            latency_ms,
            timestamp_ms: now_ms(),
            success,
            pending_count_at_start,
            reorder_depth_at_start,
        }
    }
}

// ---------------------------------------------------------------------------
// TraceCollector — bounded, thread-safe, owned by server (rule 26)
// ---------------------------------------------------------------------------

/// Bounded trace collector owned by `BrowserRuntimeServer` (rule 26).
///
/// Thread-safe via `RwLock`; bounded eviction (FIFO) when `max_entries` reached.
/// Also stores optional `CdpTrace` entries bounded by same limit.
#[derive(Debug)]
pub struct TraceCollector {
    entries: Arc<RwLock<VecDeque<TraceEntry>>>,
    cdp_traces: Arc<RwLock<VecDeque<CdpTrace>>>,
    max_entries: usize,
}

impl Default for TraceCollector {
    fn default() -> Self {
        Self::new(10000)
    }
}

impl TraceCollector {
    /// Create a new collector with explicit bound.
    pub fn new(max_entries: usize) -> Self {
        let cap = max_entries.max(1);
        Self {
            entries: Arc::new(RwLock::new(VecDeque::with_capacity(cap.min(10000)))),
            cdp_traces: Arc::new(RwLock::new(VecDeque::with_capacity(cap.min(10000)))),
            max_entries: cap,
        }
    }

    /// Record a pre-built `TraceEntry` (FIFO eviction when full).
    pub fn record(&self, entry: TraceEntry) {
        if let Ok(mut g) = self.entries.write() {
            if g.len() >= self.max_entries {
                g.pop_front();
            }
            g.push_back(entry);
        }
        tracing::trace!(total = self.len(), "trace recorded");
    }

    /// Convenience: build `TraceEntry` from `ActionRecord`, enrich with ordering
    /// diagnostics, and record it. Only reads `BrowserRuntimeDiagnostics`.
    pub fn record_from_action(&self, record: &ActionRecord, diagnostics: &BrowserRuntimeDiagnostics) {
        let entry = TraceEntry::from(record).with_diagnostics(diagnostics);
        self.record(entry);
    }

    /// Snapshot of all entries (cloned, oldest first).
    pub fn snapshot(&self) -> Vec<TraceEntry> {
        self.entries
            .read()
            .map(|g| g.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Structured JSON snapshot for diagnostics/IPC.
    ///
    /// Shape:
    /// {
    ///   total, by_state {..}, by_capability {..}, by_operation {..},
    ///   recent: [ TraceEntry, ... ]   // up to 20 most recent, oldest first among recent
    ///   entries: [ TraceEntry, ... ]  // alias for snapshot() for compat
    /// }
    pub fn snapshot_value(&self) -> Value {
        let entries = self.snapshot();
        let total = entries.len();

        let mut by_state: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        let mut by_capability: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        let mut by_operation: std::collections::HashMap<String, usize> = std::collections::HashMap::new();

        for e in &entries {
            *by_state.entry(e.state.clone()).or_insert(0) += 1;
            *by_capability.entry(e.capability_class.clone()).or_insert(0) += 1;
            *by_operation.entry(e.operation.clone()).or_insert(0) += 1;
        }

        let recent: Vec<Value> = entries
            .iter()
            .rev()
            .take(20)
            .rev()
            .map(|e| serde_json::to_value(e).unwrap_or(Value::Null))
            .collect();

        let all_values: Vec<Value> = entries
            .iter()
            .map(|e| serde_json::to_value(e).unwrap_or(Value::Null))
            .collect();

        serde_json::json!({
            "total": total,
            "by_state": by_state,
            "by_capability": by_capability,
            "by_operation": by_operation,
            "recent": recent,
            "entries": all_values,
        })
    }

    /// Number of stored entries.
    pub fn len(&self) -> usize {
        self.entries.read().map(|g| g.len()).unwrap_or(0)
    }

    /// Whether empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Clear all entries (for tests).
    pub fn clear(&self) {
        if let Ok(mut g) = self.entries.write() {
            g.clear();
        }
        if let Ok(mut g) = self.cdp_traces.write() {
            g.clear();
        }
    }

    /// Last N entries (oldest first among the N).
    pub fn last_n(&self, n: usize) -> Vec<TraceEntry> {
        self.entries
            .read()
            .map(|g| {
                g.iter()
                    .rev()
                    .take(n)
                    .cloned()
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect()
            })
            .unwrap_or_default()
    }

    // -- CdpTrace bounded storage --------------------------------------------

    /// Record a per-CDP-call latency trace (bounded, FIFO eviction).
    pub fn record_cdp(&self, trace: CdpTrace) {
        if let Ok(mut g) = self.cdp_traces.write() {
            if g.len() >= self.max_entries {
                g.pop_front();
            }
            g.push_back(trace);
        }
        tracing::trace!("cdp trace recorded");
    }

    /// Helper to build and record a CdpTrace in one call.
    pub fn record_cdp_call(
        &self,
        cdp_method: &str,
        session_id: Option<&str>,
        latency_ms: u64,
        success: bool,
        pending_count_at_start: usize,
        reorder_depth_at_start: usize,
    ) {
        let t = CdpTrace::new(
            cdp_method.to_string(),
            session_id.map(|s| s.to_string()),
            latency_ms,
            success,
            pending_count_at_start,
            reorder_depth_at_start,
        );
        self.record_cdp(t);
    }

    /// Snapshot of CDP traces.
    pub fn cdp_snapshot(&self) -> Vec<CdpTrace> {
        self.cdp_traces
            .read()
            .map(|g| g.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Number of CDP traces stored.
    pub fn cdp_len(&self) -> usize {
        self.cdp_traces.read().map(|g| g.len()).unwrap_or(0)
    }

    /// Last N CDP traces.
    pub fn cdp_last_n(&self, n: usize) -> Vec<CdpTrace> {
        self.cdp_traces
            .read()
            .map(|g| {
                g.iter()
                    .rev()
                    .take(n)
                    .cloned()
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Clear only CDP traces.
    pub fn clear_cdp(&self) {
        if let Ok(mut g) = self.cdp_traces.write() {
            g.clear();
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser_runtime::action::{ActionState, VerificationResult};
    use crate::browser_runtime::connection::{BrowserInfo, BrowserRuntimeDiagnostics};
    use std::time::SystemTime;

    fn sample_record() -> ActionRecord {
        let mut r = ActionRecord::new(
            "target-1".to_string(),
            7,
            Some("sess-abc".to_string()),
            3,
            "click".to_string(),
            "navigation".to_string(),
            42,
            100,
            5,
            true,
        );
        r.action_id = "act-test-1".to_string();
        r.resolution_method = "role+name".to_string();
        r.verification = Some(VerificationResult::Verified(serde_json::json!({"ok": true})));
        r.outcome = StepOutcome::Ok {
            verification: VerificationResult::Verified(serde_json::json!({"ok": true})),
        };
        r.state = ActionState::Completed;
        r.error_detail = Some("none".to_string());
        // Simulate completion to get latency + version bumps
        r.completed_at = Some(r.started_at + 123);
        r.latency_ms = 123;
        r.dom_version_after = 101;
        r.frame_tree_version_after = 6;
        r
    }

    fn sample_diagnostics() -> BrowserRuntimeDiagnostics {
        BrowserRuntimeDiagnostics {
            connection_id: "conn-1".to_string(),
            browser_info: BrowserInfo::default(),
            pending_request_count: 2,
            event_count: 99,
            connected_at: SystemTime::now(),
            attached_session_ids: vec!["sess-abc".to_string()],
            reorder_buffer_depth_current: 3,
            reorder_buffer_depth_max: 12,
            offloaded_decode_count: 7,
            oversize_dropped_count: 1,
        }
    }

    #[test]
    fn record_from_action_produces_correct_fields() {
        let rec = sample_record();
        let diag = sample_diagnostics();
        let entry = TraceEntry::from(&rec).with_diagnostics(&diag);

        assert_eq!(entry.action_id, rec.action_id);
        assert_eq!(entry.target_id, rec.target_id);
        assert_eq!(entry.session_id, rec.session_id);
        assert_eq!(entry.step_index, rec.step_index);
        assert_eq!(entry.operation, rec.action);
        assert_eq!(entry.capability_class, rec.capability_class);
        assert_eq!(entry.state, rec.state.name().to_string());
        assert_eq!(entry.started_at, rec.started_at);
        assert_eq!(entry.completed_at, rec.completed_at);
        assert_eq!(entry.latency_ms, rec.latency_ms);
        assert_eq!(entry.dom_version_before, rec.dom_version_before);
        assert_eq!(entry.dom_version_after, rec.dom_version_after);
        assert_eq!(entry.frame_tree_version_before, rec.frame_tree_version_before);
        assert_eq!(entry.frame_tree_version_after, rec.frame_tree_version_after);
        assert_eq!(entry.runtime_generation, rec.runtime_generation);
        assert_eq!(entry.target_generation, rec.target_generation);
        assert_eq!(entry.resolution_method, rec.resolution_method);
        assert_eq!(entry.verification, rec.verification);
        assert_eq!(entry.outcome, rec.outcome);
        assert_eq!(entry.production_profile, rec.production_profile);
        assert_eq!(entry.error_detail, rec.error_detail);
        // Not empty trace_id and timestamp
        assert!(!entry.trace_id.is_empty());
        assert!(entry.timestamp_ms > 0);
        // V3 ordering diagnostics propagated
        assert_eq!(entry.reorder_buffer_depth_current, diag.reorder_buffer_depth_current);
        assert_eq!(entry.reorder_buffer_depth_max, diag.reorder_buffer_depth_max);
        assert_eq!(entry.offloaded_decode_count, diag.offloaded_decode_count);
        assert_eq!(entry.oversize_dropped_count, diag.oversize_dropped_count);
    }

    #[test]
    fn from_without_diagnostics_defaults_zero() {
        let rec = sample_record();
        let entry = TraceEntry::from(&rec);
        assert_eq!(entry.reorder_buffer_depth_current, 0);
        assert_eq!(entry.reorder_buffer_depth_max, 0);
        assert_eq!(entry.offloaded_decode_count, 0);
        assert_eq!(entry.oversize_dropped_count, 0);
    }

    #[test]
    fn enrich_with_diagnostics_mutates() {
        let rec = sample_record();
        let diag = sample_diagnostics();
        let mut entry = TraceEntry::from(&rec);
        entry.enrich_with_diagnostics(&diag);
        assert_eq!(entry.reorder_buffer_depth_current, 3);
        assert_eq!(entry.offloaded_decode_count, 7);
    }

    #[test]
    fn bounded_eviction() {
        let collector = TraceCollector::new(3);
        let diag = sample_diagnostics();
        for i in 0..5 {
            let mut rec = sample_record();
            rec.action_id = format!("act-{i}");
            rec.step_index = i;
            collector.record_from_action(&rec, &diag);
        }
        assert_eq!(collector.len(), 3);
        let snap = collector.snapshot();
        assert_eq!(snap[0].action_id, "act-2");
        assert_eq!(snap[1].action_id, "act-3");
        assert_eq!(snap[2].action_id, "act-4");
    }

    #[test]
    fn snapshot_shape() {
        let collector = TraceCollector::new(10);
        let diag = sample_diagnostics();
        let mut rec1 = sample_record();
        rec1.action_id = "act-10".to_string();
        rec1.action = "click".to_string();
        rec1.capability_class = "navigation".to_string();
        rec1.state = ActionState::Completed;

        let mut rec2 = sample_record();
        rec2.action_id = "act-11".to_string();
        rec2.action = "eval".to_string();
        rec2.capability_class = "runtime_evaluate".to_string();
        rec2.state = ActionState::Failed;

        collector.record_from_action(&rec1, &diag);
        collector.record_from_action(&rec2, &diag);

        let v = collector.snapshot_value();
        assert_eq!(v["total"], 2);
        assert_eq!(v["by_state"]["Completed"], 1);
        assert_eq!(v["by_state"]["Failed"], 1);
        assert_eq!(v["by_capability"]["navigation"], 1);
        assert_eq!(v["by_capability"]["runtime_evaluate"], 1);
        assert_eq!(v["by_operation"]["click"], 1);
        assert_eq!(v["by_operation"]["eval"], 1);
        assert_eq!(v["recent"].as_array().map(|a| a.len()).unwrap_or(0), 2);
        assert_eq!(v["entries"].as_array().map(|a| a.len()).unwrap_or(0), 2);
    }

    #[test]
    fn ordering_diagnostics_propagated_via_collector() {
        let collector = TraceCollector::new(10);
        let diag = BrowserRuntimeDiagnostics {
            connection_id: "conn-x".to_string(),
            browser_info: BrowserInfo::default(),
            pending_request_count: 5,
            event_count: 10,
            connected_at: SystemTime::now(),
            attached_session_ids: vec![],
            reorder_buffer_depth_current: 9,
            reorder_buffer_depth_max: 99,
            offloaded_decode_count: 42,
            oversize_dropped_count: 3,
        };
        let rec = sample_record();
        collector.record_from_action(&rec, &diag);
        let snap = collector.snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].reorder_buffer_depth_current, 9);
        assert_eq!(snap[0].reorder_buffer_depth_max, 99);
        assert_eq!(snap[0].offloaded_decode_count, 42);
        assert_eq!(snap[0].oversize_dropped_count, 3);
    }

    #[test]
    fn last_n_returns_tail() {
        let collector = TraceCollector::new(10);
        let diag = sample_diagnostics();
        for i in 0..5 {
            let mut rec = sample_record();
            rec.action_id = format!("act-last-{i}");
            collector.record_from_action(&rec, &diag);
        }
        let last2 = collector.last_n(2);
        assert_eq!(last2.len(), 2);
        assert_eq!(last2[0].action_id, "act-last-3");
        assert_eq!(last2[1].action_id, "act-last-4");
    }

    #[test]
    fn cdp_trace_bounded() {
        let collector = TraceCollector::new(2);
        for i in 0..4 {
            collector.record_cdp_call("Page.navigate", Some("sess-1"), 10 + i as u64, true, i, i);
        }
        assert_eq!(collector.cdp_len(), 2);
        let snap = collector.cdp_snapshot();
        assert_eq!(snap.len(), 2);
        // FIFO eviction keeps last 2
        assert_eq!(snap[0].latency_ms, 12);
        assert_eq!(snap[1].latency_ms, 13);
    }

    #[test]
    fn cdp_trace_fields() {
        let collector = TraceCollector::new(10);
        collector.record_cdp_call("Runtime.evaluate", Some("sess-x"), 55, false, 3, 7);
        let snap = collector.cdp_snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].cdp_method, "Runtime.evaluate");
        assert_eq!(snap[0].session_id, Some("sess-x".to_string()));
        assert_eq!(snap[0].latency_ms, 55);
        assert!(!snap[0].success);
        assert_eq!(snap[0].pending_count_at_start, 3);
        assert_eq!(snap[0].reorder_depth_at_start, 7);
        assert!(snap[0].timestamp_ms > 0);
    }

    #[test]
    fn clear_empties() {
        let collector = TraceCollector::new(10);
        let diag = sample_diagnostics();
        let rec = sample_record();
        collector.record_from_action(&rec, &diag);
        collector.record_cdp_call("DOM.getDocument", None, 5, true, 0, 0);
        assert_eq!(collector.len(), 1);
        assert_eq!(collector.cdp_len(), 1);
        collector.clear();
        assert_eq!(collector.len(), 0);
        assert_eq!(collector.cdp_len(), 0);
    }

    #[test]
    fn now_ms_monotonic_nonzero() {
        let a = now_ms();
        let b = now_ms();
        assert!(a > 0);
        assert!(b >= a);
    }
}
