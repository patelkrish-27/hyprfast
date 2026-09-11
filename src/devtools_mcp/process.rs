//! Chrome DevTools MCP subprocess — stdio JSON-RPC MCP client.
//!
//! Spawns `npx -y chrome-devtools-mcp@latest` as a managed child, speaks
//! MCP over stdio (`initialize` → `tools/list` health check, then `tools/call`),
//! and owns the browser connection (the child owns the WebSocket, not us).
//! This preserves invariant I2: the only `connect_async` in this repo stays
//! in `browser_runtime/connection.rs`.

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::sync::Mutex;
use tokio::time::timeout;

const MCP_TIMEOUT: Duration = Duration::from_secs(25);
const HEALTH_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_RESTARTS: u32 = 3;
const RESTART_BACKOFF: Duration = Duration::from_secs(1);
const PROTOCOL_VERSION: &str = "2024-11-05";

/// Whether the proxy should be attempted. Opt-in via
/// `HYPRFAST_DEVTOOLS_MCP=1` (keeps existing tests green — proxy owns its own
/// browser connection and would otherwise interfere with the daemon's isolated
/// test browsers). Defaults to disabled for backward compatibility.
pub fn devtools_proxy_enabled() -> bool {
    match std::env::var("HYPRFAST_DEVTOOLS_MCP") {
        Ok(v) => matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"),
        Err(_) => false,
    }
}

fn npx_args() -> Vec<String> {
    if let Ok(extra) = std::env::var("HYPRFAST_DEVTOOLS_MCP_ARGS") {
        if !extra.trim().is_empty() {
            return shell_words(&extra);
        }
    }
    // Default: isolated context helps parallel tests; headless defers to env.
    // chrome-devtools-mcp auto-discovers Brave/Chromium at HYPRFAST_CDP_PORT.
    let mut args = vec!["-y".to_string(), "chrome-devtools-mcp@latest".to_string()];
    // Pass browser URL if user overrode CDP host/port so the MCP child
    // attaches to the same browser the daemon would.
    let host = std::env::var("HYPRFAST_CDP_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    let port = std::env::var("HYPRFAST_CDP_PORT").unwrap_or_else(|_| "9222".to_string());
    if host != "127.0.0.1" || port != "9222" {
        // Best-effort: some versions accept --browserUrl, others --browser-url.
        // We pass both via env for compatibility; the process ignores unknowns.
        args.push(format!("--browserUrl=http://{}:{}", host, port));
    }
    // Forward isolated flag for clean-slate testing if requested.
    if std::env::var("HYPRFAST_DEVTOOLS_ISOLATED").ok().as_deref() == Some("1") {
        args.push("--isolated".to_string());
    }
    args
}

fn shell_words(s: &str) -> Vec<String> {
    // Cheap split respecting quoted substrings; mirrors hypr/mod.rs style minimalism.
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_q: Option<char> = None;
    for ch in s.chars() {
        match (in_q, ch) {
            (Some(q), c) if c == q => in_q = None,
            (Some(_), c) => cur.push(c),
            (None, '\'') | (None, '"') => in_q = Some(ch),
            (None, c) if c.is_whitespace() => {
                if !cur.is_empty() { out.push(cur.clone()); cur.clear(); }
            }
            (None, c) => cur.push(c),
        }
    }
    if !cur.is_empty() { out.push(cur); }
    out
}

struct Inner {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: AtomicU64,
    tools: Vec<String>,
    initialized: bool,
}

impl Inner {
    async fn write_line(&mut self, v: &Value) -> Result<()> {
        let mut s = serde_json::to_string(v)?;
        s.push('\n');
        timeout(MCP_TIMEOUT, self.stdin.write_all(s.as_bytes()))
            .await
            .map_err(|_| anyhow::anyhow!("devtools-mcp write timed out"))?
            .context("write to devtools-mcp stdin")?;
        timeout(MCP_TIMEOUT, self.stdin.flush())
            .await
            .map_err(|_| anyhow::anyhow!("devtools-mcp flush timed out"))?
            .context("flush devtools-mcp stdin")?;
        Ok(())
    }

    async fn read_line(&mut self) -> Result<Value> {
        let mut line = String::new();
        let n = timeout(MCP_TIMEOUT, self.stdout.read_line(&mut line))
            .await
            .map_err(|_| anyhow::anyhow!("devtools-mcp read timed out"))?
            .context("read from devtools-mcp stdout")?;
        if n == 0 { bail!("devtools-mcp closed stdout"); }
        Ok(serde_json::from_str(line.trim())?)
    }

    async fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let req = json!({"jsonrpc":"2.0","id": id, "method": method, "params": params});
        self.write_line(&req).await?;
        loop {
            let resp = self.read_line().await?;
            // Skip notifications (no id)
            if resp.get("id").is_none() { continue; }
            let rid = resp.get("id").and_then(|v| v.as_u64()).unwrap_or(u64::MAX);
            if rid != id { continue; }
            if let Some(err) = resp.get("error") {
                bail!("devtools-mcp {} error: {}", method, err);
            }
            return Ok(resp.get("result").cloned().unwrap_or(Value::Null));
        }
    }
}

