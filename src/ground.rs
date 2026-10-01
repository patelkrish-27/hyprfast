//! Fast visual grounding — Astra-like click-point detection.
//! Screenshot JPEG 0.5x -> Gemini Flash vision -> {x,y} 0-1000 normalized -> global logical coords.
//! Single fused `act_fast` does ground+click/type in one MCP call (no N LLM turns).
//! OS pointer via `input::click` = trusted gesture, works on canvas/draw/color-pickers where
//! AT-SPI (`src/a11y`) and CDP AX (Accessibility.getFullAXTree) have no tree.
//!
//! Refactored to prefer candidate-based grounding (hint->Decider->rect) over raw coordinate
//! generation. Candidate grounding: collect candidates, Decider selects ID, click center.
//! Only raw coordinate if Decider explicitly supports it (don't invent).

use anyhow::{Result, Context, bail};
use serde_json::{Value, json};

/// Backend selection for grounding. Prefer candidate-based grounding over raw coordinate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroundingBackend {
    /// Original Gemini raw-coordinate grounding (screenshot -> Gemini -> x,y)
    Gemini,
    /// Candidate-based grounding: hint_snapshot -> Candidates -> Decider selects ID -> click center
    /// Never emits raw x,y unless Decider explicitly supports it (which this Decider does not)
    DeciderCandidate,
    /// Auto: try candidate-based first (Decider enabled), fallback to Gemini raw coordinate
    Auto,
}

impl Default for GroundingBackend {
    fn default() -> Self { Self::Auto }
}

impl std::fmt::Display for GroundingBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Gemini => write!(f, "gemini"),
            Self::DeciderCandidate => write!(f, "decider_candidate"),
            Self::Auto => write!(f, "auto"),
        }
    }
}

impl std::str::FromStr for GroundingBackend {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        match s.trim().to_lowercase().as_str() {
            "gemini" | "raw" | "coordinate" => Ok(Self::Gemini),
            "decider" | "decider_candidate" | "candidate" => Ok(Self::DeciderCandidate),
            "auto" | "" => Ok(Self::Auto),
            other => bail!("unknown GroundingBackend '{}': use gemini|decider_candidate|auto", other),
        }
    }
}

fn ground_prompt(instruction: &str, w: u32, h: u32) -> String {
    format!(
        "Locate ONE UI element. Return SINGLE JSON {{\"x\":1-1000,\"y\":1-1000}} for its CENTER pixels.\n\
         Target: {instruction}\n\
         Image {w}x{h}, origin top-left. Never return 0,0 unless target is at exact top-left corner. If not visible return {{\"x\":-1,\"y\":-1}}. JSON only, no markdown."
    )
}

fn extract_json_xy(text: &str) -> Result<(f64, f64)> {
    let s = text.find('{').and_then(|st| text.rfind('}').map(|en| &text[st..=en]))
        .ok_or_else(|| anyhow::anyhow!("no JSON in grounding response: {}", text.chars().take(200).collect::<String>()))?;
    let v: Value = serde_json::from_str(s).context("parse grounding JSON")?;
    let x = v.get("x").and_then(|n| n.as_f64()).or_else(|| v.get("x").and_then(|n| n.as_i64()).map(|n| n as f64)).unwrap_or(-1.0);
    let y = v.get("y").and_then(|n| n.as_f64()).or_else(|| v.get("y").and_then(|n| n.as_i64()).map(|n| n as f64)).unwrap_or(-1.0);
    if !(0.0..=1000.0).contains(&x) || !(0.0..=1000.0).contains(&y) {
        bail!("grounding out of range x={} y={}", x, y);
    }
    Ok((x, y))
}

