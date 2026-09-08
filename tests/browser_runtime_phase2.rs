//! Phase 2 DoD — daemon + lifecycle + IPC protocol.
//!
//! Unit tests (no browser): socket mode, permission table, handshake
//! mismatch, pre-handshake rejection, stale/live sockets, error wire format.
//!
//! Integration tests (REAL browser, dedicated ports/profiles — never the
//! dev browser on 9222): full lifecycle with 20 concurrent evals, and
//! per-target ordering.

use hyprfast::browser_runtime::{
    BrowserRuntime, BrowserRuntimeServer, CapabilityClass, ClientConn, IPC_PROTOCOL_VERSION,
    LifecycleState, RequestKind, RuntimeError, RuntimeStatus, ServeOptions, bind_socket_exclusive,
    probe_live,
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const PROBE_TIMEOUT: Duration = Duration::from_millis(800);

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn test_sock(test: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("hyprfast-p2-{}", test));
    std::fs::create_dir_all(&dir).expect("mkdir test dir");
    dir.join("hyprfast-browser.sock")
}

fn cleanup_sock(sock: &Path) {
    let _ = std::fs::remove_file(sock);
}

async fn wait_live(sock: &Path, deadline: Duration) {
    let start = Instant::now();
    while !probe_live(sock, PROBE_TIMEOUT).await {
        if start.elapsed() > deadline {
            panic!("daemon on {} never became live", sock.display());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Spawn a `Connected` test server on an isolated socket. `runtime=None`
/// exercises handshake/socket paths without a browser.
async fn spawn_test_server(
    sock: PathBuf,
    runtime: Option<BrowserRuntime>,
) -> tokio::task::JoinHandle<hyprfast::browser_runtime::RuntimeResult<()>> {
    let listener = bind_socket_exclusive(&sock).await.expect("bind test socket");
    let server = BrowserRuntimeServer::new_for_test(sock.clone(), runtime);
    let handle = tokio::spawn(async move { server.serve_listener(listener).await });
    wait_live(&sock, Duration::from_secs(10)).await;
    handle
}

async fn stop_test_server(sock: &Path, handle: tokio::task::JoinHandle<hyprfast::browser_runtime::RuntimeResult<()>>) {
    let mut conn = ClientConn::connect(sock).await.expect("connect for stop");
    conn.stop().await.expect("stop op");
    tokio::time::timeout(Duration::from_secs(15), handle)
        .await
        .expect("server task must finish after stop")
        .expect("join")
        .expect("serve_listener Ok");
    assert!(
        std::fs::symlink_metadata(sock).is_err(),
        "socket {} must be gone after stop",
        sock.display()
    );
}

// ---------------------------------------------------------------------------
// Unit: socket mode 0600 (rule 23 / I20)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn phase2_socket_mode_0600_after_bind() {
    let sock = test_sock("mode");
    cleanup_sock(&sock);
    let listener = bind_socket_exclusive(&sock).await.expect("bind");
    let mode = hyprfast::browser_runtime::socket_mode_octal(&sock);
    assert_eq!(mode, "0600", "socket must be owner-only, got {mode}");
    drop(listener);
    cleanup_sock(&sock);
}

// ---------------------------------------------------------------------------
// Unit: lifecycle permission table (§1)
// ---------------------------------------------------------------------------

#[test]
fn phase2_state_permission_table() {
    use LifecycleState as S;
    let cases: Vec<(S, bool, bool)> = vec![
        // (state, read_ok, state_changing_ok)
        (S::Starting, true, false),
        (S::Connecting, true, false),
        (S::Connected, true, true),
        (S::Degraded, true, false),
        (S::Reconnecting, true, false),
        (S::Disconnected, true, false),
        (S::Stopping, true, false),
        (S::Stopped, false, false),
        (S::Failed("boom".into()), true, false),
    ];
    for (state, read_ok, sc_ok) in cases {
        let name = state.name().to_string();
        assert_eq!(
            state.check(RequestKind::Read).is_ok(),
            read_ok,
            "{name} read permission"
        );
        assert_eq!(
            state.check(RequestKind::StateChanging).is_ok(),
            sc_ok,
            "{name} state-changing permission"
        );
    }
    // Structured, state-specific errors — never generic.
    assert!(matches!(
        S::Starting.check(RequestKind::StateChanging),
        Err(RuntimeError::NotReady(_))
    ));
    assert!(matches!(
        S::Connecting.check(RequestKind::StateChanging),
        Err(RuntimeError::NotReady(_))
    ));
    assert!(matches!(
        S::Degraded.check(RequestKind::StateChanging),
        Err(RuntimeError::Degraded(_))
    ));
    assert!(matches!(
        S::Reconnecting.check(RequestKind::StateChanging),
        Err(RuntimeError::Reconnecting(_))
    ));
    assert!(matches!(
        S::Disconnected.check(RequestKind::StateChanging),
        Err(RuntimeError::RuntimeDead(_))
    ));
    assert!(matches!(
        S::Stopped.check(RequestKind::Read),
        Err(RuntimeError::RuntimeDead(_))
    ));
    assert!(matches!(
        S::Stopping.check(RequestKind::StateChanging),
        Err(RuntimeError::ShuttingDown(_))
    ));
    match S::Failed("kablam".into()).check(RequestKind::StateChanging) {
        Err(RuntimeError::RuntimeDead(d)) => assert!(d.contains("kablam"), "reason preserved: {d}"),
        other => panic!("expected RuntimeDead with reason, got {other:?}"),
    }
}

#[test]
fn phase2_state_transitions_follow_diagram() {
    use LifecycleState as S;
    let mut s = S::Starting;
    assert!(s.transition_to(S::Connected).is_err(), "no skip Starting->Connected");
    assert!(s.transition_to(S::Connecting).is_ok());
    assert!(s.transition_to(S::Connected).is_ok());
    assert!(s.transition_to(S::Degraded).is_ok());
    assert!(s.transition_to(S::Starting).is_err(), "no backward edge");
    assert!(s.transition_to(S::Reconnecting).is_ok());
    assert!(s.transition_to(S::Connected).is_ok());
    // Any non-terminal state may begin shutdown; Stopped is terminal.
    assert!(s.transition_to(S::Stopping).is_ok());
    assert!(s.transition_to(S::Connected).is_err(), "nothing leaves Stopping but Stopped");
    assert!(s.transition_to(S::Stopped).is_ok());
    assert!(s.transition_to(S::Stopping).is_err(), "Stopped is terminal");
    let mut f = S::Connecting;
    assert!(f.transition_to(S::Failed("x".into())).is_ok());
    assert!(matches!(f, S::Failed(_)));
    assert!(f.transition_to(S::Stopping).is_ok(), "Failed may still shut down");
}

// ---------------------------------------------------------------------------
// Unit: capability tagging (rule 32)
// ---------------------------------------------------------------------------

#[test]
fn phase2_capability_tags_parse() {
    assert_eq!(CapabilityClass::parse("runtime_evaluate"), CapabilityClass::RuntimeEvaluate);
    assert_eq!(CapabilityClass::parse("cookies"), CapabilityClass::Cookies);
    assert_eq!(CapabilityClass::parse("clipboard"), CapabilityClass::Clipboard);
    assert_eq!(CapabilityClass::parse("file_upload"), CapabilityClass::FileUpload);
    assert_eq!(CapabilityClass::parse("download"), CapabilityClass::Download);
    assert_eq!(CapabilityClass::parse("navigation"), CapabilityClass::Navigation);
    assert_eq!(CapabilityClass::parse("none"), CapabilityClass::None);
    assert_eq!(CapabilityClass::parse("future-thing"), CapabilityClass::None);
    assert_eq!(CapabilityClass::all().len(), 6);
}

// ---------------------------------------------------------------------------
// Unit: error wire round-trip (structured, never generic)
// ---------------------------------------------------------------------------

#[test]
fn phase2_error_wire_roundtrip() {
    let variants = vec![
        RuntimeError::ConnectionFailed("c".into()),
        RuntimeError::RuntimeDead("d".into()),
        RuntimeError::Timeout { method: "M".into(), timeout_ms: 7 },
        RuntimeError::CdpError { method: "M".into(), code: -1, message: "m".into() },
        RuntimeError::InvalidResponse("i".into()),
        RuntimeError::UnsupportedBrowserProtocol("u".into()),
        RuntimeError::NotReady("n".into()),
        RuntimeError::Reconnecting("r".into()),
        RuntimeError::ShuttingDown("s".into()),
        RuntimeError::Degraded("g".into()),
        RuntimeError::ProtocolMismatch { expected: 1, got: 999 },
        RuntimeError::HandshakeRequired("h".into()),
        RuntimeError::DaemonAlreadyRunning("a".into()),
        RuntimeError::Socket("s".into()),
    ];
    for e in variants {
        let wire = e.to_wire();
        let back = RuntimeError::from_wire(&wire)
            .unwrap_or_else(|| panic!("from_wire failed for {}", e.wire_type()));
        assert_eq!(e, back, "round-trip for {}", e.wire_type());
    }
    assert!(RuntimeError::from_wire(&json!({"type": "Nope"})).is_none());
}

// ---------------------------------------------------------------------------
// Unit: handshake mismatch → structured ProtocolMismatch (rule 33)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn phase2_handshake_mismatch_is_protocol_mismatch() {
    let sock = test_sock("mismatch");
    cleanup_sock(&sock);
    let handle = spawn_test_server(sock.clone(), None).await;

    let err = match ClientConn::connect_with_protocol(&sock, IPC_PROTOCOL_VERSION + 100).await {
        Ok(_) => panic!("wrong protocol_version must be rejected"),
        Err(e) => e,
    };
    match err {
        RuntimeError::ProtocolMismatch { expected, got } => {
            assert_eq!(expected, IPC_PROTOCOL_VERSION);
            assert_eq!(got, IPC_PROTOCOL_VERSION + 100);
        }
        other => panic!("expected ProtocolMismatch, got {other:?}"),
    }

    stop_test_server(&sock, handle).await;
}

// ---------------------------------------------------------------------------
// Unit: command before handshake → rejected (I26), never a hang
// ---------------------------------------------------------------------------

#[tokio::test]
async fn phase2_pre_handshake_command_rejected() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let sock = test_sock("prehandshake");
    cleanup_sock(&sock);
    let handle = spawn_test_server(sock.clone(), None).await;

    let stream = tokio::time::timeout(Duration::from_secs(5), tokio::net::UnixStream::connect(&sock))
        .await
        .expect("connect must not hang")
        .expect("connect ok");
    let (rd, mut wr) = stream.into_split();
    let evil = json!({
        "kind": "request", "id": 1, "capability": "runtime_evaluate",
        "command": {"op": "eval", "expression": "1+1"},
    });
    let mut line = serde_json::to_string(&evil).unwrap();
    line.push('\n');
    wr.write_all(line.as_bytes()).await.expect("write");
    wr.flush().await.expect("flush");
    let mut reader = BufReader::new(rd);
    let mut resp = String::new();
    tokio::time::timeout(Duration::from_secs(5), reader.read_line(&mut resp))
        .await
        .expect("rejection must arrive, not hang")
        .expect("read");
    let v: Value = serde_json::from_str(resp.trim()).expect("JSON rejection");
    let err = v.get("error").and_then(RuntimeError::from_wire).expect("structured error");
    assert!(
        matches!(err, RuntimeError::HandshakeRequired(_)),
        "expected HandshakeRequired, got {err:?}"
    );

    // Garbage first line is also a handshake failure, not a hang.
    let stream = tokio::net::UnixStream::connect(&sock).await.expect("connect2");
    let (rd, mut wr) = stream.into_split();
    wr.write_all(b"this is not json\n").await.expect("write2");
    wr.flush().await.expect("flush2");
    let mut reader = BufReader::new(rd);
    let mut resp = String::new();
    tokio::time::timeout(Duration::from_secs(5), reader.read_line(&mut resp))
        .await
        .expect("rejection2 must arrive")
        .expect("read2");
    let v: Value = serde_json::from_str(resp.trim()).expect("JSON rejection2");
    assert!(v.get("error").is_some(), "garbage handshake gets an error: {v}");

    stop_test_server(&sock, handle).await;
}

