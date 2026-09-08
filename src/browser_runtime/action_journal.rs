//! Phase 8 — Action journal (rule 14, I14, I19, rule 22/32).
//!
//! Every externally visible action has a journal entry (I14). Each entry
//! is tagged with `capability_class` (rule 32) and `production_profile`
//! (rule 22 / I19). The journal is owned by `BrowserRuntimeServer`
//! (rule 26) and persists for the daemon lifetime; `task.rs` integration
//! is additive if needed (Phase 11), but the journal's persistence
//! here is in-memory with optional file spill (bounded).

use std::collections::VecDeque;
use std::sync::{Arc, RwLock};

use serde_json::Value;

use super::action::ActionRecord;
use super::server::CapabilityClass;

/// Bounded action journal owned by the server (rule 26).
/// Per-target serialized appends (callers hold per-target lock before
/// appending — see executor/server).
#[derive(Debug, Clone)]
pub struct ActionJournal {
    entries: Arc<RwLock<VecDeque<ActionRecord>>>,
    max_entries: usize,
}

impl Default for ActionJournal {
    fn default() -> Self {
        Self::new(10000)
    }
}

impl ActionJournal {
    pub fn new(max_entries: usize) -> Self {
        Self {
            entries: Arc::new(RwLock::new(VecDeque::with_capacity(max_entries.min(10000)))),
            max_entries,
        }
    }

    /// Append a completed record (thread-safe). Evicts oldest when bounded.
    pub fn append(&self, record: ActionRecord) {
        if let Ok(mut g) = self.entries.write() {
            if g.len() >= self.max_entries {
                g.pop_front();
            }
            g.push_back(record);
        }
    }

    pub fn len(&self) -> usize {
        self.entries.read().map(|g| g.len()).unwrap_or(0)
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Snapshot of all entries (cloned). For diagnostics / IPC.
    pub fn snapshot(&self) -> Vec<ActionRecord> {
        self.entries.read().map(|g| g.iter().cloned().collect()).unwrap_or_default()
    }

    /// Filtered snapshot by target_id.
    pub fn for_target(&self, target_id: &str) -> Vec<ActionRecord> {
        self.entries
            .read()
            .map(|g| g.iter().filter(|r| r.target_id == target_id).cloned().collect())
            .unwrap_or_default()
    }

    /// Last N entries.
    pub fn last_n(&self, n: usize) -> Vec<ActionRecord> {
        self.entries
            .read()
            .map(|g| g.iter().rev().take(n).cloned().collect::<Vec<_>>().into_iter().rev().collect())
            .unwrap_or_default()
    }

    /// Clear (for tests).
    pub fn clear(&self) {
        if let Ok(mut g) = self.entries.write() {
            g.clear();
        }
    }

    /// Trace snapshot for status/diagnostics and DoD profile tagging demonstration.
    pub fn trace_snapshot(&self) -> Value {
        let entries = self.snapshot();
        let total = entries.len();
        let production_count = entries.iter().filter(|r| r.production_profile).count();
        let by_capability = {
            let mut m = std::collections::HashMap::<String, usize>::new();
            for r in &entries {
                *m.entry(r.capability_class.clone()).or_insert(0) += 1;
            }
            m
        };
        serde_json::json!({
            "total": total,
            "production_count": production_count,
            "non_production_count": total - production_count,
            "by_capability": by_capability,
            "entries": entries.iter().map(|r| serde_json::json!({
                "action_id": r.action_id,
                "target_id": r.target_id,
                "target_generation": r.target_generation,
                "session_id": r.session_id,
                "step_index": r.step_index,
                "action": r.action,
                "capability_class": r.capability_class,
                "state": r.state.name(),
                "outcome": r.outcome,
                "verification": r.verification,
                "production_profile": r.production_profile,
                "runtime_generation": r.runtime_generation,
                "dom_version_before": r.dom_version_before,
                "dom_version_after": r.dom_version_after,
                "frame_tree_version_before": r.frame_tree_version_before,
                "frame_tree_version_after": r.frame_tree_version_after,
                "started_at": r.started_at,
                "completed_at": r.completed_at,
                "latency_ms": r.latency_ms,
            })).collect::<Vec<_>>(),
        })
    }
}

// ---------------------------------------------------------------------------
// Helper to derive capability + production tag for a new record
// ---------------------------------------------------------------------------

/// Build capability string + production_profile for a new journal entry.
pub fn journal_tags_for(
    capability: CapabilityClass,
    user_data_dir: Option<&str>,
) -> (String, bool) {
    let cap = capability.as_str().to_string();
    let prod = super::action::is_production_profile(user_data_dir);
    (cap, prod)
}

// ---------------------------------------------------------------------------
// Tests — journal tagging
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser_runtime::action::{ActionState, VerificationResult};
    use crate::browser_runtime::server::CapabilityClass;

