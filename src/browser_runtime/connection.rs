//! Phase 1 — browser-level persistent CDP transport.
//!
//! Exactly ONE browser-level WebSocket per [`BrowserRuntime`], opened
//! against the browser endpoint from `/json/version` (never a page target
//! URL). Page/tab multiplexing uses CDP flat-session mode: every
//! target-specific request carries `sessionId` over this single socket.
//!
//! This is the ONLY production file allowed to call `connect_async`
//! (contract rule 3 / invariant I2). Do not add another call site.
//!
//! Ordering pipeline (rule 28, plan §4):
//! ```text
//! reader task: read frame -> assign monotonic `sequence` immediately
//!     -> inline decode (small) | offload to decode worker (large, rule 25)
//!     -> in-order reorder buffer keyed by `sequence`, drains only the
//!        next-expected sequence -> dispatcher resolves pending requests
//!        and broadcasts CdpEvents in strict wire-arrival order.
//! ```

use std::collections::{BTreeMap, HashMap};
use std::sync::{
    Mutex,
    Weak,
    atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering},
};
use std::time::{Duration, Instant, SystemTime};

use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;

use super::error::{RuntimeError, RuntimeResult};

// ---------------------------------------------------------------------------
// Wire types
// ---------------------------------------------------------------------------

/// Outgoing CDP JSON-RPC request. `session_id` selects the flat-session
/// target; `None` addresses the browser session itself.
#[derive(Debug, Clone)]
pub struct CdpRequest {
    pub id: i64,
    pub method: String,
    pub params: Value,
    pub session_id: Option<String>,
}

impl CdpRequest {
    fn to_wire_value(&self) -> Value {
        let mut obj = serde_json::Map::with_capacity(4);
        obj.insert("id".to_string(), Value::from(self.id));
        obj.insert("method".to_string(), Value::from(self.method.clone()));
        obj.insert("params".to_string(), self.params.clone());
        if let Some(session) = &self.session_id {
            obj.insert("sessionId".to_string(), Value::from(session.clone()));
        }
        Value::Object(obj)
    }
}

/// Protocol-level CDP error object (the `error` member of a response).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CdpError {
    pub code: i64,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// Incoming CDP JSON-RPC response.
#[derive(Debug, Clone)]
pub struct CdpResponse {
    pub id: i64,
    pub result: Option<Value>,
    pub error: Option<CdpError>,
    pub session_id: Option<String>,
}

/// Broadcast CDP event with transport ordering metadata.
///
/// `sequence` is the monotonic wire-arrival sequence assigned by the reader
/// task the moment the frame was read — before any decode. Subscribers can
/// rely on `sequence` order == browser emission order on the wire (I9).
#[derive(Debug, Clone)]
pub struct CdpEvent {
    pub method: String,
    pub params: Value,
    pub session_id: Option<String>,
    pub sequence: u64,
    pub timestamp: Instant,
}

/// Browser identity captured via `Browser.getVersion` at connect time.
#[derive(Debug, Clone, Default)]
pub struct BrowserInfo {
    pub product: String,
    pub revision: String,
    pub protocol_version: String,
    pub js_version: String,
}

/// Point-in-time transport diagnostics snapshot.
#[derive(Debug, Clone)]
pub struct BrowserRuntimeDiagnostics {
    pub connection_id: String,
    pub browser_info: BrowserInfo,
    pub pending_request_count: usize,
    pub event_count: u64,
    pub connected_at: SystemTime,
    pub attached_session_ids: Vec<String>,
    /// Reorder buffer depth right now (queued, not yet dispatchable).
    pub reorder_buffer_depth_current: usize,
    /// Maximum reorder buffer depth observed over the connection lifetime.
    /// Stays 0 if decode never ran behind the wire.
    pub reorder_buffer_depth_max: usize,
    /// Frames decoded on the offload worker (size >= threshold).
    pub offloaded_decode_count: u64,
    /// Oversized frames rejected without decode (failed cleanly, no panic).
    pub oversize_dropped_count: u64,
}

// ---------------------------------------------------------------------------
// Config + sentinel codes
// ---------------------------------------------------------------------------