// ---------------------------------------------------------------------------
// Unit: stale vs live socket handling
// ---------------------------------------------------------------------------

#[tokio::test]
async fn phase2_stale_socket_removed_live_socket_refused() {
    // Stale: a plain file where the socket should be → removed, rebound 0600.
    let stale = test_sock("stale");
    cleanup_sock(&stale);
    std::fs::write(&stale, b"dead daemon left this").expect("plant stale file");
    let listener = bind_socket_exclusive(&stale).await.expect("stale must be reclaimed");
    assert_eq!(hyprfast::browser_runtime::socket_mode_octal(&stale), "0600");
    drop(listener);
    cleanup_sock(&stale);

    // Live: a serving daemon → second bind refused, socket left alone.
    let live = test_sock("live");
    cleanup_sock(&live);
    let handle = spawn_test_server(live.clone(), None).await;
    let err = bind_socket_exclusive(&live).await.expect_err("live socket must refuse");
    assert!(
        matches!(err, RuntimeError::DaemonAlreadyRunning(_)),
        "expected DaemonAlreadyRunning, got {err:?}"
    );
    assert!(probe_live(&live, PROBE_TIMEOUT).await, "live daemon undisturbed");
    stop_test_server(&live, handle).await;
}

// ---------------------------------------------------------------------------
// Unit: client disconnect mid-request does not corrupt server state
// ---------------------------------------------------------------------------

