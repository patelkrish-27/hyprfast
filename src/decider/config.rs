//! Decider external service configuration.
//!
//! The decider is an external resident service (no local model loading,
//! no Python subprocess). This module only resolves env-driven config for the
//! async HTTP client.
//!
//! # Default is health-gated, not statically off
//!
//! `DECIDER_ENABLED` is optional. When it is set (`1/0/true/false/...`) it is
//! honoured verbatim, so an explicit `0` is still a hard opt-out. When it is
//! unset, routing is decided by a cheap liveness probe of `DECIDER_URL`
//! (`GET /health`, sweeping `/health` -> `/v1/health` -> `/`, the same sweep
//! `DeciderClient::health` uses). A healthy daemon turns Decider routing on
//! with no env setup; a dead one leaves every call site on exactly the
//! deterministic fallback path it used before.
//!
//! The probe result is cached process-globally (see [`PROBE_TTL_UP`] /
//! [`PROBE_TTL_DOWN`]) because a Decider call is ~300ms and the fast lane
//! calls `from_env()` on every perception call. The probe itself is a raw
//! blocking TCP + minimal HTTP/1.1 GET with a hard [`DEFAULT_PROBE_TIMEOUT`]
//! budget: ~1ms on a healthy loopback daemon, immediate `ECONNREFUSED` on a
//! dead one, never a panic, and no per-call logging.

use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Hard budget for one whole liveness probe (all swept paths, connect + read).
///
/// 400ms is deliberately far below the 5s `DECIDER_TIMEOUT_MS` of a real
/// decide call: a probe must never be mistaken for a slow daemon, and it must
/// never be able to stall a caller for long. On loopback a healthy daemon
/// answers in ~1ms and a dead port refuses instantly, so the budget is only
/// ever paid against a black-holing host.
pub const DEFAULT_PROBE_TIMEOUT: Duration = Duration::from_millis(400);

/// How long a *positive* probe result is reused before re-probing.
///
/// 60s: in a long-lived process (MCP server) this caps staleness at one
/// health request per minute — immeasurable next to ~300ms decide calls —
/// while still noticing a daemon that dies within a minute.
pub const PROBE_TTL_UP: Duration = Duration::from_secs(60);

/// How long a *negative* probe result is reused before re-probing.
///
/// 15s: a negative result must also be cached (a dead daemon must not add a
/// health request to every call), but a daemon that is started later should
/// be picked up quickly instead of leaving the fast lane dark for a minute.
pub const PROBE_TTL_DOWN: Duration = Duration::from_secs(15);

/// Env-driven configuration for the Decider HTTP client.
#[derive(Debug, Clone)]
pub struct DeciderConfig {
    /// Whether decider calls are enabled. Resolved by [`DeciderConfig::from_env`]
    /// from `DECIDER_ENABLED` when set, otherwise from the cached `/health`
    /// probe. [`DeciderConfig::default`] keeps the conservative static value
    /// (off) because it performs no probe at all.
    pub enabled: bool,
    /// Base URL of the resident decider service (DECIDER_URL).
    pub url: String,
    /// Per-request timeout (DECIDER_TIMEOUT_MS).
    pub timeout: Duration,
    /// Maximum image dimension (DECIDER_MAX_IMAGE_DIM). Client may downscale
    /// before encoding or let the server handle it; this value is still
    /// enforced as a config knob even if resizing is server-side.
    pub max_image_dim: u32,
    /// Sampling temperature forwarded to the service (DECIDER_TEMPERATURE).
    pub temperature: f32,
}

impl Default for DeciderConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            url: "http://127.0.0.1:8001".to_string(),
            timeout: Duration::from_millis(5000),
            max_image_dim: 1280,
            temperature: 1.0,
        }
    }
}