/// Decode offload threshold (rule 25): frames at or above this size are
/// decoded on a blocking worker so the reader never stalls on a multi-MB
/// AX tree / screenshot payload. Default 2 MiB per the plan.
pub const DEFAULT_DECODE_OFFLOAD_THRESHOLD_BYTES: usize = 2 * 1024 * 1024;
/// Max accepted WS message size (rule 25 / I21). Larger frames fail cleanly
/// with `InvalidResponse` — never a panic, never an OOM.
pub const DEFAULT_MAX_MESSAGE_SIZE_BYTES: usize = 64 * 1024 * 1024;
/// Tungstenite-level reassembly ceiling. Deliberately larger than
/// [`DEFAULT_MAX_MESSAGE_SIZE_BYTES`]: oversize frames must reach the
/// reader so they fail cleanly per-request (`InvalidResponse`) instead of
/// killing the whole connection at the codec layer.
const TUNGSTENITE_MAX_MESSAGE_SIZE_BYTES: usize = 256 * 1024 * 1024;
/// Default per-call deadline for [`BrowserRuntime::call`].
pub const DEFAULT_CALL_TIMEOUT: Duration = Duration::from_secs(30);

/// Internal marker: synthetic response error code meaning "the transport
/// died while this request was in flight". Negative-test collisions with
/// real browser errors are avoided via the message prefix check.
const RUNTIME_DEAD_CODE: i64 = -32001;
const RUNTIME_DEAD_PREFIX: &str = "HYPRFAST_RUNTIME_DEAD:";
/// Internal marker: synthetic response error code meaning "this frame was
/// oversize or undecodable".
const INVALID_RESPONSE_CODE: i64 = -32002;
const INVALID_RESPONSE_PREFIX: &str = "HYPRFAST_INVALID_RESPONSE:";

#[derive(Debug, Clone, Copy)]
pub struct RuntimeConfig {
    pub decode_offload_threshold_bytes: usize,
    pub max_message_size_bytes: usize,
    pub default_call_timeout: Duration,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            decode_offload_threshold_bytes: DEFAULT_DECODE_OFFLOAD_THRESHOLD_BYTES,
            max_message_size_bytes: DEFAULT_MAX_MESSAGE_SIZE_BYTES,
            default_call_timeout: DEFAULT_CALL_TIMEOUT,
        }
    }
}

// ---------------------------------------------------------------------------
// Shared transport state
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
struct ReorderDepth {
    current: usize,
    max_observed: usize,
}

struct Shared {
    connection_id: String,
    pending: Mutex<HashMap<i64, oneshot::Sender<CdpResponse>>>,
    alive: AtomicBool,
    next_id: AtomicI64,
    next_sequence: AtomicU64,
    event_tx: broadcast::Sender<CdpEvent>,
    event_count: AtomicU64,
    reorder_depth: Mutex<ReorderDepth>,
    offloaded_count: AtomicU64,
    oversize_dropped: AtomicU64,
    attached_sessions: Mutex<Vec<String>>,
    browser_info: Mutex<BrowserInfo>,
    connected_at: SystemTime,
    writer_tx: Mutex<Option<mpsc::UnboundedSender<String>>>,
    tasks: Mutex<Vec<JoinHandle<()>>>,
}

impl Shared {
    /// Fail-closed transition: mark dead, drop the writer channel so future
    /// `call()`s fail fast, and resolve every in-flight request as
    /// `RuntimeDead` (via a synthetic marker response the receiver maps to
    /// `RuntimeError::RuntimeDead`; dropped receivers also map to it).
    fn mark_dead(&self, reason: &str) {
        if !self.alive.swap(false, Ordering::SeqCst) {
            return;
        }
        // Drop writer first: no new frames may be queued after this point.
        self.writer_tx.lock().map(|mut g| g.take()).ok();
        let pending = self
            .pending
            .lock()
            .map(|mut g| std::mem::take(&mut *g))
            .unwrap_or_default();
        for (id, tx) in pending {
            let _ = tx.send(CdpResponse {
                id,
                result: None,
                error: Some(CdpError {
                    code: RUNTIME_DEAD_CODE,
                    message: format!("{RUNTIME_DEAD_PREFIX} {reason}"),
                    data: None,
                }),
                session_id: None,
            });
        }
    }

    fn note_reorder_depth(&self, current: usize) {
        if let Ok(mut g) = self.reorder_depth.lock() {
            g.current = current;
            if current > g.max_observed {
                g.max_observed = current;
            }
        }
    }

    fn track_attached(&self, session_id: &str) {
        if let Ok(mut g) = self.attached_sessions.lock() {
            if !g.iter().any(|s| s == session_id) {
                g.push(session_id.to_string());
            }
        }
    }

    fn track_detached(&self, session_id: &str) {
        if let Ok(mut g) = self.attached_sessions.lock() {
            g.retain(|s| s != session_id);
        }
    }
}

