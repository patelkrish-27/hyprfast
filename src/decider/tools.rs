//! Central semantic perception tools for Decider-2B.
//! All tools reuse perception resolver where possible and share one image pipeline
//! (`crate::decider::image` + `crate::screenshot` / `browser::screenshot_cdp`).
//! Fallback is deterministic (heuristic / DOM) when Decider disabled/unavailable.
//! Concurrency is bounded via `DeciderClient` semaphore (4).

use std::collections::HashMap;
use std::time::Duration;

use anyhow::{bail, Result};
use serde_json::{Value, json};

use crate::decider::candidates::{Candidates, Candidate, Rect, VISION_MAX_OPTIONS, TEXT_MAX_OPTIONS};
use crate::decider::client::DeciderClient;
use crate::decider::config::DeciderConfig;
use crate::decider::image::{ImageSource, capture_with_source, encode_base64, to_data_uri_auto};
use crate::decider::types::{DeciderQuestion, DeciderRequest};
use crate::perception::resolve::{ResolveContext, ResolveStatus, ResolveTier};
use crate::perception::verify::{VerifyStatus, VerifyResult};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn rt_block_on<F: std::future::Future>(f: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread().enable_all().build().expect("decider tools rt").block_on(f)
}

fn parse_options_value(v: &Value) -> Vec<String> {
    if let Some(arr) = v.as_array() {
        return arr.iter().filter_map(|x| {
            if let Some(s) = x.as_str() {
                Some(s.to_string())
            } else if x.is_number() || x.is_boolean() {
                Some(x.to_string())
            } else {
                None
            }
        }).collect();
    }
    if let Some(s) = v.as_str() {
        let s = s.trim();
        if s.starts_with('[') && s.ends_with(']') {
            if let Ok(parsed) = serde_json::from_str::<Vec<String>>(s) {
                return parsed;
            }
        }
        if s.contains(',') {
            return s.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect();
        }
        if !s.is_empty() {
            return vec![s.to_string()];
        }
    }
    vec![]
}

fn parse_questions(v: &Value) -> Result<Vec<DeciderQuestion>> {
    if let Some(arr) = v.as_array() {
        let mut out = Vec::new();
        for item in arr {
            if let Some(q) = item.get("question").or_else(|| item.get("query")).or_else(|| item.get("prompt")).and_then(|x| x.as_str()) {
                let mut opts = Vec::new();
                if let Some(o) = item.get("options") {
                    opts = parse_options_value(o);
                }
                if opts.is_empty() {
                    opts = vec!["yes".to_string(), "no".to_string(), "uncertain".to_string()];
                }
                out.push(DeciderQuestion::new(q.to_string(), opts));
            } else if let Some(s) = item.as_str() {
                if !s.trim().is_empty() {
                    out.push(DeciderQuestion::new(s.trim().to_string(), vec!["yes".to_string(), "no".to_string(), "uncertain".to_string()]));
                }
            }
        }
        if !out.is_empty() { return Ok(out); }
    }
    if let Some(qs) = v.get("questions").and_then(|x| x.as_array()) {
        return parse_questions(&Value::Array(qs.clone()));
    }
    bail!("questions must be an array of questions or question objects");
}

fn questions_from_json_args(args: &Value) -> Result<Vec<DeciderQuestion>> {
    // 1. Check if args.questions is provided
    if let Some(qs) = args.get("questions") {
        if let Some(s) = qs.as_str() {
            let s = s.trim();
            if let Ok(parsed) = serde_json::from_str::<Value>(s) {
                if let Ok(res) = parse_questions(&parsed) {
                    return Ok(res);
                }
            }
        } else if qs.is_array() {
            if let Ok(res) = parse_questions(qs) {
                return Ok(res);
            }
            let opts: Vec<String> = qs.as_array().unwrap().iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect();
            if !opts.is_empty() {
                let q_str = args.get("question").or_else(|| args.get("query")).or_else(|| args.get("prompt"))
                    .and_then(|v| v.as_str()).unwrap_or("Select the best option");
                return Ok(vec![DeciderQuestion::new(q_str.to_string(), opts)]);
            }
        }
    }

    // 2. Check single question / query / prompt / instruction
    let q_opt = args.get("question")
        .or_else(|| args.get("query"))
        .or_else(|| args.get("prompt"))
        .or_else(|| args.get("instruction"))
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let opts = args.get("options").map(parse_options_value).unwrap_or_default();

    if let Some(q) = q_opt {
        let final_opts = if !opts.is_empty() {
            opts
        } else {
            // Default to yes/no/uncertain (detect presence / boolean mode)
            vec!["yes".to_string(), "no".to_string(), "uncertain".to_string()]
        };
        return Ok(vec![DeciderQuestion::new(q, final_opts)]);
    }

    // 3. Options provided without explicit question -> classify state
    if !opts.is_empty() {
        return Ok(vec![DeciderQuestion::new("Classify the current state".to_string(), opts)]);
    }

    // 4. If args itself is an array of questions
    if args.is_array() {
        return parse_questions(args);
    }

    bail!("decide needs a question or query (e.g. \"Is the button visible?\" or --question \"...\" --options \"...\")");
}

fn image_from_args(args: &Value) -> Option<String> {
    let candidate = args.get("image")
        .or_else(|| args.get("image_base64"))
        .or_else(|| args.get("image_data_uri"))
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    if let Some(img_str) = candidate {
        let p = std::path::Path::new(&img_str);
        if p.exists() && p.is_file() {
            if let Ok(bytes) = std::fs::read(p) {
                return Some(to_data_uri_auto(&bytes));
            }
        }
        return Some(img_str);
    }

    let auto_screenshot = args.get("screenshot").and_then(|v| v.as_bool()).unwrap_or(false)
        || args.get("use_vision").and_then(|v| v.as_bool()).unwrap_or(false)
        || args.get("vision").and_then(|v| v.as_bool()).unwrap_or(false);

    if auto_screenshot {
        let target = target_from_args(args);
        if let Some(captured) = capture_image_for_vision(None, true, None, target.as_deref()) {
            return Some(captured);
        }
    }

    None
}

fn context_from_args(args: &Value) -> String {
    args.get("context").or_else(|| args.get("state")).and_then(|v| v.as_str()).unwrap_or("").to_string()
}

fn temperature_from_args(args: &Value) -> Option<f32> {
    args.get("temperature").and_then(|v| v.as_f64()).map(|x| x as f32)
}

fn target_from_args(args: &Value) -> Option<String> {
    args.get("target").or_else(|| args.get("target_id")).and_then(|v| v.as_str()).map(|s| s.to_string()).filter(|s| !s.trim().is_empty())
}

fn runner_up(probs: &HashMap<String,f64>, choice: &str) -> Option<f64> {
    let _top = *probs.get(choice)?;
    let mut best = 0.0; let mut found=false;
    for (k,v) in probs { if k!=choice && *v>best { best=*v; found=true; } }
    if found { Some(best) } else { None }
}
fn margin(conf: Option<f64>, ru: Option<f64>) -> Option<f64> {
    match (conf, ru) { (Some(c), Some(r)) => Some(c-r), _ => None }
}

