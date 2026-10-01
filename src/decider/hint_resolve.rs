//! Hint integration with Decider-2B.
//!
//! Pipeline: hint_snapshot -> candidate list (numeric 1..N stable) -> deterministic filtering
//!          -> annotated screenshot (optional, numeric IDs) -> Decider -> candidate ID -> existing hint_click.
//! Supports up to 255 via hierarchical (text), but vision path chunk 10.
//! Also key_identify for visual/virtual keyboards same pipeline but keyboard candidates.

use std::collections::HashMap;

use anyhow::{Result, bail};
use serde_json::{Value, json};

use crate::decider::candidates::{Candidates, Rect, Candidate, VISION_MAX_OPTIONS, TEXT_MAX_OPTIONS};
use crate::decider::config::DeciderConfig;
use crate::decider::types::{DeciderQuestion, DeciderRequest};
use crate::decider::client::DeciderClient;
use crate::hint;

// ---------------------------------------------------------------------------
// Config helpers
// ---------------------------------------------------------------------------

const LOW_CONF_THRESHOLD: f64 = 0.55;

// ---------------------------------------------------------------------------
// Core: hint_resolve (single instruction)
// ---------------------------------------------------------------------------

pub async fn hint_resolve_async(instruction: &str, target: Option<&str>, use_vision: bool) -> Result<Value> {
    if instruction.trim().is_empty() { bail!("hint_resolve needs instruction"); }

    // 1. hint_snapshot -> candidate list (numeric 1..N stable)
    let snap = hint::hint_snapshot_with_target(target)?;
    let mut cands = Candidates::from_hint_snapshot(&snap, None, None, None);
    if cands.is_empty() {
        bail!("hint snapshot empty (no candidates)");
    }

    // 2. deterministic filtering (reuse perception policy: visible+enabled+non-empty geometry, then viewport filter)
    let filtered = cands.filtered_for_budget(None, None);
    if !filtered.is_empty() && filtered.len() != cands.len() {
        cands = filtered;
    }

    // 3. heuristic fast path - if single unambiguous heuristic match, skip Decider
    if let Some(label) = hint::heuristic_hint_match(instruction, &snap) {
        if let Some(cand) = cands.get_by_label(&label) {
            let res = hint::hint_click_with_target(&label, target)?;
            return Ok(json!({
                "success": true,
                "tier": "heuristic",
                "candidate": {"id": cand.id, "label": cand.label, "selector": cand.selector, "rect": {"x": cand.rect.x, "y": cand.rect.y, "width": cand.rect.width, "height": cand.rect.height}},
                "label": label,
                "id": cand.id,
                "via": "hint_resolve_heuristic",
                "instruction": instruction,
                "result": res
            }));
        }
    }

    // 4. Decide via Decider; when disabled, use deterministic matching only.
    let cfg = DeciderConfig::from_env();
    if !cfg.enabled {
        let nq = instruction.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
        let mut hits: Vec<&Candidate> = cands.iter().filter(|c| {
            let hay = format!("{} {}", c.name, c.text).to_lowercase();
            hay.contains(&nq.trim().to_lowercase())
        }).collect();
        if hits.len() == 1 {
            let cand = hits[0];
            let res = hint::hint_click_with_target(&cand.label, target)?;
            return Ok(json!({"success": true, "tier": "heuristic_fallback", "candidate": {"id": cand.id, "label": cand.label}, "label": cand.label, "id": cand.id, "via":"hint_resolve", "result": res}));
        }
        bail!("hint_resolve: {} and deterministic matching found no match for '{}'", crate::decider::config::off_reason(), instruction);
    }

    // 5. Decider selection
    let max_per = if use_vision { VISION_MAX_OPTIONS } else { TEXT_MAX_OPTIONS };
    // For annotated screenshot (vision), prepare image once
    let image_b64_opt = if use_vision {
        match prepare_annotated_image(&cands, target, false) {
            Ok((b64, meta)) => {
                // meta could be returned but we stash later
                Some(b64)
            },
            Err(_) => None,
        }
    } else { None };

    // Hierarchical chunking if needed
    let decision = decider_select_with_candidates(&cands, instruction, instruction, max_per, use_vision, image_b64_opt.clone(), target).await?;

    let choice_id = decision.choice.clone();
    let confidence = decision.confidence;
    let probs = decision.probabilities.clone();

    // Low confidence → uncertain not success
    if let Some(c) = confidence {
        if c < LOW_CONF_THRESHOLD {
            return Ok(json!({
                "success": false,
                "status": "uncertain",
                "reason": "low confidence",
                "confidence": c,
                "probabilities": probs,
                "instruction": instruction,
                "choice": choice_id,
                "candidate_count": cands.len()
            }));
        }
        if let Some(p) = &probs {
            let runner = runner_up(p, &choice_id);
            if let Some(ru) = runner {
                if (c - ru) < 0.10 {
                    return Ok(json!({
                        "success": false,
                        "status": "uncertain",
                        "reason": "low margin",
                        "confidence": c,
                        "runner_up": ru,
                        "margin": c - ru,
                        "probabilities": p,
                        "instruction": instruction,
                        "choice": choice_id
                    }));
                }
            }
        }
    }

    // Map choice_id (1..N within chunk/global) to global candidate id
    let global_id = resolve_global_id(&cands, &choice_id, max_per)?;
    let cand = cands.get(global_id).ok_or_else(|| anyhow::anyhow!("candidate {} not found", global_id))?;

    // If original hint label empty (numeric candidates), we still click via rect center fallback
    // but prefer hint_click when label exists
    let result = if !cand.label.is_empty() {
        hint::hint_click_with_target(&cand.label, target)?
    } else {
        // Click via center coordinates as fallback for label-less candidates
        let cx = cand.rect.x as f64 + cand.rect.width as f64 / 2.0;
        let cy = cand.rect.y as f64 + cand.rect.height as f64 / 2.0;
        crate::input::click(Some(cx), Some(cy), "left", false)?;
        json!({"clicked": true, "x": cx, "y": cy, "via": "rect_center"})
    };

    Ok(json!({
        "success": true,
        "tier": if use_vision { "decider_vision" } else { "decider_text" },
        "candidate": {"id": cand.id, "label": cand.label, "tag": cand.tag, "role": cand.role, "name": cand.name, "rect": {"x": cand.rect.x, "y": cand.rect.y, "width": cand.rect.width, "height": cand.rect.height}, "selector": cand.selector},
        "id": cand.id,
        "label": cand.label,
        "choice": choice_id,
        "confidence": confidence,
        "probabilities": probs,
        "via": "hint_resolve_decider",
        "instruction": instruction,
        "result": result
    }))
}

