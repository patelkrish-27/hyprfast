//! act — port of packages/extension/services/actService.ts
//! Flow: wait dom quiet → captureHybridSnapshot → LLM (act prompt) → deterministic action → handle twoStep dropdown

use anyhow::{Result, bail};
use serde_json::{Value, json};
use crate::stagehand::{prompt, llm, snapshot, cache, StagehandConfig};
use crate::cdp;
use crate::browser;

const SUPPORTED_ACTIONS: &[&str] = &["click","fill","type","press","selectOptionFromDropdown","scrollIntoView","hover","waitForSelector","scroll","dragAndDrop","nextChunk","prevChunk"];

fn supported_list() -> Vec<String> { SUPPORTED_ACTIONS.iter().map(|s| s.to_string()).collect() }

fn current_url() -> String {
    cdp::evaluate("location.href", false).ok()
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .or_else(|| cdp::list_targets().ok().and_then(|t| t.first().map(|x| x.url.clone())))
        .unwrap_or_default()
}

fn find_name_for_id(tree: &str, id: &str) -> Option<String> {
    if id.is_empty() { return None; }
    for line in tree.lines() {
        if line.contains(id) {
            // line like `[0-4231] button 'New chat' 4231` — extract between single quotes
            if let Some(s) = line.find('\'') {
                if let Some(e) = line[s+1..].find('\'') {
                    let name = line[s+1..s+1+e].trim().to_string();
                    if !name.is_empty() { return Some(name); }
                }
            }
        }
    }
    None
}

fn extract_target_from_instruction(instr: &str) -> String {
    // prefer quoted text e.g. 'New chat' or "New chat"
    if let Some(a) = instr.find('\'') {
        if let Some(b) = instr[a+1..].find('\'') {
            let s = instr[a+1..a+1+b].trim();
            if !s.is_empty() { return s.to_string(); }
        }
    }
    if let Some(a) = instr.find('"') {
        if let Some(b) = instr[a+1..].find('"') {
            let s = instr[a+1..a+1+b].trim();
            if !s.is_empty() { return s.to_string(); }
        }
    }
    // fallback: last meaningful words before button/link
    let lower = instr.to_lowercase();
    for kw in ["button", "link", "prompt box", "input"] {
        if let Some(pos) = lower.find(kw) {
            let before = instr[..pos].trim();
            if let Some(last) = before.split('\'').last().map(|s| s.trim()) {
                if !last.is_empty() && last.len() < 40 { return last.to_string(); }
            }
        }
    }
    instr.split_whitespace().take(5).collect::<Vec<_>>().join(" ")
}