static CONNECTION_COUNTER: AtomicU64 = AtomicU64::new(1);

fn new_connection_id() -> String {
    let n = CONNECTION_COUNTER.fetch_add(1, Ordering::SeqCst);
    let ms = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    format!("conn-{n}-{ms}")
}

// ---------------------------------------------------------------------------
// Pipeline internals
// ---------------------------------------------------------------------------

/// Frame handed from the reader to the dispatcher. Every frame keeps its
/// wire sequence, including ones that will never decode: skipping a
/// sequence would stall the reorder buffer forever.
enum InboundFrame {
    Raw { sequence: u64, text: String },
    Oversize { sequence: u64, id_hint: Option<i64> },
}

/// Reorder-buffer slot after the decode stage.
enum OrderedItem {
    Message(Value),
    Undecodable { id_hint: Option<i64> },
}

/// Best-effort `id` extraction from the head of a frame without a full
/// parse (used only for oversize/undecodable frames). CDP responses carry
/// `"id"` as one of the first object members.
fn extract_id_prefix(text: &str) -> Option<i64> {
    let head = &text[..text.len().min(1024)];
    let key = head.find("\"id\"")?;
    let after = &head[key + 4..];
    let colon = after.find(':')?;
    let num: String = after[colon + 1..]
        .chars()
        .skip_while(|c| c.is_whitespace())
        .take_while(|c| c.is_ascii_digit() || *c == '-')
        .collect();
    if num.is_empty() || num == "-" {
        return None;
    }
    num.parse::<i64>().ok()
}

fn parse_cdp_error(value: &Value) -> Option<CdpError> {
    let err = value.get("error")?;
    if err.is_null() {
        return None;
    }
    Some(CdpError {
        code: err.get("code").and_then(Value::as_i64).unwrap_or(0),
        message: err
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("unknown CDP error")
            .to_string(),
        data: err.get("data").cloned(),
    })
}

fn session_of(value: &Value) -> Option<String> {
    value
        .get("sessionId")
        .and_then(Value::as_str)
        .map(str::to_string)
}

// ---------------------------------------------------------------------------
// BrowserRuntime
// ---------------------------------------------------------------------------

/// Persistent browser-level CDP transport (Phase 1).
///
/// Cloneable handle over shared connection state. Concurrent `call()`s are
/// fully multiplexed over the single WebSocket (rule 29) — no global
/// serialization at this layer.
#[derive(Clone)]
pub struct BrowserRuntime {
    shared: std::sync::Arc<Shared>,
    config: RuntimeConfig,
}

impl std::fmt::Debug for BrowserRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BrowserRuntime")
            .field("connection_id", &self.shared.connection_id)
            .field("alive", &self.shared.alive.load(Ordering::SeqCst))
            .finish_non_exhaustive()
    }
}

impl BrowserRuntime {
    /// Connect to the browser-level WebSocket URL from `/json/version`
    /// with default transport config. Performs `Browser.getVersion`,
    /// flat-session `Target.setAutoAttach`, `Target.getTargets` and
    /// `Target.attachToTarget` before returning.
    pub async fn connect(debugger_ws_url: &str) -> RuntimeResult<Self> {
        Self::connect_with_config(debugger_ws_url, RuntimeConfig::default()).await
    }

