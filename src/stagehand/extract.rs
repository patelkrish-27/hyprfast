//! extract — port of packages/extension/services/extractService
//! LLM extracts structured data given instruction + DOM + optional schema (zod → JSON schema)

use anyhow::Result;
use serde_json::{Value, json};
use crate::stagehand::{prompt, llm, snapshot, StagehandConfig};

pub fn extract(instruction: &str, schema: Option<&Value>, cfg: &StagehandConfig) -> Result<Value> {
    let snap = snapshot::capture_hybrid()?;
    // Build prompts — include screenshot not yet
    let system = prompt::build_extract_system_prompt(cfg.system_prompt.as_deref(), false, false);
    // If schema provided, append to instruction like Stagehand does
    let full_instruction = if let Some(s) = schema {
        format!("{} | Schema: {} | Return ONLY valid JSON matching schema, no explanation.", instruction, serde_json::to_string(s).unwrap_or_default())
    } else {
        instruction.to_string()
    };
    let user = prompt::build_extract_user_prompt(&full_instruction, &snap.combined_tree, false, None);
    let messages = vec![system, user];
    let llm_cfg = llm::LlmConfig::from_parts(&cfg.model_name, &cfg.api_key);
    let resp = llm::generate(messages, &llm_cfg, true)?;

    crate::stagehand::instrumentation::METRICS.add("extract", &resp);
    let data = if resp.get("data").is_some() { resp.get("data").cloned().unwrap() }
    else if resp.get("structuredContent").is_some() { resp.get("structuredContent").cloned().unwrap() }
    else { resp.clone() };
    // Validate against schema if provided (lightweight: check required keys exist)
    if let Some(schema) = schema {
        if let Some(req) = schema.get("required").and_then(|v| v.as_array()) {
            for key in req {
                if let Some(k) = key.as_str() {
                    if data.get(k).is_none() && !data.is_array() {
                        eprintln!("[stagehand extract] warning: missing required key '{}' in data", k);
                    }
                }
            }
        }
    }
    Ok(json!({
        "data": data,
        "instruction": instruction,
        "snapshot_chars": snap.combined_tree.len(),
        "via": snap.via,
        "raw": resp
    }))
}
