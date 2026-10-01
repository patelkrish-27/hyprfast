//! Typed request/response structures for the Decider service.
//!
//! Wire rules:
//! - Request carries `context` (alias `state` on deserialize) + `questions` + optional `image`.
//! - Each question has `question` + `options: Vec<String>` (1..=255, IDs "1".."255" internally).
//! - Response carries per-question `decisions: Vec<DeciderDecision>` plus optional
//!   `usage` / `device` / `model` / `latency`.
//! - Response `answers` accepts TWO shapes: an array of decisions, or decider-serve's
//!   map `{"q1": {...}, "q2": {...}}` (ordered by numeric suffix).
//! - Choice validation: `choice` must be one of "1".."N" where N = options.len(),
//!   or exactly match an option text (decider-serve returns option TEXT, e.g.
//!   `"choice": "banana"` with text-keyed probabilities; `normalize_choices`
//!   maps those to numeric IDs before validation).
//! - Probabilities/confidence are preserved verbatim (not renormalized).
//! - Deserialization is robust: accepts both "context" and "state", several
//!   latency/model/device key aliases, float `latency_ms`, and numeric `choice`.

use std::collections::HashMap;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

// ---------------------------------------------------------------------------
// Helpers: context/state alias
// ---------------------------------------------------------------------------

fn get_context_from_value(v: &Value) -> String {
    v.get("context")
        .or_else(|| v.get("state"))
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string()
}

// ---------------------------------------------------------------------------
// DeciderQuestion
// ---------------------------------------------------------------------------

/// One question to decide. Options are mapped internally to IDs "1".."255".
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeciderQuestion {
    pub question: String,
    pub options: Vec<String>,
}

impl DeciderQuestion {
    pub fn new(question: impl Into<String>, options: Vec<String>) -> Self {
        Self {
            question: question.into(),
            options,
        }
    }

    /// Validate: 1..=255 options, no empty option strings.
    pub fn validate(&self) -> Result<()> {
        let n = self.options.len();
        if n == 0 {
            bail!("question '{}' has no options", self.question);
        }
        if n > 255 {
            bail!(
                "question '{}' has {n} options, exceeds max 255",
                self.question
            );
        }
        for (i, opt) in self.options.iter().enumerate() {
            if opt.trim().is_empty() {
                bail!(
                    "question '{}' option {} is empty",
                    self.question,
                    i + 1
                );
            }
        }
        Ok(())
    }

    /// Numeric IDs "1".."N" for this question's options.
    pub fn option_ids(&self) -> Vec<String> {
        (1..=self.options.len()).map(|i| i.to_string()).collect()
    }

    /// Map a numeric choice ID "1".."N" to the actual option text, if valid.
    pub fn choice_text(&self, choice_id: &str) -> Option<&str> {
        let idx: usize = choice_id.parse().ok()?;
        if idx == 0 || idx > self.options.len() {
            return None;
        }
        self.options.get(idx - 1).map(|s| s.as_str())
    }
}

// ---------------------------------------------------------------------------
// DeciderRequest
// ---------------------------------------------------------------------------

/// Typed request. Serializes as `context`; deserializes from either `context` or `state`.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct DeciderRequest {
    /// Context text (alias `state` on the wire for compat).
    #[serde(rename = "context")]
    pub context: String,
    pub questions: Vec<DeciderQuestion>,
    /// Optional image: raw base64 or data URI (`data:image/...;base64,<b64>`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    /// Optional temperature forwarded to the service. If None, service default is used.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
}

// Custom Deserialize for DeciderRequest to handle alias at top level.
impl<'de> Deserialize<'de> for DeciderRequest {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Raw {
            #[serde(default, alias = "state")]
            context: Option<String>,
            #[serde(default)]
            questions: Vec<DeciderQuestion>,
            #[serde(default)]
            image: Option<String>,
            #[serde(default)]
            temperature: Option<f32>,
        }
        let raw = Raw::deserialize(deserializer)?;
        Ok(Self {
            context: raw.context.unwrap_or_default(),
            questions: raw.questions,
            image: raw.image,
            temperature: raw.temperature,
        })
    }
}

