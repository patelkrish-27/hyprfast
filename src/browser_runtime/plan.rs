#![allow(clippy::useless_vec)]
//! Phase 7 — Structured execution plans.
//!
//! Replaces stringly-typed workflows with a tagged [`PlanStep`] enum and
//! [`ExecutionPlan`] container. Validation is syntactic only — it never
//! eagerly resolves post-navigation targets (the DOM they target may not
//! exist until prior steps run). Execution is additive via
//! `browser_execute_plan` — single-action tools are preserved.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::time::{Duration, Instant};

use super::error::{RuntimeError, RuntimeResult};
use super::server::CapabilityClass;

// ---------------------------------------------------------------------------
// PlanStep — tagged enum, each variant's tag matches the browser/mod.rs
// command coverage. `#[serde(tag="type")]` gives {"type":"navigate",...}.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PlanStep {
    /// Page.navigate (capability Navigation)
    Navigate { url: String },
    /// Click by selector/ref/element (no capability yet — read-like)
    Click {
        #[serde(default)]
        selector: Option<String>,
        #[serde(default, alias = "ref")]
        r#ref: Option<String>,
        #[serde(default)]
        element: Option<String>,
    },
    /// Type/fill into editable element (capability RuntimeEvaluate when via Runtime.evaluate)
    Type {
        text: String,
        #[serde(default)]
        selector: Option<String>,
        #[serde(default, alias = "ref")]
        r#ref: Option<String>,
        #[serde(default)]
        submit: bool,
    },
    /// Fill alias (same as Type but selector required)
    Fill {
        selector: String,
        text: String,
        #[serde(default)]
        submit: bool,
    },
    /// Select option in dropdown
    Select {
        #[serde(default)]
        selector: Option<String>,
        #[serde(default, alias = "ref")]
        r#ref: Option<String>,
        values: Vec<String>,
    },
    /// Press key via Input.dispatchKeyEvent
    Press { key: String },
    /// Hover via mouseover
    Hover {
        #[serde(default)]
        selector: Option<String>,
        #[serde(default, alias = "ref")]
        r#ref: Option<String>,
        #[serde(default)]
        element: Option<String>,
    },
    /// Wait: either timed sleep (`time`) or wait_for_url (`url_contains`/`url_equals`) or lifecycle.
    /// `time` in seconds. `timeout_ms` ceiling for URL/lifecycle waits.
    Wait {
        #[serde(default)]
        time: Option<f64>,
        #[serde(default)]
        url_contains: Option<String>,
        #[serde(default)]
        url_equals: Option<String>,
        #[serde(default)]
        lifecycle: Option<String>,
        #[serde(default)]
        timeout_ms: Option<u64>,
    },
    /// Evaluate JavaScript (capability RuntimeEvaluate)
    Eval {
        #[serde(alias = "expression", alias = "js")]
        expression: String,
    },
    /// Extract text/attribute/value from selector
    Extract {
        #[serde(default)]
        selector: Option<String>,
        #[serde(default)]
        attribute: Option<String>,
        #[serde(default)]
        instruction: Option<String>,
        #[serde(default)]
        text: Option<String>,
    },
    /// History back
    GoBack,
    /// History forward
    GoForward,
    /// List tabs/targets
    Tabs,
    /// Snapshot (AX tree)
    Snapshot {
        #[serde(default)]
        max_nodes: Option<usize>,
    },
}