    /// Same as [`BrowserRuntime::connect`] with explicit transport tuning
    /// (thresholds/timeouts). Used by tests to force the offload path with
    /// a small threshold; production uses defaults.
    pub async fn connect_with_config(
        debugger_ws_url: &str,
        config: RuntimeConfig,
    ) -> RuntimeResult<Self> {
        // THE single allowed connect_async call site (rule 3 / I2).
        // Uses an explicit codec config so the tungstenite layer never
        // kills the connection before the app-level size policy (rule 25)
        // can fail a single oversized frame cleanly.
        let mut ws_config = tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default();
        ws_config.max_message_size = Some(TUNGSTENITE_MAX_MESSAGE_SIZE_BYTES);
        ws_config.max_frame_size = Some(TUNGSTENITE_MAX_MESSAGE_SIZE_BYTES);
        let (ws, _) = tokio_tungstenite::connect_async_with_config(
            debugger_ws_url,
            Some(ws_config),
            false,
        )
        .await
        .map_err(|e| RuntimeError::ConnectionFailed(e.to_string()))?;
        let (sink, stream) = ws.split();

        let (event_tx, _) = broadcast::channel::<CdpEvent>(4096);
        let (writer_tx, writer_rx) = mpsc::unbounded_channel::<String>();
        let (inbound_tx, inbound_rx) = mpsc::unbounded_channel::<InboundFrame>();
        // Completion channel for offloaded decodes (may finish out of order).
        let (completion_tx, completion_rx) =
            mpsc::unbounded_channel::<(u64, Result<Value, String>)>();

        let shared = std::sync::Arc::new(Shared {
            connection_id: new_connection_id(),
            pending: Mutex::new(HashMap::new()),
            alive: AtomicBool::new(true),
            next_id: AtomicI64::new(1),
            next_sequence: AtomicU64::new(0),
            event_tx,
            event_count: AtomicU64::new(0),
            reorder_depth: Mutex::new(ReorderDepth::default()),
            offloaded_count: AtomicU64::new(0),
            oversize_dropped: AtomicU64::new(0),
            attached_sessions: Mutex::new(Vec::new()),
            browser_info: Mutex::new(BrowserInfo::default()),
            connected_at: SystemTime::now(),
            writer_tx: Mutex::new(Some(writer_tx)),
            tasks: Mutex::new(Vec::new()),
        });

        // Writer task: owns the SplitSink. Single writer, FIFO per send.
        let writer_weak: Weak<Shared> = std::sync::Arc::downgrade(&shared);
        let writer_handle = tokio::spawn(writer_loop(writer_weak, sink, writer_rx));

        // Reader task: owns the SplitStream. Assigns `sequence` immediately
        // on read — before any decode — then forwards (rule 28).
        let reader_weak: Weak<Shared> = std::sync::Arc::downgrade(&shared);
        let reader_handle = tokio::spawn(reader_loop(
            reader_weak,
            stream,
            inbound_tx,
            config.max_message_size_bytes,
        ));

        // Dispatcher task: decode (inline/offloaded) -> reorder buffer ->
        // ordered dispatch to pending requests + event broadcast.
        let dispatcher_weak: Weak<Shared> = std::sync::Arc::downgrade(&shared);
        let dispatcher_handle = tokio::spawn(dispatcher_loop(
            dispatcher_weak,
            inbound_rx,
            completion_rx,
            completion_tx,
            config.decode_offload_threshold_bytes,
        ));

        if let Ok(mut tasks) = shared.tasks.lock() {
            tasks.push(writer_handle);
            tasks.push(reader_handle);
            tasks.push(dispatcher_handle);
        }

        let rt = Self { shared, config };

        // ---- Initialization (fail closed, rule 34 / I25) ----
        if let Err(e) = rt.initialize().await {
            rt.shutdown().await;
            return Err(e);
        }
        Ok(rt)
    }

    /// Startup handshake: version -> flat auto-attach -> enumerate targets
    /// -> attach page targets in flatten mode. Any structural failure here
    /// aborts the runtime with `UnsupportedBrowserProtocol`.
    async fn initialize(&self) -> RuntimeResult<()> {
        // 1. Browser identity.
        let version = self
            .call(None, "Browser.getVersion", Value::Object(serde_json::Map::new()))
            .await
            .map_err(|e| {
                RuntimeError::UnsupportedBrowserProtocol(format!(
                    "Browser.getVersion failed during init: {e}"
                ))
            })?;
        {
            let info = BrowserInfo {
                product: version
                    .get("product")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                revision: version
                    .get("revision")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                protocol_version: version
                    .get("protocolVersion")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                js_version: version
                    .get("jsVersion")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
            };
            if let Ok(mut g) = self.shared.browser_info.lock() {
                *g = info;
            }
        }

        // 2. FLAT-SESSION HARD PREREQUISITE (rule 34): no fallback, no
        //    partial init. Any failure -> UnsupportedBrowserProtocol.
        self.call(
            None,
            "Target.setAutoAttach",
            serde_json::json!({
                "autoAttach": true,
                "waitForDebuggerOnStart": false,
                "flatten": true
            }),
        )
        .await
        .map_err(|e| {
            RuntimeError::UnsupportedBrowserProtocol(format!(
                "Target.setAutoAttach {{flatten:true}} failed: {e}"
            ))
        })?;

        // 3. Enumerate targets; attach page targets in flatten mode over
        //    this same socket. Never open another WebSocket (I1/I24).
        let targets = self
            .call(
                None,
                "Target.getTargets",
                Value::Object(serde_json::Map::new()),
            )
            .await
            .map_err(|e| {
                RuntimeError::UnsupportedBrowserProtocol(format!(
                    "Target.getTargets failed during init: {e}"
                ))
            })?;
        let infos = targets
            .get("targetInfos")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut attached = 0usize;
        let mut page_targets = 0usize;
        for info in &infos {
            if info.get("type").and_then(Value::as_str) != Some("page") {
                continue;
            }
            page_targets += 1;
            let Some(target_id) = info.get("targetId").and_then(Value::as_str) else {
                continue;
            };
            match self
                .call(
                    None,
                    "Target.attachToTarget",
                    serde_json::json!({ "targetId": target_id, "flatten": true }),
                )
                .await
            {
                Ok(resp) => {
                    if let Some(session_id) =
                        resp.get("sessionId").and_then(Value::as_str)
                    {
                        self.shared.track_attached(session_id);
                        attached += 1;
                    }
                }
                Err(e) => {
                    // A single vanishing target must not fail init; the
                    // flat auto-attach above still covers new targets.
                    tracing::warn!(
                        target_id = target_id,
                        error = %e,
                        "attachToTarget failed during init; continuing"
                    );
                }
            }
        }
        if page_targets > 0 && attached == 0 {
            return Err(RuntimeError::UnsupportedBrowserProtocol(format!(
                "could not attach to any of {page_targets} page target(s) in flatten mode"
            )));
        }
        Ok(())
    }