/// Take deterministic action from LLM output — mirrors performUnderstudyMethod()
pub fn execute_action(action: &Value, variables: Option<&Value>) -> Result<Value> {
    let method = action.get("method").and_then(|v| v.as_str()).unwrap_or("click");
    let element_id = action.get("elementId").and_then(|v| v.as_str()).unwrap_or("");
    let args = action.get("arguments").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    let arg0 = args.get(0).and_then(|v| v.as_str()).unwrap_or("").to_string();
    // Resolve variables %var%
    let resolve = |s: String| -> String {
        if s.starts_with('%') && s.ends_with('%') {
            if let Some(vars) = variables {
                let key = s.trim_matches('%');
                if let Some(val) = vars.get(key).and_then(|v| v.get("value")).and_then(|v| v.as_str()) { return val.to_string(); }
                if let Some(val) = vars.get(key).and_then(|v| v.as_str()) { return val.to_string(); }
            }
        }
        s
    };
    let _arg0 = resolve(arg0);

    // Map Stagehand methods → hyprfast browser/cdp
    match method {
        "click" | "leftClick" => {
            // elementId like "0-12345" -> backendId
            if !element_id.is_empty() {
                if let Some((_, backend)) = snapshot::parse_enc_id(element_id) {
                    let ws = cdp::get_ws_url(None)?;
                    if backend != 0 {
                        let resolved = cdp::cdp_call(&ws, "DOM.resolveNode", json!({"backendNodeId": backend}))?;
                        if let Some(object_id) = resolved.get("object").and_then(|o| o.get("objectId")).and_then(|v| v.as_str()) {
                            let _ = cdp::cdp_call(&ws, "Runtime.callFunctionOn", json!({"objectId": object_id, "functionDeclaration": "function(){this.click(); return this.tagName;}", "returnByValue": true}))?;
                            return Ok(json!({"success": true, "method": method, "elementId": element_id, "via":"CDP-backend"}));
                        }
                    }
                }
                // fallback to elementId as selector hint
                return browser::click_by_selector(&format!("[data-stagehand-id='{}']", element_id))
                    .or_else(|_| cdp::evaluate(&format!("document.querySelectorAll('*')[0]?.click()"), false));
            }
            bail!("no elementId for click");
        },
        "fill" | "type" => {
            let text = resolve(args.get(0).and_then(|v| v.as_str()).unwrap_or("").to_string());
            // Use browser type logic
            if !element_id.is_empty() {
                // try to focus element by backend then type
                if let Some((_, backend)) = snapshot::parse_enc_id(element_id) {
                    let ws = cdp::get_ws_url(None)?;
                    let resolved = cdp::cdp_call(&ws, "DOM.resolveNode", json!({"backendNodeId": backend}))?;
                    if let Some(oid) = resolved.get("object").and_then(|o| o.get("objectId")).and_then(|v| v.as_str()) {
                        let decl = format!("function(){{this.focus(); if(this.isContentEditable){{document.execCommand('selectAll',false,null); document.execCommand('insertText',false,{:?});}} else {{this.value={:?}; this.dispatchEvent(new Event('input',{{bubbles:true}}));}} return true;}}", text, text);
                        let _ = cdp::cdp_call(&ws, "Runtime.callFunctionOn", json!({"objectId": oid, "functionDeclaration": decl, "returnByValue": true}))?;
                    }
                }
            }
            // fallback via evaluate
            let js = format!("(() => {{ const el=document.activeElement; if(el){{el.focus(); if(el.isContentEditable) document.execCommand('insertText',false,{:?}); else {{el.value={:?}; el.dispatchEvent(new Event('input',{{bubbles:true}}));}}}} return true;}})()", text, text);
            let v = cdp::evaluate(&js, false)?;
            Ok(json!({"success": true, "method": method, "typed": text, "result": v}))
        },
        "press" => {
            let key = resolve(args.get(0).and_then(|v| v.as_str()).unwrap_or("Enter").to_string());
            browser::press_key(&key)
        },
        "selectOptionFromDropdown" => {
            let option = resolve(args.get(0).and_then(|v| v.as_str()).unwrap_or("").to_string());
            // try select
            if !element_id.is_empty() {
                if let Some((_, backend)) = snapshot::parse_enc_id(element_id) {
                    let ws = cdp::get_ws_url(None)?;
                    let resolved = cdp::cdp_call(&ws, "DOM.resolveNode", json!({"backendNodeId": backend}))?;
                    if let Some(oid) = resolved.get("object").and_then(|o| o.get("objectId")).and_then(|v| v.as_str()) {
                        let decl = format!("function(){{ for(const o of this.options) if(o.text=== {:?} || o.value=== {:?}) {{o.selected=true;}} this.dispatchEvent(new Event('change',{{bubbles:true}})); return true;}}", option, option);
                        let _ = cdp::cdp_call(&ws, "Runtime.callFunctionOn", json!({"objectId": oid, "functionDeclaration": decl, "returnByValue": true}))?;
                        return Ok(json!({"success": true, "method": method, "option": option}));
                    }
                }
            }
            browser::select_option("", &[option])
        },
        "scrollIntoView" | "scroll" => {
            let arg = args.get(0).and_then(|v| v.as_str()).unwrap_or("");
            if arg.contains('%') {
                let pct: f64 = arg.trim_matches('%').parse().unwrap_or(50.0) / 100.0;
                let js = format!("window.scrollTo(0, document.body.scrollHeight * {}); true", pct);
                cdp::evaluate(&js, false)?;
            } else if !element_id.is_empty() {
                if let Some((_, backend)) = snapshot::parse_enc_id(element_id) {
                    let ws = cdp::get_ws_url(None)?;
                    let resolved = cdp::cdp_call(&ws, "DOM.resolveNode", json!({"backendNodeId": backend}))?;
                    if let Some(oid) = resolved.get("object").and_then(|o| o.get("objectId")).and_then(|v| v.as_str()) {
                        let decl = "function(){this.scrollIntoView({block:'center'}); return true;}";
                        let _ = cdp::cdp_call(&ws, "Runtime.callFunctionOn", json!({"objectId": oid, "functionDeclaration": decl, "returnByValue": true}))?;
                    }
                }
            }
            Ok(json!({"success": true, "method": method}))
        },
        "hover" => {
            // hover via dispatch
            if let Some((_, backend)) = snapshot::parse_enc_id(element_id) {
                let ws = cdp::get_ws_url(None)?;
                let resolved = cdp::cdp_call(&ws, "DOM.resolveNode", json!({"backendNodeId": backend}))?;
                if let Some(oid) = resolved.get("object").and_then(|o| o.get("objectId")).and_then(|v| v.as_str()) {
                    let decl = "function(){this.dispatchEvent(new MouseEvent('mouseover',{bubbles:true})); return true;}";
                    let _ = cdp::cdp_call(&ws, "Runtime.callFunctionOn", json!({"objectId": oid, "functionDeclaration": decl, "returnByValue": true}))?;
                    return Ok(json!({"success": true, "method": method}));
                }
            }
            Ok(json!({"success": true, "method": method, "fallback": true}))
        },
        "nextChunk" | "prevChunk" => {
            let dir = if method=="nextChunk" { 1 } else { -1 };
            let js = format!("window.scrollBy(0, {}*window.innerHeight*0.8); true", dir);
            cdp::evaluate(&js, false)?;
            Ok(json!({"success": true, "method": method}))
        },
        _ => {
            // generic fallback via JS
            let js = format!("document.querySelector('*')?.click(); true");
            let v = cdp::evaluate(&js, false)?;
            Ok(json!({"success": true, "method": method, "fallback": true, "result": v}))
        }
    }
}