fn capture_image_for_vision(cands: Option<&Candidates>, use_vision: bool, explicit_image: Option<String>, target: Option<&str>) -> Option<String> {
    if let Some(img) = explicit_image {
        if !img.trim().is_empty() { return Some(img); }
    }
    if !use_vision { return None; }
    // Try to capture via image pipeline deterministically
    let source = if target.is_some() { ImageSource::Browser } else { ImageSource::Browser };
    let try_capture = capture_with_source(&source, 0.5).or_else(|_| capture_with_source(&ImageSource::Monitor, 0.5));
    if let Ok((bytes, _meta)) = try_capture {
        if let Some(c) = cands {
            // Annotate with candidates legend and encode
            if let Ok((enc, _meta2)) = crate::decider::image::annotate_and_encode(bytes, c, true, true) {
                return Some(enc);
            }
        } else {
            return Some(to_data_uri_auto(&bytes));
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Low-level generic decide (Unified: single/multiple questions, options, detect, classify)
// ---------------------------------------------------------------------------

pub async fn decide_async(args: Value) -> Result<Value> {
    let context = {
        let c = context_from_args(&args);
        if c.is_empty() {
            args.get("question").or_else(|| args.get("query")).and_then(|v| v.as_str()).unwrap_or("").to_string()
        } else { c }
    };
    let questions = questions_from_json_args(&args)?;
    if questions.is_empty() { bail!("decide needs at least 1 question"); }
    for q in &questions { q.validate()?; }
    let image = image_from_args(&args);
    let temp = temperature_from_args(&args);
    // Validate image eagerly to surface encoding errors without network
    if let Some(ref img) = image {
        crate::decider::types::validate_image_field(img)?;
    }
    let cfg = DeciderConfig::from_env();
    // Build request
    let mut req = DeciderRequest::new(context.clone(), questions.clone());
    if let Some(img) = image.clone() { req = req.with_image(img); }
    if let Some(t) = temp { req = req.with_temperature(t); } else if cfg.temperature != 0.0 { req = req.with_temperature(cfg.temperature); }

    // Fallback when Decider is explicitly disabled or its health probe failed:
    // deterministic heuristic still works
    if !cfg.enabled {
        let off = crate::decider::config::off_reason();
        // Return deterministic response without network: choose first option with low confidence
        let decisions: Vec<Value> = questions.iter().enumerate().map(|(i, q)| {
            let choice = "1".to_string();
            let selected = q.options.first().cloned().unwrap_or_default();
            let is_yes_no = q.options.iter().any(|o| {
                let lo = o.to_lowercase();
                lo == "yes" || lo == "no"
            });
            let answer = if is_yes_no { Some("uncertain") } else { None };
            let mut d = json!({
                "choice": choice,
                "selected": selected,
                "confidence": 0.5,
                "probabilities": { "1": 0.5 },
                "question_index": i,
                "question": q.question,
                "fallback": format!("deterministic ({off})"),
                "options": q.options
            });
            if let Some(ans) = answer { d["answer"] = json!(ans); }
            d
        }).collect();
        let mut out = json!({
            "model": null, "device": null, "latency_ms": 0,
            "fallback": format!("{off} - deterministic"),
            "context": context,
            "decisions": decisions,
            "decisions_count": decisions.len()
        });
        if decisions.len() == 1 {
            let first = &decisions[0];
            out["question"] = first["question"].clone();
            out["selected"] = first["selected"].clone();
            out["choice"] = first["choice"].clone();
            out["confidence"] = first["confidence"].clone();
            out["probabilities"] = first["probabilities"].clone();
            out["options"] = first["options"].clone();
            if let Some(ans) = first.get("answer") { out["answer"] = ans.clone(); }
        }
        return Ok(out);
    }

    let client = DeciderClient::new(cfg)?;
    let t0 = std::time::Instant::now();
    match client.decide(&req).await {
        Ok(resp) => {
            let latency = resp.latency_ms.unwrap_or(t0.elapsed().as_millis() as u64);
            let mut decisions_json = Vec::new();
            for (i, dec) in resp.decisions.iter().enumerate() {
                let q = &questions[i];
                let choice = dec.choice.clone();
                let conf = dec.confidence;
                let probs = dec.probabilities.clone();
                let ru = probs.as_ref().and_then(|p| runner_up(p, &choice));
                let m = margin(conf, ru);
                let selected_text = q.choice_text(&choice).unwrap_or(&choice).to_string();
                let is_yes_no = q.options.iter().any(|o| {
                    let lo = o.to_lowercase();
                    lo == "yes" || lo == "no"
                });
                let answer = if is_yes_no {
                    let c = choice.to_lowercase();
                    if c == "1" || c.contains("yes") { Some("yes") }
                    else if c == "2" || c.contains("no") { Some("no") }
                    else { Some("uncertain") }
                } else { None };
                let mut d = json!({
                    "question_index": i,
                    "question": q.question,
                    "choice": choice,
                    "selected": selected_text,
                    "confidence": conf,
                    "runner_up": ru,
                    "margin": m,
                    "probabilities": probs,
                    "options": q.options
                });
                if let Some(ans) = answer {
                    d["answer"] = json!(ans);
                }
                decisions_json.push(d);
            }
            let mut out = json!({
                "model": resp.model,
                "device": resp.device,
                "latency_ms": latency,
                "usage": resp.usage,
                "context": context,
                "decisions": decisions_json,
                "decisions_count": decisions_json.len(),
                "raw": serde_json::to_value(&resp).unwrap_or(Value::Null)
            });
            if decisions_json.len() == 1 {
                let first = &decisions_json[0];
                out["question"] = first["question"].clone();
                out["selected"] = first["selected"].clone();
                out["choice"] = first["choice"].clone();
                out["confidence"] = first["confidence"].clone();
                out["runner_up"] = first["runner_up"].clone();
                out["margin"] = first["margin"].clone();
                out["probabilities"] = first["probabilities"].clone();
                out["options"] = first["options"].clone();
                if let Some(ans) = first.get("answer") {
                    out["answer"] = ans.clone();
                }
            }
            Ok(out)
        },
        Err(e) => {
            // Network failure: fallback deterministic still works
            let msg = e.to_string();
            let decisions: Vec<Value> = questions.iter().enumerate().map(|(i, q)| {
                let is_yes_no = q.options.iter().any(|o| {
                    let lo = o.to_lowercase();
                    lo == "yes" || lo == "no"
                });
                let mut d = json!({
                    "question_index": i,
                    "question": q.question,
                    "choice": "1",
                    "selected": q.options.first().cloned().unwrap_or_default(),
                    "confidence": 0.4,
                    "probabilities": {"1": 0.4},
                    "fallback": "decider unavailable",
                    "error": msg,
                    "options": q.options
                });
                if is_yes_no { d["answer"] = json!("uncertain"); }
                d
            }).collect();
            let mut out = json!({
                "model": null, "device": null, "latency_ms": t0.elapsed().as_millis() as u64,
                "fallback": "decider unavailable - deterministic",
                "error": msg,
                "context": context,
                "decisions": decisions,
                "decisions_count": decisions.len()
            });
            if decisions.len() == 1 {
                let first = &decisions[0];
                out["question"] = first["question"].clone();
                out["selected"] = first["selected"].clone();
                out["choice"] = first["choice"].clone();
                out["confidence"] = first["confidence"].clone();
                out["probabilities"] = first["probabilities"].clone();
                out["options"] = first["options"].clone();
                if let Some(ans) = first.get("answer") { out["answer"] = ans.clone(); }
            }
            Ok(out)
        }
    }
}

pub fn decide(args: Value) -> Result<Value> { rt_block_on(decide_async(args)) }

// ---------------------------------------------------------------------------
// decider_batch — multiple questions same context/screenshot or multiple requests
// ---------------------------------------------------------------------------

pub async fn decider_batch_async(args: Value) -> Result<Value> {
    // Accept either:
    //  - {context, questions:[{question,options}...], image} -> single request with N questions (same screenshot)
    //  - {requests:[{context, questions, image}...]} -> batched concurrent requests (bounded 4)
    if let Some(reqs) = args.get("requests").and_then(|v| v.as_array()) {
        // Multiple requests path: bounded concurrency via DeciderClient::decide_batch
        let mut requests: Vec<DeciderRequest> = Vec::new();
        for r in reqs {
            let ctx = r.get("context").or_else(|| r.get("state")).and_then(|v| v.as_str()).unwrap_or("").to_string();
            let qs = questions_from_json_args(r)?;
            let img = r.get("image").and_then(|v| v.as_str()).map(|s| s.to_string());
            let mut req = DeciderRequest::new(ctx, qs);
            if let Some(i) = img { if !i.trim().is_empty() { req = req.with_image(i); } }
            if let Some(t) = r.get("temperature").and_then(|v| v.as_f64()) { req = req.with_temperature(t as f32); }
            requests.push(req);
        }
        if requests.is_empty() { bail!("decider_batch needs at least 1 request"); }
        if requests.len() > 12 { bail!("decider_batch max 12 requests"); }
        let cfg = DeciderConfig::from_env();
        if !cfg.enabled {
            // deterministic fallback
            let off = crate::decider::config::off_reason();
            let results: Vec<Value> = requests.iter().enumerate().map(|(i, req)| {
                let decisions: Vec<Value> = req.questions.iter().enumerate().map(|(qi, q)| json!({"question_index": qi, "question": q.question, "choice":"1","selected": q.options.get(0).cloned().unwrap_or_default(), "confidence":0.5, "fallback": off.clone() })).collect();
                json!({"request_index": i, "context": req.context, "decisions": decisions, "fallback": off.clone()})
            }).collect();
            return Ok(json!({"results": results, "count": results.len(), "via":"decider_batch_disabled_fallback"}));
        }
        let client = DeciderClient::new(cfg)?;
        let t0 = std::time::Instant::now();
        let resps = client.decide_batch(requests.clone()).await;
        let mut results = Vec::new();
        for (i, r) in resps.into_iter().enumerate() {
            match r {
                Ok(resp) => {
                    let decisions: Vec<Value> = resp.decisions.iter().enumerate().map(|(qi, dec)| {
                        let q = &requests[i].questions[qi];
                        let ru = dec.probabilities.as_ref().and_then(|p| runner_up(p, &dec.choice));
                        let m = margin(dec.confidence, ru);
                        json!({"question_index": qi, "question": q.question, "choice": dec.choice, "selected": q.choice_text(&dec.choice).unwrap_or(&dec.choice).to_string(), "confidence": dec.confidence, "runner_up": ru, "margin": m, "probabilities": dec.probabilities, "options": q.options})
                    }).collect();
                    results.push(json!({"request_index": i, "context": requests[i].context, "model": resp.model, "device": resp.device, "latency_ms": resp.latency_ms, "decisions": decisions}));
                },
                Err(e) => {
                    let req = &requests[i];
                    let decisions: Vec<Value> = req.questions.iter().enumerate().map(|(qi, q)| json!({"question_index": qi, "question": q.question, "choice":"1","selected": q.options.get(0).cloned().unwrap_or_default(), "confidence":0.4, "error": e.to_string(), "fallback":"error"} )).collect();
                    results.push(json!({"request_index": i, "context": req.context, "error": e.to_string(), "decisions": decisions}));
                }
            }
        }
        return Ok(json!({"results": results, "count": results.len(), "elapsed_ms": t0.elapsed().as_millis() as u64, "via":"decider_batch"}));
    }
    // Single-request multi-question path (same context/screenshot) — delegate to decide
    decide_async(args).await
}

pub fn decider_batch(args: Value) -> Result<Value> { rt_block_on(decider_batch_async(args)) }

// ---------------------------------------------------------------------------
// find — semantic target resolver: query -> DOM/AX/hints -> candidate filtering -> Decider if ambiguous
// ---------------------------------------------------------------------------

pub async fn find_async(args: Value) -> Result<Value> {
    let query = args.get("query").or_else(|| args.get("instruction")).or_else(|| args.get("description")).and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    if query.is_empty() { bail!("find needs query"); }
    let target = target_from_args(&args);
    let use_vision = args.get("use_vision").or_else(|| args.get("vision")).and_then(|v| v.as_bool()).unwrap_or(false);
    let allow_fallback = args.get("allow_visual_fallback").and_then(|v| v.as_bool()).unwrap_or(true);
    let image = image_from_args(&args);
    // Collect candidates is inside resolve
    let mut ctx = ResolveContext::new(query.clone());
    if let Some(t) = target.clone() { ctx = ctx.with_target(t); }
    ctx.use_vision = use_vision;
    ctx.allow_visual_fallback = allow_fallback;
    if let Some(img) = image.clone() { ctx.image = Some(img); }
    if let Some(c) = args.get("context").and_then(|v| v.as_str()) { ctx.context = Some(c.to_string()); }
    // Optional viewport filter
    if let Some(vp) = args.get("viewport").and_then(|v| v.as_object()) {
        let x = vp.get("x").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
        let y = vp.get("y").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
        let w = vp.get("width").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
        let h = vp.get("height").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
        if w>0 && h>0 { ctx.viewport = Some(Rect::new(x,y,w,h)); }
    }
    let res = crate::perception::resolve::resolve_target_async(&query, &ctx).await?;
    let mut out = res.to_json();
    out["query"] = Value::String(query);
    if let Some(t) = target { out["target"] = Value::String(t); }
    // Also provide flat candidate metadata for ease of use
    if let Some(c) = res.candidate.clone() {
        out["candidate"] = json!({"id": c.id, "label": c.label, "tag": c.tag, "role": c.role, "name": c.name, "text": c.text, "selector": c.selector, "rect": {"x": c.rect.x, "y": c.rect.y, "width": c.rect.width, "height": c.rect.height}, "visible": c.visible, "enabled": c.enabled});
        out["rect"] = json!({"x": c.rect.x, "y": c.rect.y, "width": c.rect.width, "height": c.rect.height});
        out["selector"] = Value::String(c.selector);
    }
    out["success"] = Value::Bool(res.status == ResolveStatus::Success);
    Ok(out)
}

pub fn find(args: Value) -> Result<Value> { rt_block_on(find_async(args)) }

// ---------------------------------------------------------------------------
// choose — question+options[] up to 255, numeric IDs internally when visual candidates
// ---------------------------------------------------------------------------

pub async fn choose_async(args: Value) -> Result<Value> {
    let question = args.get("question").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    if question.is_empty() { bail!("choose needs question"); }
    let opts: Vec<String> = if let Some(a) = args.get("options").and_then(|v| v.as_array()) {
        a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect()
    } else if let Some(s) = args.get("options").and_then(|v| v.as_str()) {
        serde_json::from_str::<Vec<String>>(s).unwrap_or_default()
    } else { vec![] };
    if opts.is_empty() { bail!("choose needs options[] (1..255)"); }
    if opts.len() > 255 { bail!("choose options exceed 255 (got {})", opts.len()); }
    for (i, o) in opts.iter().enumerate() { if o.trim().is_empty() { bail!("choose option {} is empty", i+1); } }
    let context = args.get("context").or_else(|| args.get("state")).and_then(|v| v.as_str()).unwrap_or(&question).to_string();
    let use_vision = args.get("use_vision").or_else(|| args.get("vision")).and_then(|v| v.as_bool()).unwrap_or(false);
    let image = image_from_args(&args);
    let target = target_from_args(&args);

    // If candidates provided (visual candidates numeric IDs), map options to candidate IDs
    // candidates: [{id, label, rect, ...}] — when present, we treat options as candidate display texts and preserve numeric ID mapping
    let candidates_value = args.get("candidates").or_else(|| args.get("hints"));
    let cands = if let Some(v) = candidates_value {
        // Try to build Candidates from that value
        let c = Candidates::from_hint_snapshot(v, None, None, None);
        if c.len() > 0 { Some(c) } else { None }
    } else { None };

    // Build DeciderRequest with single question using numeric IDs "1".."N" internally (DeciderQuestion handles this)
    let dq = DeciderQuestion::new(question.clone(), opts.clone());
    dq.validate()?;
    let cfg = DeciderConfig::from_env();
    let mut req = DeciderRequest::new(context.clone(), vec![dq]);
    // Image handling: if use_vision and candidates present, annotate
    if use_vision {
        if let Some(c) = &cands {
            if let Some(img) = capture_image_for_vision(Some(c), true, image.clone(), target.as_deref()) {
                req = req.with_image(img);
            } else if let Some(img) = image.clone() {
                req = req.with_image(img);
            }
        } else if let Some(img) = image.clone() {
            req = req.with_image(img);
        } else if let Some(img) = capture_image_for_vision(None, true, None, target.as_deref()) {
            req = req.with_image(img);
        }
    } else if let Some(img) = image.clone() {
        req = req.with_image(img);
    }
    if let Some(t) = temperature_from_args(&args) { req = req.with_temperature(t); }

    if !cfg.enabled {
        // Deterministic fallback: first option
        return Ok(json!({
            "question": question, "options": opts,
            "selected": opts.get(0).cloned().unwrap_or_default(),
            "choice": "1", "choice_id": "1",
            "confidence": 0.5, "runner_up": Value::Null, "margin": Value::Null,
            "probabilities": {"1": 0.5},
            "fallback": format!("{} - deterministic", crate::decider::config::off_reason()),
            "context": context
        }));
    }
    let client = DeciderClient::new(cfg)?;
    let resp = client.decide(&req).await;
    match resp {
        Ok(r) => {
            let dec = r.decisions.into_iter().next().ok_or_else(|| anyhow::anyhow!("empty decisions"))?;
            let choice = dec.choice.clone();
            let conf = dec.confidence;
            let probs = dec.probabilities.clone();
            let ru = probs.as_ref().and_then(|p| runner_up(p, &choice));
            let m = margin(conf, ru);
            let idx = choice.parse::<usize>().ok();
            let selected = idx.and_then(|i| if i>=1 && i<=opts.len() { Some(opts[i-1].clone()) } else { None }).unwrap_or_else(|| choice.clone());
            // If candidates mapping, also resolve candidate id
            let candidate_meta = if let (Some(c), Some(i)) = (&cands, idx) {
                if i>=1 && i<=c.len() {
                    let cand = c.as_slice()[i-1].clone();
                    Some(json!({"id": cand.id, "label": cand.label, "rect": {"x": cand.rect.x, "y": cand.rect.y, "width": cand.rect.width, "height": cand.rect.height}, "selector": cand.selector, "name": cand.name}))
                } else { None }
            } else { None };
            Ok(json!({
                "question": question, "options": opts,
                "selected": selected,
                "choice": choice, "choice_id": choice,
                "confidence": conf, "runner_up": ru, "margin": m,
                "probabilities": probs,
                "model": r.model, "device": r.device, "latency_ms": r.latency_ms,
                "candidate": candidate_meta,
                "context": context
            }))
        },
        Err(e) => {
            // fallback deterministic
            Ok(json!({
                "question": question, "options": opts,
                "selected": opts.get(0).cloned().unwrap_or_default(),
                "choice": "1", "choice_id": "1",
                "confidence": 0.4, "probabilities": {"1": 0.4},
                "fallback": "decider unavailable",
                "error": e.to_string(),
                "context": context
            }))
        }
    }
}

pub fn choose(args: Value) -> Result<Value> { rt_block_on(choose_async(args)) }

// ---------------------------------------------------------------------------
// classify — state classification from explicit options, image optional
// ---------------------------------------------------------------------------

pub async fn classify_async(args: Value) -> Result<Value> {
    // classify is alias for choose with maybe different prompt wording
    let mut a = args.clone();
    // Ensure question present
    if a.get("question").is_none() {
        if let Some(q) = a.get("query").and_then(|v| v.as_str()) { a["question"] = Value::String(q.to_string()); }
        else if let Some(q) = a.get("prompt").and_then(|v| v.as_str()) { a["question"] = Value::String(q.to_string()); }
        else { a["question"] = Value::String("Classify the current state".to_string()); }
    }
    choose_async(a).await
}
pub fn classify(args: Value) -> Result<Value> { rt_block_on(classify_async(args)) }

// ---------------------------------------------------------------------------
// detect — presence/absence YES/NO/UNCERTAIN
// ---------------------------------------------------------------------------

pub async fn detect_async(args: Value) -> Result<Value> {
    let query = args.get("query").or_else(|| args.get("question")).or_else(|| args.get("prompt")).and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    if query.is_empty() { bail!("detect needs query"); }
    let context = context_from_args(&args);
    let image = image_from_args(&args);
    let target = target_from_args(&args);
    // Detect via Decider with options yes/no/uncertain
    let prompt = format!("Is '{}' present/visible? Answer yes if present, no if absent, uncertain if unclear.", query);
    let question = DeciderQuestion::new(prompt.clone(), vec!["yes".into(), "no".into(), "uncertain".into()]);
    let ctx_str = if context.is_empty() { query.clone() } else { context.clone() };
    let mut req = DeciderRequest::new(ctx_str.clone(), vec![question]);
    if let Some(img) = image.clone() { req = req.with_image(img); }
    else if args.get("use_vision").and_then(|v| v.as_bool()).unwrap_or(false) {
        if let Some(img) = capture_image_for_vision(None, true, None, target.as_deref()) {
            req = req.with_image(img);
        }
    }
    let cfg = DeciderConfig::from_env();
    if !cfg.enabled {
        return Ok(json!({"query": query, "answer": "uncertain", "confidence": 0.5, "fallback": format!("{} (deterministic)", crate::decider::config::off_reason())}));
    }
    let client = DeciderClient::new(cfg)?;
    match client.decide(&req).await {
        Ok(resp) => {
            let dec = resp.decisions.into_iter().next().unwrap();
            let choice = dec.choice.to_lowercase();
            let answer = if choice=="1" || choice.contains("yes") { "yes" } else if choice=="2" || choice.contains("no") { "no" } else { "uncertain" };
            let ru = dec.probabilities.as_ref().and_then(|p| runner_up(p, &dec.choice));
            let m = margin(dec.confidence, ru);
            Ok(json!({"query": query, "answer": answer, "choice": dec.choice, "confidence": dec.confidence, "runner_up": ru, "margin": m, "probabilities": dec.probabilities, "model": resp.model, "device": resp.device, "latency_ms": resp.latency_ms}))
        },
        Err(e) => {
            Ok(json!({"query": query, "answer": "uncertain", "error": e.to_string(), "fallback":"error"}))
        }
    }
}
pub fn detect(args: Value) -> Result<Value> { rt_block_on(detect_async(args)) }

// ---------------------------------------------------------------------------
// identify — which candidate/entity
// ---------------------------------------------------------------------------

pub async fn identify_async(args: Value) -> Result<Value> {
    // Identify is like find but returns candidate enumeration + selected
    // Accept query + candidates array or rely on hint snapshot
    let query = args.get("query").or_else(|| args.get("question")).and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    if query.is_empty() { bail!("identify needs query"); }
    let target = target_from_args(&args);
    let use_vision = args.get("use_vision").and_then(|v| v.as_bool()).unwrap_or(false);
    let image = image_from_args(&args);
    let candidates_val = args.get("candidates").or_else(|| args.get("hints"));

    let cands = if let Some(v) = candidates_val {
        Candidates::from_hint_snapshot(v, None, None, None)
    } else {
        // collect via hint_snapshot with target
        let hv = crate::hint::hint_snapshot_with_target(target.as_deref()).unwrap_or(json!({"hints":[]}));
        Candidates::from_hint_snapshot(&hv, None, None, None)
    };
    if cands.is_empty() {
        // fallback to find (which does resolve with visual fallback)
        return find_async(args).await;
    }
    // Use choose with candidate options
    let opts: Vec<String> = cands.iter().map(|c| c.display_text()).collect();
    let mut choose_args = json!({"question": query, "options": opts, "context": query, "use_vision": use_vision});
    if let Some(img) = image.clone() { choose_args["image"] = Value::String(img); }
    if let Some(t) = target.clone() { choose_args["target"] = Value::String(t); }
    // attach candidates for mapping
    choose_args["candidates"] = serde_json::to_value(cands.as_slice()).unwrap_or(Value::Null);
    let res = choose_async(choose_args).await?;
    // Enrich with enumerated candidates
    let mut out = res;
    out["query"] = Value::String(query);
    out["candidates_count"] = json!(cands.len());
    out["candidates"] = serde_json::to_value(cands.as_slice()).unwrap_or(Value::Null);
    Ok(out)
}
pub fn identify(args: Value) -> Result<Value> { rt_block_on(identify_async(args)) }

// ---------------------------------------------------------------------------
// visual_target — description+image+candidate rects/metadata -> selected candidate ID/confidence/probs/rect
// ---------------------------------------------------------------------------

pub async fn visual_target_async(args: Value) -> Result<Value> {
    let description = args.get("description").or_else(|| args.get("query")).or_else(|| args.get("instruction")).and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    if description.is_empty() { bail!("visual_target needs description"); }
    let candidates_val = args.get("candidates").or_else(|| args.get("hints")).or_else(|| args.get("rects"));
    if candidates_val.is_none() { bail!("visual_target needs candidates [{{id,label,rect}}]"); }
    let cands = Candidates::from_hint_snapshot(candidates_val.unwrap(), None, None, None);
    if cands.is_empty() { bail!("visual_target: no candidates parsed"); }
    if cands.len() > VISION_MAX_OPTIONS {
        // hierarchical still applies but visual_target spec says Vision 10 options, so warn but chunk
        // We will hierarchically select via decider
    }
    let image = image_from_args(&args);
    let target = target_from_args(&args);
    // Capture/annotate if image not provided
    let effective_image = if let Some(img) = image.clone() { Some(img) }
    else {
        capture_image_for_vision(Some(&cands), true, None, target.as_deref())
    };
    if effective_image.is_none() { bail!("visual_target needs image (provide base64/data URI) or ensure screenshot available"); }
    let context = args.get("context").and_then(|v| v.as_str()).unwrap_or(&description).to_string();
    // Build vision question: description is the query, options are vision_text of candidates
    let use_vision = true;
    let max_per = VISION_MAX_OPTIONS;
    let ctx = ResolveContext {
        query: description.clone(),
        target: target.clone(),
        candidates: Some(cands.clone()),
        use_vision,
        viewport: None,
        annotate: true,
        image: effective_image.clone(),
        context: Some(context.clone()),
        min_geometry: None,
        allow_visual_fallback: false,
    };
    // For single page case (<=10) directly call decider
    let effective = cands.filtered_for_budget(None, None);
    let effective2 = if effective.is_empty() { cands.clone() } else { effective };
    let cfg = DeciderConfig::from_env();
    if !cfg.enabled {
        // deterministic: pick first candidate
        let first = effective2.iter().next().unwrap().clone();
        return Ok(json!({
            "description": description,
            "selected_id": first.id, "selected_label": first.label,
            "confidence": 0.5, "probabilities": {"1": 0.5},
            "candidate": {"id": first.id, "label": first.label, "rect": {"x": first.rect.x, "y": first.rect.y, "width": first.rect.width, "height": first.rect.height}, "selector": first.selector},
            "rect": {"x": first.rect.x, "y": first.rect.y, "width": first.rect.width, "height": first.rect.height},
            "fallback": format!("{} - deterministic", crate::decider::config::off_reason()),
            "candidates_count": effective2.len(),
            "vision_budget": VISION_MAX_OPTIONS
        }));
    }
    // Use decider_select path via resolve_target
    let res = crate::perception::resolve::resolve_target_async(&description, &ctx).await?;
    if res.status == ResolveStatus::Success {
        if let Some(cand) = res.candidate.clone() {
            return Ok(json!({
                "description": description,
                "selected_id": cand.id, "selected_label": cand.label,
                "confidence": res.confidence, "runner_up": res.runner_up, "margin": res.margin,
                "probabilities": res.probabilities,
                "candidate": {"id": cand.id, "label": cand.label, "rect": {"x": cand.rect.x, "y": cand.rect.y, "width": cand.rect.width, "height": cand.rect.height}, "selector": cand.selector, "name": cand.name, "role": cand.role, "tag": cand.tag},
                "rect": {"x": cand.rect.x, "y": cand.rect.y, "width": cand.rect.width, "height": cand.rect.height},
                "tier": res.tier.as_str(),
                "candidates_count": effective2.len(),
                "vision_budget": VISION_MAX_OPTIONS
            }));
        }
    }
    // Uncertain
    Ok(json!({
        "description": description,
        "status": res.status.as_str(),
        "tier": res.tier.as_str(),
        "confidence": res.confidence, "runner_up": res.runner_up, "margin": res.margin,
        "probabilities": res.probabilities,
        "meta": res.meta,
        "candidates_count": effective2.len(),
        "fallback": "uncertain"
    }))
}
pub fn visual_target(args: Value) -> Result<Value> { rt_block_on(visual_target_async(args)) }

// ---------------------------------------------------------------------------
// verify wrappers (expose perception::verify as tools)
// ---------------------------------------------------------------------------

pub async fn verify_async_tool(args: Value) -> Result<Value> {
    // Run sync version in blocking thread to avoid nested runtime panic (verify_async internally uses blocking CDP).
    let args2 = args.clone();
    tokio::task::spawn_blocking(move || verify(args2)).await.unwrap()
}
pub fn verify(args: Value) -> Result<Value> {
    let query = args.get("query").or_else(|| args.get("question")).and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    if query.is_empty() { bail!("verify needs query"); }
    let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| crate::perception::verify::verify(&query)));
    match res {
        Ok(Ok(r)) => {
            let mut j = r.to_json();
            j["query"] = Value::String(query);
            Ok(j)
        },
        Ok(Err(e)) => Ok(json!({"query": query, "status": "uncertain", "confidence": Value::Null, "fallback": "verify error", "error": e.to_string()})),
        Err(_) => Ok(json!({"query": query, "status": "uncertain", "fallback": "verify panic (nested runtime) - deterministic fallback"})),
    }
}

pub async fn verify_element_async_tool(args: Value) -> Result<Value> {
    let args2 = args.clone();
    tokio::task::spawn_blocking(move || verify_element(args2)).await.unwrap()
}
pub fn verify_element(args: Value) -> Result<Value> {
    let cand = if let Some(c) = args.get("candidate") {
        let v = c.clone();
        let id = v.get("id").and_then(|x| x.as_u64()).unwrap_or(0) as u32;
        let label = v.get("label").and_then(|x| x.as_str()).unwrap_or("").to_string();
        let selector = v.get("selector").and_then(|x| x.as_str()).unwrap_or("").to_string();
        let name = v.get("name").and_then(|x| x.as_str()).unwrap_or("").to_string();
        let rect = if let Some(r) = v.get("rect") {
            Rect::new(r.get("x").and_then(|x| x.as_i64()).unwrap_or(0) as i32, r.get("y").and_then(|x| x.as_i64()).unwrap_or(0) as i32, r.get("width").and_then(|x| x.as_i64()).unwrap_or(0) as i32, r.get("height").and_then(|x| x.as_i64()).unwrap_or(0) as i32)
        } else { Rect::new(0,0,0,0) };
        Candidate { id, label, tag: v.get("tag").and_then(|x| x.as_str()).unwrap_or("").to_string(), role: v.get("role").and_then(|x| x.as_str()).unwrap_or("").to_string(), name: name.clone(), text: v.get("text").and_then(|x| x.as_str()).unwrap_or(&name).to_string(), rect, visible: v.get("visible").and_then(|x| x.as_bool()).unwrap_or(true), enabled: v.get("enabled").and_then(|x| x.as_bool()).unwrap_or(true), selector, target_id: None, url: None, title: None }
    } else if let Some(sel) = args.get("selector").and_then(|v| v.as_str()) {
        Candidate { id: 1, label: "".into(), tag: "".into(), role: "".into(), name: sel.to_string(), text: sel.to_string(), rect: Rect::new(0,0,0,0), visible: true, enabled: true, selector: sel.to_string(), target_id: None, url: None, title: None }
    } else {
        bail!("verify_element needs candidate {{id,label,selector,rect}} or selector");
    };
    let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| crate::perception::verify::verify_element(&cand)));
    match res {
        Ok(Ok(r)) => {
            let mut j = r.to_json();
            j["candidate_id"] = json!(cand.id);
            j["selector"] = Value::String(cand.selector);
            Ok(j)
        },
        Ok(Err(e)) => Ok(json!({"candidate_id": cand.id, "selector": cand.selector, "status": "uncertain", "fallback": "verify_element error", "error": e.to_string()})),
        Err(_) => Ok(json!({"candidate_id": cand.id, "selector": cand.selector, "status": "uncertain", "fallback": "panic - deterministic"})),
    }
}

