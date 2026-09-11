//! Phase 2 — BrowserRuntime IPC client.
//!
//! CLI processes are clients of the daemon: they handshake (rule 33),
//! send capability-tagged requests (rule 32), and never own a persistent
//! CDP connection themselves (invariant I24). Each method opens a fresh
//! Unix-socket connection — persistence across CLI invocations lives in
//! the daemon, not in the client.
//!
//! [`try_evaluate_via_daemon`] implements the degraded direct-mode fallback
//! contract: it returns `None` when no daemon is reachable (the caller logs
//! a warning and uses its legacy direct path); a reachable daemon is the
//! normal path and its errors propagate.

use std::path::Path;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

use super::error::{RuntimeError, RuntimeResult};
use super::server::{
    CapabilityClass, IPC_PROTOCOL_VERSION, MAX_IPC_FRAME_BYTES, RUNTIME_VERSION, RuntimeStatus,
    browser_socket_path, probe_live,
};

const IO_TIMEOUT: Duration = Duration::from_secs(10);

/// One handshaked IPC connection. Sequential request/response per connection;
/// open one connection per concurrent caller.
pub struct ClientConn {
    reader: BufReader<tokio::net::unix::OwnedReadHalf>,
    writer: tokio::net::unix::OwnedWriteHalf,
    next_id: u64,
}

impl ClientConn {
    /// Connect + handshake with the current protocol version.
    pub async fn connect(path: &Path) -> RuntimeResult<Self> {
        Self::connect_with_protocol(path, IPC_PROTOCOL_VERSION).await
    }

    /// Connect + handshake with an explicit protocol version. Used by the
    /// mismatch test: a wrong version must yield a structured
    /// [`RuntimeError::ProtocolMismatch`], never a hang.
    pub async fn connect_with_protocol(path: &Path, protocol_version: u32) -> RuntimeResult<Self> {
        let stream = tokio::time::timeout(IO_TIMEOUT, UnixStream::connect(path))
            .await
            .map_err(|_| {
                RuntimeError::Socket(format!("connect {} timed out", path.display()))
            })?
            .map_err(|e| RuntimeError::Socket(format!("connect {}: {e}", path.display())))?;
        let (rd, mut wr) = stream.into_split();
        let hello = json!({
            "kind": "handshake",
            "protocol_version": protocol_version,
            "client_version": RUNTIME_VERSION,
            "requested_capabilities": CapabilityClass::all(),
        });
        let mut line = serde_json::to_string(&hello)
            .map_err(|e| RuntimeError::InvalidResponse(format!("encode handshake: {e}")))?;
        line.push('\n');
        tokio::time::timeout(IO_TIMEOUT, wr.write_all(line.as_bytes()))
            .await
            .map_err(|_| RuntimeError::Socket("handshake write timed out".to_string()))?
            .map_err(|e| RuntimeError::Socket(format!("handshake write: {e}")))?;
        tokio::time::timeout(IO_TIMEOUT, wr.flush())
            .await
            .map_err(|_| RuntimeError::Socket("handshake flush timed out".to_string()))?
            .map_err(|e| RuntimeError::Socket(format!("handshake flush: {e}")))?;

        let mut reader = BufReader::new(rd);
        let mut resp = String::new();
        let n = tokio::time::timeout(IO_TIMEOUT, reader.read_line(&mut resp))
            .await
            .map_err(|_| RuntimeError::Socket("handshake response timed out".to_string()))?
            .map_err(|e| RuntimeError::Socket(format!("handshake read: {e}")))?;
        if n == 0 {
            return Err(RuntimeError::Socket("daemon closed connection during handshake".to_string()));
        }
        let v: Value = serde_json::from_str(resp.trim()).map_err(|_| {
            RuntimeError::InvalidResponse("daemon handshake reply is not JSON".to_string())
        })?;
        match v.get("kind").and_then(Value::as_str) {
            Some("handshake_ok") => Ok(Self { reader, writer: wr, next_id: 1 }),
            Some("error") => {
                let e = v
                    .get("error")
                    .and_then(RuntimeError::from_wire)
                    .unwrap_or_else(|| {
                        RuntimeError::InvalidResponse(format!("unparsable daemon error: {v}"))
                    });
                Err(e)
            }
            other => Err(RuntimeError::InvalidResponse(format!(
                "unexpected handshake reply kind: {other:?}"
            ))),
        }
    }