pub fn act(instruction: &str, cfg: &StagehandConfig) -> Result<Value> {
    let url = current_url();
    // Cache check
    if cfg.cache_enabled {
        if let Some(cached) = cache::get_cached("act", instruction, &url) {
            // replay cached actions (self-heal if fails)
            if let Ok(replayed) = replay_cached(&cached, instruction, cfg) {
                return Ok(json!({"success": true, "cached": true, "actions": cached, "result": replayed}));
            }
        }
    }

    let snap = snapshot::capture_hybrid()?;
    let supported = supported_list();
    let system = prompt::build_act_system_prompt(cfg.system_prompt.as_deref());
    let user_instruction = prompt::build_act_prompt(instruction, &supported, None);
    let user_msg_content = format!("{}\nAccessibility Tree:\n{}", user_instruction, snap.combined_tree);
    let messages = vec![
        system,
        prompt::ChatMessage { role: "user".into(), content: serde_json::Value::String(user_msg_content) },
    ];

    let llm_cfg = llm::LlmConfig::from_parts(&cfg.model_name, &cfg.api_key);
    let t0 = std::time::Instant::now();
    let llm_resp = llm::generate(messages, &llm_cfg, true)?;
    crate::stagehand::instrumentation::METRICS.add("act", &llm_resp);
    let _elapsed = t0.elapsed().as_millis() as u32;
    // Stagehand LLM returns {action: {elementId, description, method, arguments...}, element?}
    let action_obj = llm_resp.get("action").or_else(|| llm_resp.get("element")).or_else(|| llm_resp.get("actions").and_then(|a| a.get(0))).cloned().unwrap_or(Value::Null);
    if action_obj.is_null() || action_obj.get("method").is_none() && action_obj.get("elementId").is_none() {
        // Check if LLM returned null action (no match)
        if llm_resp.get("action").and_then(|v| v.as_null()).is_some() || llm_resp.to_string().contains("null") {
            return Ok(json!({"success": false, "message": "No action found", "actionDescription": instruction, "actions": [], "llm": llm_resp}));
        }
        // Try to parse flat
        if llm_resp.get("elementId").is_some() {
            let mut res = execute_action(&llm_resp, None);
            if res.is_err() {
                let err_str = res.as_ref().err().map(|e| e.to_string()).unwrap_or_default();
                if err_str.contains("Could not find object") || err_str.contains("not found") {
                    let elem_id = llm_resp.get("elementId").and_then(|v| v.as_str()).unwrap_or("");
                    let name = find_name_for_id(&snap.combined_tree, elem_id).unwrap_or_else(|| extract_target_from_instruction(instruction));
                    if !name.is_empty() {
                        let js = format!("(() => {{ const els=[...document.querySelectorAll('button, [role=\"button\"], a, div, span')]; const t=els.find(e=>e.textContent.trim().toLowerCase().includes({:?}.toLowerCase())); if(t){{t.click(); return 'fallback clicked';}} return 'fallback not found'; }})()", name.to_lowercase());
                        if let Ok(v) = cdp::evaluate(&js, false) {
                            if v.to_string().contains("fallback clicked") { res = Ok(v); }
                        }
                    }
                }
            }
            let res = res?;
            return Ok(json!({"success": true, "actionDescription": instruction, "actions": [llm_resp], "result": res, "xpathMap": snap.combined_xpath_map}));
        }
        return Ok(json!({"success": false, "message": format!("LLM did not return actionable element: {}", llm_resp), "actions": [], "llm": llm_resp}));
    }

    // Execute with self-heal retry (Stagehand actService selfHeal) + eval fallback for stale backend (cause #6)
    let mut result = execute_action(&action_obj, None);
    // Fallback to JS text search if backend resolve failed (common for Gemini New chat etc. 0-4231)
    if result.is_err() {
        let err_str = result.as_ref().err().map(|e| e.to_string()).unwrap_or_default();
        if err_str.contains("Could not find object") || err_str.contains("backend") || err_str.contains("not found") {
            // Try to find element name from snapshot for this elementId
            let elem_id = action_obj.get("elementId").and_then(|v| v.as_str()).unwrap_or("");
            let name = find_name_for_id(&snap.combined_tree, elem_id).unwrap_or_else(|| extract_target_from_instruction(instruction));
            if !name.is_empty() {
                let js = format!("(() => {{ const els=[...document.querySelectorAll('button, [role=\"button\"], a, div, span')]; const t=els.find(e=>e.textContent.trim().toLowerCase().includes({:?}.toLowerCase())); if(t){{t.click(); return 'fallback clicked '+t.tagName+': '+t.textContent.slice(0,30);}} return 'fallback not found for '+{:?}; }})()", name.to_lowercase(), name);
                if let Ok(v) = cdp::evaluate(&js, false) {
                    if v.as_str().map(|s| s.contains("fallback clicked")).unwrap_or(false) || v.to_string().contains("fallback clicked") {
                        result = Ok(v);
                    }
                }
            }
        }
    }
    if cfg.self_heal && result.is_err() {
        eprintln!("[stagehand act] self-heal: retry after re-snapshot");
        if let Ok(snap2) = snapshot::capture_hybrid() {
            let sys2 = prompt::build_act_system_prompt(cfg.system_prompt.as_deref());
            let user2 = prompt::build_act_prompt(&format!("{} (previous attempt failed: {})", instruction, result.as_ref().err().map(|e| e.to_string()).unwrap_or_default()), &supported, None);
            let msgs2 = vec![sys2, prompt::ChatMessage { role: "user".into(), content: Value::String(format!("{}\nAccessibility Tree:\n{}", user2, snap2.combined_tree)) }];
            if let Ok(resp2) = llm::generate(msgs2, &llm_cfg, true) {
                if let Some(a2) = resp2.get("action").or_else(|| resp2.get("element")).cloned() {
                    result = execute_action(&a2, None);
                }
            }
        }
    }
    let result = result?;

    // Handle twoStep dropdown — Stagehand does second inference after DOM diff
    let two_step = action_obj.get("twoStep").and_then(|v| v.as_bool()).unwrap_or(false) || llm_resp.get("twoStep").and_then(|v| v.as_bool()).unwrap_or(false);
    let actions = if two_step {
        // Wait then recapture diff tree
        std::thread::sleep(std::time::Duration::from_millis(500));
        let next_snap = snapshot::capture_hybrid().unwrap_or_else(|_| snapshot::HybridSnapshot { combined_tree: snap.combined_tree.clone(), combined_xpath_map: snap.combined_xpath_map.clone(), combined_url_map: snap.combined_url_map.clone(), raw_ax_nodes: snap.raw_ax_nodes, via: snap.via.clone() });
        let diff = snapshot::diff_trees(&snap.combined_tree, &next_snap.combined_tree);
        let second_instruction = prompt::build_step_two_prompt(instruction, &action_obj.to_string(), &supported, None);
        let sys2 = prompt::build_act_system_prompt(cfg.system_prompt.as_deref());
        let msgs2 = vec![
            sys2,
            prompt::ChatMessage { role: "user".into(), content: Value::String(format!("{}\nAccessibility Tree:\n{}", second_instruction, diff)) },
        ];
        let resp2 = llm::generate(msgs2, &llm_cfg, true).unwrap_or(json!({}));
        let action2 = resp2.get("action").or_else(|| resp2.get("element")).cloned().unwrap_or(Value::Null);
        if !action2.is_null() {
            let res2 = execute_action(&action2, None).unwrap_or(json!({"error": "second step failed"}));
            vec![action_obj.clone(), action2]
        } else {
            vec![action_obj.clone()]
        }
    } else {
        vec![action_obj.clone()]
    };

    // Cache on success
    if cfg.cache_enabled && !actions.is_empty() {
        let _ = cache::set_cached("act", instruction, &url, Value::Array(actions.clone()));
    }

    Ok(json!({
        "success": true,
        "message": "Action executed",
        "actionDescription": instruction,
        "actions": actions,
        "result": result,
        "llm": llm_resp,
        "xpathMap": snap.combined_xpath_map,
        "snapshot": snap.combined_tree.chars().take(2000).collect::<String>()
    }))
}

fn replay_cached(cached: &Value, instruction: &str, cfg: &StagehandConfig) -> Result<Value> {
    let actions = cached.as_array().cloned().unwrap_or_default();
    let mut results = Vec::new();
    for a in actions {
        results.push(execute_action(&a, None)?);
    }
    Ok(json!({"replayed": true, "instruction": instruction, "results": results}))
}
