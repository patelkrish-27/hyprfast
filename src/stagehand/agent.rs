//! agent — port of Stagehand agent / operator loop
//! CUA + generic agent loop using act/extract iteratively until goal complete

use anyhow::Result;
use serde_json::{Value, json};
use crate::stagehand::{prompt, llm, StagehandConfig, snapshot};

pub fn execute(goal: &str, cfg: &StagehandConfig, max_steps: usize) -> Result<Value> {
    let system = prompt::build_operator_system_prompt(goal);
    let llm_cfg = llm::LlmConfig::from_parts(&cfg.model_name, &cfg.api_key);
    let mut steps: Vec<Value> = Vec::new();
    let mut done = false;

    for step in 0..max_steps {
        let snap = snapshot::capture_hybrid()?;
        let tree_preview = snap.combined_tree.chars().take(6000).collect::<String>();
        let history = steps.iter().map(|s| s.to_string()).collect::<Vec<_>>().join("\n");
        let user_content = format!("Goal: {}\nSteps so far:\n{}\nCurrent tree:\n{}\nWhat is the next action? Use act or extract or wait or close. Return JSON with {{tool, instruction}}.", goal, if history.is_empty() { "none".into() } else { history }, tree_preview);
        let msgs = vec![
            system.clone(),
            prompt::ChatMessage { role: "user".into(), content: Value::String(user_content) },
        ];
        let llm_resp = llm::generate(msgs, &llm_cfg, true)?;
        // LLM should return {tool:"act", instruction:"click ..."} or {tool:"extract"...}
        let tool = llm_resp.get("tool").and_then(|v| v.as_str()).unwrap_or("act").to_string();
        let instruction = llm_resp.get("instruction").and_then(|v| v.as_str()).unwrap_or("").to_string();

        let step_result = match tool.as_str() {
            "extract" => crate::stagehand::extract::extract(&instruction, None, cfg).unwrap_or(json!({"error": "extract failed"})),
            "act" => crate::stagehand::act::act(&instruction, cfg).unwrap_or(json!({"error": "act failed"})),
            "observe" => crate::stagehand::observe::observe(Some(&instruction), cfg).unwrap_or(json!({"error": "observe failed"})),
            "goto" | "navigate" => crate::browser::navigate(&instruction, None).unwrap_or(json!({"error": "navigate failed"})),
            "wait" => { std::thread::sleep(std::time::Duration::from_secs_f64(instruction.parse().unwrap_or(1.0))); json!({"waited": instruction}) },
            "close" => { done = true; json!({"closed": true, "reason": instruction}) },
            _ => crate::stagehand::act::act(&instruction, cfg).unwrap_or(json!({"error": "act fallback failed"})),
        };
        let entry = json!({"step": step+1, "tool": tool, "instruction": instruction, "result": step_result, "llm": llm_resp});
        steps.push(entry.clone());
        if done || tool=="close" { break; }
        // Check if LLM signaled completion via raw content containing "close"
        if llm_resp.to_string().to_lowercase().contains("\"close\"") || llm_resp.get("close").and_then(|v| v.as_bool()).unwrap_or(false) { break; }
    }

    Ok(json!({
        "goal": goal,
        "steps": steps,
        "completed": done,
        "total_steps": steps.len()
    }))
}