impl DeciderRequest {
    pub fn new(context: impl Into<String>, questions: Vec<DeciderQuestion>) -> Self {
        Self {
            context: context.into(),
            questions,
            image: None,
            temperature: None,
        }
    }

    pub fn with_image(mut self, image: impl Into<String>) -> Self {
        let s = image.into();
        if s.trim().is_empty() {
            self.image = None;
        } else {
            self.image = Some(s);
        }
        self
    }

    pub fn with_temperature(mut self, temp: f32) -> Self {
        self.temperature = Some(temp);
        self
    }

    /// Validate all questions and image encoding (if present).
    pub fn validate(&self) -> Result<()> {
        if self.questions.is_empty() {
            bail!("DeciderRequest has no questions");
        }
        if self.questions.len() > 255 {
            // Questions themselves not strictly limited, but keep reasonable.
            // Spec says up to 255 options per question; questions count not limited.
        }
        for q in &self.questions {
            q.validate()?;
        }
        if let Some(img) = &self.image {
            validate_image_field(img)?;
        }
        Ok(())
    }

    /// Strip data URI prefix if present, returning raw base64.
    pub fn image_b64(&self) -> Option<String> {
        self.image.as_ref().map(|s| strip_data_uri(s))
    }
}

// ---------------------------------------------------------------------------
// Image helpers
// ---------------------------------------------------------------------------

/// Validate that image field is either raw base64 or a data URI with valid base64 payload.
pub fn validate_image_field(s: &str) -> Result<()> {
    let b64 = strip_data_uri(s);
    if b64.trim().is_empty() {
        bail!("image field is empty after stripping data URI prefix");
    }
    // Validate base64 decodability (allow whitespace).
    use base64::Engine;
    // Try standard and url-safe.
    let cleaned: String = b64.chars().filter(|c| !c.is_whitespace()).collect();
    base64::engine::general_purpose::STANDARD
        .decode(&cleaned)
        .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(&cleaned))
        .context("image field is not valid base64 (or data URI base64)")?;
    Ok(())
}

/// If `s` is a data URI (`data:image/...;base64,<payload>`), return `<payload>`, else `s`.
pub fn strip_data_uri(s: &str) -> String {
    if let Some(comma) = s.find(',') {
        let prefix = &s[..comma];
        if prefix.starts_with("data:") && prefix.contains("base64") {
            return s[comma + 1..].to_string();
        }
    }
    s.to_string()
}

// ---------------------------------------------------------------------------
// DeciderDecision / DeciderResponse
// ---------------------------------------------------------------------------

/// Per-question decision.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeciderDecision {
    /// Choice as numeric ID "1".."255" (validated against the question's options).
    /// decider-serve returns the option TEXT instead ("banana"); both forms are
    /// accepted and text is normalized to a numeric ID by `normalize_choices`.
    /// A bare JSON number is also accepted for robustness.
    #[serde(deserialize_with = "de_choice_string")]
    pub choice: String,
    /// Confidence in [0,1] if provided.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
    /// Probabilities map: numeric ID -> probability. Preserved verbatim.
    /// decider-serve keys this by option TEXT; normalized by `normalize_choices`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub probabilities: Option<HashMap<String, f64>>,
}