#[tokio::test]
async fn phase2_client_disconnect_mid_request_is_harmless() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let sock = test_sock("disconnect");
    cleanup_sock(&sock);
    let handle = spawn_test_server(sock.clone(), None).await;

    // Handshake, fire an eval, then vanish without reading the reply.
    let stream = tokio::net::UnixStream::connect(&sock).await.expect("connect");
    let (rd, mut wr) = stream.into_split();
    let hello = json!({
        "kind": "handshake", "protocol_version": IPC_PROTOCOL_VERSION,
        "client_version": "test", "requested_capabilities": [],
    });
    let mut line = serde_json::to_string(&hello).unwrap();
    line.push('\n');
    wr.write_all(line.as_bytes()).await.expect("hello");
    let mut reader = BufReader::new(rd);
    let mut resp = String::new();
    reader.read_line(&mut resp).await.expect("hello_ok");
    assert!(resp.contains("handshake_ok"), "handshake first: {resp}");
    let req = json!({
        "kind": "request", "id": 1, "capability": "runtime_evaluate",
        "command": {"op": "eval", "expression": "40+2"},
    });
    let mut line = serde_json::to_string(&req).unwrap();
    line.push('\n');
    wr.write_all(line.as_bytes()).await.expect("eval");
    wr.flush().await.expect("flush");
    drop(wr);
    drop(reader);
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Server still serves new clients with correct state. `ipc_in_flight`
    // reads 1: the status request itself (its guard is held while the
    // payload is built) — i.e. the vanished eval leaked nothing.
    let mut conn = ClientConn::connect(&sock).await.expect("reconnect");
    let status = conn.status().await.expect("status after disconnect");
    assert_eq!(status.state, "Connected");
    assert_eq!(status.ipc_in_flight, 1, "only the status request itself is in flight");
    drop(conn);

    stop_test_server(&sock, handle).await;
}

