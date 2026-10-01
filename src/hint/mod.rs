//! Hint-key content script — standalone overlay for clickable/typeable elements.
//! Primary resolver: hint_snapshot -> choose/decide -> hint_click/hint_type.
//! Injection via Runtime.evaluate / Page.addScriptToEvaluateOnNewDocument
//! through the devtools proxy or browser_runtime client. Exposes
//! `window.__hyprfastHint { snapshot(), click(label), focusAndType(label,text) }`
//! callable via Runtime.evaluate (single transport, invariant I2).

use anyhow::{Result, bail};
use serde_json::{Value, json};

use crate::browser_runtime::server::CapabilityClass;

const HINT_JS: &str = include_str!("../../assets/hint.js");

fn cdp_call(method: &str, params: Value, cap: CapabilityClass) -> Result<Value> {
    crate::browser_runtime::client::cdp_call_sync(method, params, None, None, cap)
}

fn cdp_call_with_target(method: &str, params: Value, target_id: Option<&str>, cap: CapabilityClass) -> Result<Value> {
    crate::browser_runtime::client::cdp_call_sync(method, params, None, target_id, cap)
}

fn resolve_target_id(spec: &str) -> Result<String> {
    let v = cdp_call("Target.getTargets", json!({}), CapabilityClass::None)?;
    let infos = v.get("targetInfos").and_then(|x| x.as_array()).cloned().unwrap_or_default();
    let pages: Vec<Value> = infos
        .iter()
        .filter(|t| t.get("type").and_then(|x| x.as_str()) == Some("page"))
        .cloned()
        .collect();
    if pages.is_empty() {
        bail!("no page targets found for Target.getTargets");
    }
    let s = spec.trim();
    // numeric index (1-based) - e.g. "1" = first tab
    if let Ok(idx) = s.parse::<usize>() {
        if idx >= 1 && idx <= pages.len() {
            if let Some(tid) = pages[idx - 1].get("targetId").and_then(|x| x.as_str()) {
                return Ok(tid.to_string());
            }
        }
    }
    // exact targetId
    for p in &pages {
        if p.get("targetId").and_then(|x| x.as_str()) == Some(s) {
            return Ok(s.to_string());
        }
    }
    // substring match on url/title (case-insensitive)
    let low = s.to_lowercase();
    for p in &pages {
        let url = p.get("url").and_then(|x| x.as_str()).unwrap_or("").to_lowercase();
        let title = p.get("title").and_then(|x| x.as_str()).unwrap_or("").to_lowercase();
        if url.contains(&low) || title.contains(&low) {
            if let Some(tid) = p.get("targetId").and_then(|x| x.as_str()) {
                return Ok(tid.to_string());
            }
        }
    }
    let available: Vec<String> = pages
        .iter()
        .enumerate()
        .map(|(i, p)| {
            format!(
                "{}: title={:?} url={:?} targetId={}",
                i + 1,
                p.get("title").and_then(|x| x.as_str()).unwrap_or(""),
                p.get("url").and_then(|x| x.as_str()).unwrap_or(""),
                p.get("targetId").and_then(|x| x.as_str()).unwrap_or("")
            )
        })
        .collect();
    bail!(
        "no tab matches target spec '{}'. Available pages:\n{}",
        s,
        available.join("\n")
    );
}