/// Deserialize `choice` from a JSON string ("2", "banana") or a bare number (2).
fn de_choice_string<'de, D>(deserializer: D) -> std::result::Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    struct ChoiceVisitor;
    impl<'de> serde::de::Visitor<'de> for ChoiceVisitor {
        type Value = String;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("a choice string, option text, or integer")
        }
        fn visit_str<E: serde::de::Error>(self, v: &str) -> std::result::Result<String, E> {
            Ok(v.to_string())
        }
        fn visit_string<E: serde::de::Error>(self, v: String) -> std::result::Result<String, E> {
            Ok(v)
        }
        fn visit_u64<E: serde::de::Error>(self, v: u64) -> std::result::Result<String, E> {
            Ok(v.to_string())
        }
        fn visit_i64<E: serde::de::Error>(self, v: i64) -> std::result::Result<String, E> {
            Ok(v.to_string())
        }
    }
    deserializer.deserialize_any(ChoiceVisitor)
}

/// Rank an answers-map key ("q1", "q10", ...) by its trailing number so that
/// q2 sorts before q10. Keys without a numeric suffix sort last.
fn answer_key_rank(k: &str) -> (u8, u64) {
    let tail: String = k
        .chars()
        .rev()
        .take_while(|c| c.is_ascii_digit())
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    if tail.is_empty() {
        (1, 0)
    } else {
        match tail.parse::<u64>() {
            Ok(n) => (0, n),
            Err(_) => (1, 0),
        }
    }
}

impl DeciderDecision {
    /// Validate choice and probabilities keys against `question` (if provided).
    pub fn validate(&self, question: Option<&DeciderQuestion>) -> Result<()> {
        if let Some(q) = question {
            let n = q.options.len();
            // Choice must be "1".."N" or exactly match one of the option texts (lenient).
            let is_numeric_id = self.choice.parse::<usize>().map(|v| v >= 1 && v <= n).unwrap_or(false);
            let is_option_text = q.options.iter().any(|o| o == &self.choice);
            if !is_numeric_id && !is_option_text {
                bail!(
                    "invalid choice '{}' for question '{}' (options: {}, expected ID 1..{n} or exact option text)",
                    self.choice,
                    q.question,
                    n
                );
            }
            if let Some(probs) = &self.probabilities {
                for k in probs.keys() {
                    // Allow both numeric IDs and option texts as keys (preserve).
                    let valid_id = k.parse::<usize>().map(|v| v >= 1 && v <= n).unwrap_or(false);
                    let valid_text = q.options.iter().any(|o| o == k);
                    if !valid_id && !valid_text {
                        bail!(
                            "probabilities key '{}' invalid for question '{}' (expected 1..{n} or option text)",
                            k,
                            q.question
                        );
                    }
                }
                // Probabilities should be finite.
                for (k, v) in probs {
                    if !v.is_finite() {
                        bail!("probabilities[{}] is non-finite: {v}", k);
                    }
                }
            }
        } else {
            // Without question context, at least ensure choice is plausible.
            if self.choice.trim().is_empty() {
                bail!("decision choice is empty");
            }
            // If probabilities present, ensure values finite.
            if let Some(probs) = &self.probabilities {
                for (k, v) in probs {
                    if k.trim().is_empty() {
                        bail!("probabilities has empty key");
                    }
                    if !v.is_finite() {
                        bail!("probabilities[{}] is non-finite: {v}", k);
                    }
                }
            }
        }
        if let Some(c) = self.confidence {
            if !c.is_finite() {
                bail!("confidence is non-finite: {c}");
            }
            // Confidence in [0,1] is expected but not hard-enforced; warn via error if out of range?
            // Be lenient: allow [0,1] but don't fail if slightly outside.
        }
        Ok(())
    }

    /// Return choice as 1-based index if numeric, else None (when server returned option text).
    pub fn choice_index(&self) -> Option<usize> {
        self.choice.parse::<usize>().ok()
    }
}

/// Response from the decider service. Robust to key aliases.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct DeciderResponse {
    pub decisions: Vec<DeciderDecision>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Value>,
}

