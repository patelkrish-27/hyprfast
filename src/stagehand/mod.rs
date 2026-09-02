//! Stagehand port — Rust implementation of browserbase/stagehand
//! Mirrors packages/extension/prompt.ts + services/actService + observe + extract
//! Hybrid AX trimming, LLM-driven act/observe/extract, self-healing, cache, batch
//! See /home/krish/Projects/stagehand for source of truth (JS) → this is Rust port

pub mod prompt;
pub mod llm;
pub mod snapshot;
pub mod protocol;
pub mod act;
pub mod observe;
pub mod extract;
pub mod cache;
pub mod agent;
pub mod a11y;
pub mod frame;
pub mod deep_locator;
pub mod page;
pub mod context;
pub mod locator;
pub mod batch;
pub mod webmcp;
pub mod instrumentation;
pub mod cookies;
pub mod clipboard;
pub mod file_upload;

use anyhow::Result;
use serde_json::{Value, json};
fn dirs_fallback() -> std::path::PathBuf {
    if let Ok(home) = std::env::var("HOME") { std::path::PathBuf::from(home).join(".config/hyprfast/stagehand.env") } else { std::path::PathBuf::from("/tmp/stagehand.env") }
}

pub use act::act;
pub use observe::observe;
pub use extract::extract;
pub use agent::execute as agent_execute;

/// Unified stagehand config (mirrors StagehandCreateOptions)
#[derive(Debug, Clone)]
pub struct StagehandConfig {
    pub model_name: String, // e.g. "openai/gpt-4o-mini" or "anthropic/claude-3.5-sonnet"
    pub api_key: String,
    pub base_url: Option<String>,
    pub cache_enabled: bool,
    pub self_heal: bool,
    pub system_prompt: Option<String>,
}

impl StagehandConfig {
    pub fn from_env() -> Self {
        // Load optional local env file ~/.config/hyprfast/stagehand.env (not committed) — allows Gemini key without shell export
        let _ = Self::load_dotenv();
        let model_default = if std::env::var("GOOGLE_API_KEY").is_ok() || std::env::var("GEMINI_API_KEY").is_ok() || std::env::var("GOOGLE_GENERATIVE_AI_API_KEY").is_ok() {
            "google/gemini-2.5-flash"
        } else { "openai/gpt-4o-mini" };
        let model = std::env::var("STAGEHAND_MODEL").unwrap_or_else(|_| model_default.into());
        let key = std::env::var("OPENAI_API_KEY")
            .or_else(|_| std::env::var("ANTHROPIC_API_KEY"))
            .or_else(|_| std::env::var("GOOGLE_API_KEY"))
            .or_else(|_| std::env::var("GEMINI_API_KEY"))
            .or_else(|_| std::env::var("GOOGLE_GENERATIVE_AI_API_KEY"))
            .unwrap_or_default();
        Self {
            model_name: model,
            api_key: key,
            base_url: std::env::var("STAGEHAND_BASE_URL").ok(),
            cache_enabled: true,
            self_heal: true,
            system_prompt: std::env::var("STAGEHAND_SYSTEM_PROMPT").ok(),
        }
    }
    fn load_dotenv() -> Result<(), ()> {
        for path in [dirs_fallback(), std::path::PathBuf::from("/tmp/stagehand.env")] {
            if path.exists() {
                if let Ok(content) = std::fs::read_to_string(&path) {
                    for line in content.lines() {
                        let line = line.trim();
                        if line.is_empty() || line.starts_with('#') { continue; }
                        if let Some((k,v)) = line.split_once('=') {
                            let k = k.trim(); let v = v.trim().trim_matches('"').trim_matches('\'');
                            if std::env::var(k).is_err() { std::env::set_var(k, v); }
                        }
                    }
                }
            }
        }
        Ok(())
    }
    pub fn with_model(mut self, model: &str, api_key: &str) -> Self {
        self.model_name = model.to_string();
        self.api_key = api_key.to_string();
        self
    }
}

/// Helper to capture hybrid snapshot via snapshot::capture_hybrid
pub fn capture_snapshot() -> Result<snapshot::HybridSnapshot> {
    snapshot::capture_hybrid()
}

/// One-shot act: instruction → LLM → CDP click/type/etc
pub fn act_instruction(instruction: &str, cfg: &StagehandConfig) -> Result<Value> {
    act::act(instruction, cfg)
}
pub fn observe_instruction(instruction: Option<&str>, cfg: &StagehandConfig) -> Result<Value> {
    observe::observe(instruction, cfg)
}
pub fn extract_instruction(instruction: &str, schema: Option<&Value>, cfg: &StagehandConfig) -> Result<Value> {
    extract::extract(instruction, schema, cfg)
}