pub async fn verify_action_async_tool(args: Value) -> Result<Value> {
    let args2 = args.clone();
    tokio::task::spawn_blocking(move || verify_action(args2)).await.unwrap()
}
pub fn verify_action(args: Value) -> Result<Value> {
    let query = args.get("query").or_else(|| args.get("action")).and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    if query.is_empty() { bail!("verify_action needs query"); }
    let expected = args.get("expected").and_then(|v| v.as_str()).map(|s| s.to_string());
    let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| crate::perception::verify::verify_action(&query, expected.as_deref())));
    match res {
        Ok(Ok(r)) => {
            let mut j = r.to_json();
            j["query"] = Value::String(query);
            if let Some(e) = expected.clone() { j["expected"] = Value::String(e); }
            Ok(j)
        },
        Ok(Err(e)) => Ok(json!({"query": query, "status": "uncertain", "fallback": "verify_action error", "error": e.to_string()})),
        Err(_) => Ok(json!({"query": query, "status": "uncertain", "fallback": "panic - deterministic"})),
    }
}

pub async fn wait_until_async_tool(args: Value) -> Result<Value> {
    let args2 = args.clone();
    tokio::task::spawn_blocking(move || wait_until(args2)).await.unwrap()
}
pub fn wait_until(args: Value) -> Result<Value> {
    let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    if query.is_empty() { bail!("wait_until needs query"); }
    let timeout_ms = args.get("timeout_ms").or_else(|| args.get("timeout")).and_then(|v| v.as_u64()).unwrap_or(5000);
    let interval_ms = args.get("interval_ms").or_else(|| args.get("interval")).and_then(|v| v.as_u64()).unwrap_or(200);
    let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| crate::perception::verify::wait_until(&query, Duration::from_millis(timeout_ms), Duration::from_millis(interval_ms))));
    match res {
        Ok(Ok(r)) => {
            let mut j = r.to_json();
            j["query"] = Value::String(query);
            j["timeout_ms"] = json!(timeout_ms);
            j["interval_ms"] = json!(interval_ms);
            Ok(j)
        },
        Ok(Err(e)) => Ok(json!({"query": query, "status": "failure", "fallback": "wait_until error", "error": e.to_string(), "timeout_ms": timeout_ms})),
        Err(_) => Ok(json!({"query": query, "status": "uncertain", "fallback": "panic - deterministic", "timeout_ms": timeout_ms})),
    }
}