// Custom Deserialize for alias-robustness.
impl<'de> Deserialize<'de> for DeciderResponse {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Raw {
            #[serde(default, alias = "results", alias = "choices", alias = "answers")]
            decisions: Option<Vec<DeciderDecision>>,
            #[serde(default)]
            model: Option<String>,
            #[serde(default, alias = "device_name", alias = "device_type")]
            device: Option<String>,
            #[serde(default, alias = "latency", alias = "took_ms", alias = "elapsed_ms", alias = "duration_ms")]
            latency_ms: Option<u64>,
            #[serde(default)]
            latency_val: Option<Value>,
            #[serde(default)]
            latency_str: Option<String>,
            #[serde(default)]
            usage: Option<Value>,
            // Some servers wrap decisions under `data` or `output`.
            #[serde(default)]
            data: Option<Value>,
        }

        // We do a two-pass: first deserialize as Value to capture flexible latency shapes,
        // then as Raw for typed fields. Simpler: just deserialize as Value then manually extract.
        let v = Value::deserialize(deserializer)?;
        // Try typed parse for decisions/model/device/usage via Raw-like extraction from Value.
        let raw_decisions = v.get("decisions")
            .or_else(|| v.get("results"))
            .or_else(|| v.get("choices"))
            .or_else(|| v.get("answers"))
            .cloned();
        let decisions = if let Some(d) = raw_decisions {
            if let Some(arr) = d.as_array() {
                serde_json::from_value::<Vec<DeciderDecision>>(Value::Array(arr.clone())).unwrap_or_default()
            } else if let Some(map) = d.as_object() {
                // decider-serve shape: answers is a MAP {"q1": {...}, "q2": {...}}.
                // Order decisions by numeric key suffix so q1 < q2 < ... < q10.
                let mut keys: Vec<&String> = map.keys().collect();
                keys.sort_by(|a, b| {
                    answer_key_rank(a)
                        .cmp(&answer_key_rank(b))
                        .then_with(|| a.cmp(b))
                });
                keys.into_iter()
                    .filter_map(|k| {
                        map.get(k.as_str())
                            .and_then(|v| serde_json::from_value::<DeciderDecision>(v.clone()).ok())
                    })
                    .collect()
            } else {
                vec![]
            }
        } else if let Some(data) = v.get("data") {
            if let Some(arr) = data.get("decisions").or_else(|| data.get("results")).and_then(|x| x.as_array()) {
                serde_json::from_value::<Vec<DeciderDecision>>(Value::Array(arr.clone())).unwrap_or_default()
            } else { vec![] }
        } else { vec![] };

        let model = v.get("model").and_then(|x| x.as_str()).map(|s| s.to_string());
        let device = v.get("device").or_else(|| v.get("device_name")).or_else(|| v.get("device_type")).and_then(|x| x.as_str()).map(|s| s.to_string());
        let usage = v.get("usage").cloned();
        let latency_ms = v.get("latency_ms")
            .or_else(|| v.get("latency"))
            .or_else(|| v.get("took_ms"))
            .or_else(|| v.get("elapsed_ms"))
            .or_else(|| v.get("duration_ms"))
            .and_then(|x| {
                if let Some(n) = x.as_u64() { Some(n) }
                else if let Some(n) = x.as_i64() { Some(n.max(0) as u64) }
                else if let Some(f) = x.as_f64() { Some(f.max(0.0).round() as u64) }
                else if let Some(s) = x.as_str() { s.parse().ok() }
                else { None }
            });

        Ok(Self {
            decisions,
            model,
            device,
            latency_ms,
            usage,
        })
    }
}