pub struct DevToolsMcpProcess {
    inner: Mutex<Option<Inner>>,
    restarts: AtomicU64,
    last_restart: Mutex<Option<Instant>>,
    disabled: AtomicBool,
}

impl DevToolsMcpProcess {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(None),
            restarts: AtomicU64::new(0),
            last_restart: Mutex::new(None),
            disabled: AtomicBool::new(false),
        }
    }

    fn is_npx_available() -> bool {
        std::process::Command::new("npx").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
    }

    async fn spawn_inner() -> Result<Inner> {
        if !Self::is_npx_available() {
            bail!("npx not found — cannot spawn chrome-devtools-mcp");
        }
        let args = npx_args();
        let mut cmd = tokio::process::Command::new("npx");
        cmd.args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        // Propagate CDP env so child sees same browser.
        if let Ok(v) = std::env::var("HYPRFAST_CDP_HOST") { cmd.env("HYPRFAST_CDP_HOST", v); }
        if let Ok(v) = std::env::var("HYPRFAST_CDP_PORT") { cmd.env("HYPRFAST_CDP_PORT", v); }
        let mut child = cmd.spawn().context("spawn npx chrome-devtools-mcp")?;
        let stdin = child.stdin.take().context("child stdin")?;
        let stdout = child.stdout.take().context("child stdout")?;
        Ok(Inner {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            next_id: AtomicU64::new(1),
            tools: Vec::new(),
            initialized: false,
        })
    }

    async fn ensure_initialized_locked(inner: &mut Inner) -> Result<()> {
        if inner.initialized { return Ok(()); }
        // MCP initialize handshake
        let params = json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": {"name":"hyprfast","version": env!("CARGO_PKG_VERSION")}
        });
        let res = timeout(HEALTH_TIMEOUT, inner.request("initialize", params))
            .await
            .map_err(|_| anyhow::anyhow!("devtools-mcp initialize timed out"))??;
        let _ = res;
        // Notify initialized (notification, no response expected)
        let notif = json!({"jsonrpc":"2.0","method":"notifications/initialized","params":{}});
        inner.write_line(&notif).await?;
        // tools/list health check
        let list = timeout(HEALTH_TIMEOUT, inner.request("tools/list", json!({})))
            .await
            .map_err(|_| anyhow::anyhow!("devtools-mcp tools/list timed out"))??;
        let tools = list.get("tools").and_then(|v| v.as_array()).cloned().unwrap_or_default();
        inner.tools = tools.iter().filter_map(|t| t.get("name").and_then(|n| n.as_str()).map(|s| s.to_string())).collect();
        if inner.tools.is_empty() {
            bail!("devtools-mcp tools/list returned no tools");
        }
        inner.initialized = true;
        Ok(())
    }

    pub async fn ensure_alive(&self) -> Result<()> {
        if self.disabled.load(Ordering::SeqCst) { bail!("devtools-mcp disabled after repeated failures"); }
        if !devtools_proxy_enabled() { bail!("devtools-mcp disabled via HYPRFAST_DEVTOOLS_MCP"); }
        let mut guard = self.inner.lock().await;
        let need_spawn = match guard.as_mut() {
            None => true,
            Some(inner) => {
                match inner.child.try_wait() {
                    Ok(Some(_)) => true,
                    Ok(None) => false,
                    Err(_) => true,
                }
            }
        };
        if !need_spawn {
            // Ensure initialized
            if let Some(inner) = guard.as_mut() {
                if !inner.initialized {
                    Self::ensure_initialized_locked(inner).await?;
                }
            }
            return Ok(());
        }
        // Spawn with restart policy
        let restarts = self.restarts.load(Ordering::SeqCst);
        if restarts >= MAX_RESTARTS as u64 {
            // Check backoff window
            let last = self.last_restart.lock().await.clone();
            if let Some(t) = last {
                if t.elapsed() < Duration::from_secs(60) {
                    self.disabled.store(true, Ordering::SeqCst);
                    bail!("devtools-mcp restart budget exhausted ({} restarts)", MAX_RESTARTS);
                } else {
                    self.restarts.store(0, Ordering::SeqCst);
                }
            }
        }
        // Backoff
        if restarts > 0 { tokio::time::sleep(RESTART_BACKOFF).await; }
        *guard = None;
        let mut inner = Self::spawn_inner().await?;
        Self::ensure_initialized_locked(&mut inner).await?;
        *guard = Some(inner);
        self.restarts.fetch_add(1, Ordering::SeqCst);
        *self.last_restart.lock().await = Some(Instant::now());
        Ok(())
    }

    pub async fn is_available(&self) -> bool {
        if self.disabled.load(Ordering::SeqCst) { return false; }
        if !devtools_proxy_enabled() { return false; }
        // Fast-path: if we already have a live initialized child
        {
            let mut guard = self.inner.lock().await;
            if let Some(inner) = guard.as_mut() {
                let alive = inner.child.try_wait().ok().map(|o| o.is_none()).unwrap_or(false);
                if alive && inner.initialized { return true; }
            }
        }
        // Do not eagerly spawn here; availability is "can we spawn or already alive".
        // We treat npx presence as availability to avoid spawning on every check.
        Self::is_npx_available()
    }

    pub async fn call_tool(&self, name: &str, args: Value) -> Result<Value> {
        self.ensure_alive().await?;
        let mut guard = self.inner.lock().await;
        let inner = guard.as_mut().context("devtools-mcp not spawned")?;
        if !inner.tools.contains(&name.to_string()) {
            bail!("devtools-mcp tool not found: {}", name);
        }
        let params = json!({"name": name, "arguments": args});
        let res = timeout(MCP_TIMEOUT, inner.request("tools/call", params))
            .await
            .map_err(|_| anyhow::anyhow!("devtools-mcp tools/call {} timed out", name))??;
        // MCP tools/call returns {content:[{type:"text", text:"..."}]} or {isError:true}
        if res.get("isError").and_then(|v| v.as_bool()).unwrap_or(false) {
            let msg = res.get("content").and_then(|c| c.as_array())
                .and_then(|a| a.first())
                .and_then(|o| o.get("text")).and_then(|v| v.as_str()).unwrap_or("unknown error");
            bail!("devtools-mcp {} isError: {}", name, msg);
        }
        // Unwrap content text if it looks like JSON, else return raw content
        if let Some(arr) = res.get("content").and_then(|v| v.as_array()) {
            if arr.len() == 1 {
                if let Some(txt) = arr[0].get("text").and_then(|v| v.as_str()) {
                    if let Ok(val) = serde_json::from_str::<Value>(txt) {
                        return Ok(val);
                    }
                    return Ok(Value::String(txt.to_string()));
                }
            }
            return Ok(Value::Array(arr.clone()));
        }
        Ok(res)
    }

    pub async fn list_tools(&self) -> Result<Vec<String>> {
        self.ensure_alive().await?;
        let guard = self.inner.lock().await;
        Ok(guard.as_ref().map(|i| i.tools.clone()).unwrap_or_default())
    }

    pub async fn shutdown(&self) {
        let mut guard = self.inner.lock().await;
        if let Some(mut inner) = guard.take() {
            let _ = inner.child.kill().await;
        }
    }
}

// Global singleton — mirrors hyprfast daemon singleton style.
static GLOBAL: once_cell::sync::Lazy<DevToolsMcpProcess> = once_cell::sync::Lazy::new(DevToolsMcpProcess::new);

pub fn global() -> &'static DevToolsMcpProcess { &*GLOBAL }

/// Convenience for tests: reset disabled flag.
pub fn reset_for_test() {
    GLOBAL.disabled.store(false, Ordering::SeqCst);
    GLOBAL.restarts.store(0, Ordering::SeqCst);
}

/// Parse shell words for test visibility.
#[cfg(test)]
pub fn parse_shell_words(s: &str) -> Vec<String> { shell_words(s) }
