//! observe — port of packages/extension/services? / observeHandler
//! Returns Action[] matching instruction using LLM
//! Tier 2 (Milestone 3): if LLM yields no elements, fallback to hint_snapshot
//! Tier 3: vision not meaningful for observe (returns hint elements instead)
//! Vimium-primary: hint_snapshot is primary enumeration source when AX tree trimmed/small; keep AX first, hint fallback preserves coverage.

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
    let mut elements = Value::Array(all_elements.clone());
    let mut tier = "a11y".to_string();
    let mut hint_used = false;

    // Tier 2: if a11y/LLM produced no elements, fall back to hint overlay (DOM scan)
    // Vimium-primary enumeration: hint_snapshot is preferred when AX tree trimmed/small (needs_hint covers that); vision last resort kept inside hint_act for act path.
    let needs_hint = match elements.as_array() {
        Some(arr) => arr.is_empty(),
        None => true,
    };
    if needs_hint {
        if let Ok(hints_val) = crate::hint::hint_snapshot() {
            let hints_arr = hints_val.get("hints").and_then(|v| v.as_array()).cloned().unwrap_or_default();
            if !hints_arr.is_empty() {
                // Filter hints to instruction if not generic "find all"
                let generic = instr.to_lowercase().contains("all actionable") || instr.to_lowercase().contains("find all");
                let filtered: Vec<Value> = if generic {
                    hints_arr.clone()
                } else {
                    // Try heuristic single match, else ask LLM to rank, else return all
                    // Use hint resolver to pick relevant labels
                    let mut matching_labels: Vec<String> = Vec::new();
                    // heuristic: gather candidates containing target substring
                    let target_low = instr.to_lowercase();
                    let tokens: Vec<String> = target_low.split_whitespace()
                        .filter(|w| !["find","all","the","a","an","please","click","type","press"].contains(w))
                        .map(|s| s.trim_matches(|c: char| !c.is_alphanumeric()).to_string())
                        .filter(|s| !s.is_empty())
                        .collect();
                    for h in &hints_arr {
                        let label = h.get("label").and_then(|v| v.as_str()).unwrap_or("");
                        let name = h.get("name").and_then(|v| v.as_str()).unwrap_or("").to_lowercase();
                        let text = h.get("text").and_then(|v| v.as_str()).unwrap_or("").to_lowercase();
                        let role = h.get("role").and_then(|v| v.as_str()).unwrap_or("").to_lowercase();
                        if tokens.iter().any(|tok| name.contains(tok) || text.contains(tok) || role.contains(tok)) {
                            matching_labels.push(label.to_string());
                        }
                    }
                    if !matching_labels.is_empty() && matching_labels.len() <= 5 {
                        hints_arr.into_iter().filter(|h| {
                            h.get("label").and_then(|v| v.as_str()).map(|l| matching_labels.contains(&l.to_string())).unwrap_or(false)
                        }).collect()
                    } else if let Ok(Some(label)) = crate::hint::llm_hint_match(instr, &hints_val, cfg) {
                        hints_arr.into_iter().filter(|h| h.get("label").and_then(|v| v.as_str()) == Some(label.as_str())).collect()
                    } else {
                        // fallback: return all if no LLM filtering succeeded and not generic
                        // To avoid returning 60 elements for a specific query, return all if filtering found none — caller can see hint distribution
                        hints_arr.into_iter().take(20).collect()
                    }
                };
                // Convert hint entries to observe-style elements
                let hint_elements: Vec<Value> = filtered.into_iter().map(|h| {
                    let label = h.get("label").and_then(|v| v.as_str()).unwrap_or("");
                    let tag = h.get("tag").and_then(|v| v.as_str()).unwrap_or("");
                    let role = h.get("role").and_then(|v| v.as_str()).unwrap_or("");
                    let name = h.get("name").and_then(|v| v.as_str()).unwrap_or("");
                    let selector = h.get("selector").and_then(|v| v.as_str()).unwrap_or("");
                    let method = if ["input","textarea","select"].contains(&tag) { "type" } else { "click" };
                    json!({
                        "elementId": format!("hint-{}", label),
                        "description": name,
                        "method": method,
                        "arguments": [],
                        "selector": selector,
                        "role": role,
                        "tag": tag,
                        "label": label,
                        "via": "hint",
                        "hint": h
                    })
                }).collect();
                if !hint_elements.is_empty() {
                    elements = Value::Array(hint_elements);
                    tier = "hint".to_string();
                    hint_used = true;
                    crate::stagehand::instrumentation::METRICS.record_tier("hint");
                }
            }
        }
    }
    if tier == "a11y" && !needs_hint {
        // we had a11y elements — record tier
        // Only count successful a11y observe when we didn't fallback
        crate::stagehand::instrumentation::METRICS.record_tier("a11y");
    }

    // Normalize to Action shape and enrich with xpath if available
    let xpath_map = &snap.combined_xpath_map;
    let enriched = if let Some(arr) = elements.as_array() {
        let mut out = Vec::new();
        for el in arr {
            let mut e = el.clone();
            // already hint elements have selector/label — keep them
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
        "via": if hint_used { Value::String("hint".into()) } else { Value::String(snap.via.clone()) },
        "tier": tier,
        "hintUsed": hint_used,
        "raw": resp
    }))
}
