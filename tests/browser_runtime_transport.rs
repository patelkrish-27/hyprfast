//! Phase 1 DoD — transport verification against a REAL browser.
//!
//! Prerequisites (DoD launch):
//! ```sh
//! brave --headless=new --remote-debugging-port=9222 \
//!   --user-data-dir=/tmp/hyprfast-runtime-test about:blank &
//! ```
//! No mocks, no stub CDP (except the single negative-test stub, which is
//! explicitly allowed by the DoD for the `UnsupportedBrowserProtocol` path
//! and is never used as a stand-in for the real transport).

use hyprfast::browser_runtime::{BrowserRuntime, RuntimeConfig, RuntimeError};
use futures::SinkExt as _;
use serde_json::{Value, json};
use std::time::{Duration, Instant};

const HOST: &str = "127.0.0.1";
const SUITE_PORT: u16 = 9222;

fn base(port: u16) -> String {
    format!("http://{HOST}:{port}")
}

async fn browser_ws_url(port: u16) -> String {
    let v: Value = reqwest::Client::new()
        .get(format!("{}/json/version", base(port)))
        .send()
        .await
        .unwrap_or_else(|_| panic!("browser must be reachable at {} (launch per DoD)", base(port)))
        .json()
        .await
        .expect("parse /json/version");
    v["webSocketDebuggerUrl"]
        .as_str()
        .expect("webSocketDebuggerUrl present")
        .to_string()
}