async fn gemini_ground_async(b64: &str, instruction: &str, iw: u32, ih: u32, api_key: &str, model: &str) -> Result<(f64, f64, String)> {
    let url = format!("https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent?key={}", model, api_key);
    let client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(20)).no_proxy().build()?;
    let body = json!({
        "contents": [{"role": "user", "parts": [
            {"text": ground_prompt(instruction, iw, ih)},
            {"inline_data": {"mime_type": "image/jpeg", "data": b64}}
        ]}],
        "generationConfig": {"temperature": 0.0}
    });
    let resp = client.post(&url).json(&body).send().await.context("ground LLM request")?;
    let status = resp.status();
    let v: Value = resp.json().await.context("ground LLM json")?;
    if !status.is_success() { bail!("ground LLM {}: {}", status, v); }
    let text = v.get("candidates").and_then(|c| c.get(0))
        .and_then(|c| c.get("content")).and_then(|c| c.get("parts")).and_then(|p| p.get(0))
        .and_then(|p| p.get("text")).and_then(|t| t.as_str()).unwrap_or("");
    let (x, y) = extract_json_xy(text)?;
    if x < 0.0 || y < 0.0 { bail!("target not visible: {}", text.chars().take(120).collect::<String>()); }
    if x == 0.0 && y == 0.0 { bail!("grounding refused (0,0) for: {}", instruction.chars().take(80).collect::<String>()); }
    Ok((x, y, text.to_string()))
}

static GROUND_RT: std::sync::OnceLock<tokio::runtime::Runtime> = std::sync::OnceLock::new();
fn ground_rt() -> &'static tokio::runtime::Runtime {
    GROUND_RT.get_or_init(|| tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("rt"))
}

fn gemini_key_and_model() -> (String, String) {
    let key = std::env::var("GEMINI_API_KEY")
        .or_else(|_| std::env::var("GOOGLE_API_KEY"))
        .or_else(|_| std::env::var("GOOGLE_GENERATIVE_AI_API_KEY")).unwrap_or_default();
    let model = std::env::var("GROUND_MODEL").unwrap_or_else(|_| "gemini-2.5-flash".to_string());
    (key, model)
}

/// Candidate-based grounding: hint_snapshot -> Candidates -> Decider selects ID -> center rect.
/// Returns ground-like Value with x,y at candidate center plus candidate meta. Preserves Decider confidence.
pub fn ground_candidate(instruction: &str, target: Option<&str>) -> Result<Value> {
    // Collect candidates deterministically
    let snap = crate::hint::hint_snapshot_with_target(target)?;
    let cands = crate::decider::candidates::Candidates::from_hint_snapshot(&snap, None, None, None);
    if cands.is_empty() { bail!("ground_candidate: no hint candidates"); }
    // Try resolve via perception policy (which handles Decider selection + confidence)
    let ctx = crate::perception::resolve::ResolveContext::new(instruction)
        .with_target(target.unwrap_or("").to_string())
        .with_candidates(cands.clone())
        .without_visual_fallback();
    let res = crate::perception::resolve::resolve_target(instruction, &ctx)?;
    if !res.is_success() {
        bail!("ground_candidate uncertain: {}", res.meta);
    }
    let cand = res.candidate.ok_or_else(|| anyhow::anyhow!("no candidate in success"))?;
    let cx = cand.rect.x as f64 + cand.rect.width as f64 / 2.0;
    let cy = cand.rect.y as f64 + cand.rect.height as f64 / 2.0;
    Ok(json!({
        "x": cx.round(), "y": cy.round(),
        "xn": serde_json::Value::Null, "yn": serde_json::Value::Null,
        "confidence": res.confidence,
        "runner_up": res.runner_up,
        "margin": res.margin,
        "probabilities": res.probabilities,
        "candidate": {"id": cand.id, "label": cand.label, "selector": cand.selector, "rect": {"x": cand.rect.x, "y": cand.rect.y, "width": cand.rect.width, "height": cand.rect.height}},
        "id": cand.id,
        "label": cand.label,
        "tier": res.tier.as_str(),
        "via": "decider_candidate",
        "meta": res.meta
    }))
}