    async fn request(
        &mut self,
        capability: CapabilityClass,
        command: Value,
    ) -> RuntimeResult<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let req = json!({
            "kind": "request",
            "id": id,
            "capability": capability.as_str(),
            "command": command,
        });
        let mut line = serde_json::to_string(&req)
            .map_err(|e| RuntimeError::InvalidResponse(format!("encode request: {e}")))?;
        line.push('\n');
        if line.len() > MAX_IPC_FRAME_BYTES {
            return Err(RuntimeError::InvalidResponse("request too large".to_string()));
        }
        tokio::time::timeout(IO_TIMEOUT, self.writer.write_all(line.as_bytes()))
            .await
            .map_err(|_| RuntimeError::Socket("request write timed out".to_string()))?
            .map_err(|e| RuntimeError::Socket(format!("request write: {e}")))?;
        // Eval results can be large and slow (multi-MB AX-style payloads ride
        // the same path): generous per-request deadline, matching the CDP
        // transport default rather than the short IO timeout.
        let deadline = Duration::from_secs(120);
        let mut resp = String::new();
        let n = tokio::time::timeout(deadline, self.reader.read_line(&mut resp))
            .await
            .map_err(|_| RuntimeError::Timeout {
                method: "ipc.request".to_string(),
                timeout_ms: deadline.as_millis() as u64,
            })?
            .map_err(|e| RuntimeError::Socket(format!("response read: {e}")))?;
        if n == 0 {
            return Err(RuntimeError::Socket("daemon closed connection mid-request".to_string()));
        }
        let v: Value = serde_json::from_str(resp.trim())
            .map_err(|e| RuntimeError::InvalidResponse(format!("daemon reply is not JSON: {e}")))?;
        match v.get("ok").and_then(Value::as_bool) {
            Some(true) => Ok(v.get("result").cloned().unwrap_or(Value::Null)),
            _ => {
                let e = v
                    .get("error")
                    .and_then(RuntimeError::from_wire)
                    .unwrap_or_else(|| {
                        RuntimeError::InvalidResponse(format!("unparsable daemon error: {v}"))
                    });
                Err(e)
            }
        }
    }

    /// Read-only status fetch (concurrent server-side, rule 29).
    pub async fn status(&mut self) -> RuntimeResult<RuntimeStatus> {
        let v = self
            .request(CapabilityClass::None, json!({"op": "status"}))
            .await?;
        serde_json::from_value(v)
            .map_err(|e| RuntimeError::InvalidResponse(format!("decode status: {e}")))
    }

    /// `Runtime.evaluate` via the daemon (capability `runtime_evaluate`,
    /// serialized per `target_id` server-side). `session_id` selects the
    /// flat session; `None` lets the daemon pick its default.
    pub async fn evaluate(
        &mut self,
        expression: &str,
        target_id: Option<&str>,
        session_id: Option<&str>,
    ) -> RuntimeResult<Value> {
        self.request(
            CapabilityClass::RuntimeEvaluate,
            json!({
                "op": "eval",
                "expression": expression,
                "target_id": target_id,
                "session_id": session_id,
            }),
        )
        .await
    }

    /// Ask the daemon to shut down cleanly.
    pub async fn stop(&mut self) -> RuntimeResult<()> {
        self.request(CapabilityClass::None, json!({"op": "stop"})).await?;
        Ok(())
    }

    /// Generic CDP call via daemon (capability-tagged). `session_id` selects the
    /// flat session; `None` lets the daemon pick its default (or none for browser-level methods).
    pub async fn cdp_call(
        &mut self,
        method: &str,
        params: Value,
        session_id: Option<&str>,
        target_id: Option<&str>,
        capability: CapabilityClass,
    ) -> RuntimeResult<Value> {
        self.request(
            capability,
            json!({
                "op": "cdp_call",
                "method": method,
                "params": params,
                "session_id": session_id,
                "target_id": target_id,
            }),
        )
        .await
    }

    /// List targets via daemon (read-only).
    pub async fn tabs_via_daemon(&mut self) -> RuntimeResult<Value> {
        self.request(CapabilityClass::None, json!({"op": "tabs"})).await
    }

    /// Phase 4: event-driven lifecycle wait via daemon (check → subscribe → re-check → await).
    pub async fn wait_lifecycle(
        &mut self,
        lifecycle: &str,
        timeout: Duration,
    ) -> RuntimeResult<Value> {
        self.request(
            CapabilityClass::None,
            json!({
                "op": "wait_lifecycle",
                "lifecycle": lifecycle,
                "timeout_ms": timeout.as_millis() as u64,
            }),
        )
        .await
    }

    /// Phase 7: structured execution plan (additive MCP tool `browser_execute_plan`).
    pub async fn execute_plan(&mut self, plan: Value) -> RuntimeResult<Value> {
        // plan may be {steps:[...]} or bare array; server accepts both.
        self.request(
            CapabilityClass::Navigation,
            json!({
                "op": "execute_plan",
                "plan": plan,
            }),
        )
        .await
    }

    /// Convenience: execute a typed ExecutionPlan via daemon.
    pub async fn execute_typed_plan(&mut self, plan: &super::plan::ExecutionPlan) -> RuntimeResult<Value> {
        let v = serde_json::to_value(plan).unwrap_or(json!({"steps":[]}));
        self.execute_plan(v).await
    }

    /// Phase 8: execute single step via ActionExecutor (full state machine)
    pub async fn execute_action(&mut self, step: Value) -> RuntimeResult<Value> {
        self.request(
            CapabilityClass::RuntimeEvaluate,
            json!({"op": "execute_action", "step": step}),
        )
        .await
    }

    /// Phase 8: fetch action journal snapshot (capability + production_profile tagged)
    pub async fn journal(&mut self) -> RuntimeResult<Value> {
        self.request(CapabilityClass::None, json!({"op": "journal"})).await
    }
}