fn runner_up(probs: &HashMap<String,f64>, choice: &str) -> Option<f64> {
    let top = *probs.get(choice)?;
    let mut best = 0.0; let mut found=false;
    for (k,v) in probs { if k!=choice && *v>best { best=*v; found=true; } }
    if found { Some(best) } else { None }
}

fn resolve_global_id(cands: &Candidates, choice: &str, max_per: usize) -> Result<u32> {
    // First try direct id parse (global id)
    if let Ok(id) = choice.parse::<u32>() {
        if cands.get(id).is_some() {
            // Could be either chunk-local index or global id. Disambiguate:
            // If cands.len() <= max_per, choice is 1..N == global id (1-indexed)
            if cands.len() <= max_per {
                if (id as usize) >=1 && (id as usize) <= cands.len() {
                    // Both global and index overlap when ids are 1..N; prefer global lookup
                    // check if choice index maps to same id - if mismatch, prefer global
                    return Ok(id);
                }
            }
            // Hierarchical case: choice is chunk-local index, need map
            // But caller already flattened via hierarchical winners, so choice should be within final winners set (<=max_per)
            // For non-hierarchical case, direct id works
            // If hierarchical needed, decider_select_with_candidates already handled chunk mapping and returned global choice?
            // Here we treat choice as global id when found
            return Ok(id);
        }
        // Try chunk index mapping
        if (id as usize) >=1 && (id as usize) <= cands.len() {
            // index -> global via position (id = index? Since ids are 1..N, index == global id)
            if let Some(c) = cands.as_slice().get((id as usize)-1) {
                return Ok(c.id);
            }
        }
    }
    // Try option text match
    for cand in cands.iter() {
        if cand.display_text() == choice || cand.vision_text() == choice || cand.name == choice {
            return Ok(cand.id);
        }
    }
    bail!("choice '{}' does not map to a candidate", choice);
}