/// Ground with explicit backend selection.
pub fn ground_with_backend(instruction: &str, window: &str, region: &str, backend: GroundingBackend) -> Result<Value> {
    match backend {
        GroundingBackend::Gemini => ground_gemini(instruction, window, region),
        GroundingBackend::DeciderCandidate => ground_candidate(instruction, if window.is_empty() { None } else { Some(window) }),
        GroundingBackend::Auto => {
            // Prefer candidate-based when Decider routing is on — either
            // DECIDER_ENABLED explicitly, or (default) the `/health` probe in
            // DeciderConfig::from_env() found a live daemon. Only a dead daemon
            // or an explicit opt-out drops to Gemini.
            let cfg = crate::decider::config::DeciderConfig::from_env();
            // `ground_candidate` drives the *sync* resolver, which blocks on its
            // own runtime. When this sync entry point is itself called from
            // inside a runtime — the perception visual fallback
            // (`resolve::try_visual_fallback`) does exactly that — the nested
            // `block_on` panics with "Cannot start a runtime from within a
            // runtime". Since candidate grounding re-runs the very tier that
            // just failed, skip it in async contexts and go to Gemini instead of
            // crashing the caller.
            let inside_runtime = tokio::runtime::Handle::try_current().is_ok();
            if cfg.enabled && !inside_runtime {
                match ground_candidate(instruction, if window.is_empty() { None } else { Some(window) }) {
                    Ok(v) => return Ok(v),
                    Err(e) => {
                        eprintln!("[ground auto] candidate grounding failed ({}), falling back to Gemini", e);
                    }
                }
            } else if inside_runtime {
                eprintln!("[ground auto] async caller: skipping candidate grounding (would nest runtimes)");
            }
            ground_gemini(instruction, window, region)
        }
    }
}

/// Legacy ground alias now routes via Auto (prefers candidate grounding when Decider enabled)
pub fn ground(instruction: &str, window: &str, region: &str) -> Result<Value> {
    ground_with_backend(instruction, window, region, GroundingBackend::Auto)
}

/// Internal Gemini raw-coordinate grounding (preserved verbatim, now called ground_gemini)
pub fn ground_gemini(instruction: &str, window: &str, region: &str) -> Result<Value> {
    if instruction.is_empty() { bail!("ground needs instruction"); }
    let t0 = std::time::Instant::now();
    let (bytes, meta) = crate::screenshot::capture(window, region, 0.5)?;
    let shot_ms = t0.elapsed().as_millis() as u64;
    let geom = meta.get("geometry").and_then(|g| g.as_array()).cloned().unwrap_or_default();
    let (gx, gy) = (geom.get(0).and_then(|v| v.as_f64()).unwrap_or(0.0), geom.get(1).and_then(|v| v.as_f64()).unwrap_or(0.0));
    let scale = meta.get("scale").and_then(|v| v.as_f64()).unwrap_or(1.0);
    let img = meta.get("image").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    let (iw, ih) = (img.get(0).and_then(|v| v.as_u64()).unwrap_or(0) as u32, img.get(1).and_then(|v| v.as_u64()).unwrap_or(0) as u32);
    if iw == 0 || ih == 0 { bail!("screenshot gave empty image"); }
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
    let (key, model) = gemini_key_and_model();
    if key.is_empty() { bail!("no GEMINI_API_KEY/GOOGLE_API_KEY — set ~/.config/hyprfast/ground.env"); }
    let t1 = std::time::Instant::now();
    // One retry on transport timeout (first TLS handshake often slow).
    let first = ground_rt().block_on(gemini_ground_async(&b64, instruction, iw, ih, &key, &model));
    let (xn, yn, raw) = match first {
        Ok(v) => v,
        Err(e) if e.to_string().contains("timed out") || e.to_string().contains("request") => {
            ground_rt().block_on(gemini_ground_async(&b64, instruction, iw, ih, &key, &model))?
        },
        Err(e) => return Err(e),
    };
    let llm_ms = t1.elapsed().as_millis() as u64;
    // normalized 0-1000 -> image pixels -> global logical
    let px = xn / 1000.0 * iw as f64;
    let py = yn / 1000.0 * ih as f64;
    let x = gx + px / scale;
    let y = gy + py / scale;
    Ok(json!({
        "x": x.round(), "y": y.round(),
        "xn": xn, "yn": yn,
        "confidence": "vlm",
        "timing_ms": {"screenshot": shot_ms, "llm": llm_ms, "total": t0.elapsed().as_millis() as u64},
        "meta": meta,
        "raw": raw.chars().take(200).collect::<String>()
    }))
}

