//! Candidate abstraction for Decider-2B integration.
//!
//! Vision model calibrated on **10 options**, text model on **255 options**.
//! Stable numeric IDs (1..N) never renumbered after a Decider request.
//! Deterministic filtering first, then hierarchical grouping when over limits.
//!
//! Reuses hint overlay's DOM scan (`hint_snapshot`) as the canonical visual
//! candidate source, but is agnostic to origin (AX snapshot, DOM scan, etc).
use std::collections::HashMap;

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

// ---------------------------------------------------------------------------
// Constants — model calibration limits
// ---------------------------------------------------------------------------

/// Max options per visual question (image pipeline).
pub const VISION_MAX_OPTIONS: usize = 10;
/// Max options per text question.
pub const TEXT_MAX_OPTIONS: usize = 255;

// ---------------------------------------------------------------------------
// Rect
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl Rect {
    pub fn new(x: i32, y: i32, width: i32, height: i32) -> Self {
        Self { x, y, width, height }
    }
    pub fn is_empty(&self) -> bool {
        self.width <= 0 || self.height <= 0
    }
    pub fn area(&self) -> i64 {
        self.width as i64 * self.height as i64
    }
    /// Overlaps viewport?
    pub fn overlaps(&self, other: &Rect) -> bool {
        let x1 = self.x.max(other.x);
        let y1 = self.y.max(other.y);
        let x2 = (self.x + self.width).min(other.x + other.width);
        let y2 = (self.y + self.height).min(other.y + other.height);
        x2 > x1 && y2 > y1
    }
    pub fn contains(&self, px: i32, py: i32) -> bool {
        px >= self.x && px < self.x + self.width && py >= self.y && py < self.y + self.height
    }
}

// ---------------------------------------------------------------------------
// Candidate
// ---------------------------------------------------------------------------

/// One actionable candidate exposed to the Decider.
///
/// `id` is 1-indexed stable numeric ID that maps to Decider option IDs
/// "1".."N". `label` preserves the original hint overlay label (e.g. "A",
/// "AA") for UI/grounding. IDs are assigned deterministically in scan order
/// and never renumbered after a Decider round — hierarchical winners keep
/// their original `id`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Candidate {
    /// Stable numeric ID 1..N (1-indexed, deterministic).
    pub id: u32,
    /// Original hint label (e.g. "A", "S", "AA"). Empty if not from hint.
    pub label: String,
    /// DOM tag (e.g. "button", "a", "input").
    pub tag: String,
    /// ARIA role or derived role (e.g. "button", "link", "textbox").
    pub role: String,
    /// Accessible name (aria-label / innerText / placeholder) truncated <=120.
    pub name: String,
    /// Visible text content (same source as name, but preserved separately).
    pub text: String,
    /// Bounding rect in viewport CSS pixels, rounded.
    pub rect: Rect,
    /// Whether element is visible (in-viewport, not hidden, opacity>0).
    pub visible: bool,
    /// Whether element is enabled (not disabled, not aria-disabled).
    pub enabled: bool,
    /// Unique CSS selector for deterministic re-query.
    pub selector: String,
    /// Optional originating targetId (for multi-tab contexts).
    pub target_id: Option<String>,
    /// Optional page URL where candidate was found.
    pub url: Option<String>,
    /// Optional page title.
    pub title: Option<String>,
}

impl Candidate {
    /// Human-readable display for Decider `options` text.
    /// Deterministic format: includes role/tag/name/text + geometry + selector.
    pub fn display_text(&self) -> String {
        let mut parts = Vec::new();
        if !self.role.is_empty() {
            parts.push(format!("[{}]", self.role));
        }
        if !self.tag.is_empty() {
            parts.push(format!("<{}>", self.tag));
        }
        let primary = if !self.name.is_empty() { &self.name } else { &self.text };
        if !primary.is_empty() {
            parts.push(truncate(primary, 80));
        } else if !self.selector.is_empty() {
            parts.push(truncate(&self.selector, 40));
        }
        parts.push(format!(
            "id={} label={} @{},{} {}x{}",
            self.id, self.label, self.rect.x, self.rect.y, self.rect.width, self.rect.height
        ));
        parts.join(" ")
    }