    /// Concurrent CDP call over the single shared WebSocket (rule 29: no
    /// global serialization here). `session_id` selects the flat-session
    /// target (I4); `None` addresses the browser session.
    pub async fn call(
        &self,
        session_id: Option<&str>,
        method: &str,
        params: Value,
    ) -> RuntimeResult<Value> {
        self.call_with_timeout(session_id, method, params, self.config.default_call_timeout)
            .await
    }

    /// Same as [`BrowserRuntime::call`] with an explicit per-call deadline
    /// (rule 25: callers with known-large payloads pass a larger timeout).
    /// A timeout on a live connection yields `Timeout`; a dead connection
    /// always yields `RuntimeDead`, never `Timeout`.
    pub async fn call_with_timeout(
        &self,
        session_id: Option<&str>,
        method: &str,
        params: Value,
    timeout: Duration,
    ) -> RuntimeResult<Value> {
        if !self.shared.alive.load(Ordering::SeqCst) {
            return Err(RuntimeError::RuntimeDead(
                "connection is closed".to_string(),
            ));
        }
        let id = self.shared.next_id.fetch_add(1, Ordering::SeqCst);
        let request = CdpRequest {
            id,
            method: method.to_string(),
            params,
            session_id: session_id.map(str::to_string),
        };
        let text = request.to_wire_value().to_string();
        let (tx, rx) = oneshot::channel();
        if self
            .shared
            .pending
            .lock()
            .map(|mut g| g.insert(id, tx))
            .is_err()
        {
            return Err(RuntimeError::RuntimeDead("runtime state poisoned".to_string()));
        }
        let queued = self
            .shared
            .writer_tx
            .lock()
            .map(|g| {
                g.as_ref()
                    .map(|w| w.send(text).is_ok())
                    .unwrap_or(false)
            })
            .unwrap_or(false);
        if !queued {
            self.shared.pending.lock().map(|mut g| g.remove(&id)).ok();
            return Err(RuntimeError::RuntimeDead(
                "connection closed while queueing request".to_string(),
            ));
        }
        match tokio::time::timeout(timeout, rx).await {
            Err(_) => {
                // Late responses (if any) find no pending entry and are dropped.
                self.shared.pending.lock().map(|mut g| g.remove(&id)).ok();
                Err(RuntimeError::Timeout {
                    method: method.to_string(),
                    timeout_ms: timeout.as_millis().min(u128::from(u64::MAX)) as u64,
                })
            }
            Ok(Err(_)) => Err(RuntimeError::RuntimeDead(
                "response channel closed".to_string(),
            )),
            Ok(Ok(resp)) => interpret_response(method, resp),
        }
    }

    /// Subscribe to the ordered live `CdpEvent` stream. Events are
    /// broadcast in wire-sequence order (I9). Lagging receivers observe
    /// `broadcast::error::RecvError::Lagged` and must resubscribe — the
    /// transport never blocks dispatch on a slow subscriber.
    pub fn subscribe(&self) -> broadcast::Receiver<CdpEvent> {
        self.shared.event_tx.subscribe()
    }

    /// Transport liveness (writer + reader healthy, no close observed).
    pub fn is_alive(&self) -> bool {
        self.shared.alive.load(Ordering::SeqCst)
    }