/// Fused ground + action in one call: action = click | type | key | drag handled by caller via steps.
/// For v1: click | type (click then type text) | key (ground then press key).
pub fn act_fast(instruction: &str, action: &str, text: &str, window: &str) -> Result<Value> {
    act_fast_with_backend(instruction, action, text, window, GroundingBackend::Auto)
}

pub fn act_fast_with_backend(instruction: &str, action: &str, text: &str, window: &str, backend: GroundingBackend) -> Result<Value> {
    let g = ground_with_backend(instruction, window, "", backend)?;
    let x = g.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let y = g.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
    // If backend was candidate-based, we could also have clicked via hint_click already - but ground_candidate returns rect center, not a click.
    // So click is still needed here.
    match action {
        "click" | "" => {
            // Prefer hint_click if ground came from candidate (has label)
            if let Some(label) = g.get("label").and_then(|v| v.as_str()) {
                if !label.is_empty() {
                    // Use candidate label path for deterministic click via DOM
                    if let Ok(v) = crate::hint::hint_click_with_target(label, if window.is_empty() { None } else { Some(window) }) {
                        return Ok(json!({"action": "click", "x": x, "y": y, "ground": g, "click_via": "hint", "hint_result": v}));
                    }
                }
            }
            crate::input::click(Some(x), Some(y), "left", false)?;
            Ok(json!({"action": "click", "x": x, "y": y, "ground": g}))
        },
        "type" => {
            if let Some(label) = g.get("label").and_then(|v| v.as_str()) {
                if !label.is_empty() {
                    if let Ok(v) = crate::hint::hint_type_with_target(label, text, if window.is_empty() { None } else { Some(window) }) {
                        return Ok(json!({"action": "type", "x": x, "y": y, "typed": text.len(), "ground": g, "type_via": "hint", "hint_result": v}));
                    }
                }
            }
            crate::input::click(Some(x), Some(y), "left", false)?;
            std::thread::sleep(std::time::Duration::from_millis(80));
            crate::input::type_text(text)?;
            Ok(json!({"action": "type", "x": x, "y": y, "typed": text.len(), "ground": g}))
        },
        "key" => {
            crate::input::click(Some(x), Some(y), "left", false)?;
            std::thread::sleep(std::time::Duration::from_millis(80));
            crate::input::key_combo(text)?;
            Ok(json!({"action": "key", "x": x, "y": y, "pressed": text, "ground": g}))
        },
        _ => bail!("act_fast action must be click|type|key"),
    }
}

/// Batch: JSON array of {instruction, action, text} executed sequentially in one MCP call.
/// Kills N LLM turns — one screenshot+ground per step, shared process.
pub fn act_batch(steps: &Value, window: &str) -> Result<Value> {
    act_batch_with_backend(steps, window, GroundingBackend::Auto)
}
pub fn act_batch_with_backend(steps: &Value, window: &str, backend: GroundingBackend) -> Result<Value> {
    let arr = steps.as_array().ok_or_else(|| anyhow::anyhow!("act_batch needs steps array"))?;
    if arr.is_empty() { bail!("act_batch needs at least 1 step"); }
    if arr.len() > 12 { bail!("act_batch max 12 steps"); }
    let mut results = Vec::new();
    for s in arr {
        let instr = s.get("instruction").and_then(|v| v.as_str()).unwrap_or("");
        let action = s.get("action").and_then(|v| v.as_str()).unwrap_or("click");
        let text = s.get("text").and_then(|v| v.as_str()).unwrap_or("");
        let r = act_fast_with_backend(instr, action, text, window, backend).unwrap_or_else(|e| json!({"error": e.to_string(), "instruction": instr}));
        results.push(r);
        if action != "click" { std::thread::sleep(std::time::Duration::from_millis(150)); }
    }
    Ok(json!({"results": results, "steps": results.len(), "backend": backend.to_string()}))
}
