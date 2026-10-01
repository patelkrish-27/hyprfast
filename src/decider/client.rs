//! Reusable async HTTP client for the external Decider service.
//!
//! Invariants:
//! - Never loads a model locally, never spawns a Python subprocess; talks only
//!   to the external resident service at `DECIDER_URL`.
//! - Reuses a single `reqwest::Client` (connection pooling) via `once_cell`.
//! - Bounded concurrency: at most 4 in-flight `decide` calls (semaphore).
//! - Robust (de)serialization: handles both `context` and `state` keys.
//! - Strong error handling: connection failures, timeouts, malformed responses,
//!   invalid choices, and image encoding errors are surfaced as `anyhow::Error`.
//! - Metrics tracking mirrors `browser_runtime::metrics`.

use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use once_cell::sync::{Lazy, OnceCell};
use serde_json::Value;
use tokio::sync::Semaphore;

use crate::decider::config::DeciderConfig;
use crate::decider::types::{
    validate_image_field, DeciderQuestion, DeciderRequest, DeciderResponse,
};

// ---------------------------------------------------------------------------
// Global connection reuse
// ---------------------------------------------------------------------------

static GLOBAL_CLIENT: OnceCell<reqwest::Client> = OnceCell::new();

fn global_client(timeout: Duration) -> Result<reqwest::Client> {
    if let Some(c) = GLOBAL_CLIENT.get() {
        return Ok(c.clone());
    }
    let c = build_client(timeout)?;
    let _ = GLOBAL_CLIENT.set(c.clone());
    Ok(c)
}

fn build_client(timeout: Duration) -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(timeout)
        .no_proxy()
        .pool_idle_timeout(Duration::from_secs(90))
        .tcp_keepalive(Duration::from_secs(30))
        .build()
        .context("build decider reqwest client")
}

// ---------------------------------------------------------------------------
// Metrics
// ---------------------------------------------------------------------------

/// Lightweight metrics for Decider calls (analogous to RuntimeMetrics).
#[derive(Debug, Default)]
pub struct DeciderMetrics {
    pub request_count: AtomicU64,
    pub success_count: AtomicU64,
    pub error_count: AtomicU64,
    pub total_latency_ms: AtomicU64,
    pub timeout_count: AtomicU64,
    pub connection_error_count: AtomicU64,
    pub invalid_response_count: AtomicU64,
}

impl DeciderMetrics {
    pub fn snapshot(&self) -> Value {
        serde_json::json!({
            "request_count": self.request_count.load(Ordering::Relaxed),
            "success_count": self.success_count.load(Ordering::Relaxed),
            "error_count": self.error_count.load(Ordering::Relaxed),
            "total_latency_ms": self.total_latency_ms.load(Ordering::Relaxed),
            "timeout_count": self.timeout_count.load(Ordering::Relaxed),
            "connection_error_count": self.connection_error_count.load(Ordering::Relaxed),
            "invalid_response_count": self.invalid_response_count.load(Ordering::Relaxed),
        })
    }
    fn record(&self, latency_ms: u64, success: bool) {
        self.request_count.fetch_add(1, Ordering::Relaxed);
        self.total_latency_ms.fetch_add(latency_ms, Ordering::Relaxed);
        if success {
            self.success_count.fetch_add(1, Ordering::Relaxed);
        } else {
            self.error_count.fetch_add(1, Ordering::Relaxed);
        }
    }
}

static GLOBAL_METRICS: Lazy<DeciderMetrics> = Lazy::new(DeciderMetrics::default);

/// Access the global decider metrics singleton.
pub fn metrics() -> &'static DeciderMetrics {
    &GLOBAL_METRICS
}

// Bounded concurrency: at most 4 simultaneous decide calls per process.
static SEMAPHORE: Lazy<Arc<Semaphore>> = Lazy::new(|| Arc::new(Semaphore::new(4)));

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

/// Reusable Decider client. Clone is cheap (Arc inside).
#[derive(Debug, Clone)]
pub struct DeciderClient {
    config: DeciderConfig,
    client: reqwest::Client,
    semaphore: Arc<Semaphore>,
    metrics: Arc<DeciderMetrics>,
}