async fn decider_select_with_candidates(
    cands: &Candidates,
    query: &str,
    context: &str,
    max_per: usize,
    use_vision: bool,
    image_b64: Option<String>,
    _target: Option<&str>,
) -> Result<crate::decider::types::DeciderDecision> {
    let cfg = DeciderConfig::from_env();
    let client = DeciderClient::new(cfg)?;

    // If within budget, single call
    if cands.len() <= max_per {
        let opts: Vec<String> = cands.iter().map(|c| if use_vision { c.vision_text() } else { c.display_text() }).collect();
        let question = DeciderQuestion::new(query.to_string(), opts);
        let req = if let Some(img) = image_b64 {
            // Validate image before sending
            if let Err(e) = crate::decider::types::validate_image_field(&img) {
                eprintln!("[hint_resolve] image validation failed: {e}, falling back to text");
                DeciderRequest::new(context.to_string(), vec![question])
            } else {
                DeciderRequest::new(context.to_string(), vec![question]).with_image(img)
            }
        } else {
            DeciderRequest::new(context.to_string(), vec![question])
        };
        let resp = client.decide(&req).await?;
        return resp.decisions.into_iter().next().ok_or_else(|| anyhow::anyhow!("empty decisions"));
    }

    // Hierarchical chunking: first win each chunk, then final
    let chunks = cands.chunks(max_per);
    let mut winning_ids: Vec<u32> = Vec::new();
    for (idx, chunk) in chunks.iter().enumerate() {
        let opts: Vec<String> = chunk.iter().map(|c| if use_vision { c.vision_text() } else { c.display_text() }).collect();
        let q = if chunks.len() > 1 { format!("{} — group {}/{}", query, idx+1, chunks.len()) } else { query.to_string() };
        let question = DeciderQuestion::new(q, opts);
        let req = if use_vision {
            // Per-chunk annotated image: annotate that chunk's legend
            let chunk_cands = Candidates::from_vec(chunk.clone());
            let img = match prepare_annotated_image(&chunk_cands, _target, true) {
                Ok((b64, _)) => Some(b64),
                Err(_) => image_b64.clone(),
            };
            if let Some(b64) = img {
                DeciderRequest::new(context.to_string(), vec![question]).with_image(b64)
            } else {
                DeciderRequest::new(context.to_string(), vec![question])
            }
        } else {
            DeciderRequest::new(context.to_string(), vec![question])
        };
        let resp = client.decide(&req).await?;
        if let Some(dec) = resp.decisions.into_iter().next() {
            if let Ok(idx_choice) = dec.choice.parse::<usize>() {
                if idx_choice >=1 && idx_choice <= chunk.len() {
                    winning_ids.push(chunk[idx_choice-1].id);
                }
            } else {
                // option text
                if let Some(cand) = chunk.iter().find(|c| c.display_text()==dec.choice || c.vision_text()==dec.choice) {
                    winning_ids.push(cand.id);
                }
            }
        }
    }
    if winning_ids.is_empty() { bail!("hierarchical: no winners"); }
    let winners_cands = cands.group_winners(&winning_ids);
    if winners_cands.len() <= max_per {
        let opts: Vec<String> = winners_cands.iter().map(|c| if use_vision { c.vision_text() } else { c.display_text() }).collect();
        let final_q = DeciderQuestion::new(query.to_string(), opts);
        let final_req = if use_vision {
            if let Ok((b64, _)) = prepare_annotated_image(&winners_cands, _target, true) {
                DeciderRequest::new(context.to_string(), vec![final_q]).with_image(b64)
            } else if let Some(b64) = image_b64 {
                DeciderRequest::new(context.to_string(), vec![final_q]).with_image(b64)
            } else {
                DeciderRequest::new(context.to_string(), vec![final_q])
            }
        } else {
            DeciderRequest::new(context.to_string(), vec![final_q])
        };
        let resp = client.decide(&final_req).await?;
        let dec = resp.decisions.into_iter().next().ok_or_else(|| anyhow::anyhow!("empty final decision"))?;
        // Map final choice (1..N within winners) to global id via lookup
        if let Ok(idx) = dec.choice.parse::<usize>() {
            if idx>=1 && idx <= winners_cands.len() {
                let global = winners_cands.as_slice()[idx-1].id;
                // Return decision with global id as choice for caller to map
                return Ok(crate::decider::types::DeciderDecision { choice: global.to_string(), confidence: dec.confidence, probabilities: dec.probabilities });
            }
        }
        return Ok(dec);
    }
    bail!("hierarchical winners still over budget after dedup: {} > {}", winners_cands.len(), max_per);
}

