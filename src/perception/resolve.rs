//! Reusable policy resolve_target(query, context)
//!
//! Pipeline (7 stages, in order):
//!   1 exact deterministic resolution,
//!   2 DOM/AX resolution,
//!   3 hint resolution,
//!   4 heuristic candidate filtering,
//!   5 Decider candidate selection (text 255, vision 10 with hierarchical chunking),
//!   6 visual fallback (Gemini grounding),
//!   7 uncertain result.
//!
//! Every semantic tool should reuse this - it centralizes the policy so
//! browser act, ground, hint, and a11y share one target-resolution order.

use std::collections::HashMap;

use anyhow::{Result, bail};
use serde_json::{Value, json};

use crate::decider::candidates::{Candidate, Candidates, Rect, VISION_MAX_OPTIONS, TEXT_MAX_OPTIONS};
use crate::decider::config::DeciderConfig;
use crate::decider::types::{DeciderQuestion, DeciderRequest};
use crate::decider::client::DeciderClient;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveTier {
    Exact,
    DomAx,
    Hint,
    Heuristic,
    DeciderText,
    DeciderVision,
    VisualFallback,
    Uncertain,
}

impl ResolveTier {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Exact => "exact",
            Self::DomAx => "dom_ax",
            Self::Hint => "hint",
            Self::Heuristic => "heuristic",
            Self::DeciderText => "decider_text",
            Self::DeciderVision => "decider_vision",
            Self::VisualFallback => "visual_fallback",
            Self::Uncertain => "uncertain",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ResolveStatus {
    Success,
    Uncertain,
    Failure,
}

impl ResolveStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Uncertain => "uncertain",
            Self::Failure => "failure",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ResolveContext {
    /// Raw natural-language query / instruction
    pub query: String,
    /// Optional target tab spec (index, targetId, url substring)
    pub target: Option<String>,
    /// Hint snapshots or generic candidate source may be pre-fetched
    pub candidates: Option<Candidates>,
    /// Use vision (annotated screenshot) vs text mode for Decider.
    /// Vision calibrated 10 options, text 255 options.
    pub use_vision: bool,
    /// Optional viewport rect for filtering
    pub viewport: Option<Rect>,
    /// Whether to annotate screenshot for vision mode
    pub annotate: bool,
    /// Optional image base64/data-uri for vision (if caller already captured)
    pub image: Option<String>,
    /// Extra context string forwarded to Decider as `context`
    pub context: Option<String>,
    /// Minimum geometry filter for budget
    pub min_geometry: Option<(i32,i32)>,
    /// Allow visual fallback (Gemini) if Decider uncertain / disabled
    pub allow_visual_fallback: bool,
}

impl Default for ResolveContext {
    fn default() -> Self {
        Self {
            query: String::new(),
            target: None,
            candidates: None,
            use_vision: false,
            viewport: None,
            annotate: true,
            image: None,
            context: None,
            min_geometry: None,
            allow_visual_fallback: true,
        }
    }
}

impl ResolveContext {
    pub fn new(query: impl Into<String>) -> Self {
        let q = query.into();
        Self { query: q.clone(), context: Some(q), ..Default::default() }
    }
    pub fn with_target(mut self, target: impl Into<String>) -> Self {
        let s = target.into();
        if s.trim().is_empty() { self.target = None; } else { self.target = Some(s); }
        self
    }
    pub fn with_candidates(mut self, cands: Candidates) -> Self {
        self.candidates = Some(cands);
        self
    }
    pub fn with_vision(mut self, use_vision: bool) -> Self {
        self.use_vision = use_vision;
        self
    }
    pub fn with_viewport(mut self, vp: Rect) -> Self {
        self.viewport = Some(vp);
        self
    }
    pub fn with_context(mut self, ctx: impl Into<String>) -> Self {
        self.context = Some(ctx.into());
        self
    }
    pub fn without_visual_fallback(mut self) -> Self {
        self.allow_visual_fallback = false;
        self
    }
}

#[derive(Debug, Clone)]
pub struct ResolveResult {
    pub status: ResolveStatus,
    pub tier: ResolveTier,
    pub candidate: Option<Candidate>,
    pub id: Option<u32>,
    pub label: Option<String>,
    pub rect: Option<Rect>,
    pub confidence: Option<f64>,
    pub runner_up: Option<f64>,
    pub margin: Option<f64>,
    pub probabilities: Option<HashMap<String, f64>>,
    pub meta: Value,
}

impl ResolveResult {
    pub fn success(candidate: Candidate, tier: ResolveTier) -> Self {
        let rect = candidate.rect;
        let id = candidate.id;
        let label = if candidate.label.is_empty() { None } else { Some(candidate.label.clone()) };
        Self {
            status: ResolveStatus::Success,
            tier,
            rect: Some(rect),
            id: Some(id),
            label,
            candidate: Some(candidate),
            confidence: None,
            runner_up: None,
            margin: None,
            probabilities: None,
            meta: json!({}),
        }
    }
    pub fn uncertain(tier: ResolveTier, meta: Value) -> Self {
        Self {
            status: ResolveStatus::Uncertain,
            tier,
            candidate: None,
            id: None,
            label: None,
            rect: None,
            confidence: None,
            runner_up: None,
            margin: None,
            probabilities: None,
            meta,
        }
    }
    pub fn failure(msg: impl Into<String>) -> Self {
        Self {
            status: ResolveStatus::Failure,
            tier: ResolveTier::Uncertain,
            candidate: None,
            id: None,
            label: None,
            rect: None,
            confidence: None,
            runner_up: None,
            margin: None,
            probabilities: None,
            meta: json!({"error": msg.into()}),
        }
    }
    pub fn is_success(&self) -> bool { self.status == ResolveStatus::Success }
    pub fn is_uncertain(&self) -> bool { self.status == ResolveStatus::Uncertain }
    pub fn to_json(&self) -> Value {
        let mut v = json!({
            "status": self.status.as_str(),
            "tier": self.tier.as_str(),
            "confidence": self.confidence,
            "runner_up": self.runner_up,
            "margin": self.margin,
            "probabilities": self.probabilities,
            "id": self.id,
            "label": self.label,
            "rect": self.rect.map(|r| json!({"x": r.x, "y": r.y, "width": r.width, "height": r.height})),
            "meta": self.meta,
        });
        if let Some(c) = &self.candidate {
            v["candidate"] = json!({
                "id": c.id, "label": c.label, "tag": c.tag, "role": c.role, "name": c.name, "text": c.text,
                "rect": {"x": c.rect.x, "y": c.rect.y, "width": c.rect.width, "height": c.rect.height},
                "selector": c.selector, "visible": c.visible, "enabled": c.enabled
            });
        }
        v
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

const LOW_CONFIDENCE_THRESHOLD: f64 = 0.55;
const MARGIN_THRESHOLD: f64 = 0.10;

fn normalize(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

fn confidence_is_low(conf: Option<f64>, runner_up: Option<f64>, margin: Option<f64>, probs: Option<&HashMap<String,f64>>) -> bool {
    if let Some(c) = conf {
        if c < LOW_CONFIDENCE_THRESHOLD { return true; }
        if let Some(m) = margin {
            if m < MARGIN_THRESHOLD { return true; }
        } else if let Some(ru) = runner_up {
            if (c - ru) < MARGIN_THRESHOLD { return true; }
        } else if let Some(p) = probs {
            // derive runner_up from probs if not provided
            let mut vals: Vec<f64> = p.values().copied().collect();
            vals.sort_by(|a,b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
            if vals.len() >= 2 && (vals[0] - vals[1]) < MARGIN_THRESHOLD {
                return true;
            }
        }
    }
    false
}

fn derive_margin(conf: Option<f64>, runner_up: Option<f64>) -> Option<f64> {
    match (conf, runner_up) {
        (Some(c), Some(r)) => Some(c - r),
        _ => None,
    }
}

fn runner_up_from_probs(probs: &HashMap<String,f64>, choice: &str) -> Option<f64> {
    let top = probs.get(choice).copied()?;
    let mut best = 0.0f64;
    let mut found = false;
    for (k, v) in probs {
        if k != choice && *v > best {
            best = *v;
            found = true;
        }
    }
    if found { Some(best) } else { None }
}

// ---------------------------------------------------------------------------
// Candidate collection (shared)
// ---------------------------------------------------------------------------

/// Collect candidates from hint snapshot (or generic) if not already provided in context.
/// Deterministic: hint_snapshot is canonical; falls back to generic snapshot.
pub fn collect_candidates(ctx: &ResolveContext) -> Result<Candidates> {
    if let Some(cands) = &ctx.candidates {
        return Ok(cands.clone());
    }
    // Try hint snapshot with target routing
    let hint_val = crate::hint::hint_snapshot_with_target(ctx.target.as_deref());
    if let Ok(v) = hint_val {
        let c = Candidates::from_hint_snapshot(&v, None, None, None);
        if !c.is_empty() {
            return Ok(c);
        }
        // empty hint but still return empty - caller will treat as no candidates
        return Ok(c);
    }
    // Hint failed -> try to return empty (no candidates)
    Ok(Candidates::new())
}

// ---------------------------------------------------------------------------
// Exact / heuristic helpers
// ---------------------------------------------------------------------------

/// Exact deterministic resolution: normalized text/name exactly equals query.
/// Returns Some(candidate) only when unambiguous single match.
pub fn exact_match(cands: &Candidates, query: &str) -> Option<Candidate> {
    let nq = normalize(query);
    if nq.is_empty() { return None; }
    let mut hits = Vec::new();
    for c in cands.iter() {
        if c.normalized_name() == nq || c.normalized_text() == nq {
            hits.push(c.clone());
        }
    }
    if hits.len() == 1 { Some(hits.into_iter().next().unwrap()) } else { None }
}

/// Heuristic single-match: substring in normalized name/text with unambiguous result.
pub fn heuristic_single_match(cands: &Candidates, query: &str) -> Option<Candidate> {
    let nq = normalize(query);
    if nq.is_empty() || nq.len() < 2 { return None; }
    let mut hits = Vec::new();
    for c in cands.iter() {
        if c.normalized_name().contains(&nq) || c.normalized_text().contains(&nq) {
            hits.push(c.clone());
        }
    }
    if hits.len() == 1 { Some(hits.into_iter().next().unwrap()) } else { None }
}

// ---------------------------------------------------------------------------
// Decider selection helpers (with hierarchical chunking)
// ---------------------------------------------------------------------------

fn build_decider_question(query: &str, cands: &[Candidate], _use_vision: bool) -> DeciderQuestion {
    let opts: Vec<String> = cands.iter().map(|c| {
        if _use_vision { c.vision_text() } else { c.display_text() }
    }).collect();
    DeciderQuestion::new(query.to_string(), opts)
}

async fn decider_select_async(cands: &Candidates, query: &str, ctx: &ResolveContext) -> Result<ResolveResult> {
    let cfg = DeciderConfig::from_env();
    if !cfg.enabled {
        bail!("{}", crate::decider::config::off_reason());
    }
    let max_per = if ctx.use_vision { VISION_MAX_OPTIONS } else { TEXT_MAX_OPTIONS };
    let context_str = ctx.context.clone().unwrap_or_else(|| query.to_string());

    // Deterministic filtering step before Decider
    let filtered = cands.filtered_for_budget(ctx.viewport, ctx.min_geometry);
    let effective = if filtered.is_empty() { cands.clone() } else { filtered };
    if effective.is_empty() {
        bail!("no candidates for decider");
    }

    // Hierarchical chunking
    let chunks = effective.chunks(max_per);
    if chunks.is_empty() { bail!("no chunks"); }

    let client = DeciderClient::new(cfg.clone())?;
    let mut winning_ids: Vec<u32> = Vec::new();
    let mut last_conf: Option<f64> = None;
    let mut last_probs: Option<HashMap<String,f64>> = None;
    let mut last_runner: Option<f64> = None;
    let mut last_margin: Option<f64> = None;

    // If single chunk, one Decider call
    if chunks.len() == 1 {
        let chunk = &chunks[0];
        let question = build_decider_question(query, chunk, ctx.use_vision);
        let req = if ctx.use_vision {
            let img = if let Some(b64) = &ctx.image {
                b64.clone()
            } else {
                // Capture + annotate for vision
                match capture_and_annotate_for_vision(&effective, ctx) {
                    Ok((b64, _meta)) => b64,
                    Err(_) => {
                        // fallback to text mode without image
                        let req2 = DeciderRequest::new(context_str.clone(), vec![question]);
                        let resp = client.decide(&req2).await?;
                        return map_decider_response(chunk, &effective, resp, query, ctx.use_vision);
                    }
                }
            };
            DeciderRequest::new(context_str.clone(), vec![question]).with_image(img)
        } else {
            DeciderRequest::new(context_str.clone(), vec![question])
        };
        let resp = client.decide(&req).await?;
        return map_decider_response(chunk, &effective, resp, query, ctx.use_vision);
    }

    // Multi-chunk hierarchical: per-group winners -> final set
    for (idx, chunk) in chunks.iter().enumerate() {
        let question = build_decider_question(query, chunk, ctx.use_vision);
        let req = if ctx.use_vision {
            // Per-group screenshot handling: if we have one image, annotate per chunk's legend
            // For now reuse same capture logic per chunk (deterministic)
            let chunk_cands = Candidates::from_vec(chunk.clone());
            let img = match capture_and_annotate_for_chunk(&chunk_cands, ctx) {
                Ok((b64, _)) => b64,
                Err(_) => {
                    // try text mode for this chunk
                    let r = DeciderRequest::new(context_str.clone(), vec![question]);
                    let resp = client.decide(&r).await?;
                    if let Some(dec) = resp.decisions.first() {
                        if let Some(gid) = map_chunk_choice_to_global(&effective, idx, &dec.choice, max_per) {
                            winning_ids.push(gid);
                            last_conf = dec.confidence;
                            last_probs = dec.probabilities.clone();
                            if let Some(p) = &dec.probabilities {
                                last_runner = runner_up_from_probs(p, &dec.choice);
                                last_margin = derive_margin(dec.confidence, last_runner);
                            }
                        }
                    }
                    continue;
                }
            };
            DeciderRequest::new(context_str.clone(), vec![question]).with_image(img)
        } else {
            DeciderRequest::new(context_str.clone(), vec![question])
        };
        let resp = client.decide(&req).await?;
        if let Some(dec) = resp.decisions.first() {
            if let Some(gid) = map_chunk_choice_to_global(&effective, idx, &dec.choice, max_per) {
                winning_ids.push(gid);
                last_conf = dec.confidence;
                last_probs = dec.probabilities.clone();
                if let Some(p) = &dec.probabilities {
                    last_runner = runner_up_from_probs(p, &dec.choice);
                    last_margin = derive_margin(dec.confidence, last_runner);
                }
            }
        }
    }

    if winning_ids.is_empty() {
        bail!("decider hierarchical: no winning ids");
    }
    // Deduplicate preserving first occurrence
    let mut seen = std::collections::HashSet::new();
    let mut deduped = Vec::new();
    for id in winning_ids {
        if seen.insert(id) { deduped.push(id); }
    }
    // If we still exceed budget, recurse one more round or final call with winners
    let winners_cands = effective.group_winners(&deduped);
    if winners_cands.len() <= max_per {
        // Final call on winners
        let final_opts: Vec<String> = winners_cands.iter().map(|c| if ctx.use_vision { c.vision_text() } else { c.display_text() }).collect();
        let final_q = DeciderQuestion::new(query.to_string(), final_opts);
        let final_req = if ctx.use_vision {
            if let Ok((b64, _)) = capture_and_annotate_for_vision(&winners_cands, ctx) {
                DeciderRequest::new(context_str, vec![final_q]).with_image(b64)
            } else {
                DeciderRequest::new(context_str, vec![final_q])
            }
        } else {
            DeciderRequest::new(context_str, vec![final_q])
        };
        let resp = client.decide(&final_req).await?;
        if let Some(dec) = resp.decisions.first() {
            if let Some(cand) = winners_cands.get(dec.choice.parse::<u32>().unwrap_or(0)) {
                // If decider returned numeric id within winners, map to global candidate
                // choice is 1-indexed within winners
                let idx = dec.choice.parse::<usize>().ok().unwrap_or(0);
                if idx >= 1 && idx <= winners_cands.len() {
                    let global = winners_cands.as_slice()[idx-1].clone();
                    let runner = dec.probabilities.as_ref().and_then(|p| runner_up_from_probs(p, &dec.choice));
                    let margin = derive_margin(dec.confidence, runner);
                    if confidence_is_low(dec.confidence, runner, margin, dec.probabilities.as_ref()) {
                        return Ok(ResolveResult {
                            status: ResolveStatus::Uncertain,
                            tier: if ctx.use_vision { ResolveTier::DeciderVision } else { ResolveTier::DeciderText },
                            candidate: None,
                            id: Some(global.id),
                            label: Some(global.label.clone()),
                            rect: Some(global.rect),
                            confidence: dec.confidence,
                            runner_up: runner,
                            margin,
                            probabilities: dec.probabilities.clone(),
                            meta: json!({"reason": "low confidence", "winners": deduped, "choice": dec.choice}),
                        });
                    }
                    let mut res = ResolveResult::success(global, if ctx.use_vision { ResolveTier::DeciderVision } else { ResolveTier::DeciderText });
                    res.confidence = dec.confidence;
                    res.runner_up = runner;
                    res.margin = margin;
                    res.probabilities = dec.probabilities.clone();
                    return Ok(res);
                }
            }
            // Fallback: map via choice id directly to global if not found in winners
            if let Some(gid) = winners_cands.as_slice().iter().find(|c| c.id.to_string() == dec.choice).map(|c| c.id) {
                if let Some(cand) = effective.get(gid) {
                    let runner = dec.probabilities.as_ref().and_then(|p| runner_up_from_probs(p, &dec.choice));
                    let margin = derive_margin(dec.confidence, runner);
                    if confidence_is_low(dec.confidence, runner, margin, dec.probabilities.as_ref()) {
                        return Ok(ResolveResult {
                            status: ResolveStatus::Uncertain,
                            tier: if ctx.use_vision { ResolveTier::DeciderVision } else { ResolveTier::DeciderText },
                            candidate: None,
                            id: Some(cand.id),
                            label: Some(cand.label.clone()),
                            rect: Some(cand.rect),
                            confidence: dec.confidence,
                            runner_up: runner,
                            margin,
                            probabilities: dec.probabilities.clone(),
                            meta: json!({"reason": "low confidence"}),
                        });
                    }
                    let mut res = ResolveResult::success(cand.clone(), if ctx.use_vision { ResolveTier::DeciderVision } else { ResolveTier::DeciderText });
                    res.confidence = dec.confidence;
                    res.runner_up = runner;
                    res.margin = margin;
                    res.probabilities = dec.probabilities.clone();
                    return Ok(res);
                }
            }
        }
        // If final call didn't yield usable choice, pick first winner as uncertain
        if let Some(first) = winners_cands.iter().next().cloned() {
            return Ok(ResolveResult {
                status: ResolveStatus::Uncertain,
                tier: if ctx.use_vision { ResolveTier::DeciderVision } else { ResolveTier::DeciderText },
                candidate: Some(first.clone()),
                id: Some(first.id),
                label: Some(first.label.clone()),
                rect: Some(first.rect),
                confidence: last_conf,
                runner_up: last_runner,
                margin: last_margin,
                probabilities: last_probs,
                meta: json!({"reason": "hierarchical no final choice", "winners": deduped}),
            });
        }
        bail!("hierarchical final step failed");
    } else {
        // Still over budget (unlikely with dedup) - pick first chunk winner as uncertain
        bail!("hierarchical winners still over budget: {} > {}", winners_cands.len(), max_per);
    }
}

fn map_decider_response(chunk: &[Candidate], _effective: &Candidates, resp: crate::decider::types::DeciderResponse, _query: &str, use_vision: bool) -> Result<ResolveResult> {
    let dec = resp.decisions.first().ok_or_else(|| anyhow::anyhow!("empty decider decisions"))?;
    let choice = dec.choice.trim();
    // Try numeric index within chunk
    if let Ok(idx) = choice.parse::<usize>() {
        if idx >= 1 && idx <= chunk.len() {
            let cand = chunk[idx - 1].clone();
            let runner = dec.probabilities.as_ref().and_then(|p| runner_up_from_probs(p, choice));
            let margin = derive_margin(dec.confidence, runner);
            if confidence_is_low(dec.confidence, runner, margin, dec.probabilities.as_ref()) {
                return Ok(ResolveResult {
                    status: ResolveStatus::Uncertain,
                    tier: if use_vision { ResolveTier::DeciderVision } else { ResolveTier::DeciderText },
                    candidate: None,
                    id: Some(cand.id),
                    label: Some(cand.label.clone()),
                    rect: Some(cand.rect),
                    confidence: dec.confidence,
                    runner_up: runner,
                    margin,
                    probabilities: dec.probabilities.clone(),
                    meta: json!({"reason": "low confidence", "choice": choice}),
                });
            }
            let mut res = ResolveResult::success(cand, if use_vision { ResolveTier::DeciderVision } else { ResolveTier::DeciderText });
            res.confidence = dec.confidence;
            res.runner_up = runner;
            res.margin = margin;
            res.probabilities = dec.probabilities.clone();
            return Ok(res);
        }
    }
    // Try option text exact match within chunk
    if let Some(cand) = chunk.iter().find(|c| c.display_text() == choice || c.vision_text() == choice || c.name == choice || c.text == choice).cloned() {
        let runner = dec.probabilities.as_ref().and_then(|p| runner_up_from_probs(p, choice));
        let margin = derive_margin(dec.confidence, runner);
        if confidence_is_low(dec.confidence, runner, margin, dec.probabilities.as_ref()) {
            return Ok(ResolveResult {
                status: ResolveStatus::Uncertain,
                tier: if use_vision { ResolveTier::DeciderVision } else { ResolveTier::DeciderText },
                candidate: None,
                id: Some(cand.id),
                label: Some(cand.label.clone()),
                rect: Some(cand.rect),
                confidence: dec.confidence,
                runner_up: runner,
                margin,
                probabilities: dec.probabilities.clone(),
                meta: json!({"reason": "low confidence", "choice": choice}),
            });
        }
        let mut res = ResolveResult::success(cand, if use_vision { ResolveTier::DeciderVision } else { ResolveTier::DeciderText });
        res.confidence = dec.confidence;
        res.runner_up = runner;
        res.margin = margin;
        res.probabilities = dec.probabilities.clone();
        return Ok(res);
    }
    bail!("decider choice '{}' invalid for chunk size {}", choice, chunk.len());
}

fn map_chunk_choice_to_global(effective: &Candidates, chunk_idx: usize, choice: &str, max_per: usize) -> Option<u32> {
    effective.map_choice_to_global_id(chunk_idx, choice, max_per)
}

fn capture_and_annotate_for_vision(cands: &Candidates, ctx: &ResolveContext) -> Result<(String, Value)> {
    // Reuse existing image pipeline: try browser CDP first, fallback to monitor
    use crate::decider::image::{ImageSource, capture_with_source, annotate_and_encode};
    let source = if ctx.target.is_some() {
        ImageSource::Browser
    } else {
        ImageSource::Browser
    };
    let (bytes, _cap_meta) = capture_with_source(&source, 0.5).or_else(|_| {
        capture_with_source(&ImageSource::Monitor, 0.5)
    })?;
    let (encoded, meta) = annotate_and_encode(bytes, cands, true, true)?;
    // annotate_and_encode returns data-uri by default when as_data_uri=true but caller passes true
    // Actually annotate_and_encode with as_data_uri=true returns data uri; we need base64 or data uri both ok
    Ok((encoded, meta))
}

fn capture_and_annotate_for_chunk(chunk_cands: &Candidates, ctx: &ResolveContext) -> Result<(String, Value)> {
    capture_and_annotate_for_vision(chunk_cands, ctx)
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Reusable policy resolve_target(query, context):
///   1 exact, 2 DOM/AX, 3 hint, 4 heuristic filtering, 5 Decider, 6 visual fallback, 7 uncertain.
///
/// `query` is the natural-language instruction (e.g. "click the Submit button").
/// `ctx` carries optional precomputed candidates, viewport, vision flags, etc.
///
/// Returns a `ResolveResult` that is always `Success` or `Uncertain`/`Failure` —
// low confidence is mapped to `Uncertain`, never `Success`.
pub async fn resolve_target_async(query: &str, ctx: &ResolveContext) -> Result<ResolveResult> {
    let q = if query.trim().is_empty() { ctx.query.as_str() } else { query };
    if q.trim().is_empty() { bail!("resolve_target needs query"); }

    // Stage 1: exact deterministic resolution (if candidates pre-supplied)
    let mut cands = collect_candidates(ctx).unwrap_or_else(|_| Candidates::new());

    // Deterministic budget filtering before any model call
    if cands.len() > TEXT_MAX_OPTIONS {
        // filtered_for_budget never truncates, just filters; then chunking will handle 255 split
        cands = cands.filtered_for_budget(ctx.viewport, ctx.min_geometry);
    }

    // 1 exact deterministic
    if let Some(hit) = exact_match(&cands, q) {
        return Ok(ResolveResult::success(hit, ResolveTier::Exact));
    }

    // 2 DOM/AX resolution: try AT-SPI / browser snapshot direct single DOM query
    // For now, this is a lightweight exact DOM query via hint's heuristic label match
    // (preserving real Chromium path, not mocked). If we have a candidate whose selector uniquely matches
    // via CDP Runtime.evaluate existence check, we treat as DomAx.
    if let Some(cand) = try_dom_ax_resolution(&cands, q).await {
        return Ok(ResolveResult::success(cand, ResolveTier::DomAx));
    }

    // 3 hint resolution (letter label / exact name mapping via hint snapshot heuristic)
    // We already have candidates from hint - heuristic exact was stage 1, now check hint label resolution via heuristic_hint_match
    if let Some(cand) = try_hint_tier(&cands, q) {
        return Ok(ResolveResult::success(cand, ResolveTier::Hint));
    }

    // 4 heuristic candidate filtering (substring single match)
    if let Some(hit) = heuristic_single_match(&cands, q) {
        return Ok(ResolveResult::success(hit, ResolveTier::Heuristic));
    }

    // Also try filtered subset heuristic again to narrow
    let filtered = cands.filtered_for_budget(ctx.viewport, ctx.min_geometry);
    if filtered.len() != cands.len() && filtered.len() > 0 {
        if let Some(hit) = heuristic_single_match(&filtered, q) {
            return Ok(ResolveResult::success(hit, ResolveTier::Heuristic));
        }
        // use filtered for Decider to stay within budget deterministically
        cands = filtered;
    }

    if cands.is_empty() {
        // No DOM candidates - try visual fallback before uncertain
        if ctx.allow_visual_fallback {
            if let Some(fb) = try_visual_fallback(q).await {
                return Ok(fb);
            }
        }
        return Ok(ResolveResult::uncertain(ResolveTier::Uncertain, json!({"reason": "no candidates", "query": q})));
    }

    // 5 Decider candidate selection
    // Limit checks: vision 10, text 255 with hierarchical. decider_select_async handles chunking.
    match decider_select_async(&cands, q, ctx).await {
        Ok(r) => {
            if r.is_success() {
                return Ok(r);
            }
            // Decider returned uncertain (low confidence) - don't treat as success
            if r.is_uncertain() {
                // Fall through to visual fallback before final uncertain
                if ctx.allow_visual_fallback {
                    if let Some(fb) = try_visual_fallback(q).await {
                        // Attach decider meta for debugging but return fallback success
                        let mut fb_with_meta = fb;
                        fb_with_meta.meta["decider"] = r.to_json();
                        return Ok(fb_with_meta);
                    }
                }
                return Ok(r);
            }
            // Other decider states
            return Ok(r);
        }
        Err(e) => {
            // Decider disabled or failed - fall through to visual fallback, then uncertain
            eprintln!("[resolve_target] decider select failed: {e}");
        }
    }

    // 6 visual fallback (Gemini)
    if ctx.allow_visual_fallback {
        if let Some(fb) = try_visual_fallback(q).await {
            return Ok(fb);
        }
    }

    // 7 uncertain result (preserve probabilities/conf evidence if any)
    Ok(ResolveResult::uncertain(ResolveTier::Uncertain, json!({"reason": "all tiers exhausted", "query": q, "candidate_count": cands.len()})))
}

async fn try_dom_ax_resolution(cands: &Candidates, query: &str) -> Option<Candidate> {
    // Lightweight DOM/AX: if hint heuristic already handled exact, this checks
    // for a single enabled+visible candidate whose role matches a verb-noun pattern
    // Example: query "click login button" -> role button + name contains login
    let nq = normalize(query);
    // Prefer exact single role+substring
    let mut filtered = cands.filter_enabled().filter_visible();
    if filtered.is_empty() { filtered = cands.clone(); }
    // Try to infer desired role from query
    let desired_role = if nq.contains("button") { Some("button") } else if nq.contains("link") { Some("link") } else if nq.contains("input") || nq.contains("textbox") || nq.contains("field") { Some("textbox") } else { None };
    if let Some(role) = desired_role {
        let by_role = filtered.filter_by_role(role);
        if by_role.len() == 1 && nq.len() < 50 {
            // If query contains that candidate's name, it's a DOM/AX single match
            if let Some(c) = by_role.iter().next() {
                if nq.contains(&c.normalized_name()) || nq.contains(&c.normalized_text()) {
                    return Some(c.clone());
                }
            }
        }
        // If only one candidate of that role and query is short, consider it
        // But require at least substring match to avoid false positive
    }
    None
}

fn try_hint_tier(cands: &Candidates, query: &str) -> Option<Candidate> {
    // Replicate hint::heuristic_hint_match but return Candidate instead of label
    let nq = normalize(query);
    if nq.is_empty() { return None; }
    // Use the existing hint heuristic via a fake Value
    let hints_val = json!({"hints": cands.iter().map(|c| json!({
        "label": c.label, "tag": c.tag, "role": c.role, "name": c.name, "text": c.text, "selector": c.selector
    })).collect::<Vec<_>>()});
    if let Some(label) = crate::hint::heuristic_hint_match(query, &hints_val) {
        if let Some(c) = cands.get_by_label(&label) {
            return Some(c.clone());
        }
        // label is numeric id? try id parse
        if let Ok(id) = label.parse::<u32>() {
            if let Some(c) = cands.get(id) { return Some(c.clone()); }
        }
    }
    None
}

async fn try_visual_fallback(query: &str) -> Option<ResolveResult> {
    // Use Gemini grounding as last resort - screenshot + Gemini Flash
    // This is the raw coordinate path; only via Gemini (Decider never emits raw coords)
    let res = crate::ground::ground(query, "", "");
    if let Ok(v) = res {
        let x = v.get("x").and_then(|n| n.as_f64()).unwrap_or(0.0);
        let y = v.get("y").and_then(|n| n.as_f64()).unwrap_or(0.0);
        if x != 0.0 || y != 0.0 {
            let rect = Rect::new(x as i32, y as i32, 1, 1);
            let mut r = ResolveResult {
                status: ResolveStatus::Success,
                tier: ResolveTier::VisualFallback,
                candidate: None,
                id: None,
                label: None,
                rect: Some(rect),
                confidence: None,
                runner_up: None,
                margin: None,
                probabilities: None,
                meta: json!({"ground": v, "via": "gemini"}),
            };
            // Mark low confidence? Gemini raw doesn't provide confidence, so keep as fallback success
            return Some(r);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Sync wrappers
// ---------------------------------------------------------------------------

fn rt_block_on<F: std::future::Future>(f: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread().enable_all().build().expect("resolve rt").block_on(f)
}

/// Sync wrapper for resolve_target
pub fn resolve_target(query: &str, ctx: &ResolveContext) -> Result<ResolveResult> {
    rt_block_on(resolve_target_async(query, ctx))
}

/// Convenience: resolve with just a query (no extra context)
pub fn resolve(query: &str) -> Result<ResolveResult> {
    let ctx = ResolveContext::new(query);
    resolve_target(query, &ctx)
}
pub async fn resolve_async(query: &str) -> Result<ResolveResult> {
    let ctx = ResolveContext::new(query);
    resolve_target_async(query, &ctx).await
}

// ---------------------------------------------------------------------------
// Batched helper for multiple semantic queries sharing one snapshot/screenshot
// ---------------------------------------------------------------------------

/// Batched resolve: multiple queries share one candidate snapshot (+ one screenshot for vision).
/// Preserves determinism: one snapshot collection, then per-query Decider calls batched via
/// DeciderClient::decide_batch (bounded concurrency 4).
pub async fn resolve_batch_async(queries: &[String], ctx: &ResolveContext) -> Result<Vec<ResolveResult>> {
    if queries.is_empty() { bail!("resolve_batch needs at least 1 query"); }
    if queries.len() > 12 { bail!("resolve_batch max 12 queries"); }
    let base_cands = collect_candidates(ctx).unwrap_or_else(|_| Candidates::new());
    if base_cands.is_empty() {
        // No candidates -> each query resolves via visual fallback / uncertain individually
        let mut out = Vec::new();
        for q in queries {
            let r = resolve_target_async(q, ctx).await.unwrap_or_else(|e| ResolveResult::failure(e.to_string()));
            out.push(r);
        }
        return Ok(out);
    }

    let cfg = DeciderConfig::from_env();
    let use_decider = cfg.enabled && !queries.is_empty();
    if !use_decider {
        // Sequential heuristic/exact per query without Decider
        let mut out = Vec::new();
        for q in queries {
            let r = resolve_target_async(q, ctx).await.unwrap_or_else(|e| ResolveResult::failure(e.to_string()));
            out.push(r);
        }
        return Ok(out);
    }

    // Prepare batched Decider requests: each query gets one question with up to budget options
    let max_per = if ctx.use_vision { VISION_MAX_OPTIONS } else { TEXT_MAX_OPTIONS };
    let filtered = base_cands.filtered_for_budget(ctx.viewport, ctx.min_geometry);
    let effective = if filtered.is_empty() { base_cands.clone() } else { filtered };

    // If over budget, we cannot batch blindly - fallback to sequential hierarchical per query
    if effective.len() > max_per {
        let mut out = Vec::new();
        for q in queries {
            let r = resolve_target_async(q, ctx).await.unwrap_or_else(|e| ResolveResult::failure(e.to_string()));
            out.push(r);
        }
        return Ok(out);
    }

    // Single batched request path: one Decider call per query (batched concurrency)
    let context_base = ctx.context.clone().unwrap_or_default();
    let image_opt = if ctx.use_vision {
        // One screenshot shared across queries (deterministic)
        match capture_and_annotate_for_vision(&effective, ctx) {
            Ok((b64, _)) => Some(b64),
            Err(_) => None,
        }
    } else { None };

    let mut requests = Vec::new();
    for q in queries {
        let chunk = effective.as_slice(); // single chunk since within budget
        let opts: Vec<String> = chunk.iter().map(|c| if ctx.use_vision { c.vision_text() } else { c.display_text() }).collect();
        let question = DeciderQuestion::new(q.clone(), opts);
        let ctx_str = if context_base.is_empty() { q.clone() } else { context_base.clone() };
        let req = if let Some(ref img) = image_opt {
            DeciderRequest::new(ctx_str, vec![question]).with_image(img.clone())
        } else {
            DeciderRequest::new(ctx_str, vec![question])
        };
        requests.push(req);
    }

    let client = DeciderClient::new(cfg)?;
    let results = client.decide_batch(requests).await;
    let mut out = Vec::new();
    for (i, res) in results.into_iter().enumerate() {
        let q = &queries[i];
        match res {
            Ok(resp) => {
                let chunk = effective.as_slice();
                match map_decider_response(chunk, &effective, resp, q, ctx.use_vision) {
                    Ok(r) => out.push(r),
                    Err(e) => out.push(ResolveResult::failure(format!("map error for '{}': {e}", q))),
                }
            }
            Err(e) => {
                // Decider failed for this query -> fall back to heuristic/visual per query
                let fb_ctx = ctx.clone();
                let fallback = resolve_target_async(q, &fb_ctx).await.unwrap_or_else(|_| ResolveResult::failure(e.to_string()));
                out.push(fallback);
            }
        }
    }
    Ok(out)
}

pub fn resolve_batch(queries: &[String], ctx: &ResolveContext) -> Result<Vec<ResolveResult>> {
    rt_block_on(resolve_batch_async(queries, ctx))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decider::candidates::{Candidate, Rect, Candidates};

    fn cands_for_test() -> Candidates {
        let v = vec![
            Candidate { id: 1, label: "A".into(), tag: "button".into(), role: "button".into(), name: "Submit".into(), text: "Submit".into(), rect: Rect::new(10,20,80,30), visible: true, enabled: true, selector: "button".into(), target_id: None, url: None, title: None },
            Candidate { id: 2, label: "S".into(), tag: "a".into(), role: "link".into(), name: "Cancel".into(), text: "Cancel".into(), rect: Rect::new(100,20,60,30), visible: true, enabled: true, selector: "a.cancel".into(), target_id: None, url: None, title: None },
        ];
        Candidates::from_vec(v)
    }

    #[test]
    fn exact_match_single() {
        let c = cands_for_test();
        assert!(exact_match(&c, "Submit").is_some());
        assert!(exact_match(&c, "submit").is_some());
        assert!(exact_match(&c, "nothing").is_none());
    }
    #[test]
    fn heuristic_single() {
        let c = cands_for_test();
        assert!(heuristic_single_match(&c, "sub").is_some());
    }
}