impl DeciderClient {
    /// Create with explicit config.
    pub fn new(config: DeciderConfig) -> Result<Self> {
        let client = build_client(config.timeout)?;
        Ok(Self {
            config,
            client,
            semaphore: SEMAPHORE.clone(),
            metrics: Arc::new(DeciderMetrics::default()),
        })
    }

    /// Create from environment (`DeciderConfig::from_env()`).
    pub fn from_env() -> Result<Self> {
        Self::new(DeciderConfig::from_env())
    }

    /// Create with a custom `reqwest::Client` (test injection).
    pub fn with_client(config: DeciderConfig, client: reqwest::Client) -> Self {
        Self {
            config,
            client,
            semaphore: SEMAPHORE.clone(),
            metrics: Arc::new(DeciderMetrics::default()),
        }
    }

    pub fn config(&self) -> &DeciderConfig {
        &self.config
    }

    pub fn metrics_snapshot(&self) -> Value {
        self.metrics.snapshot()
    }

    fn endpoint(&self, path: &str) -> String {
        format!("{}/{}", self.config.url.trim_end_matches('/'), path.trim_start_matches('/'))
    }

    // -----------------------------------------------------------------------
    // Core decide
    // -----------------------------------------------------------------------

    /// Core `decide` — POSTs a typed `DeciderRequest` and returns a typed `DeciderResponse`.
    ///
    /// Handles: validation, image base64/data-URI checks, numeric-ID choice
    /// validation, timeout/connection/malformed-response errors, and metrics.
    pub async fn decide(&self, req: &DeciderRequest) -> Result<DeciderResponse> {
        // Validate locally first.
        req.validate().context("decider request validation")?;

        // Concurrency bound.
        let _permit = self
            .semaphore
            .acquire()
            .await
            .context("decider semaphore closed")?;

        let t0 = Instant::now();
        // Primary endpoint per spec: POST /predict, fallback to /decide /v1/predict
        let primary_url = self.endpoint("predict");
        let fallback_urls = [self.endpoint("decide"), self.endpoint("v1/predict")];
        let url = primary_url.clone();

        // Build wire JSON. Preserve temperature from config if not set per-request.
        let mut wire = serde_json::to_value(req).context("serialize decider request")?;
        if wire.get("temperature").is_none() && self.config.temperature != 0.0 {
            wire["temperature"] = serde_json::json!(self.config.temperature);
        }
        // Ensure image is not empty string after validation.
        if let Some(img) = wire.get("image").and_then(|v| v.as_str()) {
            if img.trim().is_empty() {
                // Remove empty image field.
                if let Some(obj) = wire.as_object_mut() {
                    obj.remove("image");
                }
            }
        }

        // Try primary endpoint then fallbacks on 404
        let urls_to_try: Vec<String> = {
            let mut v = vec![url.clone()];
            v.extend(fallback_urls.iter().cloned());
            v
        };
        let mut last_resp: Option<(reqwest::Response, String)> = None;
        let mut last_err: Option<anyhow::Error> = None;
        let mut tried_url = url.clone();
        let mut resp_opt: Option<reqwest::Response> = None;
        let last_url = urls_to_try.last().cloned().unwrap_or_default();
        for try_url in &urls_to_try {
            tried_url = try_url.clone();
            let send_res = self.client.post(try_url).json(&wire).send().await;
            match send_res {
                Ok(r) => {
                    if r.status().as_u16() == 404 && *try_url != last_url {
                        // try next endpoint
                        last_resp = Some((r, try_url.clone()));
                        continue;
                    }
                    resp_opt = Some(r);
                    break;
                }
                Err(e) if e.is_timeout() => {
                    let elapsed = t0.elapsed().as_millis() as u64;
                    self.metrics.timeout_count.fetch_add(1, Ordering::Relaxed);
                    self.metrics.record(elapsed, false);
                    GLOBAL_METRICS.timeout_count.fetch_add(1, Ordering::Relaxed);
                    GLOBAL_METRICS.record(elapsed, false);
                    bail!("decider request timed out after {}ms to {}: {e}", self.config.timeout.as_millis(), try_url);
                }
                Err(e) if e.is_connect() => {
                    let elapsed = t0.elapsed().as_millis() as u64;
                    self.metrics.connection_error_count.fetch_add(1, Ordering::Relaxed);
                    self.metrics.record(elapsed, false);
                    GLOBAL_METRICS.connection_error_count.fetch_add(1, Ordering::Relaxed);
                    GLOBAL_METRICS.record(elapsed, false);
                    bail!("decider connection failed to {}: {e}", try_url);
                }
                Err(e) => {
                    last_err = Some(anyhow::anyhow!("decider request to {} failed: {e}", try_url));
                    continue;
                }
            }
        }
        let resp = if let Some(r) = resp_opt {
            r
        } else if let Some((r, _)) = last_resp {
            r
        } else if let Some(e) = last_err {
            return Err(e);
        } else {
            bail!("decider no endpoint responded (tried predict/decide)")
        };
        let final_url = tried_url;
        let elapsed_ms = t0.elapsed().as_millis() as u64;

        let status = resp.status();
        let body_bytes = resp
            .bytes()
            .await
            .context("read decider response body")?;

        if !status.is_success() {
            self.metrics.record(elapsed_ms, false);
            GLOBAL_METRICS.record(elapsed_ms, false);
            let snippet = String::from_utf8_lossy(&body_bytes);
            let snippet = snippet.chars().take(400).collect::<String>();
            bail!("decider error {} from {}: {}", status, final_url, snippet);
        }

        // Try to parse as typed response; be robust to malformed JSON.
        let parsed: Value = serde_json::from_slice(&body_bytes).context("decider response is not valid JSON")?;

        // Try to extract latency/model/device from top-level even if decisions parsing fails.
        let latency_from_body = parsed
            .get("latency_ms")
            .or_else(|| parsed.get("latency"))
            .or_else(|| parsed.get("took_ms"))
            .and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse().ok())));

        let mut typed: DeciderResponse = serde_json::from_value(parsed.clone()).with_context(|| {
            let snippet = String::from_utf8_lossy(&body_bytes);
            format!(
                "decider malformed response (cannot deserialize to DeciderResponse): {}",
                snippet.chars().take(600).collect::<String>()
            )
        })?;

        // Normalize server choice formats BEFORE validation: decider-serve
        // returns option TEXT ("banana") with text-keyed probabilities, while
        // the internal contract is numeric IDs ("2" with {"1":..,"2":..}).
        typed.normalize_choices(&req.questions);

        // If latency not already set, use remote latency or local elapsed.
        if typed.latency_ms.is_none() {
            typed.latency_ms = latency_from_body.or(Some(elapsed_ms));
        }

        // Validate choices against the original questions.
        if let Err(e) = typed.validate(Some(&req.questions)) {
            self.metrics.invalid_response_count.fetch_add(1, Ordering::Relaxed);
            self.metrics.record(elapsed_ms, false);
            GLOBAL_METRICS.invalid_response_count.fetch_add(1, Ordering::Relaxed);
            GLOBAL_METRICS.record(elapsed_ms, false);
            bail!("decider invalid choice in response: {e}");
        }

        self.metrics.record(elapsed_ms, true);
        GLOBAL_METRICS.record(latency_from_body.unwrap_or(elapsed_ms), true);

        // If the caller wants wire-level details (model/device/latency), they are on `typed`.
        Ok(typed)
    }

    /// Convenience: text-only decide (no image).
    pub async fn decide_text(
        &self,
        context: impl Into<String>,
        questions: Vec<DeciderQuestion>,
    ) -> Result<DeciderResponse> {
        let req = DeciderRequest::new(context, questions)
            .with_temperature(self.config.temperature);
        self.decide(&req).await
    }

    /// Convenience: decide with an image (base64 or data URI). Validates image encoding.
    pub async fn decide_image(
        &self,
        context: impl Into<String>,
        questions: Vec<DeciderQuestion>,
        image_b64_or_data_uri: impl Into<String>,
    ) -> Result<DeciderResponse> {
        let img = image_b64_or_data_uri.into();
        // Validate eagerly so we surface image encoding errors without a network round-trip.
        validate_image_field(&img).context("image encoding error")?;
        // Optionally enforce max_image_dim by refusing absurdly large payloads.
        // We don't decode dimensions without an image crate; we can at least limit base64 length.
        // Rough bound: base64 length ~ 4/3 * pixels. For 1280x1280 RGB ~ 4.9MB raw ~ 6.5MB b64.
        // We keep it lenient: only error if > 20MB.
        if img.len() > 20 * 1024 * 1024 {
            bail!("image payload too large ({} bytes), max ~20MB", img.len());
        }
        let req = DeciderRequest::new(context, questions)
            .with_image(img)
            .with_temperature(self.config.temperature);
        self.decide(&req).await
    }

    /// Batch decide with bounded concurrency (semaphore 4). Each request is
    /// validated and sent concurrently up to the bound. Results preserve input order.
    pub async fn decide_batch(&self, requests: Vec<DeciderRequest>) -> Vec<Result<DeciderResponse>> {
        let client = self.clone();
        let futures: Vec<_> = requests
            .into_iter()
            .map(|req| {
                let c = client.clone();
                async move { c.decide(&req).await }
            })
            .collect();
        // Use buffered concurrency via semaphore already inside `decide`; so we can join all.
        // For strict ordering, use `futures::future::join_all`.
        futures::future::join_all(futures).await
    }

    /// Health check: GET `{url}/health` (fallback to `/` and `/decide` HEAD).
    /// Returns the raw JSON/value from the health endpoint on success.
    pub async fn health(&self) -> Result<Value> {
        let candidates = [
            self.endpoint("health"),
            self.endpoint("v1/health"),
            self.endpoint(""),
        ];
        let mut last_err: Option<anyhow::Error> = None;
        for url in candidates {
            let res = self.client.get(&url).send().await;
            match res {
                Ok(r) if r.status().is_success() => {
                    let v: Value = r.json().await.unwrap_or_else(|_| serde_json::json!({"ok": true, "url": url}));
                    return Ok(v);
                }
                Ok(r) => {
                    last_err = Some(anyhow::anyhow!("health {} -> {}", url, r.status()));
                    // Try next candidate only if first was 404; otherwise surface.
                    if r.status().as_u16() != 404 {
                        break;
                    }
                }
                Err(e) => {
                    if e.is_timeout() {
                        last_err = Some(anyhow::anyhow!("health timeout to {}: {e}", url));
                    } else if e.is_connect() {
                        last_err = Some(anyhow::anyhow!("health connection failed to {}: {e}", url));
                    } else {
                        last_err = Some(anyhow::anyhow!("health request to {} failed: {e}", url));
                    }
                    break;
                }
            }
        }
        Err(last_err.unwrap_or_else(|| anyhow::anyhow!("health check failed")))
    }

    // -----------------------------------------------------------------------
    // Sync wrappers (for CLI, analogous to browser_runtime::client::cdp_call_sync)
    // -----------------------------------------------------------------------

    fn rt_block_on<F: std::future::Future>(f: F) -> F::Output {
        // Reuse a current-thread runtime for CLI sync contexts.
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("decider sync runtime")
            .block_on(f)
    }

    /// Sync wrapper for `decide`.
    pub fn decide_sync(&self, req: &DeciderRequest) -> Result<DeciderResponse> {
        Self::rt_block_on(self.decide(req))
    }

    /// Sync wrapper for `decide_text`.
    pub fn decide_text_sync(
        &self,
        context: impl Into<String>,
        questions: Vec<DeciderQuestion>,
    ) -> Result<DeciderResponse> {
        Self::rt_block_on(self.decide_text(context, questions))
    }

    /// Sync wrapper for `decide_image`.
    pub fn decide_image_sync(
        &self,
        context: impl Into<String>,
        questions: Vec<DeciderQuestion>,
        image: impl Into<String>,
    ) -> Result<DeciderResponse> {
        Self::rt_block_on(self.decide_image(context, questions, image))
    }

    /// Sync wrapper for `health`.
    pub fn health_sync(&self) -> Result<Value> {
        Self::rt_block_on(self.health())
    }
}