fn prepare_annotated_image(cands: &Candidates, _target: Option<&str>, per_chunk: bool) -> Result<(String, Value)> {
    use crate::decider::image::{ImageSource, capture_with_source, annotate_and_encode};
    // Prefer browser CDP screenshot (viewport) for annotation; fallback monitor
    let (bytes, _cap_meta) = capture_with_source(&ImageSource::Browser, 0.5)
        .or_else(|_| capture_with_source(&ImageSource::Monitor, 0.5))?;
    let (encoded, meta) = annotate_and_encode(bytes, cands, true, true)?;
    // meta includes legend with numeric IDs
    if per_chunk {
        // ensure caller sees per-chunk legend size
    }
    Ok((encoded, meta))
}

// ---------------------------------------------------------------------------
// Sync wrappers
// ---------------------------------------------------------------------------

fn rt_block_on<F: std::future::Future>(f: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread().enable_all().build().expect("hint_resolve rt").block_on(f)
}

pub fn hint_resolve(instruction: &str) -> Result<Value> {
    rt_block_on(hint_resolve_async(instruction, None, false))
}
pub fn hint_resolve_with_target(instruction: &str, target: Option<&str>) -> Result<Value> {
    rt_block_on(hint_resolve_async(instruction, target, false))
}
pub fn hint_resolve_vision(instruction: &str, target: Option<&str>) -> Result<Value> {
    rt_block_on(hint_resolve_async(instruction, target, true))
}

// ---------------------------------------------------------------------------
// Batch: multiple semantic queries same snapshot/screenshot single batched Decider request
// ---------------------------------------------------------------------------