    /// Short option text for vision prompts (name + tag, bounded).
    pub fn vision_text(&self) -> String {
        let mut s = if !self.name.is_empty() {
            truncate(&self.name, 60)
        } else if !self.text.is_empty() {
            truncate(&self.text, 60)
        } else {
            format!("{} {}", self.role, self.tag)
        };
        // disambiguate visually identical names with label
        if s.len() < 60 && !self.label.is_empty() {
            s.push_str(&format!(" ({})", self.label));
        }
        s
    }

    /// Validate geometry.
    pub fn has_valid_geometry(&self) -> bool {
        !self.rect.is_empty() && self.rect.width > 0 && self.rect.height > 0
    }

    /// Is candidate inside viewport rect?
    pub fn in_viewport(&self, vp: &Rect) -> bool {
        self.rect.overlaps(vp)
    }

    /// Normalized text (lowercase, collapse whitespace, trim).
    pub fn normalized_text(&self) -> String {
        normalize(&self.text)
    }
    pub fn normalized_name(&self) -> String {
        normalize(&self.name)
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut t: String = s.chars().take(max - 3).collect();
    t.push_str("...");
    t
}

fn normalize(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

// ---------------------------------------------------------------------------
// Candidates collection
// ---------------------------------------------------------------------------

/// Deterministic collection of candidates with stable IDs.
///
/// Never silently truncates: callers must explicitly chunk/hierarchically
/// resolve when over Decider limits. Filtering is explicit and deterministic.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Candidates {
    candidates: Vec<Candidate>,
    /// label -> id mapping (original hint labels like "A" -> numeric id)
    #[serde(skip)]
    label_to_id: HashMap<String, u32>,
    /// id -> label mapping (inverse)
    #[serde(skip)]
    id_to_label: HashMap<u32, String>,
}

impl Candidates {
    /// Empty collection.
    pub fn new() -> Self {
        Self {
            candidates: Vec::new(),
            label_to_id: HashMap::new(),
            id_to_label: HashMap::new(),
        }
    }

    /// Build from a pre-sorted Vec<Candidate>, rebuilding label maps.
    /// Candidates are sorted by `id` deterministically.
    pub fn from_vec(mut v: Vec<Candidate>) -> Self {
        v.sort_by_key(|c| c.id);
        let mut label_to_id = HashMap::new();
        let mut id_to_label = HashMap::new();
        for c in &v {
            if !c.label.is_empty() {
                label_to_id.insert(c.label.clone(), c.id);
                id_to_label.insert(c.id, c.label.clone());
            }
        }
        Self {
            candidates: v,
            label_to_id,
            id_to_label,
        }
    }

    /// Number of candidates.
    pub fn len(&self) -> usize {
        self.candidates.len()
    }
    pub fn is_empty(&self) -> bool {
        self.candidates.is_empty()
    }
    pub fn iter(&self) -> std::slice::Iter<'_, Candidate> {
        self.candidates.iter()
    }
    pub fn as_slice(&self) -> &[Candidate] {
        &self.candidates
    }

    /// Get candidate by numeric `id`.
    pub fn get(&self, id: u32) -> Option<&Candidate> {
        self.candidates.iter().find(|c| c.id == id)
    }
    /// Get candidate by original hint label (e.g. "A").
    pub fn get_by_label(&self, label: &str) -> Option<&Candidate> {
        self.label_to_id
            .get(label)
            .and_then(|id| self.get(*id))
    }

    /// Numeric ID for a label, if mapped.
    pub fn id_for_label(&self, label: &str) -> Option<u32> {
        self.label_to_id.get(label).copied()
    }
    /// Original label for a numeric ID.
    pub fn label_for_id(&self, id: u32) -> Option<&str> {
        self.id_to_label.get(&id).map(|s| s.as_str())
    }