    /// Point-in-time diagnostics snapshot.
    pub fn diagnostics(&self) -> BrowserRuntimeDiagnostics {
        let (pending_request_count, attached_session_ids, browser_info) = (
            self.shared
                .pending
                .lock()
                .map(|g| g.len())
                .unwrap_or(0),
            self.shared
                .attached_sessions
                .lock()
                .map(|g| g.clone())
                .unwrap_or_default(),
            self.shared
                .browser_info
                .lock()
                .map(|g| g.clone())
                .unwrap_or_default(),
        );
        let (reorder_buffer_depth_current, reorder_buffer_depth_max) = self
            .shared
            .reorder_depth
            .lock()
            .map(|g| (g.current, g.max_observed))
            .unwrap_or((0, 0));
        BrowserRuntimeDiagnostics {
            connection_id: self.shared.connection_id.clone(),
            browser_info,
            pending_request_count,
            event_count: self.shared.event_count.load(Ordering::SeqCst),
            connected_at: self.shared.connected_at,
            attached_session_ids,
            reorder_buffer_depth_current,
            reorder_buffer_depth_max,
            offloaded_decode_count: self.shared.offloaded_count.load(Ordering::SeqCst),
            oversize_dropped_count: self.shared.oversize_dropped.load(Ordering::SeqCst),
        }
    }

    /// Fail-closed shutdown: stop accepting requests (alive=false), resolve
    /// pending as `RuntimeDead`, abort reader/writer/dispatcher tasks.
    pub async fn shutdown(&self) {
        self.shared.mark_dead("shutdown");
        let handles = self
            .shared
            .tasks
            .lock()
            .map(|mut g| std::mem::take(&mut *g))
            .unwrap_or_default();
        for h in handles {
            h.abort();
        }
    }
}

/// Map a wire response to the caller-visible result, translating internal
/// marker errors first, then real browser `CdpError`s.
fn interpret_response(method: &str, resp: CdpResponse) -> RuntimeResult<Value> {
    if let Some(err) = resp.error {
        if err.code == RUNTIME_DEAD_CODE && err.message.starts_with(RUNTIME_DEAD_PREFIX) {
            return Err(RuntimeError::RuntimeDead(err.message));
        }
        if err.code == INVALID_RESPONSE_CODE
            && err.message.starts_with(INVALID_RESPONSE_PREFIX)
        {
            return Err(RuntimeError::InvalidResponse(err.message));
        }
        return Err(RuntimeError::CdpError {
            method: method.to_string(),
            code: err.code,
            message: err.message,
        });
    }
    Ok(resp.result.unwrap_or(Value::Null))
}

// ---------------------------------------------------------------------------
// Tasks
// ---------------------------------------------------------------------------

/// Writer task: owns the WS SplitSink, sends queued request frames FIFO.
async fn writer_loop(
    shared: Weak<Shared>,
    mut sink: futures::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        Message,
    >,
    mut rx: mpsc::UnboundedReceiver<String>,
) {
    while let Some(text) = rx.recv().await {
        if shared.upgrade().is_none() {
            break;
        }
        if let Err(e) = sink.send(Message::Text(text.into())).await {
            tracing::warn!(error = %e, "CDP writer send failed; marking runtime dead");
            if let Some(s) = shared.upgrade() {
                s.mark_dead(&format!("writer send failed: {e}"));
            }
            break;
        }
    }
    // Writer exiting without an explicit close: if the runtime still looks
    // alive, the reader will observe the close independently; do not mark
    // dead here on the clean-shutdown path (mark_dead is idempotent anyway).
}