pub async fn hint_resolve_batch_async(instructions: &[String], target: Option<&str>, use_vision: bool) -> Result<Value> {
    if instructions.is_empty() { bail!("hint_resolve_batch needs at least 1 instruction"); }
    if instructions.len() > 12 { bail!("hint_resolve_batch max 12"); }

    let snap = hint::hint_snapshot_with_target(target)?;
    let cands = Candidates::from_hint_snapshot(&snap, None, None, None);
    if cands.is_empty() { bail!("hint snapshot empty"); }
    let max_per = if use_vision { VISION_MAX_OPTIONS } else { TEXT_MAX_OPTIONS };
    if cands.len() > max_per {
        // Over budget: need per-instruction hierarchical, but we can still share snapshot
        // Fall back to sequential per instruction (still one snapshot reused)
        let mut results = Vec::new();
        for instr in instructions {
            let r = hint_resolve_async(instr, target, use_vision).await.unwrap_or_else(|e| json!({"error": e.to_string(), "instruction": instr, "success": false}));
            results.push(json!({"instruction": instr, "result": r}));
        }
        return Ok(json!({"results": results, "steps": results.len(), "count": cands.len(), "via": "hint_resolve_batch_hierarchical_fallback"}));
    }

    let cfg = DeciderConfig::from_env();
    if !cfg.enabled {
        let mut results = Vec::new();
        for instr in instructions {
            if let Some(label) = crate::hint::heuristic_hint_match(instr, &snap) {
                if let Some(cand) = cands.get_by_label(&label) {
                    let res = hint::hint_click_with_target(&label, target).unwrap_or(json!({"error":"click failed"}));
                    results.push(json!({"instruction": instr, "label": label, "id": cand.id, "success": true, "result": res}));
                    continue;
                }
            }
            results.push(json!({"instruction": instr, "success": false, "error": "no match"}));
        }
        return Ok(json!({"results": results, "steps": results.len(), "count": cands.len(), "via": "hint_resolve_batch_deterministic_fallback"}));
    }

    // Single batched Decider request: prepare shared annotated image if vision
    let image_b64_opt = if use_vision {
        match prepare_annotated_image(&cands, target, false) {
            Ok((b64,_)) => Some(b64),
            Err(_) => None,
        }
    } else { None };

    let opts: Vec<String> = cands.iter().map(|c| if use_vision { c.vision_text() } else { c.display_text() }).collect();
    let mut requests = Vec::new();
    for instr in instructions {
        let q = DeciderQuestion::new(instr.clone(), opts.clone());
        let ctx_str = instr.clone();
        let req = if let Some(ref img) = image_b64_opt {
            DeciderRequest::new(ctx_str, vec![q]).with_image(img.clone())
        } else {
            DeciderRequest::new(ctx_str, vec![q])
        };
        requests.push(req);
    }
    let client = DeciderClient::new(DeciderConfig::from_env())?;
    let responses = client.decide_batch(requests).await;

    let mut results = Vec::new();
    for (i, resp_res) in responses.into_iter().enumerate() {
        let instr = &instructions[i];
        match resp_res {
            Ok(resp) => {
                if let Some(dec) = resp.decisions.into_iter().next() {
                    let choice = dec.choice.clone();
                    let conf = dec.confidence;
                    if conf.map(|c| c < LOW_CONF_THRESHOLD).unwrap_or(false) {
                        results.push(json!({"instruction": instr, "success": false, "status": "uncertain", "confidence": conf, "choice": choice, "reason": "low confidence"}));
                        continue;
                    }
                    let global = match resolve_global_id(&cands, &choice, max_per) {
                        Ok(id) => id,
                        Err(e) => {
                            results.push(json!({"instruction": instr, "success": false, "error": e.to_string(), "choice": choice}));
                            continue;
                        }
                    };
                    if let Some(cand) = cands.get(global) {
                        // Don't auto-click in batch? The spec says batch should still route to hint_click per query? But to avoid parallel CDP contention, we report selection and let caller click?
                        // Here we click sequentially for each result (deterministic)
                        let click_res = if !cand.label.is_empty() {
                            hint::hint_click_with_target(&cand.label, target).unwrap_or(json!({"error":"click failed"}))
                        } else {
                            let cx = cand.rect.x as f64 + cand.rect.width as f64/2.0;
                            let cy = cand.rect.y as f64 + cand.rect.height as f64/2.0;
                            let _ = crate::input::click(Some(cx), Some(cy), "left", false);
                            json!({"clicked": true, "via": "rect_center", "x": cx, "y": cy})
                        };
                        results.push(json!({
                            "instruction": instr,
                            "success": true,
                            "candidate": {"id": cand.id, "label": cand.label},
                            "id": cand.id, "label": cand.label, "choice": choice, "confidence": conf, "probabilities": dec.probabilities,
                            "result": click_res
                        }));
                    } else {
                        results.push(json!({"instruction": instr, "success": false, "error": format!("candidate {} not found", global)}));
                    }
                } else {
                    results.push(json!({"instruction": instr, "success": false, "error": "empty decisions"}));
                }
            },
            Err(e) => {
                results.push(json!({"instruction": instr, "success": false, "error": e.to_string()}));
            }
        }
    }
    let ok = results.iter().filter(|r| r.get("success").and_then(|v| v.as_bool()).unwrap_or(false)).count();
    Ok(json!({"results": results, "steps": results.len(), "ok": ok, "count": cands.len(), "via": "hint_resolve_batch_decider"}))
}