// ---------------------------------------------------------------------------
// Integration: real browser — lifecycle, 20 concurrent evals, status, stop
// ---------------------------------------------------------------------------

const INT_PORT_LIFECYCLE: u16 = 19226;
const INT_PORT_ORDERING: u16 = 19227;

fn browser_base(port: u16) -> String {
    format!("http://127.0.0.1:{port}")
}

async fn wait_for_browser(port: u16, deadline: Duration) {
    let start = Instant::now();
    loop {
        if reqwest::Client::new()
            .get(format!("{}/json/version", browser_base(port)))
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

async fn spawn_brave(port: u16, tag: &str) -> tokio::process::Child {
    let data_dir = format!("/tmp/hyprfast-p2-{tag}");
    let _ = std::fs::remove_dir_all(&data_dir);
    std::fs::create_dir_all(&data_dir).expect("mkdir profile");
    tokio::process::Command::new("brave")
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
        .expect("spawn brave (BLOCKED if no browser binary is installed)")
}

async fn serve_with_browser(
    sock: PathBuf,
    cdp_port: u16,
) -> tokio::task::JoinHandle<hyprfast::browser_runtime::RuntimeResult<()>> {
    let opts = ServeOptions {
        socket_path: sock.clone(),
        cdp_host: "127.0.0.1".to_string(),
        cdp_port,
        restart_on_crash: false,
    };
    let handle = tokio::spawn(async move { hyprfast::browser_runtime::serve(opts).await });
    wait_live(&sock, Duration::from_secs(25)).await;
    // Live means the handshake answers; the browser connection completes
    // asynchronously (Starting → Connecting → Connected). Wait for the
    // terminal state — evals during Connecting are *correctly* NotReady.
    let start = Instant::now();
    loop {
        let mut conn = ClientConn::connect(&sock).await.expect("connect for readiness");
        let status = conn.status().await.expect("readiness status");
        if status.state == "Connected" {
            break;
        }
        assert!(
            status.state == "Starting" || status.state == "Connecting",
            "unexpected pre-ready state: {}",
            status.state
        );
        if start.elapsed() > Duration::from_secs(25) {
            panic!("daemon never reached Connected (stuck at {})", status.state);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    handle
}

fn status_json(s: &RuntimeStatus) -> Value {
    serde_json::to_value(s).expect("status serializes")
}

#[tokio::test]
async fn phase2_lifecycle_concurrent_evals_status_stop() {
    let mut child = spawn_brave(INT_PORT_LIFECYCLE, "lifecycle").await;
    wait_for_browser(INT_PORT_LIFECYCLE, Duration::from_secs(25)).await;

    let sock = test_sock("lifecycle");
    cleanup_sock(&sock);
    let server_handle = serve_with_browser(sock.clone(), INT_PORT_LIFECYCLE).await;

    // ---- 20 concurrent evals through the daemon (DoD sequence) ----
    let start = Instant::now();
    let mut handles = Vec::new();
    for i in 1..=20u32 {
        let sock = sock.clone();
        handles.push(tokio::spawn(async move {
            let mut conn = ClientConn::connect(&sock).await.expect("connect");
            conn.evaluate(&format!("{i}+1"), None, None).await
        }));
    }
    let results = futures::future::join_all(handles).await;
    assert!(start.elapsed() < Duration::from_secs(60), "20 evals complete promptly");
    for (n, r) in (1..=20u32).zip(results) {
        let v = r.expect("join").unwrap_or_else(|e| panic!("eval {n} failed: {e}"));
        let got = v.get("result").and_then(|x| x.get("value")).and_then(Value::as_i64);
        assert_eq!(got, Some(n as i64 + 1), "eval {n} wrong value: {v}");
    }

    // ---- status: Connected, exactly 1 CDP connection, socket 0600 ----
    let mut conn = ClientConn::connect(&sock).await.expect("connect status");
    let status = conn.status().await.expect("status");
    let sj = status_json(&status);
    println!("status: {sj}");
    assert_eq!(status.state, "Connected", "full status: {sj}");
    assert_eq!(status.cdp_connections, 1, "exactly one browser connection: {sj}");
    assert_eq!(status.socket_mode, "0600", "owner-only socket: {sj}");
    assert!(!status.product.is_empty(), "browser product known: {sj}");
    assert!(status.target_count >= 1, "at least one target: {sj}");
    assert!(!status.restart_on_crash, "default-off restart policy reported: {sj}");
    // Phase 4 owns dom/navigation/runtime generations (previously placeholders); runtime_generation bumps on connect.
    assert_eq!(status.dom_version, 0, "initial dom_version before any DOM event: {sj}");
    assert_eq!(status.navigation_generation, 0, "initial navigation_generation before main-frame nav: {sj}");
    assert_eq!(status.runtime_generation, 1, "runtime_generation bumped on connect (Phase 4): {sj}");
    assert_eq!(status.frame_tree_version, 0, "Phase 5 placeholder: {sj}");
    drop(conn);

    // ---- stop: socket gone ----
    stop_test_server(&sock, server_handle).await;

    // Status after stop: daemon gone, client fails cleanly (no hang).
    let conn_attempt =
        tokio::time::timeout(Duration::from_secs(12), ClientConn::connect(&sock)).await;
    let err = match conn_attempt {
        Ok(Ok(_)) => panic!("no daemon must remain after stop"),
        Ok(Err(e)) => e,
        Err(_) => panic!("connect attempt must not hang"),
    };
    assert!(matches!(err, RuntimeError::Socket(_)), "clean socket error: {err:?}");

    child.kill().await.ok();
    let _ = child.wait().await;
    let _ = std::fs::remove_dir_all("/tmp/hyprfast-p2-lifecycle");
}

// ---------------------------------------------------------------------------
// Integration: per-target ordering (rule 29) — same target ordered,
// different targets not blocked.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn phase2_per_target_ordering() {
    let mut child = spawn_brave(INT_PORT_ORDERING, "ordering").await;
    wait_for_browser(INT_PORT_ORDERING, Duration::from_secs(25)).await;

    let sock = test_sock("ordering");
    cleanup_sock(&sock);
    let server_handle = serve_with_browser(sock.clone(), INT_PORT_ORDERING).await;

    // Slow (2s) mutation on target key "A".
    let sock_a = sock.clone();
    let slow_a = tokio::spawn(async move {
        let t = Instant::now();
        let mut conn = ClientConn::connect(&sock_a).await.expect("connect A-slow");
        let r = conn
            .evaluate(
                "new Promise((resolve) => setTimeout(() => resolve('slow-A'), 2000))",
                Some("A"),
                None,
            )
            .await;
        ("slow-A", t.elapsed(), r)
    });
    // Ensure slow-A is dispatched and holding A's serialization lock.
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Quick eval on the SAME key "A" (must queue behind slow-A) and on a
    // DIFFERENT key "B" (must not be blocked by A).
    let sock_b = sock.clone();
    let quick_b = tokio::spawn(async move {
        let t = Instant::now();
        let mut conn = ClientConn::connect(&sock_b).await.expect("connect B");
        let r = conn.evaluate("40+2", Some("B"), None).await;
        ("quick-B", t.elapsed(), r)
    });
    let sock_a2 = sock.clone();
    let quick_a = tokio::spawn(async move {
        let t = Instant::now();
        let mut conn = ClientConn::connect(&sock_a2).await.expect("connect A-quick");
        let r = conn.evaluate("41+1", Some("A"), None).await;
        ("quick-A", t.elapsed(), r)
    });

    let (name_b, lat_b, r_b) = quick_b.await.expect("join B");
    let (name_s, lat_s, r_s) = slow_a.await.expect("join slow");
    let (name_a, lat_a, r_a) = quick_a.await.expect("join quick-A");
    assert_eq!(name_b, "quick-B");
    assert_eq!(name_s, "slow-A");
    assert_eq!(name_a, "quick-A");
    r_b.expect("B eval ok");
    r_s.expect("slow eval ok");
    r_a.expect("A-quick eval ok");
    println!("latencies: B={lat_b:?} slow-A={lat_s:?} quick-A={lat_a:?}");

    // Different-target work is NOT blocked by A's in-flight mutation.
    assert!(
        lat_b < Duration::from_millis(1500),
        "key-B eval must finish while key-A is still busy: B={lat_b:?}"
    );
    // Same-target work IS ordered behind the in-flight mutation.
    assert!(
        lat_a >= Duration::from_millis(1200),
        "same-key eval must wait for the in-flight one: A={lat_a:?} slow={lat_s:?}"
    );

    stop_test_server(&sock, server_handle).await;
    child.kill().await.ok();
    let _ = child.wait().await;
    let _ = std::fs::remove_dir_all("/tmp/hyprfast-p2-ordering");
}