impl DeciderConfig {
    /// Build config from environment variables with defaults:
    /// - `DECIDER_ENABLED`  (unset = health-gated: enabled iff `DECIDER_URL`
    ///   answers `/health`; set = honoured verbatim, truthy = 1/true/yes/on/y)
    /// - `DECIDER_URL`      (default: `http://127.0.0.1:8001`)
    /// - `DECIDER_TIMEOUT_MS` (default: `5000`)
    /// - `DECIDER_MAX_IMAGE_DIM` (default: `1280`)
    /// - `DECIDER_TEMPERATURE`   (default: `1.0`)
    ///
    /// When `DECIDER_ENABLED` is unset the first call performs a cached
    /// liveness probe; later calls inside the TTL are a lock + clock read.
    pub fn from_env() -> Self {
        let url = std::env::var("DECIDER_URL")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "http://127.0.0.1:8001".to_string());
        let url = url.trim().trim_end_matches('/').to_string();
        let (enabled, _source, _reason) = resolve_enabled(&url);
        let timeout_ms: u64 = std::env::var("DECIDER_TIMEOUT_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(5000);
        let max_image_dim: u32 = std::env::var("DECIDER_MAX_IMAGE_DIM")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1280);
        let temperature: f32 = std::env::var("DECIDER_TEMPERATURE")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1.0);

        Self {
            enabled,
            url,
            timeout: Duration::from_millis(timeout_ms),
            max_image_dim: max_image_dim.max(1),
            temperature,
        }
    }

    /// Override URL (useful for tests).
    pub fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = url.into().trim_end_matches('/').to_string();
        self
    }

    /// Override timeout.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

fn parse_bool(s: &str) -> bool {
    matches!(s.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on" | "y")
}

// ---------------------------------------------------------------------------
// How routing got decided
// ---------------------------------------------------------------------------

/// Origin of the current enabled/disabled decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnableSource {
    /// `DECIDER_ENABLED` was set in the environment; honoured verbatim.
    Explicit,
    /// `DECIDER_ENABLED` was unset; decided by the cached liveness probe.
    HealthProbe,
}

/// Resolve whether Decider routing should be on for `url`, and why.
///
/// Cheap after the first call (TTL cache). Never panics; never blocks longer
/// than [`DEFAULT_PROBE_TIMEOUT`].
///
/// Returns `(enabled, source, reason)` where `reason` is `Some(..)` only when
/// routing is off, holding a short user-facing explanation.
pub fn resolve_enabled(url: &str) -> (bool, EnableSource, Option<String>) {
    match explicit_enabled() {
        Some(b) => (b, EnableSource::Explicit, explicit_reason(b)),
        None => match probe_health_cached(url, DEFAULT_PROBE_TIMEOUT) {
            Ok(()) => (true, EnableSource::HealthProbe, None),
            Err(e) => (
                false,
                EnableSource::HealthProbe,
                Some(format!("decider unavailable (health probe failed: {e})")),
            ),
        },
    }
}

/// Short, user-facing explanation of why Decider routing is currently off, or
/// `None` when it is on. Safe to interpolate into tool JSON.
pub fn disabled_reason() -> Option<String> {
    let url = std::env::var("DECIDER_URL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "http://127.0.0.1:8001".to_string());
    resolve_enabled(url.trim().trim_end_matches('/')).2
}

/// Same as [`disabled_reason`] for call sites that already know routing is off
/// (i.e. behind `if !cfg.enabled`). Never returns an empty string.
pub fn off_reason() -> String {
    disabled_reason().unwrap_or_else(|| "decider disabled".to_string())
}

/// `Some(bool)` when `DECIDER_ENABLED` carries a non-empty explicit value.
/// A set-but-empty/whitespace value is treated as unset (falls through to the
/// health probe) instead of silently meaning "off".
fn explicit_enabled() -> Option<bool> {
    std::env::var("DECIDER_ENABLED")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .map(|v| parse_bool(&v))
}

fn explicit_reason(enabled: bool) -> Option<String> {
    if enabled {
        return None;
    }
    let raw = std::env::var("DECIDER_ENABLED").unwrap_or_default();
    Some(format!(
        "decider disabled (DECIDER_ENABLED={})",
        if raw.trim().is_empty() { "0" } else { raw.trim() }
    ))
}

// ---------------------------------------------------------------------------
// Liveness probe (sync, no tokio, never panics)
// ---------------------------------------------------------------------------

/// Cached outcome of the last probe for a given URL.
#[derive(Debug, Clone)]
struct ProbeSlot {
    url: String,
    up: bool,
    detail: String,
    at: Instant,
}

static PROBE_SLOT: OnceLock<Mutex<Option<ProbeSlot>>> = OnceLock::new();

fn probe_slot() -> &'static Mutex<Option<ProbeSlot>> {
    PROBE_SLOT.get_or_init(|| Mutex::new(None))
}

/// Poison-tolerant lock: a panic in one caller must not disable the probe for
/// every later one.
fn slot_lock() -> std::sync::MutexGuard<'static, Option<ProbeSlot>> {
    probe_slot()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Process-global cached liveness probe. `Ok(())` means the daemon answered.