    /// Stable IDs in deterministic order (id ascending = scan order).
    pub fn ids(&self) -> Vec<u32> {
        self.candidates.iter().map(|c| c.id).collect()
    }

    /// Ensure within `max` limit, else error (never truncate silently).
    pub fn ensure_within_limit(&self, max: usize) -> Result<()> {
        if self.candidates.len() > max {
            bail!(
                "candidates count {} exceeds limit {} (use hierarchical grouping or filtering — never silently truncates)",
                self.candidates.len(),
                max
            );
        }
        Ok(())
    }

    /// Validate for vision (<=10) / text (<=255) budgets.
    pub fn ensure_vision_limit(&self) -> Result<()> {
        self.ensure_within_limit(VISION_MAX_OPTIONS)
    }
    pub fn ensure_text_limit(&self) -> Result<()> {
        self.ensure_within_limit(TEXT_MAX_OPTIONS)
    }

    // -----------------------------------------------------------------------
    // Conversion from hint_snapshot
    // -----------------------------------------------------------------------

    /// Convert `hint_snapshot` JSON (`{"hints":[...],"count":N}` or bare array)
    /// into `Candidates` with numeric IDs 1..N mapping to original labels.
    /// Preserves label mapping and assigns deterministic stable IDs in the
    /// hint's scan order. Optional `target_id`/`url`/`title` are applied to
    /// every candidate when provided.
    pub fn from_hint_snapshot(
        value: &Value,
        target_id: Option<String>,
        url: Option<String>,
        title: Option<String>,
    ) -> Self {
        let arr = if let Some(hints) = value.get("hints").and_then(|v| v.as_array()) {
            hints.clone()
        } else if let Some(arr) = value.as_array() {
            arr.clone()
        } else {
            // Single object degenerate case: wrap
            vec![value.clone()]
        };

        let mut out = Vec::with_capacity(arr.len());
        for (idx, h) in arr.iter().enumerate() {
            // Anonymous hints (no label) still get numeric id; label empty.
            let label = h
                .get("label")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let tag = h
                .get("tag")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let role = h
                .get("role")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let name = h
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let text = h
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let selector = h
                .get("selector")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let rect = h.get("rect").map(parse_rect).unwrap_or(Rect::new(0, 0, 0, 0));
            // Hint scan already filters visibility, but we preserve explicit fields if present.
            let visible = h
                .get("visible")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            let enabled = h
                .get("enabled")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            // Fallback rect from single top-level x,y,width,height
            let rect = if rect.is_empty() {
                let x = h.get("x").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
                let y = h.get("y").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
                let w = h.get("width").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
                let hh = h.get("height").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
                if w > 0 || hh > 0 {
                    Rect::new(x, y, w, hh)
                } else {
                    rect
                }
            } else {
                rect
            };

            let c = Candidate {
                id: (idx as u32) + 1,
                label,
                tag,
                role,
                name,
                text,
                rect,
                visible,
                enabled,
                selector,
                target_id: target_id.clone(),
                url: url.clone(),
                title: title.clone(),
            };
            out.push(c);
        }
        Self::from_vec(out)
    }

    /// Convenience: no target meta.
    pub fn from_hint_value(value: &Value) -> Self {
        Self::from_hint_snapshot(value, None, None, None)
    }

    /// Build from generic snapshot nodes (AX tree, etc) — maps any array of
    /// objects with name/role/tag/rect/selector into candidates with stable ids.
    pub fn from_generic_value(value: &Value) -> Self {
        Self::from_hint_snapshot(value, None, None, None)
    }

    // -----------------------------------------------------------------------
    // Deterministic filtering
    // -----------------------------------------------------------------------

    /// Generic filter that preserves stable IDs and deterministic order.
    pub fn filtered<F>(&self, predicate: F) -> Self
    where
        F: Fn(&Candidate) -> bool,
    {
        let v: Vec<Candidate> = self
            .candidates
            .iter()
            .filter(|c| predicate(c))
            .cloned()
            .collect();
        Self::from_vec(v)
    }

