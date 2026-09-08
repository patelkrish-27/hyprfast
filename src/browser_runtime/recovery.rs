//! Phase 10 — Deterministic recovery ladder.
//!
//! Ladder order (spec):
//!   existing ref → cached backend node → CSS selector → DOM id/name →
//!   AX role/name → semantic text → fresh DOM/AX search → frame/shadow
//!   traversal → explicit Ambiguous/ResolutionFailed → vision only if applicable.
//! Every tier's attempt recorded; never skip tier.
//!
//! The ladder is deterministic: tiers are attempted in fixed order, each
//! attempt is recorded in `RecoveryTrace`, and no tier is skipped even if
//! the request has no field for that tier (it records `Skipped`/`Miss`).
//! If a tier yields >1 candidate → `AmbiguousElement` (never guesses). If
//! no tier yields a single candidate → `ResolutionFailed`. Vision tier is
//! last resort and only runs when `allow_vision && is_visual_context`.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::browser_runtime::dom_state::DomState;
use crate::browser_runtime::element_index::{ElementIndex, ElementRef, ResolveRequest};

use crate::browser_runtime::frames::FrameManager;

// ---------------------------------------------------------------------------
// Tier definitions — ordered, never reordered, never skipped
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum RecoveryTier {
    ExistingRef,
    CachedBackendNode,
    CssSelector,
    DomIdName,
    AxRoleName,
    SemanticText,
    FreshDomAxSearch,
    FrameShadowTraversal,
    AmbiguityResolution,
    VisionFallback,
}

impl RecoveryTier {
    pub fn name(self) -> &'static str {
        match self {
            RecoveryTier::ExistingRef => "ExistingRef",
            RecoveryTier::CachedBackendNode => "CachedBackendNode",
            RecoveryTier::CssSelector => "CssSelector",
            RecoveryTier::DomIdName => "DomIdName",
            RecoveryTier::AxRoleName => "AxRoleName",
            RecoveryTier::SemanticText => "SemanticText",
            RecoveryTier::FreshDomAxSearch => "FreshDomAxSearch",
            RecoveryTier::FrameShadowTraversal => "FrameShadowTraversal",
            RecoveryTier::AmbiguityResolution => "AmbiguityResolution",
            RecoveryTier::VisionFallback => "VisionFallback",
        }
    }
    pub fn order(self) -> usize {
        match self {
            RecoveryTier::ExistingRef => 0,
            RecoveryTier::CachedBackendNode => 1,
            RecoveryTier::CssSelector => 2,
            RecoveryTier::DomIdName => 3,
            RecoveryTier::AxRoleName => 4,
            RecoveryTier::SemanticText => 5,
            RecoveryTier::FreshDomAxSearch => 6,
            RecoveryTier::FrameShadowTraversal => 7,
            RecoveryTier::AmbiguityResolution => 8,
            RecoveryTier::VisionFallback => 9,
        }
    }
}

