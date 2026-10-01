//! Verification helpers using DOM/AX events first, Decider visual verification only when necessary.
//!
//! Returns success/failure/uncertain with confidence. Preserve confidence, runner_up, margin, probabilities.
//! Low confidence → uncertain not success.

use std::collections::HashMap;
use std::time::Duration;

use anyhow::Result;
use serde_json::{Value, json};

use crate::decider::candidates::{Candidates, Rect};
use crate::decider::config::DeciderConfig;
use crate::decider::types::{DeciderQuestion, DeciderRequest};
use crate::decider::client::DeciderClient;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum VerifyStatus {
    Success,
    Failure,
    Uncertain,
}
impl VerifyStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Failure => "failure",
            Self::Uncertain => "uncertain",
        }
    }
}

#[derive(Debug, Clone)]
pub struct VerifyResult {
    pub status: VerifyStatus,
    pub confidence: Option<f64>,
    pub runner_up: Option<f64>,
    pub margin: Option<f64>,
    pub probabilities: Option<HashMap<String, f64>>,
    pub meta: Value,
    pub tier: String,
}

impl VerifyResult {
    pub fn success(conf: Option<f64>, meta: Value) -> Self {
        Self { status: VerifyStatus::Success, confidence: conf, runner_up: None, margin: None, probabilities: None, meta, tier: "dom".into() }
    }
    pub fn failure(meta: Value) -> Self {
        Self { status: VerifyStatus::Failure, confidence: None, runner_up: None, margin: None, probabilities: None, meta, tier: "dom".into() }
    }
    pub fn uncertain(conf: Option<f64>, runner_up: Option<f64>, margin: Option<f64>, probs: Option<HashMap<String,f64>>, meta: Value) -> Self {
        Self { status: VerifyStatus::Uncertain, confidence: conf, runner_up, margin, probabilities: probs, meta, tier: "uncertain".into() }
    }
    pub fn is_success(&self) -> bool { self.status == VerifyStatus::Success }
    pub fn is_uncertain(&self) -> bool { self.status == VerifyStatus::Uncertain }
    pub fn to_json(&self) -> Value {
        json!({
            "status": self.status.as_str(),
            "confidence": self.confidence,
            "runner_up": self.runner_up,
            "margin": self.margin,
            "probabilities": self.probabilities,
            "tier": self.tier,
            "meta": self.meta
        })
    }
}

const LOW_CONFIDENCE_THRESHOLD: f64 = 0.55;
const MARGIN_THRESHOLD: f64 = 0.10;