    /// Filter with `CandidateFilter` (combines multiple predicates with AND).
    pub fn filter(&self, f: &CandidateFilter) -> Self {
        self.filtered(|c| f.matches(c))
    }

    // Convenience single-dimension filters (deterministic, compose by chaining):

    pub fn filter_visible(&self) -> Self {
        self.filtered(|c| c.visible)
    }
    pub fn filter_enabled(&self) -> Self {
        self.filtered(|c| c.enabled)
    }
    pub fn filter_non_empty_rect(&self) -> Self {
        self.filtered(|c| c.has_valid_geometry())
    }
    pub fn filter_by_role(&self, role: &str) -> Self {
        let low = role.to_lowercase();
        self.filtered(|c| c.role.to_lowercase() == low)
    }
    pub fn filter_by_tag(&self, tag: &str) -> Self {
        let low = tag.to_lowercase();
        self.filtered(|c| c.tag.to_lowercase() == low)
    }
    pub fn filter_by_text_exact(&self, text: &str) -> Self {
        self.filtered(|c| c.text == text || c.name == text)
    }
    pub fn filter_by_text_normalized(&self, text: &str) -> Self {
        let n = normalize(text);
        self.filtered(|c| c.normalized_text() == n || c.normalized_name() == n)
    }
    pub fn filter_by_name_contains(&self, needle: &str) -> Self {
        let n = normalize(needle);
        if n.is_empty() {
            return self.clone();
        }
        self.filtered(|c| c.normalized_name().contains(&n) || c.normalized_text().contains(&n))
    }
    pub fn filter_viewport(&self, vp: Rect) -> Self {
        self.filtered(|c| c.in_viewport(&vp))
    }
    pub fn filter_geometry(&self, min_width: i32, min_height: i32) -> Self {
        self.filtered(|c| c.rect.width >= min_width && c.rect.height >= min_height)
    }

    /// Keep only candidates with `id` in `ids` (stable IDs preserved, order = original id order).
    pub fn keep_ids(&self, ids: &[u32]) -> Self {
        let set: std::collections::HashSet<u32> = ids.iter().copied().collect();
        self.filtered(|c| set.contains(&c.id))
    }

    /// Apply deterministic hierarchical filtering pipeline when over `max`:
    /// 1) visible+enabled+non-empty geometry, 2) viewport overlap if `viewport` given,
    /// 3) minimum geometry, then return. Never truncates; caller must chunk after.
    pub fn filtered_for_budget(
        &self,
        viewport: Option<Rect>,
        min_geometry: Option<(i32, i32)>,
    ) -> Self {
        let mut cur = self.filter_visible().filter_enabled().filter_non_empty_rect();
        if let Some(vp) = viewport {
            cur = cur.filter_viewport(vp);
        }
        if let Some((mw, mh)) = min_geometry {
            cur = cur.filter_geometry(mw, mh);
        }
        cur
    }

    // -----------------------------------------------------------------------
    // Hierarchical chunking
    // -----------------------------------------------------------------------

    /// Deterministic chunking into groups of at most `max_per_group`.
    /// Preserves scan order (id ascending) within and across groups.
    /// Never renumbers IDs — groups contain clones with original `id`.
    pub fn chunks(&self, max_per_group: usize) -> Vec<Vec<Candidate>> {
        if max_per_group == 0 {
            return vec![];
        }
        let mut groups: Vec<Vec<Candidate>> = Vec::new();
        let mut cur: Vec<Candidate> = Vec::new();
        for c in &self.candidates {
            cur.push(c.clone());
            if cur.len() >= max_per_group {
                groups.push(std::mem::take(&mut cur));
            }
        }
        if !cur.is_empty() {
            groups.push(cur);
        }
        groups
    }

    /// Chunks for text Decider (<=255 per question).
    pub fn chunks_for_text(&self) -> Vec<Vec<Candidate>> {
        self.chunks(TEXT_MAX_OPTIONS)
    }
    /// Chunks for vision Decider (<=10 per visual question).
    pub fn chunks_for_vision(&self) -> Vec<Vec<Candidate>> {
        self.chunks(VISION_MAX_OPTIONS)
    }