///
/// The cached branch is a lock + clock read. The network probe is done *outside*
/// the lock so a slow/unreachable host can never block another caller on a
/// `std::sync::Mutex`; a rare double-probe is cheaper than that risk.
pub fn probe_health_cached(url: &str, timeout: Duration) -> Result<(), String> {
    {
        let guard = slot_lock();
        if let Some(slot) = guard.as_ref() {
            if slot.url == url {
                let ttl = if slot.up { PROBE_TTL_UP } else { PROBE_TTL_DOWN };
                if slot.at.elapsed() < ttl {
                    return if slot.up {
                        Ok(())
                    } else {
                        Err(slot.detail.clone())
                    };
                }
            }
        }
    }

    let (up, detail) = match probe_health(url, timeout) {
        Ok(()) => (true, "ok".to_string()),
        Err(e) => (false, e),
    };
    *slot_lock() = Some(ProbeSlot {
        url: url.to_string(),
        up,
        detail: detail.clone(),
        at: Instant::now(),
    });
    if up {
        Ok(())
    } else {
        Err(detail)
    }
}

/// Probe paths swept, in order. Mirrors `DeciderClient::health` so the cheap
/// sync probe and the full client agree on what "healthy" means.
const PROBE_PATHS: [&str; 3] = ["/health", "/v1/health", "/"];

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProbeTarget {
    host: String,
    port: u16,
    prefix: String,
}

/// Cheap liveness probe: TCP connect + minimal `GET` on each candidate path,
/// bounded by `timeout` in total. Never panics, never blocks for longer than
/// `timeout`, and prints nothing.
pub fn probe_health(url: &str, timeout: Duration) -> Result<(), String> {
    let target = parse_probe_url(url)?;
    let deadline = Instant::now() + timeout;
    let mut last: Option<String> = None;
    for path in PROBE_PATHS {
        let full = format!("{}{}", target.prefix, path);
        match probe_path(&target, &full, deadline) {
            Ok(()) => return Ok(()),
            Err(ProbeErr::NotFound(status)) => {
                // 404 on this path: try the next candidate, like the client.
                last = Some(format!("{full} -> {status}"));
            }
            Err(ProbeErr::Fatal(msg)) => return Err(msg),
        }
    }
    Err(last.unwrap_or_else(|| "no health endpoint".to_string()))
}

enum ProbeErr {
    /// Path answered, but not with 2xx/3xx and it was a 404 (keep sweeping).
    NotFound(String),
    /// Terminal for this probe: connection refused, timeout, bad URL, 5xx, ...
    Fatal(String),
}

fn probe_path(t: &ProbeTarget, path: &str, deadline: Instant) -> Result<(), ProbeErr> {
    let addrs = resolve_addrs(t, deadline)?;
    let mut last: Option<String> = None;
    for addr in addrs {
        let remaining = remaining(deadline)?;
        let stream = match TcpStream::connect_timeout(&addr, remaining) {
            Ok(s) => s,
            Err(e) => {
                last = Some(format!("connect {addr} failed: {e}"));
                continue;
            }
        };
        let _ = stream.set_nodelay(true);
        if let Err(e) = stream.set_read_timeout(Some(remaining)) {
            last = Some(format!("set_read_timeout failed: {e}"));
            continue;
        }
        if let Err(e) = stream.set_write_timeout(Some(remaining)) {
            last = Some(format!("set_write_timeout failed: {e}"));
            continue;
        }
        let mut w = &stream;
        let req = format!(
            "GET {path} HTTP/1.1\r\nHost: {}\r\nUser-Agent: hyprfast-decider-probe\r\nAccept: application/json\r\nConnection: close\r\n\r\n",
            authority(t)
        );
        if let Err(e) = w.write_all(req.as_bytes()) {
            last = Some(format!("write {path} failed: {e}"));
            continue;
        }
        if let Err(e) = w.flush() {
            last = Some(format!("flush {path} failed: {e}"));
            continue;
        }
        // Status line only: a few hundred bytes is plenty and keeps the probe
        // from reading a full health payload.
        let mut buf = [0u8; 512];
        let mut filled = 0usize;
        let status = loop {
            if filled == buf.len() {
                break Err("status line too long".to_string());
            }
            match w.read(&mut buf[filled..]) {
                Ok(0) => break Err("connection closed before status line".to_string()),
                Ok(n) => {
                    filled += n;
                    if let Some(line) = status_line(&buf[..filled]) {
                        break line;
                    }
                }
                Err(e) => break Err(format!("read {path} failed: {e}")),
            }
        };
        match status {
            Ok(code) if (200..400).contains(&code) => return Ok(()),
            Ok(code) if code == 404 => {
                return Err(ProbeErr::NotFound(format!("{path} -> 404")));
            }
            Ok(code) => {
                return Err(ProbeErr::Fatal(format!("{path} -> HTTP {code}")));
            }
            Err(e) => {
                last = Some(e);
                continue;
            }
        }
    }
    Err(ProbeErr::Fatal(last.unwrap_or_else(|| "no address".into())))
}