fn low_confidence(conf: Option<f64>, runner_up: Option<f64>, margin: Option<f64>, probs: Option<&HashMap<String,f64>>) -> bool {
    if let Some(c) = conf {
        if c < LOW_CONFIDENCE_THRESHOLD { return true; }
        if let Some(m) = margin {
            if m < MARGIN_THRESHOLD { return true; }
        } else if let Some(ru) = runner_up {
            if (c - ru) < MARGIN_THRESHOLD { return true; }
        } else if let Some(p) = probs {
            let mut vals: Vec<f64> = p.values().copied().collect();
            vals.sort_by(|a,b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
            if vals.len() >=2 && (vals[0] - vals[1]) < MARGIN_THRESHOLD { return true; }
        }
    }
    false
}
fn derive_margin(conf: Option<f64>, ru: Option<f64>) -> Option<f64> {
    match (conf, ru) { (Some(c), Some(r)) => Some(c-r), _ => None }
}
fn runner_from_probs(probs: &HashMap<String,f64>, choice: &str) -> Option<f64> {
    let top = *probs.get(choice)?;
    let mut best = 0.0; let mut found=false;
    for (k,v) in probs { if k!=choice && *v>best { best=*v; found=true; } }
    if found { Some(best) } else { None }
}

// ---------------------------------------------------------------------------
// DOM/AX helpers
// ---------------------------------------------------------------------------

fn dom_element_exists(selector: &str) -> bool {
    if selector.trim().is_empty() { return false; }
    let js = format!("!!document.querySelector({:?})", selector);
    if let Ok(v) = crate::browser_runtime::client::evaluate_sync(&js, false) {
        return v.as_bool().unwrap_or(false);
    }
    false
}
fn dom_text_contains(selector: &str, needle: &str) -> bool {
    if selector.trim().is_empty() || needle.is_empty() { return false; }
    let js = format!("(function(){{ const el=document.querySelector({:?}); if(!el) return false; const t=(el.innerText||el.textContent||'').toLowerCase(); return t.includes({:?}.toLowerCase()); }})()", selector, needle);
    if let Ok(v) = crate::browser_runtime::client::evaluate_sync(&js, false) {
        return v.as_bool().unwrap_or(false);
    }
    false
}
fn ax_node_exists(query: &str) -> Option<bool> {
    // Try hint snapshot contains normalized query
    if let Ok(hv) = crate::hint::hint_snapshot() {
        let cands = Candidates::from_hint_value(&hv);
        let nq = query.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
        for c in cands.iter() {
            let hay = format!("{} {}", c.name, c.text).to_lowercase();
            if hay.contains(&nq) && !nq.is_empty() { return Some(true); }
        }
        // empty candidates doesn't mean failure, just no hint nodes
    }
    None
}

// ---------------------------------------------------------------------------
// Decider visual verification (only when necessary)
// ---------------------------------------------------------------------------

async fn decider_visual_verify_async(prompt: &str, image_b64: Option<String>) -> Result<VerifyResult> {
    let cfg = DeciderConfig::from_env();
    if !cfg.enabled { anyhow::bail!("{}", crate::decider::config::off_reason()); }
    let question = DeciderQuestion::new(prompt.to_string(), vec!["yes".into(), "no".into(), "uncertain".into()]);
    let context = prompt.to_string();
    let req = if let Some(img) = image_b64 {
        DeciderRequest::new(context, vec![question]).with_image(img)
    } else {
        // Capture screenshot for visual verify
        use crate::decider::image::{ImageSource, capture_with_source, encode_base64};
        let (bytes, _meta) = capture_with_source(&ImageSource::Browser, 0.5)
            .or_else(|_| capture_with_source(&ImageSource::Monitor, 0.5))?;
        let b64 = encode_base64(&bytes);
        DeciderRequest::new(prompt.to_string(), vec![question]).with_image(b64)
    };
    let client = DeciderClient::new(cfg)?;
    let resp = client.decide(&req).await?;
    let dec = resp.decisions.first().ok_or_else(|| anyhow::anyhow!("empty decisions"))?;
    let choice = dec.choice.to_lowercase();
    let is_yes = choice == "1" || choice == "yes" || choice.contains("yes");
    let is_no = choice == "2" || choice == "no" || choice.contains("no");
    let probs = dec.probabilities.clone();
    let runner = probs.as_ref().and_then(|p| runner_from_probs(p, &dec.choice));
    let margin = derive_margin(dec.confidence, runner);
    let low = low_confidence(dec.confidence, runner, margin, probs.as_ref());

    if low {
        return Ok(VerifyResult::uncertain(dec.confidence, runner, margin, probs, json!({"decider_choice": dec.choice, "prompt": prompt, "reason": "low confidence"})));
    }
    if is_yes {
        return Ok(VerifyResult { status: VerifyStatus::Success, confidence: dec.confidence, runner_up: runner, margin, probabilities: probs, meta: json!({"decider_choice": dec.choice, "prompt": prompt}), tier: "decider_vision".into() });
    }
    if is_no {
        return Ok(VerifyResult { status: VerifyStatus::Failure, confidence: dec.confidence, runner_up: runner, margin, probabilities: probs, meta: json!({"decider_choice": dec.choice, "prompt": prompt}), tier: "decider_vision".into() });
    }
    // uncertain raw
    Ok(VerifyResult::uncertain(dec.confidence, runner, margin, probs, json!({"decider_choice": dec.choice, "prompt": prompt})))
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Verify that an element/condition exists using DOM/AX events first,
/// Decider visual verification only when necessary (no DOM signal or ambiguous).
pub async fn verify_async(query: &str) -> Result<VerifyResult> {
    // 1. DOM/AX event first
    if let Some(ax_hit) = ax_node_exists(query) {
        if ax_hit {
            return Ok(VerifyResult::success(Some(0.95), json!({"via": "ax", "query": query})));
        }
    }
    // If query looks like a selector, check existence
    if query.contains('[') || query.contains('#') || query.contains('.') {
        if dom_element_exists(query) {
            return Ok(VerifyResult::success(Some(0.95), json!({"via": "dom", "selector": query})));
        } else {
            // DOM negative is strong signal - but could be stale, confirm with decider only if caller wants visual?
            // For verify, DOM absence => failure without visual unless decider forced
            return Ok(VerifyResult::failure(json!({"via": "dom", "selector": query, "found": false})));
        }
    }
    // Query as text needle - no strong DOM hit, try decider visual if enabled
    let cfg = DeciderConfig::from_env();
    if cfg.enabled {
        match decider_visual_verify_async(&format!("Is '{}' visible on the page? Answer yes/no/uncertain.", query), None).await {
            Ok(r) => return Ok(r),
            Err(e) => eprintln!("[verify] decider visual failed: {e}"),
        }
    }
    // Fallback uncertain when no DOM and no visual
    Ok(VerifyResult::uncertain(None, None, None, None, json!({"reason": "no DOM match and no decider", "query": query})))
}

/// Verify a specific candidate element is present/visible.
pub async fn verify_element_async(candidate: &crate::decider::candidates::Candidate) -> Result<VerifyResult> {
    // Selector existence is cheap DOM signal
    if !candidate.selector.is_empty() && dom_element_exists(&candidate.selector) {
        return Ok(VerifyResult::success(Some(0.92), json!({"via": "dom", "selector": candidate.selector, "candidate_id": candidate.id})));
    }
    // Rect geometry validity + AX check
    if candidate.rect.width > 0 && candidate.rect.height > 0 && candidate.visible {
        if let Some(ax) = ax_node_exists(&candidate.name) {
            if ax {
                return Ok(VerifyResult::success(Some(0.88), json!({"via": "ax+rect", "candidate_id": candidate.id})));
            }
        }
    }
    // Visual verify as last resort
    let cfg = DeciderConfig::from_env();
    if cfg.enabled {
        let prompt = format!("Is the element '{}' ({}) at {},{} {}x{} visible? yes/no/uncertain", candidate.name, candidate.role, candidate.rect.x, candidate.rect.y, candidate.rect.width, candidate.rect.height);
        match decider_visual_verify_async(&prompt, None).await {
            Ok(r) => return Ok(r),
            Err(e) => eprintln!("[verify_element] decider failed: {e}"),
        }
    }
    Ok(VerifyResult::uncertain(None, None, None, None, json!({"reason": "dom/ax inconclusive and no decider", "candidate_id": candidate.id})))
}

/// Verify an action succeeded (e.g. after click, check URL change, DOM mutation, or expected element).
pub async fn verify_action_async(query: &str, expected: Option<&str>) -> Result<VerifyResult> {
    // Check expected text appears via DOM first
    if let Some(exp) = expected {
        if dom_text_contains("body", exp) || ax_node_exists(exp).unwrap_or(false) {
            return Ok(VerifyResult::success(Some(0.9), json!({"via": "dom", "expected": exp})));
        }
    }
    // General verify
    verify_async(query).await
}

/// wait_until: poll `predicate` via DOM/AX events with timeout, Decider visual verification only when necessary.
/// The predicate is evaluated as a selector existence or text presence depending on `query` shape.
pub async fn wait_until_async(query: &str, timeout: Duration, interval: Duration) -> Result<VerifyResult> {
    let start = std::time::Instant::now();
    loop {
        // DOM fast path
        let mut dom_hit = false;
        if query.contains('[') || query.contains('#') || query.contains('.') {
            if dom_element_exists(query) { dom_hit = true; }
        }
        if !dom_hit {
            if let Some(ax) = ax_node_exists(query) { dom_hit = ax; }
        }
        if dom_hit {
            return Ok(VerifyResult::success(Some(0.93), json!({"via": "wait_until_dom", "query": query, "elapsed_ms": start.elapsed().as_millis() as u64})));
        }
        if start.elapsed() >= timeout {
            break;
        }
        tokio::time::sleep(interval).await;
    }
    // Timeout on DOM - try one visual decider check before final failure/uncertain
    let cfg = DeciderConfig::from_env();
    if cfg.enabled {
        match decider_visual_verify_async(&format!("Is '{}' visible? yes/no/uncertain", query), None).await {
            Ok(r) => {
                if r.is_success() { return Ok(r); }
                if low_confidence(r.confidence, r.runner_up, r.margin, r.probabilities.as_ref()) {
                    return Ok(VerifyResult::uncertain(r.confidence, r.runner_up, r.margin, r.probabilities.clone(), json!({"via": "wait_until_visual_uncertain", "query": query, "decider": r.meta})));
                }
                return Ok(r);
            },
            Err(e) => eprintln!("[wait_until] visual decider failed: {e}"),
        }
    }
    Ok(VerifyResult::failure(json!({"reason": "timeout", "query": query, "timeout_ms": timeout.as_millis() as u64})))
}

// ---------------------------------------------------------------------------
// Sync wrappers
// ---------------------------------------------------------------------------

fn rt_block_on<F: std::future::Future>(f: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread().enable_all().build().expect("verify rt").block_on(f)
}

pub fn verify(query: &str) -> Result<VerifyResult> { rt_block_on(verify_async(query)) }
pub fn verify_element(candidate: &crate::decider::candidates::Candidate) -> Result<VerifyResult> { rt_block_on(verify_element_async(candidate)) }
pub fn verify_action(query: &str, expected: Option<&str>) -> Result<VerifyResult> { rt_block_on(verify_action_async(query, expected)) }
pub fn wait_until(query: &str, timeout: Duration, interval: Duration) -> Result<VerifyResult> { rt_block_on(wait_until_async(query, timeout, interval)) }

/// Convenience alias for public export
pub fn verify_sync(query: &str) -> Result<VerifyResult> { verify(query) }

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn low_confidence_check() {
        assert!(low_confidence(Some(0.4), None, None, None));
        assert!(low_confidence(Some(0.9), Some(0.85), None, None));
        assert!(!low_confidence(Some(0.9), Some(0.6), None, None));
    }
}