pub async fn observe_state_async(args: Value) -> Result<Value> {
    // observe_state: state classification from explicit options, image optional — similar to classify but for UI state
    // accept query + options, optional image
    let query = args.get("query").or_else(|| args.get("question")).and_then(|v| v.as_str()).unwrap_or("describe current state").to_string();
    let opts: Vec<String> = if let Some(a) = args.get("options").and_then(|v| v.as_array()) {
        a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect()
    } else if let Some(a) = args.get("states").and_then(|v| v.as_array()) {
        a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect()
    } else { vec!["default".into()] };
    let mut ca = json!({"question": query, "options": opts});
    if let Some(c) = args.get("context").and_then(|v| v.as_str()) { ca["context"] = Value::String(c.to_string()); }
    if let Some(img) = image_from_args(&args) { ca["image"] = Value::String(img); }
    if let Some(t) = args.get("target").and_then(|v| v.as_str()) { ca["target"] = Value::String(t.to_string()); }
    if let Some(b) = args.get("use_vision").and_then(|v| v.as_bool()) { ca["use_vision"] = Value::Bool(b); }
    classify_async(ca).await
}
pub fn observe_state(args: Value) -> Result<Value> { rt_block_on(observe_state_async(args)) }

// ---------------------------------------------------------------------------
// hint_resolve / hint_resolve_batch / key_identify (expose existing)
// ---------------------------------------------------------------------------