async fn wait_for_browser(port: u16, deadline: Duration) {
    let start = Instant::now();
    loop {
        if reqwest::Client::new()
            .get(format!("{}/json/version", base(port)))
            .send()
            .await
            .is_ok()
        {
            return;
        }
        if start.elapsed() > deadline {
            panic!("browser on port {port} did not appear within {deadline:?}");
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

// ---------------------------------------------------------------------------
// Suite: transport (100 concurrent requests over ONE socket) + ordering
// ---------------------------------------------------------------------------

#[tokio::test]
async fn phase1_transport_and_ordering() {
    // ---- setup: 2 fresh real targets -------------------------------------
    let http = reqwest::Client::new();
    let mut created_ids = Vec::new();
    for _ in 0..2 {
        let t: Value = http
            .put(format!("{}/json/new", base(SUITE_PORT)))
            .query(&[("url", "about:blank")])
            .send()
            .await
            .expect("PUT /json/new")
            .json()
            .await
            .expect("parse new target");
        created_ids.push(t["id"].as_str().unwrap_or("").to_string());
    }

    let ws_url = browser_ws_url(SUITE_PORT).await;

    // ---- transport: 100 real concurrent requests, 2 sessions, 1 socket ---
    let rt = BrowserRuntime::connect(&ws_url)
        .await
        .expect("connect to real browser");
    assert!(rt.is_alive(), "runtime alive after connect");
    let diag = rt.diagnostics();
    println!("browser_info: {:?}", diag.browser_info);
    assert!(
        !diag.browser_info.product.is_empty(),
        "Browser.getVersion stored product"
    );
    assert!(
        diag.attached_session_ids.len() >= 2,
        "expected >=2 attached sessions, got {:?}",
        diag.attached_session_ids
    );
    let s1 = diag.attached_session_ids[0].clone();
    let s2 = diag.attached_session_ids[1].clone();
    println!("sessions under test: {s1} {s2}");

    let started = Instant::now();
    let mut handles = Vec::new();
    for i in 0..100usize {
        let rt = rt.clone();
        let sid = if i % 2 == 0 { s1.clone() } else { s2.clone() };
        handles.push(tokio::spawn(async move {
            rt.call(
                Some(&sid),
                "Runtime.evaluate",
                json!({"expression": "40+2", "returnByValue": true}),
            )
            .await
        }));
    }
    let results = futures::future::join_all(handles).await;
    let elapsed = started.elapsed();
    let mut failures = 0usize;
    for (i, r) in results.into_iter().enumerate() {
        match r.expect("task join") {
            Ok(v) => assert_eq!(
                v.get("result").and_then(|r| r.get("value")).and_then(Value::as_i64),
                Some(42),
                "request {i} wrong value: {v}"
            ),
            Err(e) => {
                failures += 1;
                eprintln!("request {i} failed: {e}");
            }
        }
    }
    assert_eq!(failures, 0, "100 requests, zero failures");
    println!("100 concurrent requests across 2 sessions in {elapsed:?}, 0 failures");
    let diag = rt.diagnostics();
    println!(
        "post-transport diagnostics: pending={} events={} reorder_cur={} reorder_max={} offloaded={}",
        diag.pending_request_count,
        diag.event_count,
        diag.reorder_buffer_depth_current,
        diag.reorder_buffer_depth_max,
        diag.offloaded_decode_count
    );
    assert_eq!(diag.pending_request_count, 0, "no leaked pending requests");
    rt.shutdown().await;
    drop(rt);
    tokio::time::sleep(Duration::from_secs(1)).await;

    // ---- ordering: huge AX tree (offloaded) + small evals -----------------
    // Small threshold forces the rule-25 offload path deterministically.
    let config = RuntimeConfig {
        decode_offload_threshold_bytes: 64 * 1024,
        ..RuntimeConfig::default()
    };
    let rt = BrowserRuntime::connect_with_config(&ws_url, config)
        .await
        .expect("reconnect for ordering test");
    let diag = rt.diagnostics();
    let (os1, os2) = (
        diag.attached_session_ids[0].clone(),
        diag.attached_session_ids[1].clone(),
    );
    let mut events_rx = rt.subscribe();

    // Build a multi-MB DOM on target 1 (small result: just the count).
    // Sized so the AX JSON lands in the low MBs: over the 64KB test
    // offload threshold, under the 64MB app-level cap. (120k spans
    // produced an 86MB AX payload — over every limit; tried for real.)
    let n: i64 = rt
        .call_with_timeout(
            Some(&os1),
            "Runtime.evaluate",
            json!({
                "expression": "(() => { document.body.innerHTML = Array(20000).fill('<span class=\"n\">payload-text-0123456789</span>').join(''); return document.querySelectorAll('span').length; })()",
                "returnByValue": true,
            }),
            Duration::from_secs(60),
        )
        .await
        .expect("build big DOM")["result"]["value"]
        .as_i64()
        .expect("span count");
    assert_eq!(n, 20000);
    println!("big DOM built: {n} spans");

    // Fire the huge AX fetch, then small evals on the OTHER session.
    let rt_ax = rt.clone();
    let os1c = os1.clone();
    let ax_started = Instant::now();
    let ax_handle = tokio::spawn(async move {
        rt_ax
            .call_with_timeout(
                Some(&os1c),
                "Accessibility.getFullAXTree",
                json!({}),
                Duration::from_secs(180),
            )
            .await
    });
    // Give the AX request a head start so its (large) response is decoded
    // on the worker while small frames arrive behind it.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let small_started = Instant::now();
    let mut small_handles = Vec::new();
    for i in 0..10usize {
        let rt = rt.clone();
        let sid = os2.clone();
        small_handles.push(tokio::spawn(async move {
            let t = Instant::now();
            let r = rt
                .call(
                    Some(&sid),
                    "Runtime.evaluate",
                    json!({"expression": format!("{i}*7"), "returnByValue": true}),
                )
                .await;
            (i, t.elapsed(), r)
        }));
    }
    let small_results = futures::future::join_all(small_handles).await;
    let mut small_max_latency = Duration::ZERO;
    for h in small_results {
        let (i, latency, r) = h.expect("join");
        small_max_latency = small_max_latency.max(latency);
        let v = r.unwrap_or_else(|e| panic!("small eval {i} failed: {e}"));
        assert_eq!(v.get("result").and_then(|r| r.get("value")).and_then(Value::as_i64), Some(i as i64 * 7));
    }
    let small_total = small_started.elapsed();

    // Keep streaming small evals until the AX response lands. Frames that
    // arrive while the multi-MB AX payload is still decoding on the worker
    // must wait in the reorder buffer and dispatch in wire order.
    use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
    let streaming_done = std::sync::Arc::new(AtomicBool::new(false));
    let rt_stream = rt.clone();
    let sid_stream = os2.clone();
    let done_flag = streaming_done.clone();
    let stream_handle = tokio::spawn(async move {
        let (mut count, mut ok) = (0u64, 0u64);
        while !done_flag.load(AtomicOrdering::SeqCst) && count < 20_000 {
            match rt_stream
                .call(
                    Some(&sid_stream),
                    "Runtime.evaluate",
                    json!({"expression": "1", "returnByValue": true}),
                )
                .await
            {
                Ok(_) => ok += 1,
                Err(e) => panic!("streamed eval failed: {e}"),
            }
            count += 1;
        }
        (count, ok)
    });
    let ax = ax_handle.await.expect("join").expect("AX tree call");
    streaming_done.store(true, AtomicOrdering::SeqCst);
    let (streamed, streamed_ok) = stream_handle.await.expect("join");
    assert_eq!(streamed, streamed_ok, "every streamed eval succeeds");
    println!("streamed {streamed} small evals during AX flight, all ok");
    let ax_elapsed = ax_started.elapsed();
    let nodes = ax.get("nodes").and_then(Value::as_array).cloned().unwrap_or_default();
    println!(
        "AX nodes={} in {ax_elapsed:?}; 10 small evals total={small_total:?} max_single={small_max_latency:?}",
        nodes.len()
    );
    assert!(nodes.len() > 8_000, "AX tree is genuinely large");

    let diag = rt.diagnostics();
    println!(
        "ordering diagnostics: offloaded={} reorder_cur={} reorder_max={} events={} oversize_dropped={}",
        diag.offloaded_decode_count,
        diag.reorder_buffer_depth_current,
        diag.reorder_buffer_depth_max,
        diag.event_count,
        diag.oversize_dropped_count
    );
    assert!(
        diag.offloaded_decode_count >= 1,
        "large AX response must have taken the offload path"
    );
    assert!(
        diag.reorder_buffer_depth_max >= 1,
        "a small frame must have waited in the reorder buffer behind the offloaded decode"
    );

    // Event stream arrived in non-decreasing wire-sequence order.
    let mut seqs = Vec::new();
    while let Ok(ev) = events_rx.try_recv() {
        seqs.push(ev.sequence);
    }
    println!("collected {} events during ordering window", seqs.len());
    assert!(
        seqs.windows(2).all(|w| w[0] <= w[1]),
        "events delivered in wire-sequence order"
    );

    rt.shutdown().await;
    for id in &created_ids {
        let _ = http
            .get(format!("{}/json/close/{}", base(SUITE_PORT), id))
            .send()
            .await;
    }
}

// ---------------------------------------------------------------------------
// Kill test: in-flight requests resolve RuntimeDead, never hang/Timeout
// ---------------------------------------------------------------------------

#[tokio::test]
async fn phase1_kill_browser_resolves_runtime_dead() {
    let port: u16 = 19224;
    let data_dir = "/tmp/hyprfast-runtime-killtest";
    let _ = std::fs::remove_dir_all(data_dir);
    std::fs::create_dir_all(data_dir).expect("mkdir killtest profile");

    let mut child = tokio::process::Command::new("brave")
        .args([
            "--headless=new",
            &format!("--remote-debugging-port={port}"),
            &format!("--user-data-dir={data_dir}"),
            "--no-sandbox",
            "--disable-gpu",
            "about:blank",
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn brave for kill test");
    wait_for_browser(port, Duration::from_secs(25)).await;

    let ws_url = browser_ws_url(port).await;
    let rt = BrowserRuntime::connect(&ws_url)
        .await
        .expect("connect kill-test browser");
    assert!(rt.is_alive());

    // In-flight blocking evaluate on the attached session (or browser level).
    let session = rt.diagnostics().attached_session_ids.into_iter().next();
    let rt2 = rt.clone();
    let in_flight = tokio::spawn(async move {
        rt2.call_with_timeout(
            session.as_deref(),
            "Runtime.evaluate",
            json!({
                "expression": "new Promise((resolve) => setTimeout(() => resolve(1), 60000))",
                "awaitPromise": true,
                "returnByValue": true,
            }),
            Duration::from_secs(90),
        )
        .await
    });

    // Wait until the request is genuinely dispatched (real state, not sleep).
    let start = Instant::now();
    loop {
        if rt.diagnostics().pending_request_count >= 1 {
            break;
        }
        if start.elapsed() > Duration::from_secs(10) {
            panic!("blocking call never reached in-flight state");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    child.kill().await.expect("SIGKILL browser");
    let kill_at = Instant::now();
    let outcome = tokio::time::timeout(Duration::from_secs(45), in_flight)
        .await
        .expect("in-flight request must resolve, never hang")
        .expect("join");
    let resolve_latency = kill_at.elapsed();
    println!("in-flight resolved {resolve_latency:?} after SIGKILL: {outcome:?}");
    match outcome {
        Err(RuntimeError::RuntimeDead(_)) => {}
        other => panic!("expected RuntimeDead after kill, got {other:?}"),
    }
    assert!(
        resolve_latency < Duration::from_secs(30),
        "resolved via close path, not via the 90s call timeout"
    );

    // Future calls fail fast with RuntimeDead.
    let t = Instant::now();
    let err = rt
        .call(None, "Browser.getVersion", json!({}))
        .await
        .expect_err("calls after death must fail");
    assert!(
        matches!(err, RuntimeError::RuntimeDead(_)),
        "expected immediate RuntimeDead, got {err:?}"
    );
    assert!(t.elapsed() < Duration::from_secs(5), "fails immediately");
    assert!(!rt.is_alive());
    rt.shutdown().await;
    let _ = child.wait().await;
    let _ = std::fs::remove_dir_all(data_dir);
}

// ---------------------------------------------------------------------------
// Negative test: setAutoAttach failure -> UnsupportedBrowserProtocol
// (stub endpoint used ONLY for this path, never for transport behavior)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn phase1_flat_session_failure_is_unsupported_protocol() {
    use futures::StreamExt;
    use tokio_tungstenite::tungstenite::Message;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:19225")
        .await
        .expect("bind stub");
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept");
        let mut ws = tokio_tungstenite::accept_async(stream).await.expect("ws accept");
        // 1. Browser.getVersion -> minimal ok.
        let msg = ws.next().await.expect("msg1").expect("ok1").into_text().expect("text1");
        let v: Value = serde_json::from_str(&msg).expect("json1");
        let id = v["id"].clone();
        ws.send(Message::Text(format!("{{\"id\":{id},\"result\":{{\"product\":\"Stub/1.0\"}}}}").into()))
            .await
            .expect("send1");
        // 2. Target.setAutoAttach -> protocol error (no flat sessions here).
        let msg = ws.next().await.expect("msg2").expect("ok2").into_text().expect("text2");
        let v: Value = serde_json::from_str(&msg).expect("json2");
        assert_eq!(v["method"], json!("Target.setAutoAttach"));
        let id = v["id"].clone();
        ws.send(Message::Text(
            format!("{{\"id\":{id},\"error\":{{\"code\":-32601,\"message\":\"'Target.setAutoAttach' wasn't found\"}}}}").into(),
        ))
        .await
        .expect("send2");
        tokio::time::sleep(Duration::from_secs(5)).await;
    });

    let outcome = tokio::time::timeout(
        Duration::from_secs(20),
        BrowserRuntime::connect("ws://127.0.0.1:19225"),
    )
    .await
    .expect("connect must not hang");
    match outcome {
        Err(RuntimeError::UnsupportedBrowserProtocol(detail)) => {
            println!("got expected UnsupportedBrowserProtocol: {detail}");
        }
        other => panic!("expected UnsupportedBrowserProtocol, got {other:?}"),
    }
    server.abort();
}

// ---------------------------------------------------------------------------
// No-browser unit checks: structured error surface
// ---------------------------------------------------------------------------

#[test]
fn phase1_error_display() {
    let e = RuntimeError::Timeout {
        method: "Runtime.evaluate".into(),
        timeout_ms: 500,
    };
    assert_eq!(e.to_string(), "CDP call Runtime.evaluate timed out after 500ms");
    assert!(RuntimeError::RuntimeDead("x".into()).to_string().contains("dead"));
    assert!(RuntimeError::ConnectionFailed("x".into())
        .to_string()
        .contains("failed"));
    assert!(RuntimeError::CdpError {
        method: "M".into(),
        code: -32601,
        message: "nope".into(),
    }
    .to_string()
    .contains("nope"));
    assert!(RuntimeError::InvalidResponse("big".into()).to_string().contains("big"));
    assert!(RuntimeError::UnsupportedBrowserProtocol("flat".into())
        .to_string()
        .contains("flat"));
}