impl PlanStep {
    /// Stable tag string (matches serde tag).
    pub fn tag(&self) -> &'static str {
        match self {
            PlanStep::Navigate { .. } => "navigate",
            PlanStep::Click { .. } => "click",
            PlanStep::Type { .. } => "type",
            PlanStep::Fill { .. } => "fill",
            PlanStep::Select { .. } => "select",
            PlanStep::Press { .. } => "press",
            PlanStep::Hover { .. } => "hover",
            PlanStep::Wait { .. } => "wait",
            PlanStep::Eval { .. } => "eval",
            PlanStep::Extract { .. } => "extract",
            PlanStep::GoBack => "go_back",
            PlanStep::GoForward => "go_forward",
            PlanStep::Tabs => "tabs",
            PlanStep::Snapshot { .. } => "snapshot",
        }
    }

    /// Capability class per step (rule 32). Tag-only in Phase 7 — no enforcement.
    pub fn capability(&self) -> CapabilityClass {
        match self {
            PlanStep::Navigate { .. } | PlanStep::GoBack | PlanStep::GoForward => CapabilityClass::Navigation,
            PlanStep::Eval { .. } | PlanStep::Extract { .. } | PlanStep::Type { .. } | PlanStep::Fill { .. } | PlanStep::Press { .. } | PlanStep::Click { .. } | PlanStep::Select { .. } | PlanStep::Hover { .. } => CapabilityClass::RuntimeEvaluate,
            PlanStep::Tabs | PlanStep::Snapshot { .. } | PlanStep::Wait { .. } => CapabilityClass::None,
        }
    }

    /// Syntactic validation — no DOM resolution, so post-navigation targets are not eagerly checked.
    pub fn validate(&self) -> Result<(), String> {
        match self {
            PlanStep::Navigate { url } => {
                if url.trim().is_empty() {
                    return Err("navigate: url must be non-empty".to_string());
                }
                // Accept file://, http(s)://, about:blank. Use url crate for http(s).
                if url.starts_with("http://") || url.starts_with("https://") || url.starts_with("file://") || url == "about:blank" {
                    // try parse for http(s)
                    if url.starts_with("http") {
                        url::Url::parse(url).map_err(|e| format!("navigate: invalid url {url:?}: {e}"))?;
                    }
                    Ok(())
                } else {
                    // Allow relative/localhost without scheme? Require non-empty.
                    // Be lenient: accept any non-empty string that url crate can parse or fallback.
                    if url::Url::parse(url).is_err() && !url.starts_with('/') {
                        // still allow plain localhost paths — warn but accept
                    }
                    Ok(())
                }
            }
            PlanStep::Click { selector, r#ref, element } => {
                let has = selector.as_deref().map(|s| !s.trim().is_empty()).unwrap_or(false)
                    || r#ref.as_deref().map(|s| !s.trim().is_empty()).unwrap_or(false)
                    || element.as_deref().map(|s| !s.trim().is_empty()).unwrap_or(false);
                if !has {
                    return Err("click: need selector, ref, or element".to_string());
                }
                if let Some(s) = selector { if s.contains(":contains") { return Err("click: selector must not contain :contains() — use extract/text tier instead".to_string()); } }
                Ok(())
            }
            PlanStep::Type { text: _, selector, r#ref, .. } => {
                // text may be empty (clear), but selector/ref fallback is allowed.
                // Validate selector if present
                if let Some(s) = selector { if s.contains(":contains") { return Err("type: selector must not contain :contains()".to_string()); } }
                let _ = r#ref;
                Ok(())
            }
            PlanStep::Fill { selector, text: _, submit: _ } => {
                if selector.trim().is_empty() { return Err("fill: selector must be non-empty".to_string()); }
                if selector.contains(":contains") { return Err("fill: selector must not contain :contains()".to_string()); }
                Ok(())
            }
            PlanStep::Select { selector, r#ref, values } => {
                if values.is_empty() { return Err("select: values must be non-empty".to_string()); }
                let has_sel = selector.as_deref().map(|s| !s.trim().is_empty()).unwrap_or(false)
                    || r#ref.as_deref().map(|s| !s.trim().is_empty()).unwrap_or(false);
                if !has_sel {
                    return Err("select: need selector or ref".to_string());
                }
                Ok(())
            }
            PlanStep::Press { key } => {
                if key.trim().is_empty() { return Err("press: key must be non-empty".to_string()); }
                Ok(())
            }
            PlanStep::Hover { selector, r#ref, element } => {
                let has = selector.as_deref().map(|s| !s.trim().is_empty()).unwrap_or(false)
                    || r#ref.as_deref().map(|s| !s.trim().is_empty()).unwrap_or(false)
                    || element.as_deref().map(|s| !s.trim().is_empty()).unwrap_or(false);
                if !has { return Err("hover: need selector, ref, or element".to_string()); }
                Ok(())
            }
            PlanStep::Wait { time, url_contains, url_equals, lifecycle, timeout_ms: _ } => {
                let has_time = time.is_some();
                let has_url = url_contains.as_deref().map(|s| !s.trim().is_empty()).unwrap_or(false)
                    || url_equals.as_deref().map(|s| !s.trim().is_empty()).unwrap_or(false);
                let has_lc = lifecycle.as_deref().map(|s| !s.trim().is_empty()).unwrap_or(false);
                if !has_time && !has_url && !has_lc {
                    // default wait 1s is allowed but explicit validation warns; accept as no-op wait 0?
                    // Require at least one signal: if none, treat as time=1 default? We'll allow but not error.
                    // For strict validation we require something.
                    // But to keep plan JSON flexible, allow empty Wait as 1s default? We'll not error; execute will default to 0.5s.
                    // Uncomment to enforce: return Err("wait: need time, url_contains, url_equals, or lifecycle".to_string());
                }
                if let Some(t) = time { if *t < 0.0 || *t > 300.0 { return Err(format!("wait: time {t} out of range [0,300]")); } }
                Ok(())
            }
            PlanStep::Eval { expression } => {
                if expression.trim().is_empty() { return Err("eval: expression must be non-empty".to_string()); }
                Ok(())
            }
            PlanStep::Extract { selector, instruction, text, attribute: _ } => {
                let has = selector.as_deref().map(|s| !s.trim().is_empty()).unwrap_or(false)
                    || instruction.as_deref().map(|s| !s.trim().is_empty()).unwrap_or(false)
                    || text.as_deref().map(|s| !s.trim().is_empty()).unwrap_or(false);
                if !has { return Err("extract: need selector, instruction, or text".to_string()); }
                Ok(())
            }
            PlanStep::GoBack | PlanStep::GoForward | PlanStep::Tabs => Ok(()),
            PlanStep::Snapshot { max_nodes } => {
                if let Some(n) = max_nodes { if *n == 0 || *n > 10000 { return Err(format!("snapshot: max_nodes {n} out of range")); } }
                Ok(())
            }
        }
    }
}

// ---------------------------------------------------------------------------
// ExecutionPlan
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionPlan {
    pub steps: Vec<PlanStep>,
    /// Current step index for in-flight tracking. Serialized with default 0 so
    /// a JSON plan `{steps:[...]}` without this field round-trips.
    #[serde(default)]
    pub step_index: usize,
}

impl ExecutionPlan {
    pub fn new(steps: Vec<PlanStep>) -> Self {
        Self { steps, step_index: 0 }
    }

    /// Syntactic validation without eager resolution of post-navigation targets.
    pub fn validate(&self) -> Result<(), String> {
        if self.steps.is_empty() {
            return Err("plan must have at least one step".to_string());
        }
        if self.steps.len() > 100 {
            return Err(format!("plan has {} steps, max 100", self.steps.len()));
        }
        for (i, s) in self.steps.iter().enumerate() {
            s.validate().map_err(|e| format!("step {i} ({}): {e}", s.tag()))?;
        }
        Ok(())
    }