pub async fn hint_resolve_async_tool(args: Value) -> Result<Value> {
    let instruction = args.get("instruction").or_else(|| args.get("query")).and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    if instruction.is_empty() { bail!("hint_resolve needs instruction"); }
    let target = target_from_args(&args);
    let use_vision = args.get("use_vision").or_else(|| args.get("vision")).and_then(|v| v.as_bool()).unwrap_or(false);
    crate::decider::hint_resolve::hint_resolve_async(&instruction, target.as_deref(), use_vision).await
}
pub fn hint_resolve(args: Value) -> Result<Value> { rt_block_on(hint_resolve_async_tool(args)) }

pub async fn hint_resolve_batch_async_tool(args: Value) -> Result<Value> {
    let instructions: Vec<String> = if let Some(a) = args.get("instructions").and_then(|v| v.as_array()) {
        a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect()
    } else if let Some(a) = args.get("steps").and_then(|v| v.as_array()) {
        a.iter().filter_map(|x| x.get("instruction").and_then(|v| v.as_str()).map(|s| s.to_string())).collect()
    } else if let Some(arr) = args.as_array() {
        arr.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect()
    } else { vec![] };
    if instructions.is_empty() { bail!("hint_resolve_batch needs instructions[]"); }
    let target = target_from_args(&args);
    let use_vision = args.get("use_vision").or_else(|| args.get("vision")).and_then(|v| v.as_bool()).unwrap_or(false);
    crate::decider::hint_resolve::hint_resolve_batch_async(&instructions, target.as_deref(), use_vision).await
}
pub fn hint_resolve_batch(args: Value) -> Result<Value> { rt_block_on(hint_resolve_batch_async_tool(args)) }

