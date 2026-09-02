//! LLM client — port of packages/extension/llm/* and packages/protocol ModelConfig
//! Supports OpenAI, Anthropic, Google, Groq, Azure, Cerebras via OpenAI-compatible + native
//! Falls back to STAGEHAND_MODEL env like "openai/gpt-4o-mini" or "anthropic/claude-3-5-sonnet..."

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use crate::stagehand::prompt::ChatMessage;

#[derive(Debug, Clone)]
pub struct LlmConfig {
    pub model: String,       // "openai/gpt-4o-mini" etc — provider prefix optional
    pub api_key: String,
    pub base_url: Option<String>,
    pub temperature: Option<f32>,
}

impl LlmConfig {
    pub fn from_parts(model: &str, api_key: &str) -> Self {
        Self { model: model.to_string(), api_key: api_key.to_string(), base_url: None, temperature: Some(0.0) }
    }
}

pub fn provider_and_model(model: &str) -> (String, String) {
    if model.contains('/') { let mut p = model.splitn(2, '/'); (p.next().unwrap().to_string(), p.next().unwrap().to_string()) }
    else { ("openai".to_string(), model.to_string()) }
}

fn openai_base(provider: &str, override_url: Option<&str>) -> String {
    if let Some(u) = override_url { return u.trim_end_matches('/').to_string(); }
    match provider {
        "anthropic" => "https://api.anthropic.com/v1".into(),
        "google" => "https://generativelanguage.googleapis.com/v1beta".into(),
        "groq" => "https://api.groq.com/openai/v1".into(),
        "cerebras" => "https://api.cerebras.ai/v1".into(),
        "azure" => std::env::var("AZURE_OPENAI_ENDPOINT").unwrap_or_else(|_| "https://api.openai.com/v1".into()),
        _ => "https://api.openai.com/v1".into(),
    }
}

fn normalize_messages(msgs: &[ChatMessage]) -> Value {
    let arr: Vec<Value> = msgs.iter().map(|m| {
        let content = &m.content;
        // Extract string from ChatMessage content
        let c = if content.is_string() { content.clone() }
        else if content.get("type").is_some() { content.clone() }
        else { json!({"type":"text","text": content.as_str().unwrap_or("")}) };
        json!({"role": m.role, "content": c})
    }).collect();
    Value::Array(arr)
}

/// Core generate — mirrors llm/aisdk.ts callModel
pub fn generate(messages: Vec<ChatMessage>, cfg: &LlmConfig, json_mode: bool) -> Result<Value> {
    let (provider, model) = provider_and_model(&cfg.model);
    let base = openai_base(&provider, cfg.base_url.as_deref());
    // Use blocking reqwest via cdp rt to avoid tokio mix — reuse reqwest sync
    let rt = get_rt();
    rt.block_on(async_generate(messages, &provider, &model, &base, cfg, json_mode))
}

async fn async_generate(messages: Vec<ChatMessage>, provider: &str, model: &str, base: &str, cfg: &LlmConfig, json_mode: bool) -> Result<Value> {
    let client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(45)).no_proxy().build()?;
    match provider {
        "anthropic" => anthropic_generate(&client, base, model, cfg, messages, json_mode).await,
        "google" => google_generate(&client, base, model, cfg, messages, json_mode).await,
        _ => openai_compatible_generate(&client, base, model, cfg, messages, json_mode).await,
    }
}

async fn openai_compatible_generate(client: &reqwest::Client, base: &str, model: &str, cfg: &LlmConfig, messages: Vec<ChatMessage>, json_mode: bool) -> Result<Value> {
    let url = format!("{}/chat/completions", base.trim_end_matches('/'));
    let mut body = json!({
        "model": model,
        "messages": normalize_messages(&messages),
        "temperature": cfg.temperature.unwrap_or(0.0),
    });
    if json_mode { body["response_format"] = json!({"type":"json_object"}); }
    let mut req = client.post(&url).json(&body);
    if !cfg.api_key.is_empty() { req = req.bearer_auth(&cfg.api_key); }
    // Azure uses api-key header
    if base.contains("azure") { req = req.header("api-key", &cfg.api_key); }
    let resp = req.send().await.context("LLM request failed")?;
    let status = resp.status();
    let v: Value = resp.json().await.context("parse LLM json")?;
    if !status.is_success() { bail!("LLM error {}: {}", status, v); }
    // OpenAI shape: choices[0].message.content
    let content = v.get("choices").and_then(|c| c.get(0))
        .and_then(|c| c.get("message")).and_then(|m| m.get("content"))
        .cloned().unwrap_or(Value::Null);
    // Try to parse JSON if json_mode
    if json_mode {
        let text = content.as_str().unwrap_or("");
        // content may be string that is JSON
        if let Ok(parsed) = serde_json::from_str::<Value>(text) { return Ok(parsed); }
        // already value
        if content.is_object() || content.is_array() { return Ok(content); }
    }
    // For act/observe they expect JSON object in content
    if let Some(s) = content.as_str() {
        if let Ok(j) = serde_json::from_str::<Value>(s) { return Ok(j); }
        // Heuristic: extract ```json block
        if let Some(start) = s.find('{') { if let Some(end) = s.rfind('}') { if let Ok(j) = serde_json::from_str::<Value>(&s[start..=end]) { return Ok(j); } } }
        return Ok(json!({"content": s}));
    }
    Ok(content)
}