/// Extract the numeric status code from a (possibly partial) HTTP response.
fn status_line(buf: &[u8]) -> Option<Result<u16, String>> {
    let text = String::from_utf8_lossy(buf);
    let line = text.lines().next()?;
    if !line.starts_with("HTTP/") {
        return None; // headers still arriving
    }
    let code = line.split_whitespace().nth(1)?.parse::<u16>().ok()?;
    Some(Ok(code))
}

fn remaining(deadline: Instant) -> Result<Duration, ProbeErr> {
    match deadline.checked_duration_since(Instant::now()) {
        Some(d) if !d.is_zero() => Ok(d),
        _ => Err(ProbeErr::Fatal("probe budget exhausted".to_string())),
    }
}

fn resolve_addrs(t: &ProbeTarget, deadline: Instant) -> Result<Vec<SocketAddr>, ProbeErr> {
    let port = t.port;
    if let Ok(ip) = t.host.parse::<IpAddr>() {
        return Ok(vec![SocketAddr::new(ip, port)]);
    }
    let _ = deadline; // name resolution is bounded by the OS resolver
    match (t.host.as_str(), port).to_socket_addrs() {
        Ok(it) => {
            let v: Vec<SocketAddr> = it.collect();
            if v.is_empty() {
                Err(ProbeErr::Fatal(format!("{} did not resolve", t.host)))
            } else {
                Ok(v)
            }
        }
        Err(e) => Err(ProbeErr::Fatal(format!("resolve {} failed: {e}", t.host))),
    }
}

fn authority(t: &ProbeTarget) -> String {
    if t.host.contains(':') && !t.host.starts_with('[') {
        format!("[{}]:{}", t.host, t.port)
    } else {
        format!("{}:{}", t.host, t.port)
    }
}

/// Minimal URL split. Plain `http` only: the probe is a raw socket, so an
/// `https` URL is reported as unprobeable (routing then stays on the
/// deterministic path unless `DECIDER_ENABLED=1` forces it on).
fn parse_probe_url(url: &str) -> Result<ProbeTarget, String> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return Err("empty url".to_string());
    }
    let rest = if let Some(r) = trimmed.strip_prefix("http://") {
        r
    } else if let Some(r) = trimmed.strip_prefix("//") {
        r
    } else if trimmed.starts_with("https://") {
        return Err("https url not supported by the sync probe".to_string());
    } else if trimmed.contains("://") {
        return Err(format!("unsupported scheme in {trimmed}"));
    } else {
        trimmed
    };
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], rest[i..].trim_end_matches('/')),
        None => (rest, ""),
    };
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    if authority.is_empty() {
        return Err(format!("no host in {trimmed}"));
    }
    let (host, port) = if let Some(close) = authority.strip_prefix('[').and_then(|a| a.find(']')) {
        let host = authority[1..1 + close].to_string();
        let tail = &authority[close + 2..];
        let port = parse_port(tail, trimmed)?;
        (host, port)
    } else {
        match authority.rfind(':') {
            Some(i) => (
                authority[..i].to_string(),
                parse_port(&authority[i..], trimmed)?,
            ),
            None => (authority.to_string(), 80),
        }
    };
    Ok(ProbeTarget {
        host,
        port,
        prefix: path.to_string(),
    })
}