/// One-shot `status` against the default socket path.
pub async fn status_once() -> RuntimeResult<RuntimeStatus> {
    ClientConn::connect(&browser_socket_path()).await?.status().await
}

/// One-shot `evaluate` against the default socket path.
pub async fn evaluate_once(expression: &str) -> RuntimeResult<Value> {
    ClientConn::connect(&browser_socket_path())
        .await?
        .evaluate(expression, None, None)
        .await
}

/// One-shot `stop` against the default socket path.
pub async fn stop_once() -> RuntimeResult<()> {
    ClientConn::connect(&browser_socket_path()).await?.stop().await
}

/// Whether a live daemon answers on the default socket path.
pub async fn daemon_available() -> bool {
    probe_live(&browser_socket_path(), Duration::from_millis(800)).await
}

/// Normal-path eval through the daemon, or `None` when no daemon is
/// reachable so the caller can take the degraded direct-mode path (with a
/// warning). Daemon errors (mismatch, dead runtime, CDP failure) propagate
/// as `Some(Err)` — a reachable daemon is authoritative, never silently
/// bypassed.
pub async fn try_evaluate_via_daemon(expression: &str) -> Option<RuntimeResult<Value>> {
    if !daemon_available().await {
        return None;
    }
    let conn = match ClientConn::connect(&browser_socket_path()).await {
        Ok(c) => c,
        Err(e) => return Some(Err(e)),
    };
    let mut conn = conn;
    Some(conn.evaluate(expression, None, None).await)
}

/// Generic helpers for Phase 3 migration: sync wrappers that try the daemon first,
/// falling back to an ephemeral BrowserRuntime (single WS) when no daemon is alive.
/// This keeps the WebSocket connect confined to `connection.rs` even on the degraded path.
fn rt_block_on<F: std::future::Future>(f: F) -> F::Output {
    // Reuse a current-thread runtime for CLI sync contexts; cheap to create per call for Phase 3.
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("client sync runtime")
        .block_on(f)
}