    /// Number of Decider calls required to cover all candidates at `max_per_group`.
    pub fn hierarchical_call_count(&self, max_per_group: usize) -> usize {
        if self.candidates.is_empty() {
            return 0;
        }
        (self.candidates.len() + max_per_group - 1) / max_per_group
    }

    /// Hierarchical selection helper: given per-group winning ids (one per chunk),
    /// return the final candidate set for the last Decider call (still stable ids).
    ///
    /// Deterministic: winners are collected in chunk order, duplicates removed
    /// preserving first occurrence, sorted by original id.
    pub fn group_winners(&self, winning_ids: &[u32]) -> Self {
        let mut seen = std::collections::HashSet::new();
        let mut winners = Vec::new();
        for &id in winning_ids {
            if seen.insert(id) {
                if let Some(c) = self.get(id) {
                    winners.push(c.clone());
                }
            }
        }
        winners.sort_by_key(|c| c.id);
        Self::from_vec(winners)
    }

    // -----------------------------------------------------------------------
    // Decider question conversion
    // -----------------------------------------------------------------------

    /// Convert candidates to Decider option strings (deterministic display text).
    /// Order = `id` ascending (stable). Caller builds `DeciderQuestion` from this.
    pub fn to_option_texts(&self) -> Vec<String> {
        self.candidates.iter().map(|c| c.display_text()).collect()
    }
    /// Vision-budget option texts (shorter).
    pub fn to_vision_option_texts(&self) -> Vec<String> {
        self.candidates.iter().map(|c| c.vision_text()).collect()
    }

    /// Build DeciderQuestions for hierarchical groups (text budget).
    /// Returns one question per chunk, each with <=255 options.
    /// The caller is responsible for merging group winners and final call.
    pub fn to_text_questions(&self, question_prefix: &str) -> Vec<crate::decider::types::DeciderQuestion> {
        self.chunks_for_text()
            .into_iter()
            .enumerate()
            .map(|(idx, chunk)| {
                let opts: Vec<String> = chunk.iter().map(|c| c.display_text()).collect();
                let q = if question_prefix.is_empty() {
                    format!("Select the best match (group {}/{})", idx + 1, (self.len() + TEXT_MAX_OPTIONS - 1) / TEXT_MAX_OPTIONS)
                } else {
                    format!("{} — group {}/{}", question_prefix, idx + 1, (self.len() + TEXT_MAX_OPTIONS - 1) / TEXT_MAX_OPTIONS)
                };
                crate::decider::types::DeciderQuestion::new(q, opts)
            })
            .collect()
    }

    /// Build DeciderQuestions for vision (<=10 per group, typically one per screenshot page).
    pub fn to_vision_questions(
        &self,
        question_prefix: &str,
    ) -> Vec<crate::decider::types::DeciderQuestion> {
        self.chunks_for_vision()
            .into_iter()
            .enumerate()
            .map(|(idx, chunk)| {
                let opts: Vec<String> = chunk.iter().map(|c| c.vision_text()).collect();
                let q = if question_prefix.is_empty() {
                    format!(
                        "Identify the highlighted element (page {}/{})",
                        idx + 1,
                        (self.len() + VISION_MAX_OPTIONS - 1) / VISION_MAX_OPTIONS
                    )
                } else {
                    format!("{} — page {}/{}", question_prefix, idx + 1, (self.len() + VISION_MAX_OPTIONS - 1) / VISION_MAX_OPTIONS)
                };
                crate::decider::types::DeciderQuestion::new(q, opts)
            })
            .collect()
    }

    /// Map a Decider choice ID "1".."N" within a `chunk` back to the global stable candidate id.
    /// `chunk_idx` is the 0-based group index; `choice_id` is the 1-based option within that chunk.
    pub fn map_choice_to_global_id(&self, chunk_idx: usize, choice_id: &str, max_per_group: usize) -> Option<u32> {
        let idx: usize = choice_id.parse().ok()?;
        if idx == 0 {
            return None;
        }
        let global_offset = chunk_idx * max_per_group + (idx - 1);
        self.candidates.get(global_offset).map(|c| c.id)
    }