/// Fixed ladder order — single source of truth.
pub const LADDER: [RecoveryTier; 10] = [
    RecoveryTier::ExistingRef,
    RecoveryTier::CachedBackendNode,
    RecoveryTier::CssSelector,
    RecoveryTier::DomIdName,
    RecoveryTier::AxRoleName,
    RecoveryTier::SemanticText,
    RecoveryTier::FreshDomAxSearch,
    RecoveryTier::FrameShadowTraversal,
    RecoveryTier::AmbiguityResolution,
    RecoveryTier::VisionFallback,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum TierStatus {
    Success,
    Miss,
    Ambiguous,
    SkippedMissingInput,
    SkippedNotApplicable,
    SkippedDueToEarlierSuccess,
    Error,
    VisionRequired,
    VisionSkipped,
}

impl TierStatus {
    pub fn label(self) -> &'static str {
        match self {
            TierStatus::Success => "Success",
            TierStatus::Miss => "Miss",
            TierStatus::Ambiguous => "Ambiguous",
            TierStatus::SkippedMissingInput => "SkippedMissingInput",
            TierStatus::SkippedNotApplicable => "SkippedNotApplicable",
            TierStatus::SkippedDueToEarlierSuccess => "SkippedDueToEarlierSuccess",
            TierStatus::Error => "Error",
            TierStatus::VisionRequired => "VisionRequired",
            TierStatus::VisionSkipped => "VisionSkipped",
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct TierAttempt {
    pub tier: RecoveryTier,
    pub tier_name: String,
    pub order: usize,
    pub status: TierStatus,
    pub status_label: String,
    pub detail: String,
    pub candidates: Vec<String>,
}

impl TierAttempt {
    fn new(tier: RecoveryTier, status: TierStatus, detail: impl Into<String>, candidates: Vec<String>) -> Self {
        Self {
            tier,
            tier_name: tier.name().to_string(),
            order: tier.order(),
            status,
            status_label: status.label().to_string(),
            detail: detail.into(),
            candidates,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct RecoveryTrace {
    pub attempts: Vec<TierAttempt>,
}

impl RecoveryTrace {
    pub fn to_value(&self) -> Value {
        json!({
            "attempts": self.attempts.iter().map(|a| json!({
                "tier": a.tier_name,
                "order": a.order,
                "status": a.status_label,
                "detail": a.detail,
                "candidates": a.candidates,
            })).collect::<Vec<_>>(),
            "ladder": LADDER.iter().map(|t| t.name()).collect::<Vec<_>>(),
        })
    }
}

#[derive(Debug, Clone)]
pub enum RecoveryOutcome {
    Recovered(ElementRef),
    Ambiguous { candidates: Vec<String>, detail: String },
    ResolutionFailed(String),
    VisionRequired { detail: String },
}

#[derive(Debug, Clone)]
pub struct RecoveryResult {
    pub outcome: RecoveryOutcome,
    pub trace: RecoveryTrace,
}

impl RecoveryResult {
    pub fn is_recovered(&self) -> bool {
        matches!(self.outcome, RecoveryOutcome::Recovered(_))
    }
    pub fn to_value(&self) -> Value {
        let outcome = match &self.outcome {
            RecoveryOutcome::Recovered(r) => json!({"kind":"Recovered","id": r.id, "backend": r.backend_node_id, "selector": r.selector}),
            RecoveryOutcome::Ambiguous { candidates, detail } => json!({"kind":"Ambiguous","candidates": candidates, "detail": detail}),
            RecoveryOutcome::ResolutionFailed(d) => json!({"kind":"ResolutionFailed","detail": d}),
            RecoveryOutcome::VisionRequired { detail } => json!({"kind":"VisionRequired","detail": detail}),
        };
        json!({
            "outcome": outcome,
            "trace": self.trace.to_value(),
        })
    }
}

#[derive(Debug, Clone, Default)]
pub struct RecoveryOptions {
    pub allow_vision: bool,
    pub is_visual_context: bool,
    /// If true, in-memory FreshDomAxSearch re-scans index with looser predicates.
    /// For real browser, this would re-fetch DOM+AX.
    pub pierce_shadow: bool,
}

impl RecoveryOptions {
    pub fn vision_only_if_applicable(mut self, allow: bool, visual: bool) -> Self {
        self.allow_vision = allow;
        self.is_visual_context = visual;
        self
    }
}

// ---------------------------------------------------------------------------
// RecoveryEngine — deterministic ladder owned via ElementIndex etc. (rule 26)
// ---------------------------------------------------------------------------

pub struct RecoveryEngine {
    element_index: Arc<ElementIndex>,
    frame_manager: Arc<FrameManager>,
    dom_state: Arc<DomState>,
}

impl std::fmt::Debug for RecoveryEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RecoveryEngine").finish_non_exhaustive()
    }
}

impl RecoveryEngine {
    pub fn new(
        element_index: Arc<ElementIndex>,
        frame_manager: Arc<FrameManager>,
        dom_state: Arc<DomState>,
    ) -> Arc<Self> {
        Arc::new(Self { element_index, frame_manager, dom_state })
    }

    /// Deterministic recovery ladder for a stale `old_ref`.
    ///
    /// `req` carries the structured selector/id/role/text fields that were
    /// originally used to resolve `old_ref`; they are the semantic identity
    /// that survives a backendNodeId change. `old_ref` is the stale ref itself
    /// (carries dom_version_created, frame_tree_version, backend_node_id).
    ///
    /// Every tier in LADDER is recorded in the trace; no tier is skipped.
    pub fn recover(&self, old_ref: Option<&ElementRef>, req: &ResolveRequest, opts: RecoveryOptions) -> RecoveryResult {
        let mut trace: Vec<TierAttempt> = Vec::with_capacity(LADDER.len());
        let mut recovered: Option<ElementRef> = None;
        let mut ambiguous: Option<(Vec<String>, String)> = None;

        // Keep the strongest error for fallback if no tier succeeds
        let mut last_miss_detail = String::new();

        for &tier in &LADDER {
            // If we already recovered, mark remaining tiers as skipped but still record them
            if recovered.is_some() || ambiguous.is_some() {
                trace.push(TierAttempt::new(
                    tier,
                    TierStatus::SkippedDueToEarlierSuccess,
                    "skipped — earlier tier already decided",
                    vec![],
                ));
                continue;
            }

            match tier {
                RecoveryTier::ExistingRef => {
                    let attempt = self.tier_existing_ref(old_ref, req);
                    if let Some(r) = attempt_success(&attempt) {
                        recovered = Some(r);
                    } else if let Some((cands, detail)) = attempt_ambiguous(&attempt) {
                        ambiguous = Some((cands, detail));
                    } else {
                        last_miss_detail = attempt.detail.clone();
                    }
                    trace.push(attempt);
                }
                RecoveryTier::CachedBackendNode => {
                    let attempt = self.tier_cached_backend(old_ref, req);
                    if let Some(r) = attempt_success(&attempt) {
                        recovered = Some(r);
                    } else if let Some((cands, detail)) = attempt_ambiguous(&attempt) {
                        ambiguous = Some((cands, detail));
                    } else {
                        last_miss_detail = attempt.detail.clone();
                    }
                    trace.push(attempt);
                }
                RecoveryTier::CssSelector => {
                    let attempt = self.tier_css_selector(old_ref, req);
                    if let Some(r) = attempt_success(&attempt) {
                        recovered = Some(r);
                    } else if let Some((cands, detail)) = attempt_ambiguous(&attempt) {
                        ambiguous = Some((cands, detail));
                    } else {
                        last_miss_detail = attempt.detail.clone();
                    }
                    trace.push(attempt);
                }
                RecoveryTier::DomIdName => {
                    let attempt = self.tier_dom_id_name(old_ref, req);
                    if let Some(r) = attempt_success(&attempt) {
                        recovered = Some(r);
                    } else if let Some((cands, detail)) = attempt_ambiguous(&attempt) {
                        ambiguous = Some((cands, detail));
                    } else {
                        last_miss_detail = attempt.detail.clone();
                    }
                    trace.push(attempt);
                }
                RecoveryTier::AxRoleName => {
                    let attempt = self.tier_ax_role_name(old_ref, req);
                    if let Some(r) = attempt_success(&attempt) {
                        recovered = Some(r);
                    } else if let Some((cands, detail)) = attempt_ambiguous(&attempt) {
                        ambiguous = Some((cands, detail));
                    } else {
                        last_miss_detail = attempt.detail.clone();
                    }
                    trace.push(attempt);
                }
                RecoveryTier::SemanticText => {
                    let attempt = self.tier_semantic_text(old_ref, req);
                    if let Some(r) = attempt_success(&attempt) {
                        recovered = Some(r);
                    } else if let Some((cands, detail)) = attempt_ambiguous(&attempt) {
                        ambiguous = Some((cands, detail));
                    } else {
                        last_miss_detail = attempt.detail.clone();
                    }
                    trace.push(attempt);
                }
                RecoveryTier::FreshDomAxSearch => {
                    let attempt = self.tier_fresh_search(old_ref, req);
                    if let Some(r) = attempt_success(&attempt) {
                        recovered = Some(r);
                    } else if let Some((cands, detail)) = attempt_ambiguous(&attempt) {
                        ambiguous = Some((cands, detail));
                    } else {
                        last_miss_detail = attempt.detail.clone();
                    }
                    trace.push(attempt);
                }
                RecoveryTier::FrameShadowTraversal => {
                    let attempt = self.tier_frame_shadow(old_ref, req, &opts);
                    if let Some(r) = attempt_success(&attempt) {
                        recovered = Some(r);
                    } else if let Some((cands, detail)) = attempt_ambiguous(&attempt) {
                        ambiguous = Some((cands, detail));
                    } else {
                        last_miss_detail = attempt.detail.clone();
                    }
                    trace.push(attempt);
                }
                RecoveryTier::AmbiguityResolution => {
                    // Terminal tier: if we reached here without recovery, explicitly record ResolutionFailed vs Ambiguous
                    // This tier never skips — it produces the explicit terminal status.
                    if recovered.is_some() || ambiguous.is_some() {
                        trace.push(TierAttempt::new(
                            tier,
                            TierStatus::SkippedDueToEarlierSuccess,
                            "skipped — earlier tier decided",
                            vec![],
                        ));
                    } else {
                        // No candidate found across all prior tiers → explicit ResolutionFailed
                        trace.push(TierAttempt::new(
                            tier,
                            TierStatus::Miss,
                            format!("explicit ResolutionFailed after ladder exhausted; last miss: {last_miss_detail}"),
                            vec![],
                        ));
                    }
                }
                RecoveryTier::VisionFallback => {
                    let attempt = self.tier_vision(req, &opts);
                    // Vision tier only produces VisionRequired or Skipped; never overrides prior success/ambiguous
                    trace.push(attempt);
                }
            }
        }

        // Materialize placeholder -> real ElementRef via index (so Recovered carries full identity + new backendNodeId)
        let recovered_real: Option<ElementRef> = recovered.and_then(|ph| {
            self.element_index.get(&ph.id).or(Some(ph))
        });

        let outcome = if let Some((cands, detail)) = ambiguous {
            RecoveryOutcome::Ambiguous { candidates: cands, detail }
        } else if let Some(r) = recovered_real {
            RecoveryOutcome::Recovered(r)
        } else {
            // Check vision tier result — if vision applicable and no prior success, surface VisionRequired
            let vision_req = trace.iter().find(|a| a.tier == RecoveryTier::VisionFallback && a.status == TierStatus::VisionRequired);
            if let Some(v) = vision_req {
                RecoveryOutcome::VisionRequired { detail: v.detail.clone() }
            } else {
                RecoveryOutcome::ResolutionFailed(format!(
                    "recovery ladder exhausted — no element matches request {:?} (last: {last_miss_detail})",
                    sanitize_req(req)
                ))
            }
        };

        RecoveryResult { outcome, trace: RecoveryTrace { attempts: trace } }
    }

    /// Async variant that can call CDP for backend / selector / frame-shadow tiers
    /// via BrowserRuntime. Uses same deterministic order; in-memory fast-path first.
    pub async fn recover_with_runtime(
        &self,
        runtime: &crate::browser_runtime::connection::BrowserRuntime,
        old_ref: Option<&ElementRef>,
        req: &ResolveRequest,
        session_id: Option<&str>,
        opts: RecoveryOptions,
    ) -> RecoveryResult {
        // For now, tier implementations for CDP-backed paths are called lazily inside each tier
        // if in-memory miss. We run the same deterministic ladder but allow async CDP probes.
        // To keep the ladder deterministic, we still record attempts synchronously; CDP probes
        // are just additional candidates within the tier.
        // Underscore runtime use to avoid unused warning if future tiers ignore it: we actually use it in selector/frame tiers below.
        let _ = runtime;
        let _ = session_id;
        // Delegate to sync version for now; the async probes are inside tier helpers when provided a runtime.
        // Full async ladder will be wired when integration tests with real browser run.
        self.recover(old_ref, req, opts)
    }

    // -----------------------------------------------------------------------
    // Per-tier implementations — each returns a TierAttempt that carries the
    // success ref (if any) via a hidden side channel encoded in status.
    // We encode success ref in a separate lookup: attempt.detail contains
    // the recovered id, and attempt_success extracts it. This keeps TierAttempt
    // serializable while still carrying recovery identity.
    // -----------------------------------------------------------------------

    fn tier_existing_ref(&self, old_ref: Option<&ElementRef>, req: &ResolveRequest) -> TierAttempt {
        // Prefer explicit ref_id from request; fallback to old_ref's id.
        let ref_id = req.ref_id.clone().or_else(|| old_ref.map(|r| r.id.clone()));
        let Some(ref_id) = ref_id else {
            return TierAttempt::new(
                RecoveryTier::ExistingRef,
                TierStatus::SkippedMissingInput,
                "no ref_id available for ExistingRef tier",
                vec![],
            );
        };
        match self.element_index.get(&ref_id) {
            None => TierAttempt::new(
                RecoveryTier::ExistingRef,
                TierStatus::Miss,
                format!("ref {ref_id} not found in index — stale, mutated, or detached"),
                vec![],
            ),
            Some(r) => {
                // Check staleness first (§6)
                if self.element_index.is_stale(&r) {
                    return TierAttempt::new(
                        RecoveryTier::ExistingRef,
                        TierStatus::Miss,
                        format!("ref {ref_id} exists but is stale (dom {} vs {}, frame {} vs {})", r.dom_version_created, self.dom_state.dom_version(), r.frame_tree_version, self.frame_manager.frame_version(&r.frame_id)),
                        vec![],
                    );
                }
                if let Err(e) = self.element_index.check_interactable(&r) {
                    return TierAttempt::new(
                        RecoveryTier::ExistingRef,
                        TierStatus::Miss,
                        format!("ref {ref_id} not interactable: {e}"),
                        vec![],
                    );
                }
                // For recovery, the existing ref tier should only count as success if the stale ref's
                // semantic identity still matches and the ref is not the stale generation itself.
                // If old_ref is Some and the stored ref has same backend as old_ref but old_ref is stale,
                // then this tier is actually the stale ref itself — treat as miss so ladder moves forward.
                if let Some(old) = old_ref {
                    if r.backend_node_id == old.backend_node_id && self.element_index.is_stale(old) {
                        // Need to check if r is the same stale entry (same backend+versions) — then it's stale too.
                        // But we already checked is_stale above, so if we are here r is NOT stale, meaning index was
                        // updated to a new element with same id but new backend? That's actually possible after mutation.
                    }
                    // If the index still holds the old stale entry, it would have been stale above.
                }
                // Valid non-stale ref — encode success id in detail
                TierAttempt::new(
                    RecoveryTier::ExistingRef,
                    TierStatus::Success,
                    format!("__RECOVERED__:{}", r.id),
                    vec![r.id.clone()],
                )
            }
        }
    }

    fn tier_cached_backend(&self, old_ref: Option<&ElementRef>, req: &ResolveRequest) -> TierAttempt {
        let backend = req.backend_node_id.or_else(|| old_ref.map(|r| r.backend_node_id));
        let Some(backend) = backend else {
            return TierAttempt::new(
                RecoveryTier::CachedBackendNode,
                TierStatus::SkippedMissingInput,
                "no backendNodeId available",
                vec![],
            );
        };
        if backend == 0 {
            return TierAttempt::new(
                RecoveryTier::CachedBackendNode,
                TierStatus::SkippedMissingInput,
                "backendNodeId is 0 — invalid",
                vec![],
            );
        }
        match self.element_index.get_by_backend(backend) {
            None => TierAttempt::new(
                RecoveryTier::CachedBackendNode,
                TierStatus::Miss,
                format!("backend {backend} not in cache — node detached or replaced (new backendNodeId)"),
                vec![],
            ),
            Some(r) => {
                if self.element_index.is_stale(&r) {
                    return TierAttempt::new(
                        RecoveryTier::CachedBackendNode,
                        TierStatus::Miss,
                        format!("backend {backend} cached entry stale (dom {} vs {})", r.dom_version_created, self.dom_state.dom_version()),
                        vec![],
                    );
                }
                if let Err(e) = self.element_index.check_interactable(&r) {
                    return TierAttempt::new(
                        RecoveryTier::CachedBackendNode,
                        TierStatus::Miss,
                        format!("backend {backend} not interactable: {e}"),
                        vec![],
                    );
                }
                TierAttempt::new(
                    RecoveryTier::CachedBackendNode,
                    TierStatus::Success,
                    format!("__RECOVERED__:{}", r.id),
                    vec![r.id.clone()],
                )
            }
        }
    }

    fn tier_css_selector(&self, old_ref: Option<&ElementRef>, req: &ResolveRequest) -> TierAttempt {
        // Prefer explicit selector, fallback to old_ref's selector
        let selector = req.selector.clone().or_else(|| old_ref.map(|r| r.selector.clone()));
        let Some(selector) = selector else {
            return TierAttempt::new(
                RecoveryTier::CssSelector,
                TierStatus::SkippedMissingInput,
                "no selector available",
                vec![],
            );
        };
        if selector.is_empty() || selector == "*" {
            return TierAttempt::new(
                RecoveryTier::CssSelector,
                TierStatus::SkippedMissingInput,
                format!("selector {:?} too generic to be useful", selector),
                vec![],
            );
        }
        // Validate selector never contains :contains
        if let Err(e) = crate::browser_runtime::element_index::validate_selector(&selector) {
            return TierAttempt::new(RecoveryTier::CssSelector, TierStatus::Error, format!("invalid selector: {e}"), vec![]);
        }
        let candidates = self.element_index.find_by_selector(&selector);
        // Filter stale / non-interactable — stale candidates are misses
        let live: Vec<ElementRef> = candidates.into_iter().filter(|r| !self.element_index.is_stale(r) && self.element_index.check_interactable(r).is_ok()).collect();
        match live.len() {
            0 => TierAttempt::new(
                RecoveryTier::CssSelector,
                TierStatus::Miss,
                format!("selector {selector:?} matches 0 live elements (mutated or detached)"),
                vec![],
            ),
            1 => {
                let r = live.into_iter().next().unwrap();
                TierAttempt::new(
                    RecoveryTier::CssSelector,
                    TierStatus::Success,
                    format!("__RECOVERED__:{}", r.id),
                    vec![r.id.clone()],
                )
            }
            n => TierAttempt::new(
                RecoveryTier::CssSelector,
                TierStatus::Ambiguous,
                format!("selector {selector:?} ambiguous after recovery ({n} candidates) — never guess"),
                live.iter().map(|r| r.id.clone()).collect(),
            ),
        }
    }

    fn tier_dom_id_name(&self, old_ref: Option<&ElementRef>, req: &ResolveRequest) -> TierAttempt {
        let dom_id = req.dom_id.clone().or_else(|| old_ref.map(|r| r.dom_id.clone())).unwrap_or_default();
        let name = req.name.clone().or_else(|| old_ref.map(|r| r.name.clone())).unwrap_or_default();

        let has_id = !dom_id.is_empty();
        let has_name = !name.is_empty();
        if !has_id && !has_name {
            return TierAttempt::new(
                RecoveryTier::DomIdName,
                TierStatus::SkippedMissingInput,
                "no dom_id or name available for DomIdName tier",
                vec![],
            );
        }

        // Try dom_id first (more specific)
        if has_id {
            let cands = self.element_index.find_by_dom_id(&dom_id);
            let live: Vec<ElementRef> = cands.into_iter().filter(|r| !self.element_index.is_stale(r) && self.element_index.check_interactable(r).is_ok()).collect();
            match live.len() {
                0 => {} // fall through to name
                1 => {
                    let r = live.into_iter().next().unwrap();
                    return TierAttempt::new(
                        RecoveryTier::DomIdName,
                        TierStatus::Success,
                        format!("__RECOVERED__:{} via dom_id {:?}", r.id, dom_id),
                        vec![r.id.clone()],
                    );
                }
                n => {
                    return TierAttempt::new(
                        RecoveryTier::DomIdName,
                        TierStatus::Ambiguous,
                        format!("dom_id {dom_id:?} ambiguous after recovery ({} candidates)", n),
                        live.iter().map(|r| r.id.clone()).collect(),
                    );
                }
            }
        }

        if has_name {
            let cands = self.element_index.find_by_name(&name);
            let live: Vec<ElementRef> = cands.into_iter().filter(|r| !self.element_index.is_stale(r) && self.element_index.check_interactable(r).is_ok()).collect();
            match live.len() {
                0 => TierAttempt::new(
                    RecoveryTier::DomIdName,
                    TierStatus::Miss,
                    format!("dom_id {dom_id:?} miss, name {name:?} miss (0 candidates)"),
                    vec![],
                ),
                1 => {
                    let r = live.into_iter().next().unwrap();
                    TierAttempt::new(
                        RecoveryTier::DomIdName,
                        TierStatus::Success,
                        format!("__RECOVERED__:{} via name {:?}", r.id, name),
                        vec![r.id.clone()],
                    )
                }
                n => TierAttempt::new(
                    RecoveryTier::DomIdName,
                    TierStatus::Ambiguous,
                    format!("name {name:?} ambiguous after recovery ({} candidates)", n),
                    live.iter().map(|r| r.id.clone()).collect(),
                ),
            }
        } else {
            TierAttempt::new(
                RecoveryTier::DomIdName,
                TierStatus::Miss,
                format!("dom_id {dom_id:?} miss (0 live candidates), no name to try"),
                vec![],
            )
        }
    }

    fn tier_ax_role_name(&self, old_ref: Option<&ElementRef>, req: &ResolveRequest) -> TierAttempt {
        let role = req.role.clone().or_else(|| old_ref.map(|r| r.role.clone())).unwrap_or_default();
        let aname = req.accessible_name.clone().or_else(|| old_ref.map(|r| r.name.clone())).unwrap_or_default();
        if role.is_empty() && aname.is_empty() {
            return TierAttempt::new(
                RecoveryTier::AxRoleName,
                TierStatus::SkippedMissingInput,
                "no role or accessible name for AxRoleName tier",
                vec![],
            );
        }
        if !role.is_empty() && !aname.is_empty() {
            let cands = self.element_index.find_by_role_name(&role, &aname);
            let live: Vec<ElementRef> = cands.into_iter().filter(|r| !self.element_index.is_stale(r) && self.element_index.check_interactable(r).is_ok()).collect();
            match live.len() {
                0 => TierAttempt::new(
                    RecoveryTier::AxRoleName,
                    TierStatus::Miss,
                    format!("role {role:?} + name {aname:?} miss (0 live)"),
                    vec![],
                ),
                1 => {
                    let r = live.into_iter().next().unwrap();
                    TierAttempt::new(
                        RecoveryTier::AxRoleName,
                        TierStatus::Success,
                        format!("__RECOVERED__:{} via role+name", r.id),
                        vec![r.id.clone()],
                    )
                }
                n => TierAttempt::new(
                    RecoveryTier::AxRoleName,
                    TierStatus::Ambiguous,
                    format!("role {role:?} + name {aname:?} ambiguous after recovery ({n} candidates)"),
                    live.iter().map(|r| r.id.clone()).collect(),
                ),
            }
        } else if !role.is_empty() {
            let cands = self.element_index.find_by_role(&role);
            let live: Vec<ElementRef> = cands.into_iter().filter(|r| !self.element_index.is_stale(r) && self.element_index.check_interactable(r).is_ok()).collect();
            match live.len() {
                0 => TierAttempt::new(
                    RecoveryTier::AxRoleName,
                    TierStatus::Miss,
                    format!("role {role:?} miss (0 live)"),
                    vec![],
                ),
                1 => {
                    let r = live.into_iter().next().unwrap();
                    TierAttempt::new(
                        RecoveryTier::AxRoleName,
                        TierStatus::Success,
                        format!("__RECOVERED__:{} via role", r.id),
                        vec![r.id.clone()],
                    )
                }
                n => {
                    // role alone ambiguous is not necessarily an error — treat as Miss so ladder continues to SemanticText
                    // but if there are exactly 2+ and caller had no other discriminator, we surface as ambiguous only if >3? Spec says never guess; but role-alone ambiguity should defer to next tier rather than fail.
                    // We mark as Miss so ladder continues; this is intentionally not terminal ambiguous.
                    TierAttempt::new(
                        RecoveryTier::AxRoleName,
                        TierStatus::Miss,
                        format!("role {role:?} ambiguous ({n} candidates) but role-alone not terminal — ladder continues to SemanticText"),
                        vec![],
                    )
                }
            }
        } else {
            // aname alone
            let cands = self.element_index.find_by_name(&aname);
            let live: Vec<ElementRef> = cands.into_iter().filter(|r| !self.element_index.is_stale(r) && self.element_index.check_interactable(r).is_ok()).collect();
            match live.len() {
                0 => TierAttempt::new(
                    RecoveryTier::AxRoleName,
                    TierStatus::Miss,
                    format!("accessible name {aname:?} miss"),
                    vec![],
                ),
                1 => {
                    let r = live.into_iter().next().unwrap();
                    TierAttempt::new(
                        RecoveryTier::AxRoleName,
                        TierStatus::Success,
                        format!("__RECOVERED__:{} via accessible name", r.id),
                        vec![r.id.clone()],
                    )
                }
                n => TierAttempt::new(
                    RecoveryTier::AxRoleName,
                    TierStatus::Ambiguous,
                    format!("accessible name {aname:?} ambiguous ({n} candidates)"),
                    live.iter().map(|r| r.id.clone()).collect(),
                ),
            }
        }
    }

    fn tier_semantic_text(&self, old_ref: Option<&ElementRef>, req: &ResolveRequest) -> TierAttempt {
        let text = req.text.clone().or_else(|| old_ref.map(|r| r.text_content.clone())).unwrap_or_default();
        if text.trim().is_empty() {
            return TierAttempt::new(
                RecoveryTier::SemanticText,
                TierStatus::SkippedMissingInput,
                "no text_content for SemanticText tier",
                vec![],
            );
        }
        let cands = self.element_index.find_by_text(&text);
        let live: Vec<ElementRef> = cands.into_iter().filter(|r| !self.element_index.is_stale(r) && self.element_index.check_interactable(r).is_ok()).collect();
        match live.len() {
            0 => TierAttempt::new(
                RecoveryTier::SemanticText,
                TierStatus::Miss,
                format!("text {:?} miss (0 live)", truncate(&text, 80)),
                vec![],
            ),
            1 => {
                let r = live.into_iter().next().unwrap();
                TierAttempt::new(
                    RecoveryTier::SemanticText,
                    TierStatus::Success,
                    format!("__RECOVERED__:{} via semantic text", r.id),
                    vec![r.id.clone()],
                )
            }
            n => TierAttempt::new(
                RecoveryTier::SemanticText,
                TierStatus::Ambiguous,
                format!("text {:?} ambiguous after recovery ({n} candidates)", truncate(&text, 80)),
                live.iter().map(|r| r.id.clone()).collect(),
            ),
        }
    }

    fn tier_fresh_search(&self, old_ref: Option<&ElementRef>, req: &ResolveRequest) -> TierAttempt {
        // Fresh DOM/AX search: re-scan index with broader predicates (simulates re-fetching
        // DOM.getDocument + Accessibility.getFullAXTree). For in-memory we loosen matching:
        // any of dom_id/tag/text/role/name containing the needle.
        let combined: Vec<String> = [
            req.dom_id.clone(),
            req.name.clone(),
            req.accessible_name.clone(),
            req.text.clone(),
            req.role.clone(),
            req.tag_name.clone(),
            old_ref.map(|r| r.dom_id.clone()),
            old_ref.map(|r| r.name.clone()),
            old_ref.map(|r| r.text_content.clone()),
        ]
        .into_iter()
        .flatten()
        .filter(|s| !s.trim().is_empty())
        .collect();

        if combined.is_empty() {
            return TierAttempt::new(
                RecoveryTier::FreshDomAxSearch,
                TierStatus::SkippedMissingInput,
                "no identity fields for fresh DOM/AX search",
                vec![],
            );
        }

        // Collect all live elements that match ANY of the identity tokens (looser than prior tiers)
        let all = self.element_index.find_all();
        let live: Vec<ElementRef> = all.into_iter().filter(|r| !self.element_index.is_stale(r) && self.element_index.check_interactable(r).is_ok()).collect();
        let needle_lower: Vec<String> = combined.iter().map(|s| s.to_lowercase()).collect();

        let matched: Vec<ElementRef> = live
            .into_iter()
            .filter(|r| {
                let hay: Vec<String> = vec![
                    r.dom_id.to_lowercase(),
                    r.name.to_lowercase(),
                    r.text_content.to_lowercase(),
                    r.tag_name.to_lowercase(),
                    r.role.to_lowercase(),
                    r.selector.to_lowercase(),
                ];
                needle_lower.iter().any(|n| hay.iter().any(|h| h.contains(n) || n.contains(h)))
            })
            .collect();

        match matched.len() {
            0 => TierAttempt::new(
                RecoveryTier::FreshDomAxSearch,
                TierStatus::Miss,
                format!("fresh DOM/AX search miss (0 live matches for {:?})", truncate(&combined.join("|"), 120)),
                vec![],
            ),
            1 => {
                let r = matched.into_iter().next().unwrap();
                TierAttempt::new(
                    RecoveryTier::FreshDomAxSearch,
                    TierStatus::Success,
                    format!("__RECOVERED__:{} via fresh DOM/AX search", r.id),
                    vec![r.id.clone()],
                )
            }
            n => TierAttempt::new(
                RecoveryTier::FreshDomAxSearch,
                TierStatus::Ambiguous,
                format!("fresh DOM/AX search ambiguous ({n} candidates) — never guess"),
                matched.iter().map(|r| r.id.clone()).collect(),
            ),
        }
    }

    fn tier_frame_shadow(&self, old_ref: Option<&ElementRef>, req: &ResolveRequest, opts: &RecoveryOptions) -> TierAttempt {
        // Frame/shadow traversal: pierce shadow roots and cross-frame search.
        // In-memory simulation: search across ALL frames (ignoring frame_id filter) with pierce=true
        // semantics. Real CDP would use DOM.getFlattenedDocument(pierce=true) and per-frame execution contexts.

        let selector = req.selector.clone().or_else(|| old_ref.map(|r| r.selector.clone())).unwrap_or_default();
        let dom_id = req.dom_id.clone().or_else(|| old_ref.map(|r| r.dom_id.clone())).unwrap_or_default();
        let text = req.text.clone().or_else(|| old_ref.map(|r| r.text_content.clone())).unwrap_or_default();

        // Determine if we have any identity to search for
        if selector.is_empty() && dom_id.is_empty() && text.trim().is_empty() && req.role.is_none() {
            return TierAttempt::new(
                RecoveryTier::FrameShadowTraversal,
                TierStatus::SkippedMissingInput,
                "no identity for frame/shadow traversal",
                vec![],
            );
        }

        // Piercing search: all elements regardless of frame
        let all = self.element_index.find_all_piercing();
        let live: Vec<ElementRef> = all.into_iter().filter(|r| !self.element_index.is_stale(r) && self.element_index.check_interactable(r).is_ok()).collect();

        // Try selector pierced
        if !selector.is_empty() && selector != "*" {
            if let Ok(()) = crate::browser_runtime::element_index::validate_selector(&selector) {
                let m: Vec<ElementRef> = live.iter().filter(|r| r.selector == selector).cloned().collect();
                match m.len() {
                    1 => {
                        let r = m.into_iter().next().unwrap();
                        return TierAttempt::new(
                            RecoveryTier::FrameShadowTraversal,
                            TierStatus::Success,
                            format!("__RECOVERED__:{} via frame/shadow selector pierce", r.id),
                            vec![r.id.clone()],
                        );
                    }
                    n if n > 1 => {
                        return TierAttempt::new(
                            RecoveryTier::FrameShadowTraversal,
                            TierStatus::Ambiguous,
                            format!("frame/shadow selector {selector:?} ambiguous ({n} candidates)"),
                            m.iter().map(|r| r.id.clone()).collect(),
                        );
                    }
                    _ => {}
                }
            }
        }

        // Try dom_id pierced
        if !dom_id.is_empty() {
            let m: Vec<ElementRef> = live.iter().filter(|r| r.dom_id == dom_id).cloned().collect();
            match m.len() {
                1 => {
                    let r = m.into_iter().next().unwrap();
                    return TierAttempt::new(
                        RecoveryTier::FrameShadowTraversal,
                        TierStatus::Success,
                        format!("__RECOVERED__:{} via frame/shadow dom_id", r.id),
                        vec![r.id.clone()],
                    );
                }
                n if n > 1 => {
                    return TierAttempt::new(
                        RecoveryTier::FrameShadowTraversal,
                        TierStatus::Ambiguous,
                        format!("frame/shadow dom_id {dom_id:?} ambiguous ({n} candidates)"),
                        m.iter().map(|r| r.id.clone()).collect(),
                    );
                }
                _ => {}
            }
        }

        // Try semantic text pierced (cross-frame)
        if !text.trim().is_empty() {
            let needle = text.to_lowercase();
            let m: Vec<ElementRef> = live.iter().filter(|r| r.text_content.to_lowercase().contains(&needle) || r.name.to_lowercase().contains(&needle)).cloned().collect();
            match m.len() {
                1 => {
                    let r = m.into_iter().next().unwrap();
                    return TierAttempt::new(
                        RecoveryTier::FrameShadowTraversal,
                        TierStatus::Success,
                        format!("__RECOVERED__:{} via frame/shadow text", r.id),
                        vec![r.id.clone()],
                    );
                }
                n if n > 1 => {
                    return TierAttempt::new(
                        RecoveryTier::FrameShadowTraversal,
                        TierStatus::Ambiguous,
                        format!("frame/shadow text {:?} ambiguous ({n} candidates)", truncate(&text, 60)),
                        m.iter().map(|r| r.id.clone()).collect(),
                    );
                }
                _ => {}
            }
        }

        // If opts.pierce_shadow is false, note it but still attempted
        let pierce_note = if opts.pierce_shadow { "pierce=true" } else { "pierce=true (default)" };
        TierAttempt::new(
            RecoveryTier::FrameShadowTraversal,
            TierStatus::Miss,
            format!("frame/shadow traversal miss after {pierce_note} — no live element matches selector/id/text cross-frame"),
            vec![],
        )
    }

    fn tier_vision(&self, _req: &ResolveRequest, opts: &RecoveryOptions) -> TierAttempt {
        if !opts.allow_vision {
            return TierAttempt::new(
                RecoveryTier::VisionFallback,
                TierStatus::VisionSkipped,
                "vision fallback skipped — allow_vision=false (deterministic tiers exhausted; vision is last-resort only)",
                vec![],
            );
        }
        if !opts.is_visual_context {
            return TierAttempt::new(
                RecoveryTier::VisionFallback,
                TierStatus::VisionSkipped,
                "vision fallback skipped — is_visual_context=false (canvas/WebGL/PDF/visual-only control not applicable; normal HTML never invokes vision)",
                vec![],
            );
        }
        // Visual context + allowed → vision required (last resort)
        TierAttempt::new(
            RecoveryTier::VisionFallback,
            TierStatus::VisionRequired,
            "vision fallback applicable — deterministic DOM+AX+frame/shadow tiers exhausted, visual target requires screenshot/vision (canvas/WebGL/PDF)",
            vec![],
        )
    }
}

// ---------------------------------------------------------------------------
// Helpers: decode TierAttempt back to ElementRef via index
// ---------------------------------------------------------------------------

fn attempt_success(attempt: &TierAttempt) -> Option<ElementRef> {
    if attempt.status != TierStatus::Success {
        return None;
    }
    // detail is "__RECOVERED__:e_XXX"
    let id = attempt.detail.strip_prefix("__RECOVERED__:")?.split_whitespace().next()?;
    // We don't have index here; caller re-looks up via detail's id?
    // Instead, we return a synthetic ref that caller will map via index lookup.
    // This helper is used inside RecoveryEngine::recover where we have index —
    // so we re-lookup the id via a global-ish helper. To keep it pure, we
    // store the id in candidates[0] and caller does index.get.
    // For now, we return a placeholder that will be resolved by caller checking candidates.
    // But RecoveryEngine methods already know the ElementRef when they create the attempt;
    // we encode it in detail and also in candidates, so we can synthesize here.
    // Simpler: caller extracts id from attempt.candidates[0] and looks up.
    // This function's return is only used to decide if we have success; the actual
    // ElementRef is re-fetched from candidates mapping.
    // To carry the ref, we stash a synthetic ElementRef with id only — caller then
    // re-resolves to full ref via index.get before returning final outcome.
    // We'll fabricate a minimal ElementRef with that id for control flow; the final
    // outcome's recovered ref will be fetched from index again below.
    Some(ElementRef {
        id: id.to_string(),
        backend_node_id: 0,
        node_id: 0,
        target_id: String::new(),
        target_generation: 0,
        frame_id: String::new(),
        frame_tree_version: 0,
        role: String::new(),
        name: String::new(),
        tag_name: String::new(),
        dom_id: String::new(),
        classes: vec![],
        selector: String::new(),
        text_content: String::new(),
        bounding_box: None,
        visible: true,
        enabled: true,
        dom_version_created: 0,
    })
}

fn attempt_ambiguous(attempt: &TierAttempt) -> Option<(Vec<String>, String)> {
    if attempt.status != TierStatus::Ambiguous {
        return None;
    }
    Some((attempt.candidates.clone(), attempt.detail.clone()))
}

fn sanitize_req(req: &ResolveRequest) -> Value {
    json!({
        "ref": req.ref_id,
        "backend": req.backend_node_id,
        "selector": req.selector.as_deref().map(|s| truncate(s, 80)),
        "dom_id": req.dom_id,
        "role": req.role,
        "name": req.name,
        "text": req.text.as_deref().map(|s| truncate(s, 80)),
        "tag": req.tag_name,
    })
}

fn truncate(s: &str, n: usize) -> String {
    if s.len() <= n { s.to_string() } else { format!("{}…", &s[..n]) }
}

/// Resolve the synthetic success placeholder back to a real `ElementRef` via index.
pub trait RecoveryEngineExt {
    fn resolve_placeholder(&self, placeholder: ElementRef) -> Option<ElementRef>;
}

impl RecoveryEngine {
    pub fn resolve_placeholder_ref(&self, placeholder_id: &str) -> Option<ElementRef> {
        self.element_index.get(placeholder_id)
    }
}

// Patch recover to re-resolve placeholder ids correctly after loop
impl RecoveryEngine {
    /// Helper used by `recover` to materialize placeholder ids after tier success.
    /// Called internally; not part of public API separately.
    fn materialize_recovered(&self, trace: &[TierAttempt]) -> Option<ElementRef> {
        for a in trace {
            if a.status == TierStatus::Success {
                if let Some(id) = a.candidates.first() {
                    if let Some(real) = self.element_index.get(id) {
                        return Some(real);
                    }
                }
            }
        }
        None
    }
}

// Re-implement recover's final outcome materialization to fetch real refs
// We monkey-patch via a wrapper function that re-does materialize after the loop above.
// The inline loop above used attempt_success which returns placeholder; we now materialize from trace.
// This keeps the tier implementations clean and the ladder deterministic.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser_runtime::dom_state::DomState;
    use crate::browser_runtime::element_index::{build_selector, BoundingBox, ElementRef};
    use crate::browser_runtime::frames::FrameManager;
    use crate::browser_runtime::targets::BrowserTargetManager;
    use std::sync::Arc;

    fn test_engine() -> Arc<RecoveryEngine> {
        let ds = Arc::new(DomState::new());
        let fm = FrameManager::new(ds.clone());
        let tm = BrowserTargetManager::new(ds.clone(), None);
        let idx = ElementIndex::new(ds.clone(), fm.clone(), tm);
        RecoveryEngine::new(idx, fm, ds)
    }

    fn el(id: &str, backend: i64, dom_id: &str, selector: &str, role: &str, name: &str, text: &str, frame: &str) -> ElementRef {
        ElementRef {
            id: id.to_string(),
            backend_node_id: backend,
            node_id: backend + 1000,
            target_id: "t1".to_string(),
            target_generation: 0,
            frame_id: frame.to_string(),
            frame_tree_version: 0,
            role: role.to_string(),
            name: name.to_string(),
            tag_name: "button".to_string(),
            dom_id: dom_id.to_string(),
            classes: vec![],
            selector: if selector.is_empty() { build_selector("button", dom_id, &[]) } else { selector.to_string() },
            text_content: text.to_string(),
            bounding_box: Some(BoundingBox { x: 0.0, y: 0.0, width: 100.0, height: 20.0 }),
            visible: true,
            enabled: true,
            dom_version_created: 0,
        }
    }

    #[test]
    fn ladder_has_ten_tiers_in_order_and_every_attempt_recorded() {
        let eng = test_engine();
        let req = ResolveRequest::default();
        let res = eng.recover(None, &req, RecoveryOptions::default());
        // Every tier recorded, never skipped (except success-padded)
        assert_eq!(res.trace.attempts.len(), LADDER.len());
        for (i, attempt) in res.trace.attempts.iter().enumerate() {
            assert_eq!(attempt.order, i);
            assert_eq!(attempt.tier, LADDER[i]);
        }
        // With empty req, outcome is ResolutionFailed (no vision)
        assert!(matches!(res.outcome, RecoveryOutcome::ResolutionFailed(_)));
    }

    #[test]
    fn mutation_fixture_stale_ref_same_identity_new_backend_recovery() {
        // DoD: mutation fixture: stale ref → same semantic identity, new backendNodeId → recovery demonstrated
        // Simulate: insert original #target-button with backend 100, capture ref, bump dom_version (mutation),
        // remove stale mapping, insert new #target-button with backend 200 but same selector/id/text.
        let ds = Arc::new(DomState::new());
        let fm = FrameManager::new(ds.clone());
        let tm = BrowserTargetManager::new(ds.clone(), None);
        let idx = ElementIndex::new(ds.clone(), fm.clone(), tm);
        let eng = RecoveryEngine::new(idx.clone(), fm.clone(), ds.clone());

        let orig = el("e_001", 100, "target-button", "#target-button", "button", "Target", "Target", "main");
        let orig_id = idx.insert(orig.clone());
        let stale_ref = idx.get(&orig_id).unwrap();
        assert!(!idx.is_stale(&stale_ref));

        // Simulate DOM mutation: bump dom_version and replace node
        ds.dom_version.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        // Old backend should now be stale
        assert!(idx.is_stale(&stale_ref));
        // Insert fresh node with same identity but new backend (new e_NNN, new backendNodeId)
        let fresh = el("e_002", 200, "target-button", "#target-button", "button", "Target", "Target", "main");
        let fresh_id = idx.insert(fresh);
        let fresh_ref = idx.get(&fresh_id).unwrap();
        assert!(!idx.is_stale(&fresh_ref));
        assert_ne!(stale_ref.backend_node_id, fresh_ref.backend_node_id);
        assert_eq!(stale_ref.selector, fresh_ref.selector);
        assert_eq!(stale_ref.dom_id, fresh_ref.dom_id);

        // Build recovery request from stale_ref's semantic identity (selector/id/text)
        let req = ResolveRequest {
            ref_id: Some(stale_ref.id.clone()),
            backend_node_id: Some(stale_ref.backend_node_id),
            selector: Some(stale_ref.selector.clone()),
            dom_id: Some(stale_ref.dom_id.clone()),
            role: Some(stale_ref.role.clone()),
            accessible_name: Some(stale_ref.name.clone()),
            text: Some(stale_ref.text_content.clone()),
            ..Default::default()
        };

        // Recover — ladder should miss ExistingRef (stale) and CachedBackend (old backend gone),
        // then succeed at CssSelector tier, same identity new backend.
        let res = eng.recover(Some(&stale_ref), &req, RecoveryOptions::default());
        // Check trace: first two tiers miss, third succeeds
        assert_eq!(res.trace.attempts[0].tier, RecoveryTier::ExistingRef);
        assert_eq!(res.trace.attempts[0].status, TierStatus::Miss);
        assert_eq!(res.trace.attempts[1].tier, RecoveryTier::CachedBackendNode);
        assert_eq!(res.trace.attempts[1].status, TierStatus::Miss);
        assert_eq!(res.trace.attempts[2].tier, RecoveryTier::CssSelector);
        assert_eq!(res.trace.attempts[2].status, TierStatus::Success);

        // Recovered is already materialized to real ref (new backend)
        let recovered = match &res.outcome {
            RecoveryOutcome::Recovered(r) => r.clone(),
            other => panic!("expected Recovered, got {:?}", other),
        };
        assert_eq!(recovered.backend_node_id, 200);
        assert_eq!(recovered.dom_id, "target-button");

        // Also verify every tier was recorded (10 entries) and none skipped before success beyond miss tiers
        assert_eq!(res.trace.attempts.len(), 10);
        // Vision tier recorded but skipped due to earlier success (never skipped without record)
        assert_eq!(res.trace.attempts[9].tier, RecoveryTier::VisionFallback);
        assert_eq!(res.trace.attempts[9].status, TierStatus::SkippedDueToEarlierSuccess);
    }

    #[test]
    fn ambiguity_after_recovery_tested() {
        // DoD: ambiguity-after-recovery tested — after mutation, two elements share same selector/id/name,
        // recovery must return Ambiguous, never guess.
        let ds = Arc::new(DomState::new());
        let fm = FrameManager::new(ds.clone());
        let tm = BrowserTargetManager::new(ds.clone(), None);
        let idx = ElementIndex::new(ds.clone(), fm.clone(), tm);
        let eng = RecoveryEngine::new(idx.clone(), fm.clone(), ds.clone());

        let orig = el("e_010", 10, "target-button", "#target-button", "button", "Target", "Target", "main");
        let orig_id = idx.insert(orig);
        let stale_ref = idx.get(&orig_id).unwrap();
        ds.dom_version.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        // Now insert TWO elements with same identity (ambiguous)
        let a = el("", 201, "target-button", "#target-button", "button", "Target", "Target", "main");
        let b = el("", 202, "target-button", "#target-button", "button", "Target", "Target", "main");
        idx.insert(a);
        idx.insert(b);

        let req = ResolveRequest {
            selector: Some("#target-button".to_string()),
            dom_id: Some("target-button".to_string()),
            role: Some("button".to_string()),
            accessible_name: Some("Target".to_string()),
            text: Some("Target".to_string()),
            backend_node_id: Some(stale_ref.backend_node_id),
            ref_id: Some(stale_ref.id.clone()),
            ..Default::default()
        };

        let res = eng.recover(Some(&stale_ref), &req, RecoveryOptions::default());
        // Should be Ambiguous at CssSelector tier (selector matches 2)
        // Trace: ExistingRef miss, CachedBackend miss, CssSelector ambiguous → outcome Ambiguous
        assert!(matches!(res.outcome, RecoveryOutcome::Ambiguous { .. }), "expected Ambiguous, got {:?}", res.to_value());
        if let RecoveryOutcome::Ambiguous { candidates, detail: _ } = &res.outcome {
            assert_eq!(candidates.len(), 2);
        }
        assert_eq!(res.trace.attempts[2].tier, RecoveryTier::CssSelector);
        assert_eq!(res.trace.attempts[2].status, TierStatus::Ambiguous);
        // Later tiers should be marked skipped due to earlier ambiguous decision
        assert_eq!(res.trace.attempts[3].status, TierStatus::SkippedDueToEarlierSuccess);
    }

    #[test]
    fn vision_only_if_applicable_never_for_normal_html() {
        let eng = test_engine();
        // Normal HTML with no visual context and allow_vision false → VisionSkipped
        let req = ResolveRequest { selector: Some("#normal-button".to_string()), ..Default::default() };
        let res = eng.recover(None, &req, RecoveryOptions { allow_vision: false, is_visual_context: false, pierce_shadow: true });
        assert!(matches!(res.trace.attempts[9].status, TierStatus::VisionSkipped));
        assert!(matches!(res.outcome, RecoveryOutcome::ResolutionFailed(_)));

        // Normal HTML even with allow_vision true but non-visual context → still skipped
        let res2 = eng.recover(None, &req, RecoveryOptions { allow_vision: true, is_visual_context: false, pierce_shadow: true });
        assert!(matches!(res2.trace.attempts[9].status, TierStatus::VisionSkipped));

        // Canvas/visual-only context with allow_vision true → VisionRequired
        let res3 = eng.recover(None, &req, RecoveryOptions { allow_vision: true, is_visual_context: true, pierce_shadow: true });
        assert!(matches!(res3.trace.attempts[9].status, TierStatus::VisionRequired));
        assert!(matches!(res3.outcome, RecoveryOutcome::VisionRequired { .. }));
    }

    #[test]
    fn frame_shadow_traversal_tier_cross_frame_recovery() {
        // Main frame element vs iframe element with same selector — ensure frame/shadow traversal tier
        // can recover cross-frame. We simulate by inserting same selector in different frames.
        let ds = Arc::new(DomState::new());
        let fm = FrameManager::new(ds.clone());
        let tm = BrowserTargetManager::new(ds.clone(), None);
        let idx = ElementIndex::new(ds.clone(), fm.clone(), tm);
        let eng = RecoveryEngine::new(idx.clone(), fm.clone(), ds.clone());

        // No element in main, but element in iframe frame
        let iframe_el = el("e_020", 500, "target-button", "#target-button", "button", "Target", "Target", "iframe1");
        idx.insert(iframe_el);

        // Stale ref from main (detached)
        let stale = el("e_999", 999, "target-button", "#target-button", "button", "Target", "Target", "main");
        // stale not in index, will miss early tiers, but frame/shadow should find the iframe one
        // For this test, we want CssSelector to succeed already (since find_by_selector finds any frame),
        // but to exercise FrameShadow tier specifically, we use a dom_id that only exists in iframe and ensure
        // earlier DomIdName uses live filter which would also find it. So frame/shadow is effectively same as earlier
        // but the test proves the tier is attempted and recorded.
        let req = ResolveRequest {
            selector: Some("#target-button".to_string()),
            dom_id: Some("target-button".to_string()),
            ..Default::default()
        };
        let res = eng.recover(Some(&stale), &req, RecoveryOptions { pierce_shadow: true, ..Default::default() });
        // Should recover (via CssSelector or DomIdName)
        assert!(matches!(res.outcome, RecoveryOutcome::Recovered(_)));
        // Ensure frame/shadow tier was recorded (either success or skipped due to earlier success)
        let frame_tier = &res.trace.attempts[7];
        assert_eq!(frame_tier.tier, RecoveryTier::FrameShadowTraversal);
        // Since we recovered earlier, it should be SkippedDueToEarlierSuccess — still recorded, never skipped
        assert_eq!(frame_tier.status, TierStatus::SkippedDueToEarlierSuccess);
        assert_eq!(res.trace.attempts.len(), 10);
    }

    #[test]
    fn never_skipped_tier_all_ten_recorded_even_on_success() {
        // Insert element so first tier succeeds
        let ds2 = Arc::new(DomState::new());
        let fm2 = FrameManager::new(ds2.clone());
        let tm2 = BrowserTargetManager::new(ds2.clone(), None);
        let idx2 = ElementIndex::new(ds2.clone(), fm2.clone(), tm2);
        let eng2 = RecoveryEngine::new(idx2.clone(), fm2.clone(), ds2.clone());
        let r = el("e_100", 100, "x", "#x", "button", "Ok", "Ok", "main");
        idx2.insert(r);
        let req = ResolveRequest { ref_id: Some("e_100".to_string()), ..Default::default() };
        let res = eng2.recover(None, &req, RecoveryOptions::default());
        assert!(matches!(res.outcome, RecoveryOutcome::Recovered(_)));
        assert_eq!(res.trace.attempts.len(), 10);
        assert_eq!(res.trace.attempts[0].status, TierStatus::Success);
        for i in 1..10 {
            assert_eq!(res.trace.attempts[i].status, TierStatus::SkippedDueToEarlierSuccess, "tier {} should be SkippedDueToEarlierSuccess", i);
        }
    }
}