fn ensure_hint_script() -> Result<()> {
    ensure_hint_script_with_target(None)
}
fn ensure_hint_script_with_target(target: Option<&str>) -> Result<()> {
    let call = |method: &str, params: Value, cap: CapabilityClass| -> Result<Value> {
        if let Some(tid) = target {
            cdp_call_with_target(method, params, Some(tid), cap)
        } else {
            cdp_call(method, params, cap)
        }
    };
    // Check if already injected
    let check = json!({
        "expression": "typeof window.__hyprfastHint !== 'undefined'",
        "returnByValue": true,
        "awaitPromise": false
    });
    let v = call("Runtime.evaluate", check, CapabilityClass::RuntimeEvaluate)?;
    let already = v.get("result").and_then(|r| r.get("value")).and_then(|x| x.as_bool()).unwrap_or(false);
    if already {
        return Ok(());
    }
    // Inject for future navigations: Page.addScriptToEvaluateOnNewDocument
    let _ = call(
        "Page.addScriptToEvaluateOnNewDocument",
        json!({"source": HINT_JS}),
        CapabilityClass::None,
    );
    // Inject into current document
    let eval = json!({
        "expression": HINT_JS,
        "returnByValue": true,
        "awaitPromise": false
    });
    let res = call("Runtime.evaluate", eval, CapabilityClass::RuntimeEvaluate)?;
    if let Some(exc) = res.get("exceptionDetails") {
        bail!("hint injection exception: {}", exc);
    }
    // Verify
    let verify = json!({
        "expression": "typeof window.__hyprfastHint !== 'undefined'",
        "returnByValue": true,
        "awaitPromise": false
    });
    let v2 = call("Runtime.evaluate", verify, CapabilityClass::RuntimeEvaluate)?;
    let ok = v2.get("result").and_then(|r| r.get("value")).and_then(|x| x.as_bool()).unwrap_or(false);
    if !ok {
        bail!("hint script failed to install window.__hyprfastHint");
    }
    Ok(())
}

/// Scan DOM for hint-eligible elements and overlay labels.
/// Returns compact list [{label, tag, role, name, rect, selector, text}]
pub fn hint_snapshot() -> Result<Value> {
    hint_snapshot_with_target(None)
}

/// Target-aware snapshot: spec can be index (1-based), targetId, or url/title substring.
/// Routes Runtime.evaluate directly to the target's session (no focus needed) via target_id.
pub fn hint_snapshot_with_target(target: Option<&str>) -> Result<Value> {
    let tid_opt: Option<String> = if let Some(spec) = target {
        let s = spec.trim();
        if s.is_empty() { None } else {
            let tid = resolve_target_id(s)?;
            // Best-effort activate for UI visibility, but routing does not depend on it
            let _ = cdp_call_with_target(
                "Target.activateTarget",
                json!({"targetId": tid}),
                Some(&tid),
                CapabilityClass::None,
            );
            let _ = cdp_call("Target.activateTarget", json!({"targetId": tid}), CapabilityClass::None);
            Some(tid)
        }
    } else { None };
    ensure_hint_script_with_target(tid_opt.as_deref())?;
    let expr = "JSON.stringify(window.__hyprfastHint.snapshot())";
    let params = json!({
        "expression": expr,
        "returnByValue": true,
        "awaitPromise": false,
        "userGesture": true
    });
    let v = if let Some(ref tid) = tid_opt {
        cdp_call_with_target("Runtime.evaluate", params, Some(tid), CapabilityClass::RuntimeEvaluate)?
    } else {
        cdp_call("Runtime.evaluate", params, CapabilityClass::RuntimeEvaluate)?
    };
    if let Some(exc) = v.get("exceptionDetails") {
        bail!("hint snapshot exception: {}", exc);
    }
    let raw = v.get("result").and_then(|r| r.get("value")).and_then(|x| x.as_str()).unwrap_or("[]");
    let arr: Value = serde_json::from_str(raw).unwrap_or(Value::Array(vec![]));
    let count = arr.as_array().map(|a| a.len()).unwrap_or(0);
    Ok(json!({"hints": arr, "count": count, "via": "hint"}))
}