    // -----------------------------------------------------------------------
    // Serialization helpers
    // -----------------------------------------------------------------------

    /// Serialize to JSON array (for wire/debug), includes stable ids.
    pub fn to_json(&self) -> Value {
        serde_json::to_value(&self.candidates).unwrap_or(Value::Array(vec![]))
    }

    /// Create from JSON array previously produced by `to_json`.
    pub fn from_json(value: &Value) -> Result<Self> {
        let v: Vec<Candidate> =
            serde_json::from_value(value.clone()).map_err(|e| anyhow::anyhow!("parse candidates: {e}"))?;
        Ok(Self::from_vec(v))
    }
}

// ---------------------------------------------------------------------------
// CandidateFilter — deterministic multi-predicate
// ---------------------------------------------------------------------------

/// Deterministic filter combining multiple predicates with AND logic.
/// Each field is optional; when `None`, that dimension is not filtered.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CandidateFilter {
    /// Exact text or name match (case-sensitive).
    pub text_exact: Option<String>,
    /// Normalized text or name match (case-insensitive, whitespace collapsed).
    pub text_normalized: Option<String>,
    /// Substring in normalized name/text (case-insensitive, whitespace collapsed).
    pub name_contains: Option<String>,
    /// Exact role match (case-insensitive).
    pub role: Option<String>,
    /// Exact tag match (case-insensitive).
    pub tag: Option<String>,
    /// Require visible == true when Some(true), invisible when Some(false).
    pub visible: Option<bool>,
    /// Require enabled == true when Some(true).
    pub enabled: Option<bool>,
    /// Minimum geometry (width, height) — rejects smaller.
    pub min_width: Option<i32>,
    pub min_height: Option<i32>,
    /// Viewport rect — only keep candidates overlapping it.
    pub viewport: Option<Rect>,
    /// Allowed numeric IDs (when Some, only these ids pass).
    pub ids: Option<Vec<u32>>,
}

impl CandidateFilter {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn with_text_exact(mut self, t: impl Into<String>) -> Self {
        self.text_exact = Some(t.into());
        self
    }
    pub fn with_text_normalized(mut self, t: impl Into<String>) -> Self {
        self.text_normalized = Some(t.into());
        self
    }
    pub fn with_name_contains(mut self, t: impl Into<String>) -> Self {
        self.name_contains = Some(t.into());
        self
    }
    pub fn with_role(mut self, r: impl Into<String>) -> Self {
        self.role = Some(r.into());
        self
    }
    pub fn with_tag(mut self, t: impl Into<String>) -> Self {
        self.tag = Some(t.into());
        self
    }
    pub fn with_visible(mut self, v: bool) -> Self {
        self.visible = Some(v);
        self
    }
    pub fn with_enabled(mut self, v: bool) -> Self {
        self.enabled = Some(v);
        self
    }
    pub fn with_min_geometry(mut self, w: i32, h: i32) -> Self {
        self.min_width = Some(w);
        self.min_height = Some(h);
        self
    }
    pub fn with_viewport(mut self, r: Rect) -> Self {
        self.viewport = Some(r);
        self
    }
    pub fn with_ids(mut self, ids: Vec<u32>) -> Self {
        self.ids = Some(ids);
        self
    }