pub async fn key_identify_async_tool(args: Value) -> Result<Value> {
    let key = args.get("key").or_else(|| args.get("query")).and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    if key.is_empty() { bail!("key_identify needs key"); }
    let target = target_from_args(&args);
    let use_vision = args.get("use_vision").and_then(|v| v.as_bool()).unwrap_or(false);
    let rect = if let Some(r) = args.get("rect").or_else(|| args.get("keyboard_rect")) {
        let x = r.get("x").and_then(|x| x.as_i64()).unwrap_or(0) as i32;
        let y = r.get("y").and_then(|x| x.as_i64()).unwrap_or(0) as i32;
        let w = r.get("width").and_then(|x| x.as_i64()).unwrap_or(0) as i32;
        let h = r.get("height").and_then(|x| x.as_i64()).unwrap_or(0) as i32;
        if w>0 && h>0 { Some(Rect::new(x,y,w,h)) } else { None }
    } else { None };
    crate::decider::hint_resolve::key_identify_async(&key, rect, target.as_deref(), use_vision).await
}
pub fn key_identify(args: Value) -> Result<Value> { rt_block_on(key_identify_async_tool(args)) }

// ---------------------------------------------------------------------------
// Composites: find_and_click, find_and_type
// ---------------------------------------------------------------------------