async fn anthropic_generate(client: &reqwest::Client, base: &str, model: &str, cfg: &LlmConfig, messages: Vec<ChatMessage>, json_mode: bool) -> Result<Value> {
    let url = format!("{}/messages", base.trim_end_matches('/'));
    // Anthropic splits system
    let system = messages.iter().find(|m| m.role=="system").map(|m| m.content.as_str().unwrap_or("").to_string()).unwrap_or_default();
    let user_msgs: Vec<Value> = messages.iter().filter(|m| m.role!="system").map(|m| {
        let text = m.content.as_str().unwrap_or("").to_string();
        json!({"role": m.role, "content": [{"type":"text","text": text}]})
    }).collect();
    let mut body = json!({"model": model, "max_tokens": 4096, "messages": user_msgs});
    if !system.is_empty() { body["system"] = json!(system); }
    // anthropic supports no json_mode natively — prompt does it
    let resp = client.post(&url)
        .header("x-api-key", &cfg.api_key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .json(&body)
        .send().await.context("Anthropic request")?;
    let status = resp.status();
    let v: Value = resp.json().await?;
    if !status.is_success() { bail!("Anthropic error {}: {}", status, v); }
    let text = v.get("content").and_then(|c| c.get(0)).and_then(|x| x.get("text")).and_then(|t| t.as_str()).unwrap_or("");
    if let Ok(j) = serde_json::from_str::<Value>(text) { Ok(j) } else { Ok(json!({"content": text})) }
}

async fn google_generate(client: &reqwest::Client, base: &str, model: &str, cfg: &LlmConfig, messages: Vec<ChatMessage>, _json_mode: bool) -> Result<Value> {
    // Google Generative Language: uses generateContent
    let url = format!("{}/models/{}:generateContent?key={}", base.trim_end_matches('/'), model, cfg.api_key);
    let contents: Vec<Value> = messages.iter().map(|m| {
        let role = if m.role=="assistant" { "model" } else { "user" };
        let text = m.content.as_str().unwrap_or("").to_string();
        json!({"role": role, "parts": [{"text": text}]})
    }).collect();
    let body = json!({"contents": contents, "generationConfig": {"temperature": cfg.temperature.unwrap_or(0.0)}});
    let resp = client.post(&url).json(&body).send().await.context("Google LLM")?;
    let status = resp.status();
    let v: Value = resp.json().await?;
    if !status.is_success() { bail!("Google error {}: {}", status, v); }
    let text = v.get("candidates").and_then(|c| c.get(0))
        .and_then(|c| c.get("content")).and_then(|c| c.get("parts")).and_then(|p| p.get(0))
        .and_then(|p| p.get("text")).and_then(|t| t.as_str()).unwrap_or("");
    if let Ok(j) = serde_json::from_str::<Value>(text) { return Ok(j); }
    if let Some(s) = text.find('{').and_then(|st| text.rfind('}').map(|en| &text[st..=en])) {
        if let Ok(j) = serde_json::from_str::<Value>(s) { return Ok(j); }
    }
    Ok(json!({"content": text}))
}

// Runtime helper — reuse hyprfast cdp rt trick without circular dep
static RT: std::sync::OnceLock<tokio::runtime::Runtime> = std::sync::OnceLock::new();
fn get_rt() -> &'static tokio::runtime::Runtime {
    RT.get_or_init(|| tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("rt"))
}

/// Parse LLM act response into Stagehand Action shape
pub fn parse_act_response(v: &Value) -> Option<Value> {
    // Stagehand expects {element: {elementId, description, method, arguments?}, ...} or {action: ...}
    if v.get("element").is_some() || v.get("action").is_some() { return Some(v.clone()); }
    // sometimes LLM returns {elementId, method} flat
    if v.get("elementId").is_some() { return Some(json!({"action": v})); }
    None
}
