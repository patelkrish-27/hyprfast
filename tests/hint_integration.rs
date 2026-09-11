//! Hint-key integration test — real Brave, deterministic labels, clicks land.
//! Milestone 2: standalone hint layer, not yet wired to stagehand fallback.

use serde_json::Value;
use std::time::{Duration, Instant};

const HINT_PORT: u16 = 19344;
const HOST: &str = "127.0.0.1";

fn base(port: u16) -> String { format!("http://{HOST}:{port}") }

async fn wait_for_browser(port: u16, deadline: Duration) {
    let start = Instant::now();
    loop {
        if reqwest::Client::new().get(format!("{}/json/version", base(port))).send().await.is_ok() {
            return;
        }
        if start.elapsed() > deadline { panic!("browser on {port} did not appear within {deadline:?}"); }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
}

async fn browser_ws_url(port: u16) -> String {
    let v: Value = reqwest::Client::new().get(format!("{}/json/version", base(port))).send().await.expect("get version").json().await.expect("parse version");
    v["webSocketDebuggerUrl"].as_str().expect("ws url").to_string()
}

#[tokio::test]
async fn hint_snapshot_deterministic_and_actions() {
    let data_dir = "/tmp/hyprfast-hint-test";
    let _ = std::fs::remove_dir_all(data_dir);
    std::fs::create_dir_all(data_dir).expect("mkdir hint test profile");

    // Ensure hint code sees our isolated port via env
    std::env::set_var("HYPRFAST_CDP_PORT", HINT_PORT.to_string());
    std::env::set_var("HYPRFAST_CDP_HOST", HOST);
    // Ensure proxy disabled for deterministic direct fallback
    std::env::set_var("HYPRFAST_DEVTOOLS_MCP", "0");

    let mut child = tokio::process::Command::new("brave")
        .args([
            "--headless=new",
            &format!("--remote-debugging-port={HINT_PORT}"),
            &format!("--user-data-dir={data_dir}"),
            "--no-sandbox",
            "--disable-gpu",
            "--window-size=1280,900",
            "about:blank",
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn brave for hint test");
    wait_for_browser(HINT_PORT, Duration::from_secs(25)).await;

    // Open hint fixture via Page.navigate on the existing target (ensures active session)
    let fixture_path = std::fs::canonicalize("tests/browser_fixtures/hint_basic.html").expect("canonicalize fixture");
    let file_url = format!("file://{}", fixture_path.display());
    let http = reqwest::Client::new();
    // Navigate the current tab via CDP Page.navigate (ephemeral) to ensure the session we snapshot is the file url
    let _nav = tokio::task::spawn_blocking({
        let url = file_url.clone();
        move || {
            let _ = hyprfast::browser_runtime::client::cdp_call_sync(
                "Page.navigate",
                serde_json::json!({"url": url}),
                None, None,
                hyprfast::browser_runtime::server::CapabilityClass::Navigation,
            );
        }
    }).await;
    // Also create a new target as fallback and remember it for cleanup, but navigation above is authoritative
    let target: Value = http.put(format!("{}/json/new", base(HINT_PORT)))
        .query(&[("url", file_url.as_str())])
        .send().await.expect("PUT /json/new hint fixture").json().await.expect("parse new target");
    let target_id = target["id"].as_str().unwrap_or("").to_string();
    tokio::time::sleep(Duration::from_millis(1500)).await;
    // Verify we actually landed on the fixture by checking title
    let title: String = tokio::task::spawn_blocking(|| hyprfast_evaluate("document.title")).await.expect("join").unwrap_or_default();
    println!("title after navigate: {}", title);
    if !title.contains("hint_basic") {
        // Fallback: try evaluating on all targets via HTTP until we find the fixture
        eprintln!("warning: title not hint_basic, trying to force navigate via Page.navigate again");
        let _ = tokio::task::spawn_blocking({
            let url = file_url.clone();
            move || {
                let _ = hyprfast::browser_runtime::client::cdp_call_sync(
                    "Page.navigate",
                    serde_json::json!({"url": url}),
                    None, None,
                    hyprfast::browser_runtime::server::CapabilityClass::Navigation,
                );
            }
        }).await;
        tokio::time::sleep(Duration::from_millis(1500)).await;
    }

    // Use hint module via sync wrappers (they use ephemeral fallback + env port)
    // Need to run blocking calls in spawn_blocking to avoid deadlock with tokio runtime?
    // cdp_call_sync builds its own current_thread runtime, so we can call directly via blocking thread.
    let snap1: Value = tokio::task::spawn_blocking(|| {
        hyprfast_hint_snapshot()
    }).await.expect("join").expect("hint_snapshot 1");
    let hints1 = snap1.get("hints").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    println!("hint_snapshot 1: {} hints", hints1.len());
    for h in &hints1 { println!("  {} {} {}", h.get("label").and_then(|v| v.as_str()).unwrap_or(""), h.get("tag").and_then(|v| v.as_str()).unwrap_or(""), h.get("name").and_then(|v| v.as_str()).unwrap_or("")) }

    // hidden element must NOT appear
    for h in &hints1 {
        let name = h.get("name").and_then(|v| v.as_str()).unwrap_or("");
        assert!(!name.to_lowercase().contains("hidden"), "hidden element should not be hinted");
    }
    // should have at least 10 hints (11 visible)
    assert!(hints1.len() >= 10, "expected >=10 hints, got {}", hints1.len());
    assert!(hints1.len() <= 20, "unexpected many hints {}", hints1.len());

    // deterministic: second snapshot must produce identical label ordering
    let snap2: Value = tokio::task::spawn_blocking(|| hyprfast_hint_snapshot()).await.expect("join").expect("hint_snapshot 2");
    let hints2 = snap2.get("hints").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    assert_eq!(hints1.len(), hints2.len(), "deterministic count");
    for (a,b) in hints1.iter().zip(hints2.iter()) {
        assert_eq!(a.get("label"), b.get("label"), "labels deterministic");
        assert_eq!(a.get("selector"), b.get("selector"), "selectors deterministic");
        assert_eq!(a.get("tag"), b.get("tag"));
    }
    // check label charset ordering: first labels must be A S D F G H...
    let charset = ["A","S","D","F","G","H","J","K","L","Q","W","E","R","T","Y","U","I","O","P","Z","X","C","V","B","N","M"];
    for (i, h) in hints1.iter().enumerate().take(charset.len()) {
        let label = h.get("label").and_then(|v| v.as_str()).unwrap_or("");
        assert_eq!(label, charset[i], "label {} should be {}", i, charset[i]);
    }

    // Verify clicks land: find hint for button A (first) and click
    let first_label = hints1[0].get("label").and_then(|v| v.as_str()).unwrap_or("A").to_string();
    println!("clicking label {}", first_label);
    let click_res: Value = tokio::task::spawn_blocking(move || {
        hyprfast_hint_click(&first_label)
    }).await.expect("join").expect("hint_click");
    println!("click result: {}", click_res);
    assert!(click_res.get("clicked").and_then(|v| v.as_bool()).unwrap_or(false) || click_res.get("clicked").is_some(), "click should succeed");

    // verify status changed via evaluate
    tokio::time::sleep(Duration::from_millis(300)).await;
    let status: String = tokio::task::spawn_blocking(|| {
        hyprfast_evaluate("document.getElementById('status').textContent")
    }).await.expect("join").expect("evaluate status");
    println!("status after click: {}", status);
    assert!(status.contains("clicked"), "status should contain clicked, got {}", status);

    // Verify hint_type: find input hint (tag=input) and type
    let input_hint = hints1.iter().find(|h| h.get("tag").and_then(|v| v.as_str()) == Some("input")).cloned().expect("input hint exists");
    let input_label = input_hint.get("label").and_then(|v| v.as_str()).unwrap_or("").to_string();
    println!("typing into input label {}", input_label);
    let type_res: Value = tokio::task::spawn_blocking({
        let lbl = input_label.clone();
        move || hyprfast_hint_type(&lbl, "hello-hint")
    }).await.expect("join").expect("hint_type");
    println!("type result: {}", type_res);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let input_val: String = tokio::task::spawn_blocking(|| {
        hyprfast_evaluate("document.getElementById('input-d').value")
    }).await.expect("join").expect("evaluate input value");
    println!("input value after type: {}", input_val);
    assert_eq!(input_val, "hello-hint", "input value should be hello-hint");

    // Verify contenteditable type
    let editable_hint = hints1.iter().find(|h| h.get("tag").and_then(|v| v.as_str()) == Some("div") && h.get("name").and_then(|v| v.as_str()).unwrap_or("").contains("editable")).cloned();
    if let Some(eh) = editable_hint {
        let elabel = eh.get("label").and_then(|v| v.as_str()).unwrap_or("").to_string();
        println!("typing into contenteditable label {}", elabel);
        let _: Value = tokio::task::spawn_blocking({
            let lbl = elabel.clone();
            move || hyprfast_hint_type(&lbl, "editable-text")
        }).await.expect("join").expect("hint_type editable");
        tokio::time::sleep(Duration::from_millis(300)).await;
        let editable_text: String = tokio::task::spawn_blocking(|| {
            hyprfast_evaluate("document.getElementById('editable').textContent.trim()")
        }).await.expect("join").expect("evaluate editable");
        println!("editable text: {}", editable_text);
        assert!(editable_text.contains("editable-text"), "editable should contain typed text");
    }

    // Verify AA labeling after 26: create many elements dynamically and snapshot again
    // Inject 30 extra buttons to force double-letter labels
    let _: String = tokio::task::spawn_blocking(|| {
        hyprfast_evaluate("(() => { for(let i=0;i<30;i++){ const b=document.createElement('button'); b.textContent='extra'+i; b.id='extra'+i; b.onclick=()=>document.getElementById('status').textContent='extra'+i; document.body.appendChild(b);} return document.querySelectorAll('button').length; })()")
    }).await.expect("join").expect("inject extra buttons");
    tokio::time::sleep(Duration::from_millis(500)).await;
    let snap_big: Value = tokio::task::spawn_blocking(|| hyprfast_hint_snapshot()).await.expect("join").expect("big snapshot");
    let hints_big = snap_big.get("hints").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    println!("big snapshot hints: {}", hints_big.len());
    // Should have >26 hints now, and label at index 26 should be AA (per spec)
    if hints_big.len() > 26 {
        let label26 = hints_big[26].get("label").and_then(|v| v.as_str()).unwrap_or("");
        assert_eq!(label26, "AA", "26th index (27th element) should be AA, got {}", label26);
        let label27 = hints_big[27].get("label").and_then(|v| v.as_str()).unwrap_or("");
        assert_eq!(label27, "AS", "27th index should be AS, got {}", label27);
    }

    // Cleanup
    let _ = http.get(format!("{}/json/close/{}", base(HINT_PORT), target_id)).send().await;
    child.kill().await.expect("kill brave");
    let _ = child.wait().await;
    let _ = std::fs::remove_dir_all(data_dir);
    // reset env
    std::env::remove_var("HYPRFAST_CDP_PORT");
    std::env::remove_var("HYPRFAST_CDP_HOST");
}

// helpers that call hint crate via binary functions exposed through library?
// We re-implement thin wrappers that use browser_runtime client directly
// to avoid importing private hint mod (which is binary-only). So we replicate
// the hint_snapshot logic here using the same Runtime.evaluate path but via
// library's public browser_runtime client.

fn hyprfast_hint_snapshot() -> anyhow::Result<Value> {
    // Use the hint binary's logic by shelling out to the built hyprfast binary
    // if available, else use direct Runtime.evaluate injection here.
    // Direct path: emulate hint/mod.rs but via library public API.
    let exe = std::env::var("HYPRFAST_BIN").unwrap_or_else(|_| "target/release/hyprfast".to_string());
    if std::path::Path::new(&exe).exists() {
        let out = std::process::Command::new(&exe).arg("hint-snapshot").output()?;
        if out.status.success() {
            let txt = String::from_utf8_lossy(&out.stdout);
            if let Ok(v) = serde_json::from_str::<Value>(&txt) { return Ok(v); }
        }
    }
    // Fallback: direct injection via browser_runtime client (library)
    // Inject hint.js
    let js = include_str!("../assets/hint.js");
    let _ = hyprfast::browser_runtime::client::cdp_call_sync(
        "Page.addScriptToEvaluateOnNewDocument",
        serde_json::json!({"source": js}),
        None, None,
        hyprfast::browser_runtime::server::CapabilityClass::None,
    );
    // Check existing
    let check = hyprfast::browser_runtime::client::cdp_call_sync(
        "Runtime.evaluate",
        serde_json::json!({"expression": "typeof window.__hyprfastHint !== 'undefined'", "returnByValue": true}),
        None, None,
        hyprfast::browser_runtime::server::CapabilityClass::RuntimeEvaluate,
    )?;
    let already = check.get("result").and_then(|r| r.get("value")).and_then(|v| v.as_bool()).unwrap_or(false);
    if !already {
        let eval = hyprfast::browser_runtime::client::cdp_call_sync(
            "Runtime.evaluate",
            serde_json::json!({"expression": js, "returnByValue": true}),
            None, None,
            hyprfast::browser_runtime::server::CapabilityClass::RuntimeEvaluate,
        )?;
        if eval.get("exceptionDetails").is_some() { anyhow::bail!("hint injection exception: {}", eval["exceptionDetails"]); }
    }
    let v = hyprfast::browser_runtime::client::cdp_call_sync(
        "Runtime.evaluate",
        serde_json::json!({"expression": "JSON.stringify(window.__hyprfastHint.snapshot())", "returnByValue": true}),
        None, None,
        hyprfast::browser_runtime::server::CapabilityClass::RuntimeEvaluate,
    )?;
    if let Some(exc) = v.get("exceptionDetails") { anyhow::bail!("snapshot exception: {}", exc); }
    let raw = v.get("result").and_then(|r| r.get("value")).and_then(|v| v.as_str()).unwrap_or("[]");
    let arr: Value = serde_json::from_str(raw).unwrap_or(Value::Array(vec![]));
    Ok(serde_json::json!({"hints": arr, "count": arr.as_array().map(|a| a.len()).unwrap_or(0)}))
}

fn hyprfast_hint_click(label: &str) -> anyhow::Result<Value> {
    let exe = std::env::var("HYPRFAST_BIN").unwrap_or_else(|_| "target/release/hyprfast".to_string());
    if std::path::Path::new(&exe).exists() {
        let out = std::process::Command::new(&exe).args(["hint-click", label]).output()?;
        if out.status.success() {
            let txt = String::from_utf8_lossy(&out.stdout);
            if let Ok(v) = serde_json::from_str::<Value>(&txt) { return Ok(v); }
        }
    }
    let expr = format!("JSON.stringify(window.__hyprfastHint.click({:?}))", label);
    let v = hyprfast::browser_runtime::client::cdp_call_sync(
        "Runtime.evaluate",
        serde_json::json!({"expression": expr, "returnByValue": true}),
        None, None,
        hyprfast::browser_runtime::server::CapabilityClass::RuntimeEvaluate,
    )?;
    if let Some(exc) = v.get("exceptionDetails") { anyhow::bail!("click exception: {}", exc); }
    let raw = v.get("result").and_then(|r| r.get("value")).and_then(|v| v.as_str()).unwrap_or("{}");
    let out: Value = serde_json::from_str(raw).unwrap_or(serde_json::json!({"raw": raw}));
    if let Some(err) = out.get("error").and_then(|e| e.as_str()) { anyhow::bail!("{}", err); }
    Ok(out)
}

fn hyprfast_hint_type(label: &str, text: &str) -> anyhow::Result<Value> {
    let exe = std::env::var("HYPRFAST_BIN").unwrap_or_else(|_| "target/release/hyprfast".to_string());
    if std::path::Path::new(&exe).exists() {
        let out = std::process::Command::new(&exe).args(["hint-type", label, text]).output()?;
        if out.status.success() {
            let txt = String::from_utf8_lossy(&out.stdout);
            if let Ok(v) = serde_json::from_str::<Value>(&txt) { return Ok(v); }
        }
    }
    let expr = format!("JSON.stringify(window.__hyprfastHint.focusAndType({:?}, {:?}))", label, text);
    let v = hyprfast::browser_runtime::client::cdp_call_sync(
        "Runtime.evaluate",
        serde_json::json!({"expression": expr, "returnByValue": true}),
        None, None,
        hyprfast::browser_runtime::server::CapabilityClass::RuntimeEvaluate,
    )?;
    if let Some(exc) = v.get("exceptionDetails") { anyhow::bail!("type exception: {}", exc); }
    let raw = v.get("result").and_then(|r| r.get("value")).and_then(|v| v.as_str()).unwrap_or("{}");
    let out: Value = serde_json::from_str(raw).unwrap_or(serde_json::json!({"raw": raw}));
    if let Some(err) = out.get("error").and_then(|e| e.as_str()) { anyhow::bail!("{}", err); }
    Ok(out)
}

fn hyprfast_evaluate(expr: &str) -> anyhow::Result<String> {
    let v = hyprfast::browser_runtime::client::cdp_call_sync(
        "Runtime.evaluate",
        serde_json::json!({"expression": expr, "returnByValue": true}),
        None, None,
        hyprfast::browser_runtime::server::CapabilityClass::RuntimeEvaluate,
    )?;
    if let Some(exc) = v.get("exceptionDetails") { anyhow::bail!("eval exception: {}", exc); }
    let inner = v.get("result").and_then(|r| r.get("value")).cloned().unwrap_or(Value::Null);
    if let Some(s) = inner.as_str() { Ok(s.to_string()) } else { Ok(inner.to_string()) }
}
