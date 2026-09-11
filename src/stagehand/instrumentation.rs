//! Port of packages/extension/tracing.ts + metrics.ts + instrumentedDecorator
use std::sync::atomic::{AtomicU64, Ordering};
use serde_json::Value;

#[derive(Debug, Default)] pub struct Metrics {
    pub act_prompt_tokens: AtomicU64, pub act_completion_tokens: AtomicU64,
    pub extract_prompt_tokens: AtomicU64, pub extract_completion_tokens: AtomicU64,
    pub observe_prompt_tokens: AtomicU64, pub observe_completion_tokens: AtomicU64,
    pub tier_a11y: AtomicU64, pub tier_hint: AtomicU64, pub tier_vision: AtomicU64,
}
impl Metrics {
    pub fn snapshot(&self) -> Value { serde_json::json!({
        "actPromptTokens": self.act_prompt_tokens.load(Ordering::Relaxed),
        "actCompletionTokens": self.act_completion_tokens.load(Ordering::Relaxed),
        "extractPromptTokens": self.extract_prompt_tokens.load(Ordering::Relaxed),
        "extractCompletionTokens": self.extract_completion_tokens.load(Ordering::Relaxed),
        "observePromptTokens": self.observe_prompt_tokens.load(Ordering::Relaxed),
        "observeCompletionTokens": self.observe_completion_tokens.load(Ordering::Relaxed),
        "tierA11y": self.tier_a11y.load(Ordering::Relaxed),
        "tierHint": self.tier_hint.load(Ordering::Relaxed),
        "tierVision": self.tier_vision.load(Ordering::Relaxed),
        "tier": {
            "a11y": self.tier_a11y.load(Ordering::Relaxed),
            "hint": self.tier_hint.load(Ordering::Relaxed),
            "vision": self.tier_vision.load(Ordering::Relaxed)
        },
    })}
    pub fn add(&self, kind: &str, usage: &Value) {
        let in_tok = usage.get("inputTokens").and_then(|v| v.as_u64()).unwrap_or(0);
        let out_tok = usage.get("outputTokens").and_then(|v| v.as_u64()).unwrap_or(0);
        match kind {
            "act" => { self.act_prompt_tokens.fetch_add(in_tok, Ordering::Relaxed); self.act_completion_tokens.fetch_add(out_tok, Ordering::Relaxed); },
            "extract" => { self.extract_prompt_tokens.fetch_add(in_tok, Ordering::Relaxed); self.extract_completion_tokens.fetch_add(out_tok, Ordering::Relaxed); },
            "observe" => { self.observe_prompt_tokens.fetch_add(in_tok, Ordering::Relaxed); self.observe_completion_tokens.fetch_add(out_tok, Ordering::Relaxed); },
            _ => {}
        }
    }
    pub fn record_tier(&self, tier: &str) {
        match tier {
            "a11y" => { self.tier_a11y.fetch_add(1, Ordering::Relaxed); },
            "hint" => { self.tier_hint.fetch_add(1, Ordering::Relaxed); },
            "vision" => { self.tier_vision.fetch_add(1, Ordering::Relaxed); },
            _ => {}
        }
    }
    pub fn clear_tiers(&self) {
        self.tier_a11y.store(0, Ordering::Relaxed);
        self.tier_hint.store(0, Ordering::Relaxed);
        self.tier_vision.store(0, Ordering::Relaxed);
    }
}
pub static METRICS: Metrics = Metrics{ act_prompt_tokens: AtomicU64::new(0), act_completion_tokens: AtomicU64::new(0), extract_prompt_tokens: AtomicU64::new(0), extract_completion_tokens: AtomicU64::new(0), observe_prompt_tokens: AtomicU64::new(0), observe_completion_tokens: AtomicU64::new(0), tier_a11y: AtomicU64::new(0), tier_hint: AtomicU64::new(0), tier_vision: AtomicU64::new(0) };
pub fn log(level: &str, msg: &str, data: Value) { eprintln!("[stagehand] {} {} {}", level.to_uppercase(), msg, data); }