    pub fn from_json(v: &Value) -> RuntimeResult<Self> {
        // Accept either {steps:[...]} object or bare array for convenience
        if let Some(arr) = v.as_array() {
            let steps: Vec<PlanStep> = serde_json::from_value(Value::Array(arr.clone()))
                .map_err(|e| RuntimeError::InvalidResponse(format!("plan steps decode: {e}")))?;
            return Ok(Self { steps, step_index: 0 });
        }
        serde_json::from_value(v.clone())
            .map_err(|e| RuntimeError::InvalidResponse(format!("plan decode: {e}")))
    }

    pub fn to_json(&self) -> Value {
        serde_json::to_value(self).unwrap_or(json!({"steps":[]}))
    }

    /// Capability list for journal tagging (one per step, in order).
    pub fn capabilities(&self) -> Vec<String> {
        self.steps.iter().map(|s| s.capability().as_str().to_string()).collect()
    }
}

// ---------------------------------------------------------------------------
// Execution result types (for DoD verification)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepOutcome {
    pub step_index: usize,
    pub step_type: String,
    pub capability: String,
    pub ok: bool,
    pub result: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub latency_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanExecutionResult {
    pub ok: bool,
    pub steps_executed: usize,
    pub total_steps: usize,
    pub results: Vec<StepOutcome>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_title: Option<String>,
    pub dom_version: u64,
    pub frame_tree_version: u64,
}

// ---------------------------------------------------------------------------
// Execution engine (async, uses the single BrowserRuntime transport)
// ---------------------------------------------------------------------------

impl ExecutionPlan {
    /// Execute sequentially against a live `BrowserRuntime` + `DomState`.
    /// Steps are run in order; on first hard failure the plan stops and
    /// `ok=false` is returned with partial results. `Inconclusive` is not
    /// modeled until Phase 8 — here any returned `result` with ok=true keeps going.
    pub async fn execute_with_runtime(
        &self,
        runtime: &super::connection::BrowserRuntime,
        dom_state: &super::dom_state::DomState,
        frame_manager: &super::frames::FrameManager,
    ) -> PlanExecutionResult {
        // Validate first (syntax only)
        if let Err(e) = self.validate() {
            return PlanExecutionResult {
                ok: false,
                steps_executed: 0,
                total_steps: self.steps.len(),
                results: vec![StepOutcome {
                    step_index: 0,
                    step_type: "validate".to_string(),
                    capability: "none".to_string(),
                    ok: false,
                    result: Value::Null,
                    error: Some(e),
                    latency_ms: 0,
                }],
                final_url: None,
                final_title: None,
                dom_version: dom_state.dom_version(),
                frame_tree_version: frame_manager.frame_tree_version(),
            };
        }

        let mut results = Vec::with_capacity(self.steps.len());
        let mut ok = true;

        for (idx, step) in self.steps.iter().enumerate() {
            let start = Instant::now();
            let cap = step.capability().as_str().to_string();
            let tag = step.tag().to_string();
            let outcome: Result<Value, String> = execute_step(step, runtime, dom_state).await;
            let latency_ms = start.elapsed().as_millis() as u64;
            match outcome {
                Ok(v) => {
                    results.push(StepOutcome {
                        step_index: idx,
                        step_type: tag,
                        capability: cap,
                        ok: true,
                        result: v,
                        error: None,
                        latency_ms,
                    });
                }
                Err(e) => {
                    results.push(StepOutcome {
                        step_index: idx,
                        step_type: tag.clone(),
                        capability: cap,
                        ok: false,
                        result: Value::Null,
                        error: Some(e.clone()),
                        latency_ms,
                    });
                    ok = false;
                    // Stop on hard failure (Failed). Phase 8 will distinguish Inconclusive -> continue.
                    // For now, any error stops subsequent steps — but still report them as skipped?
                    break;
                }
            }
        }

        // Final URL/title/dom state verification (real browser state)
        let (final_url, final_title) = fetch_url_title(runtime).await;
        PlanExecutionResult {
            ok,
            steps_executed: results.len(),
            total_steps: self.steps.len(),
            results,
            final_url,
            final_title,
            dom_version: dom_state.dom_version(),
            frame_tree_version: frame_manager.frame_tree_version(),
        }
    }