fn activate_if_target(target: Option<&str>) -> Result<()> {
    if let Some(spec) = target {
        let s = spec.trim();
        if !s.is_empty() {
            let tid = resolve_target_id(s)?;
            let _ = cdp_call_with_target(
                "Target.activateTarget",
                json!({"targetId": tid}),
                Some(&tid),
                CapabilityClass::None,
            );
            let _ = cdp_call("Target.activateTarget", json!({"targetId": tid}), CapabilityClass::None);
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
    }
    Ok(())
}

/// Click element by hint label.
pub fn hint_click(label: &str) -> Result<Value> {
    hint_click_with_target(label, None)
}
pub fn hint_click_with_target(label: &str, target: Option<&str>) -> Result<Value> {
    if label.is_empty() { bail!("hint_click needs label"); }
    let tid_opt = target.and_then(|s| { let t=s.trim(); if t.is_empty() {None} else { resolve_target_id(t).ok() } });
    // best-effort activate for visibility
    if let Some(ref tid) = tid_opt {
        let _ = cdp_call_with_target("Target.activateTarget", json!({"targetId": tid}), Some(tid), CapabilityClass::None);
    }
    ensure_hint_script_with_target(tid_opt.as_deref())?;
    let expr = format!("JSON.stringify(window.__hyprfastHint.click({:?}))", label);
    let params = json!({
        "expression": expr,
        "returnByValue": true,
        "awaitPromise": false,
        "userGesture": true
    });
    let v = if let Some(ref tid) = tid_opt {
        cdp_call_with_target("Runtime.evaluate", params, Some(tid), CapabilityClass::RuntimeEvaluate)?
    } else {
        cdp_call("Runtime.evaluate", params, CapabilityClass::RuntimeEvaluate)?
    };
    if let Some(exc) = v.get("exceptionDetails") {
        bail!("hint click exception: {}", exc);
    }
    let raw = v.get("result").and_then(|r| r.get("value")).and_then(|x| x.as_str()).unwrap_or("{}");
    let out: Value = serde_json::from_str(raw).unwrap_or(json!({"raw": raw}));
    if let Some(err) = out.get("error").and_then(|e| e.as_str()) {
        bail!("{}", err);
    }
    Ok(out)
}

/// Focus element by hint label and type text.
pub fn hint_type(label: &str, text: &str) -> Result<Value> {
    hint_type_with_target(label, text, None)
}
pub fn hint_type_with_target(label: &str, text: &str, target: Option<&str>) -> Result<Value> {
    if label.is_empty() { bail!("hint_type needs label"); }
    let tid_opt = target.and_then(|s| { let t=s.trim(); if t.is_empty() {None} else { resolve_target_id(t).ok() } });
    if let Some(ref tid) = tid_opt {
        let _ = cdp_call_with_target("Target.activateTarget", json!({"targetId": tid}), Some(tid), CapabilityClass::None);
    }
    ensure_hint_script_with_target(tid_opt.as_deref())?;
    let expr = format!("JSON.stringify(window.__hyprfastHint.focusAndType({:?}, {:?}))", label, text);
    let params = json!({
        "expression": expr,
        "returnByValue": true,
        "awaitPromise": false,
        "userGesture": true
    });
    let v = if let Some(ref tid) = tid_opt {
        cdp_call_with_target("Runtime.evaluate", params, Some(tid), CapabilityClass::RuntimeEvaluate)?
    } else {
        cdp_call("Runtime.evaluate", params, CapabilityClass::RuntimeEvaluate)?
    };
    if let Some(exc) = v.get("exceptionDetails") {
        bail!("hint type exception: {}", exc);
    }
    let raw = v.get("result").and_then(|r| r.get("value")).and_then(|x| x.as_str()).unwrap_or("{}");
    let out: Value = serde_json::from_str(raw).unwrap_or(json!({"raw": raw}));
    if let Some(err) = out.get("error").and_then(|e| e.as_str()) {
        bail!("{}", err);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Milestone 3: hint resolver (heuristic + LLM) + fallback chain helpers
// ---------------------------------------------------------------------------

fn extract_target_hint(instr: &str) -> String {
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
    let lower = instr.to_lowercase();
    for kw in ["button", "link", "prompt box", "input", "field"] {
        if let Some(pos) = lower.find(kw) {
            let before = instr[..pos].trim();
            if let Some(last) = before.split('\'').last().map(|s| s.trim()) {
                if !last.is_empty() && last.len() < 40 { return last.to_string(); }
            }
        }
    }
    // fallback: take words excluding common verbs
    let stop = ["click", "the", "a", "an", "press", "type", "fill", "into", "on", "please"];
    let words: Vec<String> = instr.split_whitespace()
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_string())
        .filter(|w| !w.is_empty() && !stop.contains(&w.to_lowercase().as_str()))
        .collect();
    if words.is_empty() {
        return instr.split_whitespace().take(5).collect::<Vec<_>>().join(" ");
    }
    words.join(" ").chars().take(40).collect()
}

/// Heuristic exact text/role match without LLM if simple.
/// Returns Some(label) only when unambiguous single match.
pub fn heuristic_hint_match(instruction: &str, hints_val: &Value) -> Option<String> {
    let target = extract_target_hint(instruction);
    if target.is_empty() { return None; }
    let low = target.to_lowercase();
    let arr = hints_val.get("hints").and_then(|v| v.as_array())
        .or_else(|| hints_val.as_array())?;
    if arr.is_empty() { return None; }
    let mut candidates: Vec<String> = Vec::new();
    for h in arr {
        let label = h.get("label").and_then(|v| v.as_str()).unwrap_or("").to_string();
        if label.is_empty() { continue; }
        let name = h.get("name").and_then(|v| v.as_str()).unwrap_or("").to_lowercase();
        let text = h.get("text").and_then(|v| v.as_str()).unwrap_or("").to_lowercase();
        let role = h.get("role").and_then(|v| v.as_str()).unwrap_or("").to_lowercase();
        let tag = h.get("tag").and_then(|v| v.as_str()).unwrap_or("").to_lowercase();
        let hay = format!("{} {} {} {}", name, text, role, tag);
        // exact or substring match
        if name == low || text == low {
            candidates.push(label);
        } else if hay.contains(&low) {
            candidates.push(label);
        } else if low.len() >= 3 && (name.contains(&low) || text.contains(&low)) {
            candidates.push(label);
        }
    }
    if candidates.len() == 1 {
        return Some(candidates[0].clone());
    }
    None
}

/// Resolve a hint label through deterministic matching, then Decider selection.
pub fn resolve_hint_for_instruction(instruction: &str, hints_val: &Value, target: Option<&str>) -> Option<String> {
    resolve_hint_with_candidates(instruction, hints_val, target, None)
}

/// [`resolve_hint_for_instruction`] with a pre-filtered candidate set.
///
/// The Decider tier re-collects candidates from the live page when the context
/// carries none, so a caller that has already narrowed the field (a type action
/// must not land on a link) has to hand the narrowed set over explicitly —
/// otherwise the second tier silently widens the search back to the full page
/// and undoes the filter.
pub fn resolve_hint_with_candidates(
    instruction: &str,
    hints_val: &Value,
    target: Option<&str>,
    candidates: Option<crate::decider::Candidates>,
) -> Option<String> {
    if let Some(label) = heuristic_hint_match(instruction, hints_val) {
        return Some(label);
    }
    let mut ctx = crate::perception::ResolveContext::new(instruction);
    if let Some(target) = target { ctx = ctx.with_target(target); }
    if let Some(c) = candidates { ctx = ctx.with_candidates(c); }
    crate::perception::resolve_target(instruction, &ctx).ok().and_then(|r| r.label)
}

/// Try hint tier for instruction. Returns Ok((Value, label)) on success.
pub fn try_hint_tier(instruction: &str, method: &str, type_text: &str, target: Option<&str>) -> Result<(Value, String)> {
    let hints = hint_snapshot()?;
    let count = hints.get("count").and_then(|v| v.as_u64()).unwrap_or(0);
    if count == 0 {
        bail!("hint tier: no hints (canvas/custom-drawn, fallback to vision)");
    }
    let label = resolve_hint_for_instruction(instruction, &hints, target)
        .ok_or_else(|| anyhow::anyhow!("hint tier: no label matches instruction {:?}", instruction))?;
    let is_type = matches!(method, "fill" | "type" | "type_text");
    let res = if is_type {
        let txt = if type_text.is_empty() {
            // try to extract quoted text from instruction
            extract_target_hint(instruction)
        } else { type_text.to_string() };
        // if still empty, bail to vision
        hint_type(&label, &txt)?
    } else {
        hint_click(&label)?
    };
    Ok((res, label))
}

/// Try vision tier as last resort via ground.rs.
/// Returns Ok(Value) if grounded and clicked/typed.
pub fn try_vision_tier(instruction: &str, method: &str, type_text: &str, window: &str) -> Result<Value> {
    let is_type = matches!(method, "fill" | "type" | "type_text");
    if is_type {
        let txt = if type_text.is_empty() { extract_target_hint(instruction) } else { type_text.to_string() };
        crate::ground::act_fast(instruction, "type", &txt, window)
    } else {
        crate::ground::act_fast(instruction, "click", "", window)
    }
}

// ---------------------------------------------------------------------------
// Vimium-primary + parallel batch (user requested: hint primary, parallel)
// ---------------------------------------------------------------------------

/// Clear hint overlay without new snapshot.
pub fn hint_clear() -> Result<Value> {
    hint_clear_with_target(None)
}
pub fn hint_clear_with_target(target: Option<&str>) -> Result<Value> {
    let tid_opt = target.and_then(|s| { let t=s.trim(); if t.is_empty() {None} else { resolve_target_id(t).ok() } });
    ensure_hint_script_with_target(tid_opt.as_deref())?;
    let params = json!({
        "expression": "JSON.stringify((window.__hyprfastHint.clear(), {cleared:true}))",
        "returnByValue": true,
        "awaitPromise": false
    });
    let v = if let Some(ref tid) = tid_opt {
        cdp_call_with_target("Runtime.evaluate", params, Some(tid), CapabilityClass::RuntimeEvaluate)?
    } else {
        cdp_call("Runtime.evaluate", params, CapabilityClass::RuntimeEvaluate)?
    };
    let raw = v.get("result").and_then(|r| r.get("value")).and_then(|x| x.as_str()).unwrap_or("{}");
    let out: Value = serde_json::from_str(raw).unwrap_or(json!({"cleared": true}));
    Ok(out)
}

/// Vimium-primary single action: snapshot once -> resolve label -> click/type.
/// This is the fast path: no AX tree, no screenshot. Falls back to vision if hint empty.
pub fn hint_act(instruction: &str, method: &str, type_text: &str) -> Result<Value> {
    hint_act_with_target(instruction, method, type_text, None)
}
pub fn hint_act_with_target(
    instruction: &str,
    method: &str,
    type_text: &str,
    target: Option<&str>,
) -> Result<Value> {
    let hints = hint_snapshot_with_target(target)?;
    let count = hints.get("count").and_then(|v| v.as_u64()).unwrap_or(0);
    if count == 0 {
        // No DOM candidates -> vision last resort (keep screenshot+vision if nothing works)
        return try_vision_tier(instruction, method, type_text, "");
    }
    let is_type = matches!(method, "fill" | "type" | "type_text");
    // A type instruction resolved against the full page lands on whatever
    // matches its wording best, which is very often a link or a row rather
    // than a field — "type despacito into the search box" was matching a
    // result link whose text mentioned despacito. Typing there overwrote the
    // element's own label and still reported success, so the caller moved on
    // believing the text had been entered. For a type action the editable
    // elements are the only sane candidates, so they go first.
    if is_type {
        if let Some(label) = editable_only_hints(&hints).and_then(|e| {
            // The narrowed set has to reach the Decider tier too, or it
            // re-collects from the page and picks the link again. A single
            // editable element needs no model at all — there is nothing to
            // choose between, and the Decider rejects a one-option question.
            let cands = crate::decider::Candidates::from_hint_snapshot(&e, None, None, None);
            if cands.len() == 1 {
                cands.iter().next().map(|c| c.label.clone())
            } else {
                resolve_hint_with_candidates(instruction, &e, target, Some(cands))
            }
        }) {
            let txt = if type_text.is_empty() { extract_target_hint(instruction) } else { type_text.to_string() };
            let res = hint_type_with_target(&label, &txt, target)?;
            return Ok(json!({"success": true, "tier": "hint", "label": label, "via": "hint_act", "result": res, "count": count}));
        }
    }
    if let Some(label) = resolve_hint_for_instruction(instruction, &hints, target) {
        let res = if is_type {
            let txt = if type_text.is_empty() { extract_target_hint(instruction) } else { type_text.to_string() };
            hint_type_with_target(&label, &txt, target)?
        } else {
            hint_click_with_target(&label, target)?
        };
        return Ok(json!({"success": true, "tier": "hint", "label": label, "via": "hint_act", "result": res, "count": count}));
    }
    // No label matched -> try vision as last resort per user preference
    try_vision_tier(instruction, method, type_text, "")
}

/// The subset of a `hint_snapshot` payload that can actually receive typed
/// text: `input` (excluding button-ish and checkbox types), `textarea`,
/// `select`, and anything `contenteditable`.
///
/// A type instruction is frequently phrased against a *place* ("the search
/// box") while the resolver's best lexical match on the page is a link whose
/// text mentions search. Restricting the candidates to editable elements is
/// what keeps `type X into the search box` from landing on a result link.
fn editable_only_hints(hints_val: &Value) -> Option<Value> {
    let hints = hints_val.get("hints")?.as_array()?;
    let editable: Vec<&Value> = hints.iter().filter(|h| hint_is_editable(h)).collect();
    if editable.is_empty() {
        return None;
    }
    let mut out = hints_val.clone();
    out["hints"] = Value::Array(editable.into_iter().cloned().collect());
    out["count"] = json!(out["hints"].as_array().map(|a| a.len()).unwrap_or(0));
    Some(out)
}

/// True when the hint's element can hold typed text.
fn hint_is_editable(h: &Value) -> bool {
    let tag = h.get("tag").and_then(|v| v.as_str()).unwrap_or("").to_ascii_lowercase();
    if tag == "textarea" || tag == "select" {
        return true;
    }
    if tag != "input" {
        // `contenteditable` is not in the hint payload, but a non-input element
        // is only a plausible type target when its selector says so.
        return h
            .get("selector")
            .and_then(|v| v.as_str())
            .is_some_and(|s| s.contains("contenteditable"));
    }
    match h.get("type").and_then(|v| v.as_str()).unwrap_or("text").to_ascii_lowercase().as_str() {
        "button" | "submit" | "reset" | "checkbox" | "radio" | "file" | "image" | "range" | "color" => false,
        _ => true,
    }
}

/// Vimium-primary parallel batch: one snapshot + Decider selection + parallel dispatches.
pub fn hint_batch(steps: &[Value]) -> Result<Value> {
    hint_batch_with_target(steps, None)
}
pub fn hint_batch_with_target(
    steps: &[Value],
    target: Option<&str>,
) -> Result<Value> {
    if steps.is_empty() { bail!("hint_batch needs at least 1 step"); }
    if steps.len() > 12 { bail!("hint_batch max 12 steps"); }
    let hints = hint_snapshot_with_target(target)?;
    let count = hints.get("count").and_then(|v| v.as_u64()).unwrap_or(0);
    if count == 0 {
        // No hints -> per-step vision fallback sequentially (vision needs screenshot per step)
        let mut results = Vec::new();
        for s in steps {
            let instr = s.get("instruction").and_then(|v| v.as_str()).unwrap_or("");
            let method = s.get("action").and_then(|v| v.as_str()).or_else(|| s.get("method").and_then(|v| v.as_str())).unwrap_or("click");
            let text = s.get("text").and_then(|v| v.as_str()).unwrap_or("");
            let r = try_vision_tier(instr, method, text, "").unwrap_or_else(|e| json!({"error": e.to_string(), "instruction": instr, "tier": "vision"}));
            results.push(json!({"instruction": instr, "tier": "vision", "result": r}));
        }
        return Ok(json!({"results": results, "steps": results.len(), "count": 0, "via": "vision_fallback"}));
    }
    // Collect instructions and methods
    let instrs: Vec<String> = steps.iter().map(|s| s.get("instruction").and_then(|v| v.as_str()).unwrap_or("").to_string()).collect();
    let methods: Vec<String> = steps.iter().map(|s| s.get("action").and_then(|v| v.as_str()).or_else(|| s.get("method").and_then(|v| v.as_str())).unwrap_or("click").to_string()).collect();
    let texts: Vec<String> = steps.iter().map(|s| s.get("text").and_then(|v| v.as_str()).unwrap_or("").to_string()).collect();

    // Phase 1: heuristic fast-path per instruction (no LLM)
    let mut labels: Vec<Option<String>> = Vec::with_capacity(instrs.len());
    for (i, instr) in instrs.iter().enumerate() {
        labels.push(resolve_hint_for_instruction(instr, &hints, target));
    }
    // Phase 3: parallel dispatch via std::thread::scope (CDP transport concurrent, I5 per-target serialization still via server queue)
    let hints_for_threads = &hints;
    let target_owned = target.map(|s| s.to_string());
    let results: Vec<Value> = std::thread::scope(|scope| {
        let mut handles = Vec::new();
        for i in 0..instrs.len() {
            let label_opt = labels[i].clone();
            let instr = instrs[i].clone();
            let method = methods[i].clone();
            let text = texts[i].clone();
            let hints_ref = hints_for_threads;
            let t = target_owned.clone();
            handles.push(scope.spawn(move || {
                if let Some(label) = label_opt {
                    let is_type = matches!(method.as_str(), "fill" | "type" | "type_text");
                    let res = if is_type {
                        let txt = if text.is_empty() { extract_target_hint(&instr) } else { text.clone() };
                        hint_type_with_target(&label, &txt, t.as_deref())
                    } else {
                        hint_click_with_target(&label, t.as_deref())
                    };
                    match res {
                        Ok(v) => {
                            json!({"instruction": instr, "label": label, "tier": "hint", "success": true, "result": v})
                        },
                        Err(e) => {
                            // dispatch failed -> vision fallback per step
                            match try_vision_tier(&instr, &method, &text, "") {
                                Ok(v) => json!({"instruction": instr, "label": label, "tier": "vision", "success": true, "result": v, "hint_error": e.to_string()}),
                                Err(ve) => json!({"instruction": instr, "label": label, "tier": "error", "success": false, "error": format!("hint:{} vision:{}", e, ve)}),
                            }
                        }
                    }
                } else {
                    // no hint label -> vision fallback
                    match try_vision_tier(&instr, &method, &text, "") {
                        Ok(v) => json!({"instruction": instr, "tier": "vision", "success": true, "result": v}),
                        Err(e) => {
                            // also record that hint had candidates but no match
                            json!({"instruction": instr, "tier": "error", "success": false, "error": e.to_string(), "hint_count": hints_ref.get("count").cloned().unwrap_or(json!(0))})
                        }
                    }
                }
            }));
        }
        handles.into_iter().map(|h| h.join().unwrap_or(json!({"error": "thread panic"}))).collect()
    });

    let ok_count = results.iter().filter(|r| r.get("success").and_then(|v| v.as_bool()).unwrap_or(false)).count();
    Ok(json!({"results": results, "steps": results.len(), "ok": ok_count, "count": count, "via": "hint_batch_parallel"}))
}