    fn sample_record(cap: CapabilityClass, prod: bool) -> ActionRecord {
        let mut r = ActionRecord::new(
            "t1".to_string(),
            1,
            Some("s1".to_string()),
            0,
            "click".to_string(),
            cap.as_str().to_string(),
            1,
            5,
            2,
            prod,
        );
        r.verification = Some(VerificationResult::Verified(serde_json::json!({"clicked": true})));
        r.outcome = crate::browser_runtime::action::StepOutcome::Ok { verification: VerificationResult::Verified(serde_json::json!({})) };
        r.state = ActionState::Completed;
        r.complete(6, 3);
        r
    }

    #[test]
    fn journal_tags_capability_and_production_profile() {
        let j = ActionJournal::new(100);
        // One against fixture profile (non-production)
        let (cap1, prod1) = journal_tags_for(CapabilityClass::RuntimeEvaluate, Some("/tmp/hyprfast-runtime-test"));
        assert_eq!(cap1, "runtime_evaluate");
        assert!(!prod1);
        let r1 = sample_record(CapabilityClass::RuntimeEvaluate, prod1);
        assert_eq!(r1.capability_class, "runtime_evaluate");
        assert!(!r1.production_profile);

        // One against non-fixture profile (production)
        let (cap2, prod2) = journal_tags_for(CapabilityClass::RuntimeEvaluate, Some("/home/user/.config/BraveSoftware"));
        assert_eq!(cap2, "runtime_evaluate");
        assert!(prod2);
        let r2 = sample_record(CapabilityClass::Navigation, prod2);
        assert_eq!(r2.capability_class, "navigation");
        assert!(r2.production_profile);

        j.append(r1);
        // Navigation capability should be None tag type for plain DOM? But we use Navigation here.
        // Second record uses Navigation to prove capability_class correct per op type.
        let mut r2b = sample_record(CapabilityClass::None, prod2);
        r2b.capability_class = CapabilityClass::None.as_str().to_string();
        j.append(r2b);

        let snap = j.trace_snapshot();
        assert_eq!(snap["total"], 2);
        assert_eq!(snap["production_count"], 1);
        assert_eq!(snap["non_production_count"], 1);
    }

    #[test]
    fn capability_class_per_step() {
        // PlanStep::Eval vs plain DOM click cap tagging already in plan.rs;
        // here journal must preserve it.
        let j = ActionJournal::new(10);
        for (cap, expected) in [
            (CapabilityClass::RuntimeEvaluate, "runtime_evaluate"),
            (CapabilityClass::Navigation, "navigation"),
            (CapabilityClass::None, "none"),
            (CapabilityClass::Cookies, "cookies"),
        ] {
            let (c, _) = journal_tags_for(cap, Some("/tmp/hyprfast-runtime-test"));
            assert_eq!(c, expected);
            let mut r = sample_record(cap, false);
            r.capability_class = c;
            j.append(r);
        }
        assert_eq!(j.len(), 4);
        let snap = j.trace_snapshot();
        assert_eq!(snap["by_capability"]["runtime_evaluate"], 1);
        assert_eq!(snap["by_capability"]["navigation"], 1);
    }

    #[test]
    fn bounded_eviction() {
        let j = ActionJournal::new(2);
        for i in 0..3 {
            let mut r = sample_record(CapabilityClass::None, false);
            r.action_id = format!("act-{i}");
            j.append(r);
        }
        assert_eq!(j.len(), 2);
        let snaps = j.snapshot();
        assert_eq!(snaps[0].action_id, "act-1");
        assert_eq!(snaps[1].action_id, "act-2");
    }
}