pub async fn find_and_click_async(args: Value) -> Result<Value> {
    let query = args.get("query").or_else(|| args.get("instruction")).and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    if query.is_empty() { bail!("find_and_click needs query"); }
    let target = target_from_args(&args);
    let use_vision = args.get("use_vision").and_then(|v| v.as_bool()).unwrap_or(false);
    let image = image_from_args(&args);
    let mut find_args = json!({"query": query, "use_vision": use_vision});
    if let Some(t) = target.clone() { find_args["target"] = Value::String(t); }
    if let Some(img) = image.clone() { find_args["image"] = Value::String(img); }
    let found = find_async(find_args).await?;
    if found.get("success").and_then(|v| v.as_bool()).unwrap_or(false) {
        let candidate = found.get("candidate").cloned().unwrap_or(Value::Null);
        let selector = candidate.get("selector").and_then(|v| v.as_str()).unwrap_or("");
        let label = candidate.get("label").and_then(|v| v.as_str()).unwrap_or("");
        let rect = candidate.get("rect").cloned().unwrap_or(Value::Null);
        // Try hint_click first if label present
        let click_res = if !label.is_empty() {
            crate::hint::hint_click_with_target(label, target.as_deref()).unwrap_or_else(|e| json!({"error": e.to_string()}))
        } else if !selector.is_empty() {
            crate::browser::click_by_selector(selector).unwrap_or_else(|e| json!({"error": e.to_string()}))
        } else if rect.get("x").is_some() {
            let x = rect.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let y = rect.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let w = rect.get("width").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let h = rect.get("height").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let cx = x + w/2.0; let cy = y + h/2.0;
            crate::input::click(Some(cx), Some(cy), "left", false).map(|_| json!({"clicked": true, "x": cx, "y": cy})).unwrap_or_else(|e| json!({"error": e.to_string()}))
        } else {
            json!({"error": "no selector/label/rect to click"})
        };
        Ok(json!({"query": query, "found": found, "clicked": click_res, "success": true}))
    } else {
        Ok(json!({"query": query, "found": found, "success": false, "error": "find failed or uncertain, not clicked"}))
    }
}
pub fn find_and_click(args: Value) -> Result<Value> { rt_block_on(find_and_click_async(args)) }

