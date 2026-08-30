//! hyprfastd — persistent daemon with socket2 + snapshot cache
//! hypruse does per-call hyprctl forks and per-call socket2 connects.
//! hyprfastd keeps one socket2 connection and one snapshot cache, so
//! `desktop` is <1ms (no IPC) and `wait_for` is instant (already subscribed).

use anyhow::{Result, Context};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use tokio::net::{UnixListener, UnixStream};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::broadcast;

pub fn daemon_path() -> PathBuf {
    let r = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| format!("/run/user/{}", nix::unistd::getuid()));
    PathBuf::from(r).join("hyprfastd.sock")
}

fn hypr_socket2_path() -> Result<PathBuf> {
    let r = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| format!("/run/user/{}", nix::unistd::getuid()));
    let sig = std::env::var("HYPRLAND_INSTANCE_SIGNATURE").context("HYPRLAND_INSTANCE_SIGNATURE")?;
    Ok(PathBuf::from(r).join("hypr").join(sig).join(".socket2.sock"))
}

type SharedCache = Arc<RwLock<Value>>;
type EventTx = broadcast::Sender<String>;

async fn snapshot_direct() -> Value {
    // Call raw snapshot without daemon recursion
    tokio::task::spawn_blocking(|| crate::hypr::snapshot_raw().unwrap_or(json!({}))).await.unwrap_or(json!({}))
}

async fn event_loop(cache: SharedCache, tx: EventTx) {
    let path = match hypr_socket2_path() { Ok(p)=>p, Err(_)=>return };
    // Reconnect loop
    loop {
        let stream = match UnixStream::connect(&path).await {
            Ok(s)=>s, Err(_)=>{ tokio::time::sleep(std::time::Duration::from_secs(1)).await; continue; }
        };
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line).await {
                Ok(0)=>break, // eof, reconnect
                Ok(_)=>{
                    let trimmed = line.trim().to_string();
                    if trimmed.is_empty() { continue; }
                    // broadcast raw line
                    let _ = tx.send(trimmed.clone());
                    // Update cache on structural events
                    let is_structural = trimmed.starts_with("openwindow>>") || trimmed.starts_with("closewindow>>") || trimmed.starts_with("movewindow>>") || trimmed.starts_with("workspace>>") || trimmed.starts_with("openlayer>>") || trimmed.starts_with("closelayer>>");
                    if is_structural {
                        let snap = snapshot_direct().await;
                        *cache.write().unwrap() = snap;
                    }
                },
                Err(_)=>break,
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
}

async fn handle_client(mut stream: UnixStream, cache: SharedCache, tx: EventTx) -> Result<()> {
    let (r, mut w) = stream.split();
    let mut reader = BufReader::new(r);
    let mut line = String::new();
    reader.read_line(&mut line).await?;
    let req: Value = serde_json::from_str(line.trim()).unwrap_or(json!({}));
    let method = req.get("method").and_then(|v| v.as_str()).unwrap_or("");
    let resp = match method {
        "desktop" => {
            let snap = cache.read().map(|v| v.clone()).unwrap_or(json!({}));
            // if cache empty, fetch
            let snap = if snap.is_null() || snap.as_object().map(|o| o.is_empty()).unwrap_or(true) { snapshot_direct().await } else { snap };
            json!({"result": snap})
        },
        "wait_for" => {
            let event = req.get("event").and_then(|v| v.as_str()).unwrap_or("window_open");
            let m = req.get("match").and_then(|v| v.as_str()).unwrap_or("");
            let timeout = req.get("timeout").and_then(|v| v.as_f64()).unwrap_or(5.0);
            // Use broadcast with timeout
            let mut rx = tx.subscribe();
            let needle = m.to_lowercase();
            let want: std::collections::HashSet<String> = match event {
                "window_open" => ["openwindow".into()].into(),
                "window_close" => ["closewindow".into()].into(),
                "workspace" => ["workspace".into()].into(),
                "title_change" => ["windowtitlev2".into()].into(),
                "layer_open" => ["openlayer".into()].into(),
                "layer_close" => ["closelayer".into()].into(),
                _ => [event.to_string()].into(),
            };
            let start = tokio::time::Instant::now();
            let timeout_dur = std::time::Duration::from_secs_f64(timeout);
            let mut result = json!({"timeout": true});
            loop {
                let elapsed = start.elapsed();
                if elapsed >= timeout_dur { break; }
                let remain = timeout_dur - elapsed;
                match tokio::time::timeout(remain, rx.recv()).await {
                    Ok(Ok(line)) => {
                        if let Some((name,_)) = line.split_once(">>") {
                            if !want.contains(name) { continue; }
                            if !needle.is_empty() && !line.to_lowercase().contains(&needle) { continue; }
                            result = json!({"event": name, "line": line});
                            break;
                        }
                    },
                    Ok(Err(_)) => break,
                    Err(_) => break, // timeout
                }
            }
            json!({"result": result})
        },
        _ => json!({"error": "unknown method"}),
    };
    let out = serde_json::to_string(&resp)? + "\n";
    w.write_all(out.as_bytes()).await?;
    w.flush().await?;
    Ok(())
}

pub async fn run_daemon() -> Result<()> {
    let path = daemon_path();
    // Remove stale socket
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path)?;
    let cache: SharedCache = Arc::new(RwLock::new(snapshot_direct().await));
    let (tx, _rx) = broadcast::channel::<String>(100);
    // Spawn event loop
    let cache2 = cache.clone();
    let tx2 = tx.clone();
    tokio::spawn(event_loop(cache2, tx2));
    eprintln!("hyprfastd listening on {}", path.display());
    loop {
        let (stream, _) = listener.accept().await?;
        let c = cache.clone();
        let t = tx.clone();
        tokio::spawn(async move { let _ = handle_client(stream, c, t).await; });
    }
}