pub fn hint_resolve_batch(instructions: &[String], target: Option<&str>) -> Result<Value> {
    rt_block_on(hint_resolve_batch_async(instructions, target, false))
}
pub fn hint_resolve_batch_with_target(instructions: &[String], target: Option<&str>, use_vision: bool) -> Result<Value> {
    rt_block_on(hint_resolve_batch_async(instructions, target, use_vision))
}

// ---------------------------------------------------------------------------
// key_identify for visual/virtual keyboards same pipeline but keyboard candidates
// ---------------------------------------------------------------------------

/// Identify a key on a visual/virtual keyboard. Same pipeline but filtered to keyboard candidates.
/// Keyboard candidates are those with role in [button, key] or tag [button, div with key-like text] and inside keyboard rect if provided.
pub async fn key_identify_async(key_query: &str, keyboard_rect: Option<Rect>, target: Option<&str>, use_vision: bool) -> Result<Value> {
    if key_query.trim().is_empty() { bail!("key_identify needs key query"); }
    let snap = hint::hint_snapshot_with_target(target)?;
    let all = Candidates::from_hint_snapshot(&snap, None, None, None);
    if all.is_empty() { bail!("key_identify: no candidates from hint"); }

    // Filter to keyboard-like candidates: role button/key, or single-char name, inside keyboard viewport if given
    let mut keyboard_cands = all.filtered(|c| {
        let is_key_role = matches!(c.role.to_lowercase().as_str(), "button" | "key" | "keyboard" | "generic");
        let single = c.name.trim().len() == 1 || c.text.trim().len() == 1;
        let looks_key = is_key_role || single || c.tag == "button" || c.text.len() <= 3;
        // viewport filter if provided
        if let Some(rect) = keyboard_rect {
            return looks_key && c.rect.overlaps(&rect);
        }
        looks_key
    });
    if keyboard_cands.is_empty() {
        // Fallback to all if filtering too aggressive
        keyboard_cands = all.clone();
        if let Some(rect) = keyboard_rect {
            keyboard_cands = keyboard_cands.filter_viewport(rect);
        }
    }
    // Numeric 1..N stable preserved via from_hint already; but we rebuilt via filtered which preserves ids
    // For keyboard overlay, we want to ensure numeric IDs are contiguous 1..N for the filtered set? But spec says numeric 1..N stable - filtered keeps original ids, which is good for mapping.
    // However visual annotation expects contiguous for legend; we keep original ids (non-contiguous is ok but legend shows gap). To make legend cleaner, renumber via from_vec with stable order? Spec says numeric 1..N stable never renumbered after Decider request - but for key_identify filtered set we can keep original ids.
    // Decide: keep original ids for traceability.

    // Heuristic single char exact before Decider
    let nq = key_query.trim();
    if nq.len() == 1 {
        if let Some(cand) = keyboard_cands.iter().find(|c| c.name.trim() == nq || c.text.trim() == nq || c.name.trim().to_lowercase() == nq.to_lowercase()).cloned() {
            let res = if !cand.label.is_empty() { hint::hint_click_with_target(&cand.label, target)? } else {
                let cx = cand.rect.x as f64 + cand.rect.width as f64/2.0;
                let cy = cand.rect.y as f64 + cand.rect.height as f64/2.0;
                let _ = crate::input::click(Some(cx), Some(cy), "left", false);
                json!({"clicked": true, "via": "rect_center"})
            };
            return Ok(json!({"success": true, "tier": "heuristic_keyboard", "candidate": {"id": cand.id, "label": cand.label, "name": cand.name}, "label": cand.label, "id": cand.id, "key": key_query, "result": res}));
        }
    }

    // Decider path same as hint_resolve but scoped to keyboard candidates
    let cfg = DeciderConfig::from_env();
    if !cfg.enabled {
        // Try hint heuristic fallback for keyboard
        if let Some(cand) = keyboard_cands.iter().find(|c| c.normalized_name().contains(&nq.to_lowercase()) || c.normalized_text().contains(&nq.to_lowercase())).cloned() {
            let res = if !cand.label.is_empty() { hint::hint_click_with_target(&cand.label, target)? } else { json!({}) };
            return Ok(json!({"success": true, "tier": "heuristic_keyboard_fallback", "candidate": {"id": cand.id, "label": cand.label}, "id": cand.id, "result": res}));
        }
        bail!("key_identify: {} and no heuristic match for key '{}'", crate::decider::config::off_reason(), key_query);
    }

    let max_per = if use_vision { VISION_MAX_OPTIONS } else { TEXT_MAX_OPTIONS };
    let image_opt = if use_vision {
        match prepare_annotated_image(&keyboard_cands, target, false) {
            Ok((b64,_)) => Some(b64),
            Err(_) => None,
        }
    } else { None };

    let decision = decider_select_with_candidates(&keyboard_cands, key_query, &format!("Identify keyboard key '{}'", key_query), max_per, use_vision, image_opt, target).await?;
    let choice = decision.choice.clone();
    if let Some(c) = decision.confidence { if c < LOW_CONF_THRESHOLD {
        return Ok(json!({"success": false, "status": "uncertain", "reason": "low confidence", "confidence": c, "choice": choice, "key": key_query}));
    }}
    let global = resolve_global_id(&keyboard_cands, &choice, max_per)?;
    let cand = keyboard_cands.get(global).ok_or_else(|| anyhow::anyhow!("key candidate {} not found", global))?;
    let res = if !cand.label.is_empty() { hint::hint_click_with_target(&cand.label, target)? } else {
        let cx = cand.rect.x as f64 + cand.rect.width as f64/2.0;
        let cy = cand.rect.y as f64 + cand.rect.height as f64/2.0;
        let _ = crate::input::click(Some(cx), Some(cy), "left", false);
        json!({"clicked": true, "via": "rect_center", "x": cx, "y": cy})
    };
    Ok(json!({
        "success": true,
        "tier": if use_vision { "decider_vision_keyboard" } else { "decider_text_keyboard" },
        "candidate": {"id": cand.id, "label": cand.label, "name": cand.name, "rect": {"x": cand.rect.x, "y": cand.rect.y, "width": cand.rect.width, "height": cand.rect.height}},
        "id": cand.id, "label": cand.label, "choice": choice, "confidence": decision.confidence, "probabilities": decision.probabilities,
        "key": key_query, "result": res
    }))
}

pub fn key_identify(key_query: &str, keyboard_rect: Option<Rect>, target: Option<&str>) -> Result<Value> {
    rt_block_on(key_identify_async(key_query, keyboard_rect, target, false))
}
pub fn key_identify_sync(key_query: &str, keyboard_rect: Option<Rect>, target: Option<&str>) -> Result<Value> {
    key_identify(key_query, keyboard_rect, target)
}