pub async fn find_and_type_async(args: Value) -> Result<Value> {
    let query = args.get("query").or_else(|| args.get("instruction")).and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    let text = args.get("text").and_then(|v| v.as_str()).unwrap_or("").to_string();
    if query.is_empty() { bail!("find_and_type needs query"); }
    if text.is_empty() { bail!("find_and_type needs text"); }
    let target = target_from_args(&args);
    let use_vision = args.get("use_vision").and_then(|v| v.as_bool()).unwrap_or(false);
    let mut find_args = json!({"query": query, "use_vision": use_vision});
    if let Some(t) = target.clone() { find_args["target"] = Value::String(t); }
    let found = find_async(find_args).await?;
    if found.get("success").and_then(|v| v.as_bool()).unwrap_or(false) {
        let candidate = found.get("candidate").cloned().unwrap_or(Value::Null);
        let label = candidate.get("label").and_then(|v| v.as_str()).unwrap_or("");
        let selector = candidate.get("selector").and_then(|v| v.as_str()).unwrap_or("");
        let type_res = if !label.is_empty() {
            crate::hint::hint_type_with_target(label, &text, target.as_deref()).unwrap_or_else(|e| json!({"error": e.to_string()}))
        } else if !selector.is_empty() {
            crate::browser::type_text(selector, &text, false, Some(selector)).unwrap_or_else(|e| json!({"error": e.to_string()}))
        } else {
            // fallback: click center then type
            let rect = candidate.get("rect").cloned().unwrap_or(Value::Null);
            let x = rect.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let y = rect.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let w = rect.get("width").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let h = rect.get("height").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let cx = x + w/2.0; let cy = y + h/2.0;
            let _ = crate::input::click(Some(cx), Some(cy), "left", false);
            std::thread::sleep(std::time::Duration::from_millis(80));
            crate::input::type_text(&text).map(|_| json!({"typed": text.len()})).unwrap_or_else(|e| json!({"error": e.to_string()}))
        };
        Ok(json!({"query": query, "text": text, "found": found, "typed": type_res, "success": true}))
    } else {
        Ok(json!({"query": query, "text": text, "found": found, "success": false, "error": "find failed or uncertain, not typed"}))
    }
}
pub fn find_and_type(args: Value) -> Result<Value> { rt_block_on(find_and_type_async(args)) }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unified_decide_single_question_choose() {
        // Routing is health-gated, so this runs against the live daemon when one
        // is up and against the deterministic fallback when it is not. Assert
        // the invariants that hold in BOTH modes, not one mode's answer.
        let options = ["Continue Shopping", "Proceed to Checkout", "Cancel"];
        let args = json!({
            "question": "Which button proceeds to payment?",
            "options": options
        });
        let res = decide(args).expect("decide should succeed");
        let selected = res["selected"].as_str().expect("selected string");
        assert!(options.contains(&selected), "selected {selected:?} not in options");
        let choice = res["choice"].as_str().expect("choice string");
        assert!(["1", "2", "3"].contains(&choice), "choice {choice:?} out of range");
        assert!(res["confidence"].as_f64().is_some());
        assert!(res["decisions"].is_array());
        assert_eq!(res["decisions"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn test_unified_decide_comma_separated_options() {
        let args = json!({
            "question": "Which action should be taken?",
            "options": "Save, Discard, Cancel"
        });
        let res = decide(args).expect("decide should succeed");
        assert_eq!(res["selected"], "Save");
        let opts = res["options"].as_array().unwrap();
        assert_eq!(opts.len(), 3);
        assert_eq!(opts[0], "Save");
        assert_eq!(opts[1], "Discard");
        assert_eq!(opts[2], "Cancel");
    }

    #[test]
    fn test_unified_decide_auto_detect() {
        // No options provided: automatically acts as detect (yes/no/uncertain)
        let args = json!({
            "question": "Is the error banner visible?"
        });
        let res = decide(args).expect("decide should succeed");
        assert!(res["answer"].is_string());
        let opts = res["options"].as_array().unwrap();
        let expected = json!(["yes", "no", "uncertain"]);
        assert_eq!(opts, expected.as_array().unwrap());
    }

    #[test]
    fn test_unified_decide_classify() {
        // Options provided without question: defaults to "Classify the current
        // state". The selected option is daemon-dependent (health-gated), so
        // only the default question and option membership are asserted.
        let options = ["logged_out", "login_screen", "dashboard"];
        let args = json!({
            "options": options
        });
        let res = decide(args).expect("decide should succeed");
        assert_eq!(res["question"], "Classify the current state");
        let selected = res["selected"].as_str().expect("selected string");
        assert!(options.contains(&selected), "selected {selected:?} not in options");
    }

    #[test]
    fn test_unified_decide_batch_multiple_questions() {
        // Multiple questions in one call
        let args = json!({
            "questions": [
                {"question": "Is dark mode enabled?", "options": ["yes", "no"]},
                {"question": "Which tab is focused?", "options": ["Profile", "Security", "Billing"]}
            ]
        });
        let res = decide(args).expect("decide should succeed");
        let decisions = res["decisions"].as_array().unwrap();
        assert_eq!(decisions.len(), 2);
        assert_eq!(decisions[0]["question"], "Is dark mode enabled?");
        assert_eq!(decisions[1]["question"], "Which tab is focused?");
    }

    #[test]
    fn test_unified_image_file_path() {
        // Create a temporary file and verify image_from_args detects and encodes it
        let tmp = std::env::temp_dir().join("hyprfast_test_unified_image.png");
        std::fs::write(&tmp, b"dummy png content").unwrap();
        let args = json!({
            "question": "Is image present?",
            "image": tmp.to_str().unwrap()
        });
        let img = image_from_args(&args);
        assert!(img.is_some());
        let uri = img.unwrap();
        assert!(uri.starts_with("data:image/"));
        let _ = std::fs::remove_file(tmp);
    }
}