/// Reader task: owns the WS SplitStream. Assigns the monotonic `sequence`
/// IMMEDIATELY on read — before any size check or decode — then forwards
/// to the dispatcher (rule 28). Never blocks on decode.
async fn reader_loop(
    shared: Weak<Shared>,
    mut stream: futures::stream::SplitStream<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    >,
    inbound_tx: mpsc::UnboundedSender<InboundFrame>,
    max_message_size_bytes: usize,
) {
    while let Some(msg) = stream.next().await {
        let Some(s) = shared.upgrade() else {
            break;
        };
        // Control frames carry no CDP payload and consume no dispatch
        // sequence; only data frames participate in the reorder buffer, so
        // only they are assigned one — immediately on read, before any
        // size check or decode (rule 28).
        match msg {
            Ok(Message::Text(text)) => {
                let sequence = s.next_sequence.fetch_add(1, Ordering::SeqCst);
                let text = text.to_string();
                if text.len() > max_message_size_bytes {
                    s.oversize_dropped.fetch_add(1, Ordering::SeqCst);
                    let id_hint = extract_id_prefix(&text);
                    if inbound_tx
                        .send(InboundFrame::Oversize { sequence, id_hint })
                        .is_err()
                    {
                        break;
                    }
                    tracing::warn!(
                        sequence,
                        bytes = text.len(),
                        "oversized CDP frame rejected without decode"
                    );
                    continue;
                }
                if inbound_tx.send(InboundFrame::Raw { sequence, text }).is_err() {
                    break;
                }
            }
            Ok(Message::Binary(bytes)) => {
                let sequence = s.next_sequence.fetch_add(1, Ordering::SeqCst);
                if bytes.len() > max_message_size_bytes {
                    s.oversize_dropped.fetch_add(1, Ordering::SeqCst);
                    let id_hint =
                        String::from_utf8_lossy(&bytes).as_ref().pipe_extract_id();
                    if inbound_tx
                        .send(InboundFrame::Oversize { sequence, id_hint })
                        .is_err()
                    {
                        break;
                    }
                    continue;
                }
                match String::from_utf8(bytes.to_vec()) {
                    Ok(text) => {
                        if inbound_tx.send(InboundFrame::Raw { sequence, text }).is_err() {
                            break;
                        }
                    }
                    Err(_) => {
                        // Non-UTF8 binary frame: undecodable; keep the
                        // sequence alive with a tombstone so the reorder
                        // buffer cannot stall.
                        if inbound_tx
                            .send(InboundFrame::Oversize {
                                sequence,
                                id_hint: None,
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                }
            }
            Ok(Message::Close(_)) => break,
            Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_)) => {
                // No sequence consumed (see above); nothing to forward.
            }
            Err(e) => {
                tracing::warn!(error = %e, "CDP reader error; marking runtime dead");
                s.mark_dead(&format!("reader error: {e}"));
                break;
            }
        }
    }
    drop(inbound_tx);
    if let Some(s) = shared.upgrade() {
        if s.alive.load(Ordering::SeqCst) {
            s.mark_dead("reader: connection closed by peer");
        }
    }
}

/// Dispatcher task: decode (inline under threshold, worker at/over it),
/// reassemble through the in-order reorder buffer, then dispatch the
/// strictly-ordered stream to pending requests / event broadcast.
async fn dispatcher_loop(
    shared: Weak<Shared>,
    mut inbound_rx: mpsc::UnboundedReceiver<InboundFrame>,
    mut completion_rx: mpsc::UnboundedReceiver<(u64, Result<Value, String>)>,
    completion_tx: mpsc::UnboundedSender<(u64, Result<Value, String>)>,
    offload_threshold_bytes: usize,
) {
    let mut expected: u64 = 0;
    let mut buffer: BTreeMap<u64, OrderedItem> = BTreeMap::new();
    let mut inbound_open = true;
    let mut pending_offloads: usize = 0;

    macro_rules! insert_and_drain {
        ($seq:expr, $item:expr) => {{
            buffer.insert($seq, $item);
            drain_ready(&shared, &mut buffer, &mut expected);
        }};
    }

    loop {
        if shared.upgrade().is_none() {
            break;
        }
        if !inbound_open && pending_offloads == 0 {
            drain_ready(&shared, &mut buffer, &mut expected);
            break;
        }
        tokio::select! {
            biased;
            frame = inbound_rx.recv(), if inbound_open => {
                match frame {
                    None => {
                        inbound_open = false;
                    }
                    Some(InboundFrame::Oversize { sequence, id_hint }) => {
                        insert_and_drain!(sequence, OrderedItem::Undecodable { id_hint });
                    }
                    Some(InboundFrame::Raw { sequence, text }) => {
                        if text.len() >= offload_threshold_bytes {
                            // Large frame: decode off the hot path (rule 25).
                            // Completion may arrive out of order; the reorder
                            // buffer restores wire order (rule 28).
                            pending_offloads += 1;
                            if let Some(s) = shared.upgrade() {
                                s.offloaded_count.fetch_add(1, Ordering::SeqCst);
                            }
                            let tx = completion_tx.clone();
                            tokio::task::spawn_blocking(move || {
                                let parsed: Result<Value, String> =
                                    serde_json::from_str(&text).map_err(|e| e.to_string());
                                let _ = tx.send((sequence, parsed));
                            });
                        } else {
                            match serde_json::from_str::<Value>(&text) {
                                Ok(v) => insert_and_drain!(sequence, OrderedItem::Message(v)),
                                Err(_) => {
                                    let id_hint = extract_id_prefix(&text);
                                    insert_and_drain!(sequence, OrderedItem::Undecodable { id_hint });
                                }
                            }
                        }
                    }
                }
            },
            done = completion_rx.recv() => {
                match done {
                    None => {
                        // All offload workers finished without sending (only
                        // possible during teardown); avoid spinning.
                        if !inbound_open {
                            drain_ready(&shared, &mut buffer, &mut expected);
                            break;
                        }
                    }
                    Some((sequence, parsed)) => {
                        pending_offloads = pending_offloads.saturating_sub(1);
                        match parsed {
                            Ok(v) => insert_and_drain!(sequence, OrderedItem::Message(v)),
                            Err(_) => {
                                // Offloaded decode failed: no full text kept
                                // (it was moved); attribute by id is best
                                // effort — without the text we cannot scan.
                                insert_and_drain!(sequence, OrderedItem::Undecodable { id_hint: None });
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Drain every contiguous slot starting at `*expected`, dispatching each in
/// wire order. State-mutation order == wire arrival order (I9/I22).
fn drain_ready(
    shared: &Weak<Shared>,
    buffer: &mut BTreeMap<u64, OrderedItem>,
    expected: &mut u64,
) {
    while let Some(item) = buffer.remove(expected) {
        if let Some(s) = shared.upgrade() {
            dispatch_item(&s, *expected, item);
            s.note_reorder_depth(buffer.len());
        }
        *expected += 1;
    }
    if let Some(s) = shared.upgrade() {
        s.note_reorder_depth(buffer.len());
    }
}

/// Dispatch one ordered item: `id` -> pending request, `method` -> event
/// broadcast, neither -> drop (never panic, I21).
fn dispatch_item(shared: &Shared, sequence: u64, item: OrderedItem) {
    match item {
        OrderedItem::Undecodable { id_hint } => {
            if let Some(id) = id_hint {
                if let Some(tx) = shared.pending.lock().map(|mut g| g.remove(&id)).ok().flatten()
                {
                    let _ = tx.send(CdpResponse {
                        id,
                        result: None,
                        error: Some(CdpError {
                            code: INVALID_RESPONSE_CODE,
                            message: format!(
                                "{INVALID_RESPONSE_PREFIX} frame #{sequence} could not be decoded"
                            ),
                            data: None,
                        }),
                        session_id: None,
                    });
                }
            }
        }
        OrderedItem::Message(value) => {
            if let Some(id) = value.get("id").and_then(Value::as_i64) {
                let response = CdpResponse {
                    id,
                    result: value.get("result").cloned(),
                    error: parse_cdp_error(&value),
                    session_id: session_of(&value),
                };
                if let Some(tx) = shared.pending.lock().map(|mut g| g.remove(&id)).ok().flatten()
                {
                    let _ = tx.send(response);
                }
                // Late/unknown ids (timed-out callers): drop.
            } else if let Some(method) = value.get("method").and_then(Value::as_str) {
                let event = CdpEvent {
                    method: method.to_string(),
                    params: value.get("params").cloned().unwrap_or(Value::Null),
                    session_id: session_of(&value),
                    sequence,
                    timestamp: Instant::now(),
                };
                shared.event_count.fetch_add(1, Ordering::SeqCst);
                // Minimal session bookkeeping for diagnostics (not a target
                // manager — Phase 5 owns target state).
                if event.method == "Target.attachedToTarget" {
                    if let Some(sid) = event
                        .params
                        .get("sessionId")
                        .and_then(Value::as_str)
                    {
                        shared.track_attached(sid);
                    }
                } else if event.method == "Target.detachedFromTarget" {
                    if let Some(sid) = event
                        .params
                        .get("sessionId")
                        .and_then(Value::as_str)
                    {
                        shared.track_detached(sid);
                    }
                }
                let _ = shared.event_tx.send(event);
            }
            // Frames with neither id nor method: not CDP responses/events;
            // drop cleanly.
        }
    }
}

// Small helper to reuse the id-prefix scan on lossy binary heads.
trait PipeExtractId {
    fn pipe_extract_id(&self) -> Option<i64>;
}

impl PipeExtractId for str {
    fn pipe_extract_id(&self) -> Option<i64> {
        extract_id_prefix(self)
    }
}
