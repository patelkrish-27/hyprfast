//! Structured errors for the persistent browser runtime (Phases 1–2).
//!
//! Every failure mode the transport and the Phase 2 daemon/IPC layer can
//! produce has an explicit variant. Generic `anyhow` errors are never used
//! where one of these applies.

use std::fmt;

/// Structured runtime error. All variants are terminal, caller-actionable
/// categories — never opaque strings.
#[derive(Debug, Clone, PartialEq)]
pub enum RuntimeError {
    /// Initial WebSocket handshake / TCP connect to the browser-level
    /// endpoint failed.
    ConnectionFailed(String),
    /// The browser-level connection is dead (closed, killed, or never
    /// established). Contains an optional detail string.
    RuntimeDead(String),
    /// A CDP call exceeded its deadline while the connection itself was
    /// still alive. Never produced for a dead connection — that is
    /// `RuntimeDead`, never `Timeout`.
    Timeout {
        method: String,
        timeout_ms: u64,
    },
    /// The browser answered with a CDP protocol-level error object.
    CdpError {
        method: String,
        code: i64,
        message: String,
    },
    /// A frame arrived that could not be decoded, was oversized, or
    /// violated the expected response/event shape. Never panics.
    InvalidResponse(String),
    /// Flat-session prerequisites (`Target.setAutoAttach { flatten: true }`
    /// and the target/session model) could not be established. The runtime
    /// refuses to initialize — no fallback, no partial init (rule 34 / I25).
    UnsupportedBrowserProtocol(String),
    // ---- Phase 2: lifecycle / IPC --------------------------------------
    /// Lifecycle state is `Starting` or `Connecting`: the runtime exists
    /// but is not ready to accept new actions (§1 permission table).
    NotReady(String),
    /// Lifecycle state is `Reconnecting`: new state-changing dispatch is
    /// rejected while a reconnect generation is in progress (§1 table).
    Reconnecting(String),
    /// Lifecycle state is `Stopping`: the server is draining and refuses
    /// new work (§1 table).
    ShuttingDown(String),
    /// Lifecycle state is `Degraded`: read-only requests are served but
    /// no new state-changing dispatch is accepted (§1 table).
    Degraded(String),
    /// IPC `protocol_version` mismatch on handshake (rule 33 / I26).
    /// Structured expected/got pair — never a hang, never generic.
    ProtocolMismatch { expected: u32, got: u32 },
    /// A command frame arrived on an IPC connection before a successful
    /// handshake (invariant I26).
    HandshakeRequired(String),
    /// A second daemon was started while a live one owns the socket.
    DaemonAlreadyRunning(String),
    /// Unix-socket setup failure (directory ownership, bind, permissions;
    /// rule 23).
    Socket(String),
    // ---- Phase 6: element resolution -----------------------------------
    /// ElementRef's versions are behind current runtime state (§6 snapshot consistency).
    StaleElementRef(String),
    /// Multiple equally-valid candidates; must not guess (rule 7 / I18).
    AmbiguousElement { candidates: Vec<String>, detail: String },
    /// Deterministic resolution exhausted without a match.
    ResolutionFailed(String),
    /// Pre-action interactability failure (rule 10).
    ElementNotInteractable(String),
    /// Invalid selector (e.g. would have been :contains) — never generated (B1).
    InvalidSelector(String),
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RuntimeError::ConnectionFailed(detail) => {
                write!(f, "CDP connection failed: {detail}")
            }
            RuntimeError::RuntimeDead(detail) => {
                write!(f, "browser runtime is dead: {detail}")
            }
            RuntimeError::Timeout { method, timeout_ms } => {
                write!(f, "CDP call {method} timed out after {timeout_ms}ms")
            }
            RuntimeError::CdpError {
                method,
                code,
                message,
            } => {
                write!(f, "CDP {method} error {code}: {message}")
            }
            RuntimeError::InvalidResponse(detail) => {
                write!(f, "invalid CDP response: {detail}")
            }
            RuntimeError::UnsupportedBrowserProtocol(detail) => {
                write!(f, "unsupported browser protocol (flat session required): {detail}")
            }
            RuntimeError::NotReady(detail) => {
                write!(f, "browser runtime not ready: {detail}")
            }
            RuntimeError::Reconnecting(detail) => {
                write!(f, "browser runtime reconnecting: {detail}")
            }
            RuntimeError::ShuttingDown(detail) => {
                write!(f, "browser runtime shutting down: {detail}")
            }
            RuntimeError::Degraded(detail) => {
                write!(f, "browser runtime degraded (read-only): {detail}")
            }
            RuntimeError::ProtocolMismatch { expected, got } => {
                write!(
                    f,
                    "IPC protocol mismatch: daemon speaks {expected}, client spoke {got}"
                )
            }
            RuntimeError::HandshakeRequired(detail) => {
                write!(f, "IPC handshake required before commands: {detail}")
            }
            RuntimeError::DaemonAlreadyRunning(detail) => {
                write!(f, "browser runtime daemon already running: {detail}")
            }
            RuntimeError::Socket(detail) => {
                write!(f, "browser runtime socket error: {detail}")
            }
            RuntimeError::StaleElementRef(detail) => {
                write!(f, "stale element ref: {detail}")
            }
            RuntimeError::AmbiguousElement { candidates, detail } => {
                write!(f, "ambiguous element ({detail}): candidates={candidates:?}")
            }
            RuntimeError::ResolutionFailed(detail) => {
                write!(f, "resolution failed: {detail}")
            }
            RuntimeError::ElementNotInteractable(detail) => {
                write!(f, "element not interactable: {detail}")
            }
            RuntimeError::InvalidSelector(detail) => {
                write!(f, "invalid selector: {detail}")
            }
        }
    }
}

