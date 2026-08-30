use anyhow::{Result, Context};
use std::os::unix::net::UnixStream;
use std::io::{BufRead, BufReader};
use std::time::{Duration, Instant};
use serde_json::Value;

fn socket2_path() -> Result<std::path::PathBuf> {
    let runtime = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| format!("/run/user/{}", nix::unistd::getuid()));
    let sig = std::env::var("HYPRLAND_INSTANCE_SIGNATURE").context("HYPRLAND_INSTANCE_SIGNATURE not set")?;
    Ok(std::path::PathBuf::from(runtime).join("hypr").join(sig).join(".socket2.sock"))
}

fn parse_event(line: &str) -> Option<(String, Value)> {
    let (name, data) = line.split_once(">>")?;
    let val = match name {
        "openwindow" => { let parts: Vec<&str>=data.splitn(4, ',').collect(); serde_json::json!({"address": format!("0x{}",parts.get(0).unwrap_or(&"")), "workspace": parts.get(1).unwrap_or(&""), "class": parts.get(2).unwrap_or(&""), "title": parts.get(3).unwrap_or(&"")}) },
        "closewindow" => serde_json::json!({"address": format!("0x{}", data)}),
        "movewindow" => { let p:Vec<&str>=data.splitn(2,',').collect(); serde_json::json!({"address": format!("0x{}",p.get(0).unwrap_or(&"")), "workspace": p.get(1).unwrap_or(&"")}) },
        "workspace" => serde_json::json!({"name": data}),
        "windowtitlev2" => { let p:Vec<&str>=data.splitn(2,',').collect(); serde_json::json!({"address": format!("0x{}",p.get(0).unwrap_or(&"")), "title": p.get(1).unwrap_or(&"")}) },
        "openlayer" => serde_json::json!({"namespace": data}),
        "closelayer" => serde_json::json!({"namespace": data}),
        "urgent" => serde_json::json!({"address": format!("0x{}", data)}),
        "screencast" => { let p:Vec<&str>=data.splitn(2,',').collect(); serde_json::json!({"state": p.get(0).unwrap_or(&""), "owner": p.get(1).unwrap_or(&"")}) },
        _ => serde_json::json!({"data": data}),
    };
    Some((name.to_string(), val))
}

pub fn wait_for_direct(event: &str, match_str: &str, timeout_s: f64) -> Result<Value> {
    wait_for_inner(event, match_str, timeout_s)
}
pub fn wait_for(event: &str, match_str: &str, timeout_s: f64) -> Result<Value> {
    if crate::daemon::is_running() {
        if let Ok(v) = crate::daemon::client_wait_for(event, match_str, timeout_s) { return Ok(v); }
    }
    wait_for_inner(event, match_str, timeout_s)
}
fn wait_for_inner(event: &str, match_str: &str, timeout_s: f64) -> Result<Value> {
    let need: std::collections::HashSet<String> = match event {
        "window_open" => ["openwindow".into()].into(),
        "window_close" => ["closewindow".into()].into(),
        "workspace" => ["workspace".into()].into(),
        "title_change" => ["windowtitlev2".into()].into(),
        "layer_open" => ["openlayer".into()].into(),
        "layer_close" => ["closelayer".into()].into(),
        "urgent" => ["urgent".into()].into(),
        "screencast" => ["screencast".into()].into(),
        _ => anyhow::bail!("unknown event {}", event),
    };
    let needle = match_str.to_lowercase();
    let path = socket2_path()?;
    let stream = UnixStream::connect(&path).with_context(|| format!("connect {}", path.display()))?;
    stream.set_read_timeout(Some(Duration::from_millis(200)))?;
    let mut reader = BufReader::new(stream);
    let deadline = Instant::now() + Duration::from_secs_f64(timeout_s);
    let mut buf = String::new();
    while Instant::now() < deadline {
        buf.clear();
        match reader.read_line(&mut buf) {
            Ok(0) => break,
            Ok(_) => {
                let line = buf.trim();
                if line.is_empty() { continue; }
                if let Some((name, payload)) = parse_event(line) {
                    if !need.contains(&name) { continue; }
                    if !needle.is_empty() {
                        let hay = payload.to_string().to_lowercase();
                        if !hay.contains(&needle) { continue; }
                    }
                    return Ok(serde_json::json!({"event": name, "payload": payload}));
                }
            },
            Err(e) if e.kind()==std::io::ErrorKind::WouldBlock||e.kind()==std::io::ErrorKind::TimedOut => continue,
            Err(e) => anyhow::bail!("socket read {}", e),
        }
    }
    Ok(serde_json::json!({"timeout": true, "event": event}))
}