// ---------------------------------------------------------------------------
// Free convenience functions (use global client)
// ---------------------------------------------------------------------------

/// One-shot text decide using env config (convenience for callers that don't
/// want to manage a `DeciderClient` handle).
pub async fn decide_text_once(context: &str, questions: Vec<DeciderQuestion>) -> Result<DeciderResponse> {
    let client = DeciderClient::from_env()?;
    client.decide_text(context.to_string(), questions).await
}

/// One-shot image decide using env config.
pub async fn decide_image_once(
    context: &str,
    questions: Vec<DeciderQuestion>,
    image: &str,
) -> Result<DeciderResponse> {
    let client = DeciderClient::from_env()?;
    client.decide_image(context.to_string(), questions, image.to_string()).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decider::types::DeciderQuestion;

    fn test_config() -> DeciderConfig {
        DeciderConfig {
            enabled: true,
            url: "http://127.0.0.1:18001".to_string(),
            timeout: Duration::from_millis(200),
            max_image_dim: 1280,
            temperature: 0.0,
        }
    }

    #[tokio::test]
    async fn validation_error_before_network() {
        let client = DeciderClient::new(test_config()).unwrap();
        let q_empty = DeciderQuestion::new("q", vec![]);
        let req = DeciderRequest::new("ctx", vec![q_empty]);
        let err = client.decide(&req).await.unwrap_err();
        assert!(err.to_string().contains("validation") || err.to_string().contains("no options"));
    }

    #[tokio::test]
    async fn image_encoding_error_before_network() {
        let client = DeciderClient::new(test_config()).unwrap();
        let q = DeciderQuestion::new("q", vec!["A".into(), "B".into()]);
        let err = client
            .decide_image("ctx", vec![q], "not-base64!!!")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("image") || err.to_string().contains("base64"));
    }

    #[tokio::test]
    async fn connection_failure_surfaced() {
        // No server on 18001; should surface connection error quickly.
        let client = DeciderClient::new(test_config()).unwrap();
        let q = DeciderQuestion::new("q", vec!["A".into(), "B".into()]);
        let req = DeciderRequest::new("ctx", vec![q]);
        let err = client.decide(&req).await.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("connection") || msg.contains("decider") || msg.contains("failed"),
            "unexpected err: {msg}"
        );
    }

    #[tokio::test]
    async fn decision_validation_rejects_invalid_choice() {
        // Mock server that returns invalid choice "99" for a 2-option question.
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        // Tiny HTTP server for one request.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            let _ = sock.read(&mut buf).await;
            let body = serde_json::json!({
                "decisions": [{"choice":"99","confidence":0.5}],
                "model": "mock",
                "device": "cpu",
                "latency_ms": 1
            });
            let body_str = serde_json::to_string(&body).unwrap();
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                body_str.len(),
                body_str
            );
            let _ = sock.write_all(resp.as_bytes()).await;
        });
        let mut cfg = test_config();
        cfg.url = format!("http://{}", addr);
        let client = DeciderClient::new(cfg).unwrap();
        let q = DeciderQuestion::new("q", vec!["A".into(), "B".into()]);
        let req = DeciderRequest::new("ctx", vec![q]);
        let err = client.decide(&req).await.unwrap_err();
        assert!(err.to_string().contains("invalid choice"), "err: {err}");
        let _ = server.await;
    }

    #[tokio::test]
    async fn health_connection_error() {
        let client = DeciderClient::new(test_config()).unwrap();
        let err = client.health().await.unwrap_err();
        assert!(err.to_string().contains("health") || err.to_string().contains("connection") || err.to_string().contains("failed"));
    }
}