impl DeciderResponse {
    /// Normalize decider-serve's text-shaped decisions to the internal numeric-ID
    /// contract: an exact option-text `choice` becomes its 1-based index, and
    /// option-text `probabilities` keys become numeric IDs. Choices that are
    /// already valid numeric IDs (or match nothing) are left untouched — the
    /// latter still fail `validate`, preserving the invalid-choice error path.
    pub fn normalize_choices(&mut self, questions: &[DeciderQuestion]) {
        for (dec, q) in self.decisions.iter_mut().zip(questions.iter()) {
            let n = q.options.len();
            let already_numeric = dec
                .choice
                .parse::<usize>()
                .map(|v| v >= 1 && v <= n)
                .unwrap_or(false);
            if !already_numeric {
                if let Some(pos) = q.options.iter().position(|o| o == &dec.choice) {
                    dec.choice = (pos + 1).to_string();
                }
            }
            if let Some(probs) = dec.probabilities.take() {
                let mut out = HashMap::with_capacity(probs.len());
                for (k, v) in probs {
                    if let Some(pos) = q.options.iter().position(|o| o == &k) {
                        out.insert((pos + 1).to_string(), v);
                    } else {
                        out.insert(k, v);
                    }
                }
                dec.probabilities = Some(out);
            }
        }
    }