    pub fn matches(&self, c: &Candidate) -> bool {
        if let Some(ref t) = self.text_exact {
            if !(c.text == *t || c.name == *t) {
                return false;
            }
        }
        if let Some(ref t) = self.text_normalized {
            let n = normalize(t);
            if !(c.normalized_text() == n || c.normalized_name() == n) {
                return false;
            }
        }
        if let Some(ref needle) = self.name_contains {
            let n = normalize(needle);
            if !n.is_empty()
                && !(c.normalized_name().contains(&n) || c.normalized_text().contains(&n))
            {
                return false;
            }
        }
        if let Some(ref r) = self.role {
            if c.role.to_lowercase() != r.to_lowercase() {
                return false;
            }
        }
        if let Some(ref t) = self.tag {
            if c.tag.to_lowercase() != t.to_lowercase() {
                return false;
            }
        }
        if let Some(v) = self.visible {
            if c.visible != v {
                return false;
            }
        }
        if let Some(v) = self.enabled {
            if c.enabled != v {
                return false;
            }
        }
        if let Some(mw) = self.min_width {
            if c.rect.width < mw {
                return false;
            }
        }
        if let Some(mh) = self.min_height {
            if c.rect.height < mh {
                return false;
            }
        }
        if let Some(vp) = self.viewport {
            if !c.in_viewport(&vp) {
                return false;
            }
        }
        if let Some(ref ids) = self.ids {
            if !ids.contains(&c.id) {
                return false;
            }
        }
        true
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn parse_rect(v: &Value) -> Rect {
    let x = v
        .get("x")
        .and_then(|x| x.as_i64())
        .or_else(|| v.get("left").and_then(|x| x.as_i64()))
        .unwrap_or(0) as i32;
    let y = v
        .get("y")
        .and_then(|x| x.as_i64())
        .or_else(|| v.get("top").and_then(|x| x.as_i64()))
        .unwrap_or(0) as i32;
    let w = v
        .get("width")
        .and_then(|x| x.as_i64())
        .unwrap_or(0) as i32;
    let h = v
        .get("height")
        .and_then(|x| x.as_i64())
        .unwrap_or(0) as i32;
    Rect::new(x, y, w, h)
}

// ---------------------------------------------------------------------------
// Tests — deterministic, no network
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_hints() -> Value {
        json!({
            "hints": [
                {"label":"A","tag":"button","role":"button","name":"Submit","text":"Submit","rect":{"x":10,"y":20,"width":80,"height":30},"selector":"button","visible":true,"enabled":true},
                {"label":"S","tag":"a","role":"link","name":"Cancel","text":"Cancel","rect":{"x":100,"y":20,"width":60,"height":30},"selector":"a.cancel","visible":true,"enabled":true},
                {"label":"D","tag":"input","role":"textbox","name":"Search","text":"","rect":{"x":10,"y":60,"width":200,"height":28},"selector":"input","visible":true,"enabled":false}
            ],
            "count": 3
        })
    }

    #[test]
    fn from_hint_preserves_label_mapping() {
        let cands = Candidates::from_hint_value(&sample_hints());
        assert_eq!(cands.len(), 3);
        assert_eq!(cands.id_for_label("A"), Some(1));
        assert_eq!(cands.id_for_label("S"), Some(2));
        assert_eq!(cands.label_for_id(1), Some("A"));
        assert_eq!(cands.label_for_id(2), Some("S"));
        assert_eq!(cands.get(1).unwrap().name, "Submit");
        // stable order
        assert_eq!(cands.ids(), vec![1, 2, 3]);
    }

    #[test]
    fn filtering_is_deterministic() {
        let cands = Candidates::from_hint_value(&sample_hints());
        let visible = cands.filter_visible();
        assert_eq!(visible.len(), 3);
        let enabled = cands.filter_enabled();
        assert_eq!(enabled.len(), 2);
        assert_eq!(enabled.ids(), vec![1, 2]);
        let by_role = cands.filter_by_role("button");
        assert_eq!(by_role.len(), 1);
        assert_eq!(by_role.get(1).unwrap().label, "A");
        let by_text = cands.filter_by_text_exact("Cancel");
        assert_eq!(by_text.len(), 1);
        assert_eq!(by_text.get(2).unwrap().label, "S");
        // normalized
        let normed = cands.filter_by_text_normalized("  submit ");
        assert_eq!(normed.len(), 1);
        assert_eq!(normed.get(1).unwrap().label, "A");
        // combined filter keeps stable ids
        let f = CandidateFilter::new().with_role("link").with_visible(true);
        let filtered = cands.filter(&f);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered.get(2).unwrap().label, "S");
    }