fn parse_port(tail: &str, url: &str) -> Result<u16, String> {
    let s = tail.strip_prefix(':').unwrap_or("");
    if s.is_empty() {
        return Ok(80);
    }
    s.parse::<u16>()
        .map_err(|e| format!("bad port in {url}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn default_values() {
        let c = DeciderConfig::default();
        assert_eq!(c.url, "http://127.0.0.1:8001");
        assert_eq!(c.timeout, Duration::from_millis(5000));
        assert_eq!(c.max_image_dim, 1280);
        assert!((c.temperature - 1.0).abs() < f32::EPSILON);
        assert!(!c.enabled);
    }

    #[test]
    fn parse_bool_truthy() {
        assert!(parse_bool("1"));
        assert!(parse_bool("true"));
        assert!(parse_bool("YES"));
        assert!(parse_bool("on"));
        assert!(!parse_bool("0"));
        assert!(!parse_bool("false"));
    }

    // -----------------------------------------------------------------------
    // URL parsing
    // -----------------------------------------------------------------------

    #[test]
    fn parse_probe_url_forms() {
        let t = parse_probe_url("http://127.0.0.1:8001").unwrap();
        assert_eq!(t, ProbeTarget { host: "127.0.0.1".into(), port: 8001, prefix: String::new() });

        let t = parse_probe_url("http://127.0.0.1:8001/").unwrap();
        assert_eq!(t.port, 8001);
        assert_eq!(t.prefix, "");

        let t = parse_probe_url("http://localhost:8001/api").unwrap();
        assert_eq!(t.host, "localhost");
        assert_eq!(t.port, 8001);
        assert_eq!(t.prefix, "/api");

        let t = parse_probe_url("http://[::1]:8001").unwrap();
        assert_eq!(t.host, "::1");
        assert_eq!(t.port, 8001);

        let t = parse_probe_url("127.0.0.1:8001").unwrap();
        assert_eq!(t.port, 8001);

        assert!(parse_probe_url("https://127.0.0.1:8001").is_err());
        assert!(parse_probe_url("").is_err());
        assert!(parse_probe_url("ftp://x:1").is_err());
    }

    #[test]
    fn authority_brackets_ipv6() {
        let t = ProbeTarget { host: "::1".into(), port: 8001, prefix: String::new() };
        assert_eq!(authority(&t), "[::1]:8001");
        let t = ProbeTarget { host: "127.0.0.1".into(), port: 8001, prefix: String::new() };
        assert_eq!(authority(&t), "127.0.0.1:8001");
    }

    // -----------------------------------------------------------------------
    // Probe behaviour
    // -----------------------------------------------------------------------

    /// Serve `n` requests on an ephemeral port with a canned response, so the
    /// probe has something real to talk to. Returns the base URL.
    fn spawn_canned_server(n: usize, response: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for _ in 0..n {
                let Ok((mut sock, _)) = listener.accept() else { return };
                let mut buf = [0u8; 2048];
                let _ = sock.read(&mut buf);
                let _ = sock.write_all(response.as_bytes());
                let _ = sock.flush();
            }
        });
        format!("http://{addr}")
    }

    #[test]
    fn probe_health_enabled_path() {
        let url = spawn_canned_server(1, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 15\r\n\r\n{\"status\":\"ok\"}");
        let r = probe_health(&url, Duration::from_millis(1000));
        assert!(r.is_ok(), "expected healthy, got {r:?}");
    }

    #[test]
    fn probe_health_sweeps_404_to_health() {
        // First candidate 404s, second answers: the sweep must find /v1/health.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for i in 0..2 {
                let Ok((mut sock, _)) = listener.accept() else { return };
                let mut buf = [0u8; 2048];
                let _ = sock.read(&mut buf);
                let seen = String::from_utf8_lossy(&buf);
                if i == 0 && seen.contains("GET /health ") {
                    let _ = sock.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
                } else {
                    let _ = sock.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}");
                }
                let _ = sock.flush();
            }
        });
        let url = format!("http://{addr}");
        let r = probe_health(&url, Duration::from_millis(1000));
        assert!(r.is_ok(), "sweep should fall through to /v1/health, got {r:?}");
    }

    #[test]
    fn probe_health_reports_server_error_as_down() {
        let url = spawn_canned_server(1, "HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\n\r\n");
        let r = probe_health(&url, Duration::from_millis(1000));
        assert!(r.is_err(), "503 must not count as healthy");
        let msg = r.err().unwrap_or_default();
        assert!(msg.contains("503"), "err was {msg}");
    }

    #[test]
    fn probe_health_dead_port_is_down_and_fast() {
        // Bind then drop so the port is almost certainly closed.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        let url = format!("http://{addr}");
        let t0 = Instant::now();
        let r = probe_health(&url, Duration::from_millis(5000));
        let dt = t0.elapsed();
        assert!(r.is_err(), "dead port must be down");
        assert!(dt < Duration::from_millis(500), "probe took {dt:?} on a refused port");
    }

    #[test]
    fn probe_health_never_panics_on_garbage_url() {
        for bad in ["", "   ", "http://", "not a url at all", "http://[::1", "http://h:99999"] {
            let _ = probe_health(bad, Duration::from_millis(50));
        }
    }

    #[test]
    fn probe_health_respects_timeout_budget() {
        // A listener that accepts but never answers: the probe must give up
        // within the budget instead of hanging the caller.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let mut held = Vec::new();
            while let Ok((sock, _)) = listener.accept() {
                held.push(sock); // accept, never respond
            }
        });
        let url = format!("http://{addr}");
        let t0 = Instant::now();
        let r = probe_health(&url, Duration::from_millis(200));
        let dt = t0.elapsed();
        assert!(r.is_err(), "silent server must be down");
        assert!(dt < Duration::from_millis(1200), "probe overran budget: {dt:?}");
    }

    #[test]
    fn cached_probe_reuses_result_within_ttl() {
        let url = spawn_canned_server(1, "HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}");
        let first = probe_health_cached(&url, Duration::from_millis(1000));
        assert!(first.is_ok(), "{first:?}");
        // Server is gone after one request; a fresh probe would now fail, so a
        // success here proves the cached (no-network) branch was used.
        let second = probe_health_cached(&url, Duration::from_millis(1000));
        assert!(second.is_ok(), "cached result should be reused: {second:?}");
        // Different URL is a different cache key -> must re-probe and fail.
        let other = probe_health_cached("http://127.0.0.1:1", Duration::from_millis(300));
        assert!(other.is_err(), "{other:?}");
    }

    #[test]
    fn cached_negative_is_cached_too() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        let url = format!("http://{addr}");
        assert!(probe_health_cached(&url, Duration::from_millis(300)).is_err());
        let t0 = Instant::now();
        assert!(probe_health_cached(&url, Duration::from_millis(300)).is_err());
        assert!(
            t0.elapsed() < Duration::from_millis(100),
            "negative result should be cached, not re-probed"
        );
    }

    /// The latency claim behind the negative cache: against a daemon that
    /// accepts but never answers, the FIRST probe must burn its whole budget,
    /// and every later call inside the TTL must be effectively free. Without
    /// the cache a dead-but-listening daemon would cost `budget` per
    /// perception call.
    #[test]
    fn cached_negative_against_unresponsive_daemon_is_free() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let mut held = Vec::new();
            while let Ok((s, _)) = listener.accept() {
                held.push(s); // accept, never respond
            }
        });
        let url = format!("http://{addr}");
        let budget = Duration::from_millis(200);

        let t0 = Instant::now();
        assert!(probe_health_cached(&url, budget).is_err());
        let first = t0.elapsed();
        let t1 = Instant::now();
        assert!(probe_health_cached(&url, budget).is_err());
        let second = t1.elapsed();
        let t2 = Instant::now();
        assert!(probe_health_cached(&url, budget).is_err());
        let third = t2.elapsed();

        assert!(
            first >= Duration::from_millis(150),
            "first probe should have paid its budget, took {first:?}"
        );
        assert!(
            second < Duration::from_millis(20) && third < Duration::from_millis(20),
            "cached negatives must be free: {second:?} / {third:?} vs first {first:?}"
        );
    }

    // -----------------------------------------------------------------------
    // Enabled resolution
    // -----------------------------------------------------------------------

    // `resolve_enabled` reads process env, so these cases are exercised through
    // explicit values only where the env is deterministic. Env mutating tests
    // in one process are not safe here, so the explicit/unset split is covered
    // by the pure helpers plus the live end-to-end checks.
    #[test]
    fn explicit_enabled_reads_env() {
        // Not set in the test process: treated as unset (probe decides).
        if std::env::var("DECIDER_ENABLED").is_err() {
            assert_eq!(explicit_enabled(), None);
        }
    }

    #[test]
    fn explicit_reason_is_none_when_enabled() {
        assert_eq!(explicit_reason(true), None);
    }

    #[test]
    fn probe_health_cached_is_healthy_for_live_daemon() {
        // Only meaningful when a daemon actually runs; skipped otherwise.
        let url = std::env::var("DECIDER_URL").unwrap_or_else(|_| "http://127.0.0.1:8001".into());
        if probe_health(&url, Duration::from_millis(400)).is_err() {
            eprintln!("skip: no decider daemon at {url}");
            return;
        }
        assert!(probe_health_cached(&url, Duration::from_millis(400)).is_ok());
    }
}