    /// Validate all decisions against the corresponding questions (if provided).
    pub fn validate(&self, questions: Option<&[DeciderQuestion]>) -> Result<()> {
        if let Some(qs) = questions {
            if self.decisions.len() != qs.len() {
                bail!(
                    "response decisions count {} != questions count {}",
                    self.decisions.len(),
                    qs.len()
                );
            }
            for (dec, q) in self.decisions.iter().zip(qs.iter()) {
                dec.validate(Some(q))?;
            }
        } else {
            for dec in &self.decisions {
                dec.validate(None)?;
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Wire helpers: serialize u8 IDs as "1".."255" etc. (not needed as separate type
// but documented here for the invariant).
// ---------------------------------------------------------------------------

pub fn is_valid_option_id(s: &str, n: usize) -> bool {
    match s.parse::<usize>() {
        Ok(v) => v >= 1 && v <= n && n <= 255,
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alias_context_and_state() {
        let json_state = serde_json::json!({
            "state": "hello",
            "questions": [{"question":"q1","options":["A","B"]}]
        });
        let req: DeciderRequest = serde_json::from_value(json_state).unwrap();
        assert_eq!(req.context, "hello");

        let json_ctx = serde_json::json!({
            "context": "world",
            "questions": [{"question":"q1","options":["A","B"]}]
        });
        let req2: DeciderRequest = serde_json::from_value(json_ctx).unwrap();
        assert_eq!(req2.context, "world");

        // serialize always uses `context`
        let v = serde_json::to_value(&req2).unwrap();
        assert!(v.get("context").is_some());
        assert!(v.get("state").is_none());
    }

    #[test]
    fn question_validation() {
        let q = DeciderQuestion::new("pick", vec!["A".into(), "B".into()]);
        assert!(q.validate().is_ok());
        let q_empty = DeciderQuestion::new("pick", vec![]);
        assert!(q_empty.validate().is_err());
        let many: Vec<String> = (0..256).map(|i| format!("opt{i}")).collect();
        let q_many = DeciderQuestion::new("pick", many);
        assert!(q_many.validate().is_err());
    }

    #[test]
    fn decision_validation_numeric_ids() {
        let q = DeciderQuestion::new("q", vec!["A".into(), "B".into(), "C".into()]);
        let d_ok = DeciderDecision { choice: "2".into(), confidence: Some(0.9), probabilities: Some([("1".into(), 0.1), ("2".into(), 0.9)].into()) };
        assert!(d_ok.validate(Some(&q)).is_ok());
        let d_bad = DeciderDecision { choice: "5".into(), confidence: None, probabilities: None };
        assert!(d_bad.validate(Some(&q)).is_err());
        // option text is also accepted leniently
        let d_text = DeciderDecision { choice: "B".into(), confidence: None, probabilities: None };
        assert!(d_text.validate(Some(&q)).is_ok());
    }

    #[test]
    fn response_alias_latency() {
        let v = serde_json::json!({
            "decisions": [{"choice":"1","confidence":0.8,"probabilities":{"1":0.8,"2":0.2}}],
            "latency": 123,
            "model": "m1",
            "device": "cuda"
        });
        let r: DeciderResponse = serde_json::from_value(v).unwrap();
        assert_eq!(r.latency_ms, Some(123));
        assert_eq!(r.model.as_deref(), Some("m1"));
    }

    #[test]
    fn image_data_uri() {
        use base64::Engine;
        let raw = base64::engine::general_purpose::STANDARD.encode(b"hello");
        let uri = format!("data:image/png;base64,{}", raw);
        assert!(validate_image_field(&uri).is_ok());
        assert_eq!(strip_data_uri(&uri), raw);
        assert!(validate_image_field("not-base64!!!").is_err());
    }

    #[test]
    fn preserve_probabilities() {
        let v = serde_json::json!({
            "decisions": [{"choice":"1","probabilities":{"1":0.7,"2":0.3},"confidence":0.7}],
        });
        let r: DeciderResponse = serde_json::from_value(v).unwrap();
        assert_eq!(r.decisions[0].probabilities.as_ref().unwrap().get("1"), Some(&0.7));
    }

    #[test]
    fn live_serve_shape_map_answers_text_choice() {
        // Exact wire shape of decider-serve (Mapika/decider-2b-vision, /predict):
        // answers is a MAP, choice is option TEXT, probabilities keyed by text,
        // latency_ms is a float.
        let v = serde_json::json!({
            "answers": {"q1": {"type": "choice", "choice": "banana", "confidence": 0.5312,
                               "probabilities": {"apple": 0.4688, "banana": 0.5312}}},
            "usage": {"input_tokens": 26, "output_tokens": 0},
            "device": "cuda",
            "model": "Mapika/decider-2b-vision",
            "latency_ms": 257.0
        });
        let mut r: DeciderResponse = serde_json::from_value(v).unwrap();
        assert_eq!(r.decisions.len(), 1);
        assert_eq!(r.latency_ms, Some(257));
        assert_eq!(r.model.as_deref(), Some("Mapika/decider-2b-vision"));
        let qs = vec![DeciderQuestion::new("pick", vec!["apple".into(), "banana".into()])];
        r.normalize_choices(&qs);
        assert_eq!(r.decisions[0].choice, "2");
        let probs = r.decisions[0].probabilities.as_ref().unwrap();
        assert_eq!(probs.get("2"), Some(&0.5312));
        assert_eq!(probs.get("1"), Some(&0.4688));
        assert!(r.validate(Some(&qs)).is_ok());
    }

    #[test]
    fn answers_map_ordering_q10_after_q2() {
        let v = serde_json::json!({
            "answers": {
                "q10": {"choice": "J", "confidence": 0.1},
                "q2": {"choice": "B", "confidence": 0.2},
                "q1": {"choice": "A", "confidence": 0.3}
            }
        });
        let r: DeciderResponse = serde_json::from_value(v).unwrap();
        let got: Vec<&str> = r.decisions.iter().map(|d| d.choice.as_str()).collect();
        assert_eq!(got, vec!["A", "B", "J"]);
    }

    #[test]
    fn numeric_choice_and_float_latency() {
        let v = serde_json::json!({
            "decisions": [{"choice": 2, "confidence": 0.9}],
            "latency_ms": 12.7
        });
        let r: DeciderResponse = serde_json::from_value(v).unwrap();
        assert_eq!(r.decisions[0].choice, "2");
        assert_eq!(r.latency_ms, Some(13));
    }

    #[test]
    fn normalize_leaves_unknown_choice_for_validation() {
        let v = serde_json::json!({
            "answers": {"q1": {"choice": "zzz-unknown", "confidence": 0.5}}
        });
        let mut r: DeciderResponse = serde_json::from_value(v).unwrap();
        let qs = vec![DeciderQuestion::new("pick", vec!["A".into(), "B".into()])];
        r.normalize_choices(&qs);
        assert_eq!(r.decisions[0].choice, "zzz-unknown");
        assert!(r.validate(Some(&qs)).is_err());
    }
}