async fn ephemeral_cdp_call(method: &str, params: Value, session_id: Option<&str>) -> RuntimeResult<Value> {
    // Degraded direct mode: brief ephemeral BrowserRuntime that still uses the single allowed WS connect.
    let host = std::env::var("HYPRFAST_CDP_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    let port: u16 = std::env::var("HYPRFAST_CDP_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(9222);
    let ws_url = super::server::discover_ws_url_for_fallback(&host, port)
        .await
        .map_err(|e| RuntimeError::ConnectionFailed(e.to_string()))?;
    let rt = super::connection::BrowserRuntime::connect(&ws_url)
        .await
        .map_err(|e| RuntimeError::ConnectionFailed(e.to_string()))?;
    // Mirror daemon session selection: browser-level methods (Target/Browser) need no session.
    let effective_session: Option<String> = session_id.map(|s| s.to_string()).or_else(|| {
        if method.starts_with("Target.") || method.starts_with("Browser.") {
            None
        } else {
            rt.diagnostics().attached_session_ids.into_iter().next()
        }
    });
    let res = rt.call(effective_session.as_deref(), method, params).await;
    rt.shutdown().await;
    res
}

/// Try daemon first for a generic CDP call; on no daemon, use ephemeral fallback with warning.
pub async fn try_cdp_call_via_daemon(
    method: &str,
    params: Value,
    session_id: Option<&str>,
    target_id: Option<&str>,
    capability: CapabilityClass,
) -> RuntimeResult<Value> {
    // DevTools MCP proxy: prefer stdio proxy when available (owns browser connection).
    // Maps a subset of CDP methods to equivalent DevTools MCP tools, preserving semantics.
    if crate::devtools_mcp::process::devtools_proxy_enabled() {
        if crate::devtools_mcp::proxy::global_proxy().is_available().await {
            if let Some(res) = try_devtools_proxy_for_cdp(method, &params).await {
                match res {
                    Ok(v) => return Ok(v),
                    Err(e) => {
                        // Proxy attempted but failed — fall through to daemon path with warning,
                        // do not silently swallow proxy errors for actionable diagnostics.
                        tracing::warn!(method, error=%e, "devtools-mcp proxy failed, falling back to daemon");
                    }
                }
            }
        }
    }
    if daemon_available().await {
        let mut conn = ClientConn::connect(&browser_socket_path()).await?;
        return conn.cdp_call(method, params, session_id, target_id, capability).await;
    }
    eprintln!(
        "warning: browser-runtime daemon unavailable; using degraded direct mode for {} (run `hyprfast browser-runtime start` for the persistent path)",
        method
    );
    ephemeral_cdp_call(method, params, session_id).await
}

async fn try_devtools_proxy_for_cdp(method: &str, params: &Value) -> Option<RuntimeResult<Value>> {
    let proxy = crate::devtools_mcp::proxy::global_proxy();
    match method {
        "Runtime.evaluate" => {
            let expr = params.get("expression").and_then(|v| v.as_str()).unwrap_or("");
            if expr.is_empty() { return None; }
            match proxy.evaluate(expr).await {
                Ok(v) => Some(Ok(v)),
                Err(e) => Some(Err(RuntimeError::InvalidResponse(e.to_string()))),
            }
        }
        "Page.captureScreenshot" => {
            match proxy.screenshot().await {
                Ok((bytes, meta)) => {
                    use base64::Engine;
                    let data = base64::engine::general_purpose::STANDARD.encode(&bytes);
                    let mut out = meta;
                    out["data"] = Value::String(data);
                    Some(Ok(out))
                }
                Err(e) => Some(Err(RuntimeError::InvalidResponse(e.to_string()))),
            }
        }
        "Accessibility.getFullAXTree" => {
            match proxy.snapshot(60).await {
                Ok(v) => Some(Ok(v)),
                Err(e) => Some(Err(RuntimeError::InvalidResponse(e.to_string()))),
            }
        }
        "Target.getTargets" => {
            match proxy.tabs().await {
                Ok(v) => Some(Ok(v)),
                Err(e) => Some(Err(RuntimeError::InvalidResponse(e.to_string()))),
            }
        }
        "Page.navigate" => {
            let url = params.get("url").and_then(|v| v.as_str()).unwrap_or("");
            if url.is_empty() { return None; }
            match proxy.navigate(url).await {
                Ok(v) => Some(Ok(v)),
                Err(e) => Some(Err(RuntimeError::InvalidResponse(e.to_string()))),
            }
        }
        _ => None,
    }
}

/// Sync wrapper for `try_cdp_call_via_daemon` (for sync browser/stagehand helpers).
pub fn cdp_call_sync(
    method: &str,
    params: Value,
    session_id: Option<&str>,
    target_id: Option<&str>,
    capability: CapabilityClass,
) -> anyhow::Result<Value> {
    rt_block_on(try_cdp_call_via_daemon(method, params, session_id, target_id, capability))
        .map_err(|e| anyhow::anyhow!(e.to_string()))
}

/// Sync evaluate wrapper (capability runtime_evaluate, preserves old cdp::evaluate semantics).
pub fn evaluate_sync(expression: &str, await_promise: bool) -> anyhow::Result<Value> {
    let params = serde_json::json!({
        "expression": expression,
        "returnByValue": true,
        "awaitPromise": await_promise,
        "userGesture": true
    });
    let v = cdp_call_sync(
        "Runtime.evaluate",
        params,
        None,
        None,
        CapabilityClass::RuntimeEvaluate,
    )?;
    if let Some(exc) = v.get("exceptionDetails") {
        anyhow::bail!("evaluate exception: {}", exc);
    }
    Ok(v.get("result").and_then(|r| r.get("value")).cloned().unwrap_or(v))
}

/// Sync eval that returns the full Runtime.evaluate result (for CDP callers needing result wrapper).
pub fn evaluate_raw_sync(expression: &str, await_promise: bool) -> anyhow::Result<Value> {
    let params = serde_json::json!({
        "expression": expression,
        "returnByValue": true,
        "awaitPromise": await_promise,
        "userGesture": true
    });
    cdp_call_sync(
        "Runtime.evaluate",
        params,
        None,
        None,
        CapabilityClass::RuntimeEvaluate,
    )
}

/// Phase 7: execute plan via daemon with degraded fallback (ephemeral runtime when no daemon).
pub fn execute_plan_sync(plan: Value) -> anyhow::Result<Value> {
    rt_block_on(async move {
        if daemon_available().await {
            let mut conn = ClientConn::connect(&browser_socket_path()).await.map_err(|e| anyhow::anyhow!(e.to_string()))?;
            return conn.execute_plan(plan.clone()).await.map_err(|e| anyhow::anyhow!(e.to_string()));
        }
        // Degraded: parse plan and run via ephemeral runtime (still single WS via connection.rs)
        let p = super::plan::ExecutionPlan::from_json(&plan).map_err(|e| anyhow::anyhow!(e.to_string()))?;
        let v = p.execute_sync_degraded().map_err(|e| anyhow::anyhow!(e.to_string()))?;
        serde_json::to_value(&v).map_err(|e| anyhow::anyhow!(e.to_string()))
    })
}

/// Phase 4: event-driven lifecycle wait — daemon path when available, polling fallback otherwise.
/// Polling fallback uses condition polling against real `document.readyState` (allowed per rule 5)
/// when no suitable event exists (no daemon to provide ordered `Page.lifecycleEvent`).
pub fn wait_for_lifecycle_sync(lifecycle: &str, timeout: Duration) -> anyhow::Result<Value> {
    rt_block_on(async move {
        if daemon_available().await {
            let mut conn = match ClientConn::connect(&browser_socket_path()).await {
                Ok(c) => c,
                Err(e) => return Err(e),
            };
            return conn.wait_lifecycle(lifecycle, timeout).await;
        }
        // Degraded direct mode fallback: condition polling against real browser state.
        let start = std::time::Instant::now();
        while start.elapsed() < timeout {
            // Use ephemeral call via cdp_call_sync's fallback path conceptually;
            // here we directly call ephemeral to avoid recursion.
            let res = ephemeral_cdp_call(
                "Runtime.evaluate",
                serde_json::json!({
                    "expression": "document.readyState",
                    "returnByValue": true,
                }),
                None,
            )
            .await;
            let ready = res
                .ok()
                .and_then(|v| {
                    v.get("result")
                        .and_then(|r| r.get("value"))
                        .and_then(Value::as_str)
                        .map(|s| s.to_string())
                });
            // Map lifecycle name to readyState expectation
            let desired_ready = match lifecycle {
                "load" | "complete" => "complete",
                "DOMContentLoaded" | "interactive" => "interactive",
                _ => lifecycle,
            };
            if let Some(r) = ready.as_deref() {
                if r == desired_ready || (desired_ready == "complete" && r == "complete") {
                    return Ok(serde_json::json!({"lifecycle": lifecycle, "readyState": r}));
                }
                if lifecycle == "load" && r == "complete" {
                    return Ok(serde_json::json!({"lifecycle": lifecycle, "readyState": r}));
                }
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        Err(RuntimeError::Timeout {
            method: format!("wait_for_lifecycle({lifecycle})"),
            timeout_ms: timeout.as_millis() as u64,
        })
    })
    .map_err(|e| anyhow::anyhow!(e.to_string()))
}
