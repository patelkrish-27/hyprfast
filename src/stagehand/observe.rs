//! observe — port of packages/extension/services? / observeHandler
//! Returns Action[] matching instruction using LLM

use anyhow::Result;
use serde_json::{Value, json};
use crate::stagehand::{prompt, llm, snapshot, StagehandConfig};

const SUPPORTED_ACTIONS: &[&str] = &["click","fill","type","press","selectOptionFromDropdown","scrollIntoView","hover","dragAndDrop","nextChunk","prevChunk"];

pub fn observe(instruction: Option<&str>, cfg: &StagehandConfig) -> Result<Value> {
    let instr = instruction.unwrap_or("find all actionable elements");
    // Chunking: if tree > 8000 chars, self-chunk like Stagehand observeService (splits by lines)
    let snap = snapshot::capture_hybrid()?;
    let trees: Vec<String> = if snap.combined_tree.len() > 8000 {
        // simple chunk by lines ~150 lines per chunk
        let lines: Vec<&str> = snap.combined_tree.lines().collect();
        lines.chunks(150).map(|c| c.join("\n")).collect()
    } else { vec![snap.combined_tree.clone()] };
    let supported: Vec<String> = SUPPORTED_ACTIONS.iter().map(|s| s.to_string()).collect();
    let mut all_elements = Vec::new();
    let mut raw_last = Value::Null;
    for chunk in &trees {
        let system = prompt::build_observe_system_prompt(cfg.system_prompt.as_deref(), Some(&supported), None);
        let user = prompt::build_observe_user_message(instr, chunk);
        let messages = vec![system, user];
        let llm_cfg = llm::LlmConfig::from_parts(&cfg.model_name, &cfg.api_key);
        let resp = llm::generate(messages, &llm_cfg, true)?;
        raw_last = resp.clone();
        let elements = if resp.is_array() { resp.clone() }
        else if let Some(arr) = resp.get("elements").or_else(|| resp.get("data")) { arr.clone() }
        else if resp.get("elementId").is_some() { Value::Array(vec![resp.clone()]) }
        else { resp.clone() };
        if let Some(arr) = elements.as_array() { all_elements.extend(arr.clone()); }
        crate::stagehand::instrumentation::METRICS.add("observe", &resp);
    }
    let resp = raw_last;
    let elements = Value::Array(all_elements.clone());

    // Normalize to Action shape and enrich with xpath if available
    let xpath_map = &snap.combined_xpath_map;
    let enriched = if let Some(arr) = elements.as_array() {
        let mut out = Vec::new();
        for el in arr {
            let mut e = el.clone();
            if let Some(enc) = el.get("elementId").and_then(|v| v.as_str()) {
                if let Some(xpath) = xpath_map.get(enc).cloned().or_else(|| xpath_map.get(&enc.to_string()).cloned()) {
                    e["xpath"] = xpath;
                    // Also provide selector fallback using xpath
                    e["selector"] = Value::String(format!("xpath={}", e["xpath"].as_str().unwrap_or("")));
                }
            }
            out.push(e);
        }
        Value::Array(out)
    } else { elements };

    Ok(json!({
        "data": enriched,
        "snapshot": snap.combined_tree.chars().take(2000).collect::<String>(),
        "via": snap.via,
        "raw": resp
    }))
}