    #[test]
    fn never_truncate() {
        let mut v = Vec::new();
        for i in 1..=300u32 {
            v.push(Candidate {
                id: i,
                label: format!("L{}", i),
                tag: "div".into(),
                role: "generic".into(),
                name: format!("item {}", i),
                text: format!("item {}", i),
                rect: Rect::new(0, 0, 10, 10),
                visible: true,
                enabled: true,
                selector: format!("#id{}", i),
                target_id: None,
                url: None,
                title: None,
            });
        }
        let cands = Candidates::from_vec(v);
        assert!(cands.ensure_within_limit(255).is_err());
        // hierarchical chunking
        let groups = cands.chunks_for_text();
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].len(), 255);
        assert_eq!(groups[1].len(), 45);
        // first group's first id is 1, second group's first id is 256 (stable)
        assert_eq!(groups[0][0].id, 1);
        assert_eq!(groups[1][0].id, 256);
    }

    #[test]
    fn vision_chunks_of_ten() {
        let mut v = Vec::new();
        for i in 1..=23u32 {
            v.push(Candidate {
                id: i,
                label: format!("L{}", i),
                tag: "button".into(),
                role: "button".into(),
                name: format!("btn {}", i),
                text: format!("btn {}", i),
                rect: Rect::new(0, 0, 10, 10),
                visible: true,
                enabled: true,
                selector: format!("#b{}", i),
                target_id: None,
                url: None,
                title: None,
            });
        }
        let cands = Candidates::from_vec(v);
        let vg = cands.chunks_for_vision();
        assert_eq!(vg.len(), 3);
        assert_eq!(vg[0].len(), 10);
        assert_eq!(vg[1].len(), 10);
        assert_eq!(vg[2].len(), 3);
        // winners -> final set
        let winners = cands.group_winners(&[1, 11, 21]);
        assert_eq!(winners.len(), 3);
        assert_eq!(winners.ids(), vec![1, 11, 21]);
    }

    #[test]
    fn map_choice_to_global() {
        let mut v = Vec::new();
        for i in 1..=23u32 {
            v.push(Candidate {
                id: i,
                label: format!("L{}", i),
                tag: "div".into(),
                role: "generic".into(),
                name: format!("n{}", i),
                text: format!("t{}", i),
                rect: Rect::new(0, 0, 10, 10),
                visible: true,
                enabled: true,
                selector: "".into(),
                target_id: None,
                url: None,
                title: None,
            });
        }
        let cands = Candidates::from_vec(v);
        // chunk 0 choice "3" -> global id 3
        assert_eq!(cands.map_choice_to_global_id(0, "3", 10), Some(3));
        // chunk 1 choice "2" -> offset 10 +1 = 12
        assert_eq!(cands.map_choice_to_global_id(1, "2", 10), Some(12));
        // chunk 2 choice "1" -> offset 20+0 = 21
        assert_eq!(cands.map_choice_to_global_id(2, "1", 10), Some(21));
    }

    #[test]
    fn viewport_and_geometry() {
        let cands = Candidates::from_hint_value(&sample_hints());
        let vp = Rect::new(0, 0, 90, 50); // only first hint overlaps
        let filtered = cands.filter_viewport(vp);
        assert!(filtered.get(1).is_some());
        // second hint at x=100 is outside
        assert!(filtered.get(2).is_none());
        let geom = cands.filter_geometry(70, 25);
        // first button 80x30 passes, second 60x30 fails width, third 200x28 passes
        assert!(geom.get(1).is_some());
        assert!(geom.get(2).is_none());
    }

    #[test]
    fn keep_ids_stable() {
        let cands = Candidates::from_hint_value(&sample_hints());
        let kept = cands.keep_ids(&[3, 1]);
        // order is original id ascending, not request order
        assert_eq!(kept.ids(), vec![1, 3]);
        assert_eq!(kept.get(1).unwrap().label, "A");
        assert_eq!(kept.get(3).unwrap().label, "D");
    }
}