    /// Fallback execution that routes through the degraded direct path helpers
    /// (ephemeral BrowserRuntime) when no daemon is available. Used by the
    /// client-side `execute_plan_sync` helper.
    pub fn execute_sync_degraded(&self) -> anyhow::Result<PlanExecutionResult> {
        // For degraded mode we block_on a fresh ephemeral runtime per plan.
        // This keeps transport confined to connection.rs even on the fallback path.
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
        rt.block_on(async {
            // Discover ephemeral runtime
            let host = std::env::var("HYPRFAST_CDP_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
            let port: u16 = std::env::var("HYPRFAST_CDP_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(9222);
            let ws_url = super::server::discover_ws_url_for_fallback(&host, port).await.map_err(|e| anyhow::anyhow!(e.to_string()))?;
            let runtime = super::connection::BrowserRuntime::connect(&ws_url).await.map_err(|e| anyhow::anyhow!(e.to_string()))?;
            let ds = super::dom_state::DomState::new();
            let fm = super::frames::FrameManager::new(std::sync::Arc::new(super::dom_state::DomState::new()));
            // Note: ds vs fm separate — for degraded verification we only use fallback dom_version 0; ok.
            let res = self.execute_with_runtime(&runtime, &ds, &fm).await;
            runtime.shutdown().await;
            Ok(res)
        })
    }
}

async fn fetch_url_title(runtime: &super::connection::BrowserRuntime) -> (Option<String>, Option<String>) {
    let session = runtime.diagnostics().attached_session_ids.into_iter().next();
    let url_val = runtime.call(session.as_deref(), "Runtime.evaluate", json!({"expression": "location.href", "returnByValue": true})).await.ok();
    let title_val = runtime.call(session.as_deref(), "Runtime.evaluate", json!({"expression": "document.title", "returnByValue": true})).await.ok();
    let url = url_val.and_then(|v| v.get("result").and_then(|r| r.get("value")).and_then(|vv| vv.as_str()).map(|s| s.to_string()).or_else(|| v.get("value").and_then(|vv| vv.as_str()).map(|s| s.to_string())));
    let title = title_val.and_then(|v| v.get("result").and_then(|r| r.get("value")).and_then(|vv| vv.as_str()).map(|s| s.to_string()).or_else(|| v.get("value").and_then(|vv| vv.as_str()).map(|s| s.to_string())));
    (url, title)
}

async fn execute_step(
    step: &PlanStep,
    runtime: &super::connection::BrowserRuntime,
    dom_state: &super::dom_state::DomState,
) -> Result<Value, String> {
    let session = runtime.diagnostics().attached_session_ids.into_iter().next();
    let sess = session.as_deref();
    match step {
        PlanStep::Navigate { url } => {
            // Page.navigate via daemon's single WS, capability Navigation
            let res = runtime.call(sess, "Page.navigate", json!({"url": url})).await.map_err(|e| e.to_string())?;
            // Best-effort event-driven wait, then fallback polling for degraded path where DomState isn't fed.
            let sub = || runtime.subscribe();
            let _ = dom_state.wait_for_lifecycle(sub, "load", Duration::from_secs(5)).await;
            // Fallback polling: wait for document.readyState == complete (covers degraded ephemeral where DomState events aren't pumped)
            let _ = wait_for_ready_complete(runtime, Duration::from_secs(8)).await;
            Ok(res)
        }
        PlanStep::Click { selector, r#ref, element } => {
            let sel = selector.clone().or_else(|| r#ref.clone()).or_else(|| element.clone()).unwrap_or_default();
            if sel.trim().is_empty() { return Err("click: no selector".to_string()); }
            // Backend numeric ref path: DOM.resolveNode + callFunctionOn
            if sel.chars().all(|c| c.is_ascii_digit()) {
                let backend: i64 = sel.parse().map_err(|_| "click: invalid backend id".to_string())?;
                let resolved = runtime.call(sess, "DOM.resolveNode", json!({"backendNodeId": backend})).await.map_err(|e| e.to_string())?;
                let oid = resolved.get("object").and_then(|o| o.get("objectId")).and_then(|v| v.as_str()).ok_or("click: resolve failed missing objectId")?;
                let clicked = runtime.call(sess, "Runtime.callFunctionOn", json!({"objectId": oid, "functionDeclaration": "function(){ this.click(); return this.tagName; }", "returnByValue": true})).await.map_err(|e| e.to_string())?;
                let _ = runtime.call(sess, "Runtime.releaseObject", json!({"objectId": oid})).await;
                return Ok(clicked);
            }
            // Structured selector via Runtime.evaluate with JSON-escaped selector (no :contains, no injection)
            let sel_json = serde_json::to_string(&sel).unwrap();
            let js = format!("(() => {{ const el=document.querySelector({}); if(!el) return {{error:'not found', selector:{}}}; el.click(); const r=el.getBoundingClientRect(); return {{clicked:true, tag: el.tagName, x: r.x, y: r.y}}; }})()", sel_json, sel_json);
            let v = runtime.call(sess, "Runtime.evaluate", json!({"expression": js, "returnByValue": true, "awaitPromise": true})).await.map_err(|e| e.to_string())?;
            let inner = v.get("result").and_then(|x| x.get("value")).cloned().unwrap_or(v.clone());
            if inner.get("error").is_some() { return Err(format!("click: {}", inner)); }
            Ok(inner)
        }
        PlanStep::Type { text, selector, r#ref, submit } => {
            let sel = selector.clone().or_else(|| r#ref.clone()).unwrap_or_else(|| "input, textarea, [contenteditable]".to_string());
            let sel_json = serde_json::to_string(&sel).unwrap();
            let text_json = serde_json::to_string(text).unwrap();
            let submit_js = if *submit { "true" } else { "false" };
            let js = format!(r#"(() => {{
  let el=document.querySelector({sel});
  if(!el) el=document.activeElement;
  if(!el || (el.tagName!=='INPUT' && el.tagName!=='TEXTAREA' && !el.isContentEditable)) {{
    el=document.querySelector('input, textarea, [contenteditable=true]');
  }}
  if(!el) return {{error:'no editable element'}};
  el.focus();
  if(el.isContentEditable) {{
    document.execCommand('selectAll', false, null);
    document.execCommand('insertText', false, {txt});
  }} else {{
    el.value={txt};
    el.dispatchEvent(new Event('input',{{bubbles:true}}));
    el.dispatchEvent(new Event('change',{{bubbles:true}}));
  }}
  if({sub}) {{
    el.dispatchEvent(new KeyboardEvent('keydown',{{key:'Enter',code:'Enter',keyCode:13,bubbles:true}}));
    // also submit form if present
    if(el.form) el.form.dispatchEvent(new Event('submit',{{bubbles:true,cancelable:true}}));
    // fallback: dispatch Enter on document
    if({sub}) {{ el.dispatchEvent(new KeyboardEvent('keyup',{{key:'Enter',bubbles:true}})); }}
  }}
  return {{typed: {txt}.length, tag: el.tagName, value: (el.value||el.textContent||'').slice(0,500)}};
}})()"#, sel=sel_json, txt=text_json, sub=submit_js);
            let v = runtime.call(sess, "Runtime.evaluate", json!({"expression": js, "returnByValue": true, "awaitPromise": true})).await.map_err(|e| e.to_string())?;
            let inner = v.get("result").and_then(|x| x.get("value")).cloned().unwrap_or(v.clone());
            if inner.get("error").is_some() { return Err(format!("type: {}", inner)); }
            // If submit requested, trigger real form submission and wait for navigation
            if *submit {
                let _ = runtime.call(sess, "Runtime.evaluate", json!({"expression": "(() => { const ae=document.activeElement; if(ae&&ae.form) { ae.form.submit(); return true; } const f=document.querySelector('form'); if(f) { f.submit(); return true; } return false; })()", "returnByValue": true})).await;
                let _ = wait_for_ready_complete(runtime, Duration::from_secs(5)).await;
            }
            Ok(inner)
        }
        PlanStep::Fill { selector, text, submit } => {
            let sel_json = serde_json::to_string(selector).unwrap();
            let txt_json = serde_json::to_string(text).unwrap();
            let js = format!("(() => {{ const el=document.querySelector({}); if(!el) return {{error:'not found'}}; el.focus(); el.value={}; el.dispatchEvent(new Event('input',{{bubbles:true}})); el.dispatchEvent(new Event('change',{{bubbles:true}})); if({}) {{ if(el.form) el.form.submit(); else el.form&&el.form.dispatchEvent(new Event('submit',{{bubbles:true}})); }} return {{filled:true, value: el.value}}; }})()", sel_json, txt_json, if *submit {"true"} else {"false"});
            let v = runtime.call(sess, "Runtime.evaluate", json!({"expression": js, "returnByValue": true})).await.map_err(|e| e.to_string())?;
            let inner = v.get("result").and_then(|x| x.get("value")).cloned().unwrap_or(v);
            if inner.get("error").is_some() { return Err(format!("fill: {}", inner)); }
            Ok(inner)
        }
        PlanStep::Select { selector, r#ref, values } => {
            let sel = selector.clone().or_else(|| r#ref.clone()).unwrap_or_else(|| "select".to_string());
            let sel_json = serde_json::to_string(&sel).unwrap();
            let vals_json = serde_json::to_string(values).unwrap();
            let js = format!("(() => {{ const el=document.querySelector({}) || document.querySelector('select'); if(!el) return {{error:'no select'}}; const vals={}; for(const o of el.options) {{ if(vals.includes(o.value) || vals.includes(o.text)) o.selected=true; }} el.dispatchEvent(new Event('change',{{bubbles:true}})); return {{selected: vals}}; }})()", sel_json, vals_json);
            let v = runtime.call(sess, "Runtime.evaluate", json!({"expression": js, "returnByValue": true})).await.map_err(|e| e.to_string())?;
            let inner = v.get("result").and_then(|x| x.get("value")).cloned().unwrap_or(v);
            if inner.get("error").is_some() { return Err(format!("select: {}", inner)); }
            Ok(inner)
        }
        PlanStep::Press { key } => {
            let (cdp_key, code) = map_key(key);
            // Use Input.dispatchKeyEvent where available, else fallback to JS
            let _ = runtime.call(sess, "Input.dispatchKeyEvent", json!({"type":"keyDown","key": cdp_key, "code": code})).await;
            let _ = runtime.call(sess, "Input.dispatchKeyEvent", json!({"type":"keyUp","key": cdp_key, "code": code})).await;
            // Also dispatch JS for form handling
            let ck = serde_json::to_string(&cdp_key).unwrap();
            let _ = runtime.call(sess, "Runtime.evaluate", json!({"expression": format!("document.dispatchEvent(new KeyboardEvent('keydown',{{key:{},bubbles:true}}))", ck), "returnByValue": true})).await;
            if key.to_lowercase()=="enter" {
                // Real form submission via submit() (not just dispatchEvent which may be cancelable and not navigate)
                let _ = runtime.call(sess, "Runtime.evaluate", json!({"expression": "(() => { const ae=document.activeElement; if(ae&&ae.form) { ae.form.submit(); return 'submitted via ae.form'; } const f=document.querySelector('form'); if(f) { f.submit(); return 'submitted via query'; } // fallback: try to trigger via Enter on input\n const el=document.activeElement;\n if(el) { el.dispatchEvent(new KeyboardEvent('keydown',{key:'Enter',code:'Enter',bubbles:true})); el.dispatchEvent(new KeyboardEvent('keyup',{key:'Enter',bubbles:true})); }\n return location.href; })()", "returnByValue": true})).await;
                // Wait for navigation to commit
                let _ = wait_for_ready_complete(runtime, Duration::from_secs(5)).await;
                let v = runtime.call(sess, "Runtime.evaluate", json!({"expression": "location.href", "returnByValue": true})).await.map_err(|e| e.to_string())?;
                return Ok(v.get("result").and_then(|r| r.get("value")).cloned().unwrap_or(v));
            }
            Ok(json!({"pressed": key}))
        }
        PlanStep::Hover { selector, r#ref, element } => {
            let sel = selector.clone().or_else(|| r#ref.clone()).or_else(|| element.clone()).unwrap_or_default();
            let sel_json = serde_json::to_string(&sel).unwrap();
            let js = format!("(() => {{ const el=document.querySelector({}); if(!el) return {{error:'not found'}}; el.dispatchEvent(new MouseEvent('mouseover',{{bubbles:true}})); const r=el.getBoundingClientRect(); return {{hovered:true, x:r.x, y:r.y}}; }})()", sel_json);
            let v = runtime.call(sess, "Runtime.evaluate", json!({"expression": js, "returnByValue": true})).await.map_err(|e| e.to_string())?;
            let inner = v.get("result").and_then(|x| x.get("value")).cloned().unwrap_or(v);
            if inner.get("error").is_some() { return Err(format!("hover: {}", inner)); }
            Ok(inner)
        }
        PlanStep::Wait { time, url_contains, url_equals, lifecycle, timeout_ms } => {
            if let Some(uc) = url_contains.as_deref().filter(|s| !s.trim().is_empty()) {
                let timeout = Duration::from_millis(timeout_ms.unwrap_or(10000));
                return wait_for_url_contains(runtime, uc, timeout).await;
            }
            if let Some(ue) = url_equals.as_deref().filter(|s| !s.trim().is_empty()) {
                let timeout = Duration::from_millis(timeout_ms.unwrap_or(10000));
                return wait_for_url_equals(runtime, ue, timeout).await;
            }
            if let Some(lc) = lifecycle.as_deref().filter(|s| !s.trim().is_empty()) {
                let timeout = Duration::from_millis(timeout_ms.unwrap_or(10000));
                let sub = || runtime.subscribe();
                match dom_state.wait_for_lifecycle(sub, lc, timeout).await {
                    Ok(()) => return Ok(json!({"lifecycle": lc})),
                    Err(_) => {
                        // Fallback polling for degraded ephemeral path where DomState isn't fed
                        if lc == "load" || lc == "complete" {
                            wait_for_ready_complete(runtime, timeout).await.map_err(|e| e.to_string())?;
                            return Ok(json!({"lifecycle": lc, "via":"poll"}));
                        }
                        return Err(format!("wait lifecycle {} timeout", lc));
                    }
                }
            }
            if let Some(t) = time {
                // Condition polling is allowed per rule 5 when no event exists; but wait with time is intentional.
                // Use tokio sleep (not std::thread::sleep which would block). This is the user-requested delay.
                tokio::time::sleep(Duration::from_secs_f64(*t)).await;
                return Ok(json!({"waited": t}));
            }
            // default: small no-op
            tokio::time::sleep(Duration::from_millis(200)).await;
            Ok(json!({"waited": 0.2}))
        }
        PlanStep::Eval { expression } => {
            let v = runtime.call(sess, "Runtime.evaluate", json!({"expression": expression, "returnByValue": true, "awaitPromise": true})).await.map_err(|e| e.to_string())?;
            if let Some(exc) = v.get("exceptionDetails") { return Err(format!("eval exception: {}", exc)); }
            Ok(v.get("result").and_then(|r| r.get("value")).cloned().unwrap_or(v))
        }
        PlanStep::Extract { selector, attribute, instruction, text: _ } => {
            let sel = selector.clone().or_else(|| instruction.clone()).unwrap_or_else(|| "body".to_string());
            let sel_json = serde_json::to_string(&sel).unwrap();
            let attr_json = attribute.as_deref().map(|a| serde_json::to_string(a).unwrap()).unwrap_or("null".to_string());
            // Return textContent, value, or attribute. Structured: no :contains.
            let js = if attribute.is_some() {
                format!("(() => {{ const el=document.querySelector({}); if(!el) return {{error:'not found'}}; const a={}; return {{value: el.getAttribute(JSON.parse(a))||'', tag: el.tagName}}; }})()", sel_json, attr_json)
            } else {
                format!("(() => {{ const el=document.querySelector({}); if(!el) return {{error:'not found', sel:{}}}; const v=(el.value!==undefined? el.value : (el.textContent||el.innerText||'')); return {{value: String(v).trim().slice(0,5000), tag: el.tagName, url: location.href}}; }})()", sel_json, sel_json)
            };
            let v = runtime.call(sess, "Runtime.evaluate", json!({"expression": js, "returnByValue": true})).await.map_err(|e| e.to_string())?;
            let inner = v.get("result").and_then(|x| x.get("value")).cloned().unwrap_or(v);
            if inner.get("error").is_some() { return Err(format!("extract: {}", inner)); }
            Ok(inner)
        }
        PlanStep::GoBack => {
            let v = runtime.call(sess, "Runtime.evaluate", json!({"expression": "history.back(); location.href", "returnByValue": true})).await.map_err(|e| e.to_string())?;
            let sub = || runtime.subscribe();
            let _ = dom_state.wait_for_lifecycle(sub, "load", Duration::from_secs(5)).await;
            Ok(v.get("result").and_then(|r| r.get("value")).cloned().unwrap_or(v))
        }
        PlanStep::GoForward => {
            let v = runtime.call(sess, "Runtime.evaluate", json!({"expression": "history.forward(); location.href", "returnByValue": true})).await.map_err(|e| e.to_string())?;
            let sub = || runtime.subscribe();
            let _ = dom_state.wait_for_lifecycle(sub, "load", Duration::from_secs(5)).await;
            Ok(v.get("result").and_then(|r| r.get("value")).cloned().unwrap_or(v))
        }
        PlanStep::Tabs => {
            let v = runtime.call(None, "Target.getTargets", json!({})).await.map_err(|e| e.to_string())?;
            Ok(v)
        }
        PlanStep::Snapshot { max_nodes } => {
            let limit = max_nodes.unwrap_or(60);
            // Try AX tree first
            if let Ok(val) = runtime.call(sess, "Accessibility.getFullAXTree", json!({})).await {
                if let Some(nodes) = val.get("nodes").and_then(|v| v.as_array()) {
                    if !nodes.is_empty() {
                        let slice: Vec<Value> = nodes.iter().take(limit).cloned().collect();
                        return Ok(json!({"snapshot": slice, "via":"Accessibility", "count": slice.len()}));
                    }
                }
            }
            // fallback JS snapshot
            let js = r#"(() => {
  const MAX=80;
  const out=[];
  const walker=document.createTreeWalker(document.body, NodeFilter.SHOW_ELEMENT);
  let n=walker.currentNode;
  let c=0;
  while(n && c<MAX){
    const el=n;
    const tag=el.tagName.toLowerCase();
    const role=el.getAttribute('role')||({'a':'link','button':'button','input':'textbox','select':'combobox','textarea':'textbox'}[tag]||tag);
    const name=(el.getAttribute('aria-label')||el.innerText||el.value||'').trim().slice(0,120);
    const rect=el.getBoundingClientRect();
    if(rect.width>0 && rect.height>0) out.push({role,name: name||tag, tag, x:Math.round(rect.x), y:Math.round(rect.y)});
    c++;
    n=walker.nextNode();
  }
  return out;
})()"#;
            let v = runtime.call(sess, "Runtime.evaluate", json!({"expression": js, "returnByValue": true})).await.map_err(|e| e.to_string())?;
            Ok(v.get("result").and_then(|r| r.get("value")).cloned().unwrap_or(v))
        }
    }
}

async fn wait_for_url_contains(runtime: &super::connection::BrowserRuntime, needle: &str, timeout: Duration) -> Result<Value, String> {
    let needle_owned = needle.to_string();
    let rt = runtime.clone();
    // Unified with condition engine's UrlContains — single check/subscribe/re-check/await primitive
    let res = crate::browser_runtime::wait::wait_for(
        {
            let needle = needle_owned.clone();
            let rt = rt.clone();
            move || {
                let needle = needle.clone();
                let rt = rt.clone();
                async move {
                    let sess = rt.diagnostics().attached_session_ids.into_iter().next();
                    let v = rt
                        .call(sess.as_deref(), "Runtime.evaluate", json!({"expression": "location.href", "returnByValue": true}))
                        .await;
                    let href = v.ok().and_then(|val| {
                        val.get("result")
                            .and_then(|r| r.get("value"))
                            .and_then(|x| x.as_str())
                            .map(|s| s.to_string())
                            .or_else(|| val.get("value").and_then(|x| x.as_str()).map(|s| s.to_string()))
                    });
                    href.map(|h| h.contains(&needle)).unwrap_or(false)
                }
            }
        },
        || runtime.subscribe(),
        |ev| ev.method == "Page.frameNavigated" || ev.method == "Page.lifecycleEvent",
        timeout,
    )
    .await
    .map_err(|e| e.to_string())?;
    let _ = res;
    // Fetch final url for return payload
    let sess = runtime.diagnostics().attached_session_ids.into_iter().next();
    let v = runtime
        .call(sess.as_deref(), "Runtime.evaluate", json!({"expression": "location.href", "returnByValue": true}))
        .await
        .map_err(|e| e.to_string())?;
    let href = v
        .get("result")
        .and_then(|r| r.get("value"))
        .and_then(|x| x.as_str())
        .or_else(|| v.get("value").and_then(|x| x.as_str()))
        .unwrap_or("")
        .to_string();
    if href.contains(&needle_owned) {
        Ok(json!({"url": href, "contains": needle_owned}))
    } else {
        Err(format!("wait_for_url timeout: location.href {href:?} never contained {:?}", needle_owned))
    }
}

async fn wait_for_url_equals(runtime: &super::connection::BrowserRuntime, expected: &str, timeout: Duration) -> Result<Value, String> {
    let expected_owned = expected.to_string();
    let rt = runtime.clone();
    let res = crate::browser_runtime::wait::wait_for(
        {
            let expected = expected_owned.clone();
            let rt = rt.clone();
            move || {
                let expected = expected.clone();
                let rt = rt.clone();
                async move {
                    let sess = rt.diagnostics().attached_session_ids.into_iter().next();
                    let v = rt
                        .call(sess.as_deref(), "Runtime.evaluate", json!({"expression": "location.href", "returnByValue": true}))
                        .await;
                    let href = v.ok().and_then(|val| {
                        val.get("result")
                            .and_then(|r| r.get("value"))
                            .and_then(|x| x.as_str())
                            .map(|s| s.to_string())
                    });
                    href.map(|h| h == expected).unwrap_or(false)
                }
            }
        },
        || runtime.subscribe(),
        |ev| ev.method == "Page.frameNavigated" || ev.method == "Page.lifecycleEvent",
        timeout,
    )
    .await
    .map_err(|e| e.to_string())?;
    let _ = res;
    let sess = runtime.diagnostics().attached_session_ids.into_iter().next();
    let v = runtime
        .call(sess.as_deref(), "Runtime.evaluate", json!({"expression": "location.href", "returnByValue": true}))
        .await
        .map_err(|e| e.to_string())?;
    let href = v
        .get("result")
        .and_then(|r| r.get("value"))
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    if href == expected_owned {
        Ok(json!({"url": href}))
    } else {
        Err(format!("wait_for_url_equals timeout: expected {:?}, got {:?}", expected_owned, href))
    }
}

async fn wait_for_ready_complete(runtime: &super::connection::BrowserRuntime, timeout: Duration) -> Result<(), String> {
    let rt = runtime.clone();
    let res = crate::browser_runtime::wait::wait_for(
        {
            let rt = rt.clone();
            move || {
                let rt = rt.clone();
                async move {
                    let sess = rt.diagnostics().attached_session_ids.into_iter().next();
                    let v = rt
                        .call(sess.as_deref(), "Runtime.evaluate", json!({"expression": "document.readyState", "returnByValue": true}))
                        .await;
                    let ready = v.ok().and_then(|val| {
                        val.get("result")
                            .and_then(|r| r.get("value"))
                            .and_then(|x| x.as_str())
                            .map(|s| s.to_string())
                    });
                    ready.as_deref() == Some("complete")
                }
            }
        },
        || runtime.subscribe(),
        |ev| ev.method == "Page.lifecycleEvent",
        timeout,
    )
    .await;
    match res {
        Ok(_) => Ok(()),
        Err(e) if matches!(e, crate::browser_runtime::error::RuntimeError::Timeout { .. }) => {
            // Not fatal — caller may still succeed if URL already changed; return Ok to allow wait_for_url to decide
            Ok(())
        }
        Err(e) => Err(e.to_string()),
    }
}

fn map_key(k: &str) -> (String, String) {
    let lower = k.to_lowercase();
    match lower.as_str() {
        "enter" => ("Enter".into(), "Enter".into()),
        "escape" | "esc" => ("Escape".into(), "Escape".into()),
        "tab" => ("Tab".into(), "Tab".into()),
        "arrowleft" => ("ArrowLeft".into(), "ArrowLeft".into()),
        "arrowright" => ("ArrowRight".into(), "ArrowRight".into()),
        "arrowup" => ("ArrowUp".into(), "ArrowUp".into()),
        "arrowdown" => ("ArrowDown".into(), "ArrowDown".into()),
        "backspace" => ("Backspace".into(), "Backspace".into()),
        "delete" | "del" => ("Delete".into(), "Delete".into()),
        _ if k.len()==1 => (k.to_string(), format!("Key{}", k.to_uppercase())),
        _ => (k.to_string(), k.to_string()),
    }
}

// ---------------------------------------------------------------------------
// Tests — validate without eager resolution
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn validate_rejects_empty_plan() {
        let p = ExecutionPlan { steps: vec![], step_index: 0 };
        assert!(p.validate().is_err());
    }

    #[test]
    fn validate_navigate_requires_url() {
        let p = ExecutionPlan { steps: vec![PlanStep::Navigate { url: "".to_string() }], step_index: 0 };
        assert!(p.validate().is_err());
        let p2 = ExecutionPlan { steps: vec![PlanStep::Navigate { url: "https://example.com".to_string() }], step_index: 0 };
        assert!(p2.validate().is_ok());
    }

    #[test]
    fn validate_does_not_eagerly_resolve_post_navigation_targets() {
        // Click selector that doesn't exist yet — validation must pass because
        // the element will exist after the prior navigate step runs.
        let p = ExecutionPlan { steps: vec![
            PlanStep::Navigate { url: "https://example.com".to_string() },
            PlanStep::Click { selector: Some("#future-button".to_string()), r#ref: None, element: None },
        ], step_index: 0 };
        assert!(p.validate().is_ok(), "validate must not eagerly resolve post-navigation target");
    }

    #[test]
    fn json_round_trip_tagged() {
        let plan = ExecutionPlan { steps: vec![
            PlanStep::Navigate { url: "https://example.com".to_string() },
            PlanStep::Click { selector: Some("#x".to_string()), r#ref: None, element: None },
            PlanStep::Type { text: "hello".to_string(), selector: Some("#in".to_string()), r#ref: None, submit: true },
            PlanStep::Wait { time: None, url_contains: Some("q=hello".to_string()), url_equals: None, lifecycle: None, timeout_ms: Some(5000) },
            PlanStep::Extract { selector: Some("#results".to_string()), attribute: None, instruction: None, text: None },
        ], step_index: 2 };
        let v = plan.to_json();
        // tagged enum checks
        assert_eq!(v["steps"][0]["type"], "navigate");
        assert_eq!(v["steps"][1]["type"], "click");
        assert_eq!(v["steps"][4]["type"], "extract");
        assert_eq!(v["step_index"], 2);
        let back = ExecutionPlan::from_json(&v).unwrap();
        assert_eq!(back.steps.len(), 5);
        assert_eq!(back.step_index, 2);
    }

    #[test]
    fn bare_array_json_accepted() {
        let v = json!([{"type":"navigate","url":"https://example.com"}, {"type":"snapshot"}]);
        let p = ExecutionPlan::from_json(&v).unwrap();
        assert_eq!(p.steps.len(), 2);
    }

    #[test]
    fn capability_tagging_per_step() {
        let steps = vec![
            PlanStep::Navigate { url: "https://example.com".to_string() },
            PlanStep::Eval { expression: "1+1".to_string() },
            PlanStep::Tabs,
            PlanStep::Click { selector: Some("#a".to_string()), r#ref: None, element: None },
        ];
        assert_eq!(steps[0].capability(), CapabilityClass::Navigation);
        assert_eq!(steps[1].capability(), CapabilityClass::RuntimeEvaluate);
        assert_eq!(steps[2].capability(), CapabilityClass::None);
        assert_eq!(steps[3].capability(), CapabilityClass::RuntimeEvaluate);
    }

    #[test]
    fn invalid_contains_rejected() {
        let s = PlanStep::Click { selector: Some("button:contains(\"x\")".to_string()), r#ref: None, element: None };
        assert!(s.validate().is_err());
    }

    #[test]
    fn wait_variants_validate() {
        let w = PlanStep::Wait { time: Some(1.5), url_contains: None, url_equals: None, lifecycle: None, timeout_ms: None };
        assert!(w.validate().is_ok());
        let w2 = PlanStep::Wait { time: Some(-1.0), url_contains: None, url_equals: None, lifecycle: None, timeout_ms: None };
        assert!(w2.validate().is_err());
        let w3 = PlanStep::Wait { time: None, url_contains: Some("q=".to_string()), url_equals: None, lifecycle: None, timeout_ms: Some(3000) };
        assert!(w3.validate().is_ok());
    }

    #[test]
    fn extract_validate() {
        let e = PlanStep::Extract { selector: Some("#x".to_string()), attribute: None, instruction: None, text: None };
        assert!(e.validate().is_ok());
        let e2 = PlanStep::Extract { selector: None, attribute: None, instruction: None, text: None };
        assert!(e2.validate().is_err());
    }
}