impl std::error::Error for RuntimeError {}

impl RuntimeError {
    /// Stable wire tag for the IPC error envelope. Every variant maps to
    /// exactly one tag; unknown tags from a newer daemon decode to
    /// `InvalidResponse`, never to a guessed variant.
    pub fn wire_type(&self) -> &'static str {
        match self {
            RuntimeError::ConnectionFailed(_) => "ConnectionFailed",
            RuntimeError::RuntimeDead(_) => "RuntimeDead",
            RuntimeError::Timeout { .. } => "Timeout",
            RuntimeError::CdpError { .. } => "CdpError",
            RuntimeError::InvalidResponse(_) => "InvalidResponse",
            RuntimeError::UnsupportedBrowserProtocol(_) => "UnsupportedBrowserProtocol",
            RuntimeError::NotReady(_) => "NotReady",
            RuntimeError::Reconnecting(_) => "Reconnecting",
            RuntimeError::ShuttingDown(_) => "ShuttingDown",
            RuntimeError::Degraded(_) => "Degraded",
            RuntimeError::ProtocolMismatch { .. } => "ProtocolMismatch",
            RuntimeError::HandshakeRequired(_) => "HandshakeRequired",
            RuntimeError::DaemonAlreadyRunning(_) => "DaemonAlreadyRunning",
            RuntimeError::Socket(_) => "Socket",
            RuntimeError::StaleElementRef(_) => "StaleElementRef",
            RuntimeError::AmbiguousElement { .. } => "AmbiguousElement",
            RuntimeError::ResolutionFailed(_) => "ResolutionFailed",
            RuntimeError::ElementNotInteractable(_) => "ElementNotInteractable",
            RuntimeError::InvalidSelector(_) => "InvalidSelector",
        }
    }

    /// Structured IPC encoding: `{"type": <tag>, ...variant fields}`.
    pub fn to_wire(&self) -> serde_json::Value {
        use serde_json::json;
        match self {
            RuntimeError::ConnectionFailed(d)
            | RuntimeError::RuntimeDead(d)
            | RuntimeError::NotReady(d)
            | RuntimeError::Reconnecting(d)
            | RuntimeError::ShuttingDown(d)
            | RuntimeError::Degraded(d)
            | RuntimeError::HandshakeRequired(d)
            | RuntimeError::DaemonAlreadyRunning(d)
            |             RuntimeError::Socket(d)
            | RuntimeError::InvalidResponse(d)
            | RuntimeError::UnsupportedBrowserProtocol(d)
            | RuntimeError::StaleElementRef(d)
            | RuntimeError::ResolutionFailed(d)
            | RuntimeError::ElementNotInteractable(d)
            | RuntimeError::InvalidSelector(d) => {
                json!({"type": self.wire_type(), "detail": d})
            }
            RuntimeError::Timeout { method, timeout_ms } => {
                json!({"type": "Timeout", "method": method, "timeout_ms": timeout_ms})
            }
            RuntimeError::CdpError { method, code, message } => {
                json!({"type": "CdpError", "method": method, "code": code, "message": message})
            }
            RuntimeError::ProtocolMismatch { expected, got } => {
                json!({"type": "ProtocolMismatch", "expected": expected, "got": got})
            }
            RuntimeError::AmbiguousElement { candidates, detail } => {
                json!({"type": "AmbiguousElement", "candidates": candidates, "detail": detail})
            }
        }
    }

    /// Decode [`RuntimeError::to_wire`]. Returns `None` only for a truly
    /// unparsable envelope (caller maps that to `InvalidResponse`).
    pub fn from_wire(v: &serde_json::Value) -> Option<Self> {
        let t = v.get("type")?.as_str()?;
        let detail = || v.get("detail").and_then(|d| d.as_str()).unwrap_or("").to_string();
        Some(match t {
            "ConnectionFailed" => RuntimeError::ConnectionFailed(detail()),
            "RuntimeDead" => RuntimeError::RuntimeDead(detail()),
            "Timeout" => RuntimeError::Timeout {
                method: v.get("method").and_then(|m| m.as_str()).unwrap_or("").to_string(),
                timeout_ms: v.get("timeout_ms").and_then(|m| m.as_u64()).unwrap_or(0),
            },
            "CdpError" => RuntimeError::CdpError {
                method: v.get("method").and_then(|m| m.as_str()).unwrap_or("").to_string(),
                code: v.get("code").and_then(|c| c.as_i64()).unwrap_or(0),
                message: v.get("message").and_then(|m| m.as_str()).unwrap_or("").to_string(),
            },
            "InvalidResponse" => RuntimeError::InvalidResponse(detail()),
            "UnsupportedBrowserProtocol" => RuntimeError::UnsupportedBrowserProtocol(detail()),
            "NotReady" => RuntimeError::NotReady(detail()),
            "Reconnecting" => RuntimeError::Reconnecting(detail()),
            "ShuttingDown" => RuntimeError::ShuttingDown(detail()),
            "Degraded" => RuntimeError::Degraded(detail()),
            "ProtocolMismatch" => RuntimeError::ProtocolMismatch {
                expected: v.get("expected").and_then(|e| e.as_u64()).unwrap_or(0) as u32,
                got: v.get("got").and_then(|g| g.as_u64()).unwrap_or(0) as u32,
            },
            "HandshakeRequired" => RuntimeError::HandshakeRequired(detail()),
            "DaemonAlreadyRunning" => RuntimeError::DaemonAlreadyRunning(detail()),
            "Socket" => RuntimeError::Socket(detail()),
            "StaleElementRef" => RuntimeError::StaleElementRef(detail()),
            "AmbiguousElement" => RuntimeError::AmbiguousElement {
                candidates: v
                    .get("candidates")
                    .and_then(|c| c.as_array())
                    .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
                    .unwrap_or_default(),
                detail: detail(),
            },
            "ResolutionFailed" => RuntimeError::ResolutionFailed(detail()),
            "ElementNotInteractable" => RuntimeError::ElementNotInteractable(detail()),
            "InvalidSelector" => RuntimeError::InvalidSelector(detail()),
            _ => return None,
        })
    }
}

/// Convenience alias used across `browser_runtime`.
pub type RuntimeResult<T> = Result<T, RuntimeError>;