// Client helpers — try daemon first, fallback to direct
pub fn client_snapshot() -> Result<Value> {
    let path = daemon_path();
    if !path.exists() { return crate::hypr::snapshot(); }
    // Sync client via blocking
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    rt.block_on(async {
        let mut stream = UnixStream::connect(&path).await.context("connect daemon")?;
        let req = json!({"method":"desktop"});
        stream.write_all((serde_json::to_string(&req)? + "\n").as_bytes()).await?;
        stream.flush().await?;
        let mut reader = BufReader::new(&mut stream);
        let mut line = String::new();
        reader.read_line(&mut line).await?;
        let resp: Value = serde_json::from_str(&line)?;
        resp.get("result").cloned().ok_or_else(|| anyhow::anyhow!("no result"))
    })
}

pub fn client_wait_for(event: &str, m: &str, timeout: f64) -> Result<Value> {
    let path = daemon_path();
    if !path.exists() { return crate::events::wait_for_direct(event, m, timeout); }
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    rt.block_on(async {
        let mut stream = UnixStream::connect(&path).await.context("connect daemon")?;
        let req = json!({"method":"wait_for","event":event,"match":m,"timeout":timeout});
        stream.write_all((serde_json::to_string(&req)? + "\n").as_bytes()).await?;
        let mut reader = BufReader::new(&mut stream);
        let mut line = String::new();
        // Wait with timeout + buffer
        let read_fut = reader.read_line(&mut line);
        let res = tokio::time::timeout(std::time::Duration::from_secs_f64(timeout+1.0), read_fut).await;
        let n = res.context("daemon timeout")?.context("read")?;
        if n==0 { anyhow::bail!("daemon closed"); }
        let resp: Value = serde_json::from_str(&line)?;
        resp.get("result").cloned().ok_or_else(|| anyhow::anyhow!("no result"))
    })
}

pub fn is_running() -> bool {
    daemon_path().exists()
}
