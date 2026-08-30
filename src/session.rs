//! Screenshot session tracking — records every `hyprfast screenshot` path
//! and clears them after successful task completion.
//! List lives at `$XDG_RUNTIME_DIR/hyprfast-session.json` (fallback `/tmp`)
//! Format: JSON array of strings.

use anyhow::{Result, Context};
use std::path::PathBuf;
use std::sync::Mutex;
use std::collections::HashSet;

static LOCK: Mutex<()> = Mutex::new(());

fn session_path() -> PathBuf {
    let runtime = std::env::var("XDG_RUNTIME_DIR")
        .unwrap_or_else(|_| format!("/run/user/{}", nix::unistd::getuid()));
    PathBuf::from(runtime).join("hyprfast-session.json")
}

fn read_list() -> Vec<String> {
    let p = session_path();
    if !p.exists() { return Vec::new(); }
    let data = std::fs::read_to_string(&p).unwrap_or_default();
    serde_json::from_str::<Vec<String>>(&data).unwrap_or_default()
}

fn write_list(list: &[String]) -> Result<()> {
    let p = session_path();
    if let Some(parent) = p.parent() { let _ = std::fs::create_dir_all(parent); }
    let data = serde_json::to_string_pretty(list)?;
    std::fs::write(&p, data).with_context(|| format!("write {}", p.display()))?;
    Ok(())
}

/// Record a new screenshot path into the session list (deduped).
pub fn record(path: &str) -> Result<()> {
    let _g = LOCK.lock().unwrap();
    let mut list = read_list();
    if !list.contains(&path.to_string()) {
        list.push(path.to_string());
        write_list(&list)?;
    }
    Ok(())
}

pub fn list() -> Vec<String> { let _g = LOCK.lock().unwrap(); read_list() }

/// Clear tracked screenshots: delete each file if it exists, then truncate the session file.
/// Returns (removed count, failed count, bytes freed).
pub fn clear() -> Result<serde_json::Value> {
    let _g = LOCK.lock().unwrap();
    let list = read_list();
    let mut removed = 0usize;
    let mut failed = 0usize;
    let mut bytes: u64 = 0;
    let mut seen = HashSet::new();
    for path in &list {
        if !seen.insert(path.clone()) { continue; }
        if let Ok(meta) = std::fs::metadata(path) {
            bytes += meta.len();
            if std::fs::remove_file(path).is_ok() { removed += 1; } else { failed += 1; }
        } else {
            // already gone counts as removed
            removed += 1;
        }
    }
    // Include stale /tmp/hyprfast-*.png that escaped tracking (best-effort) if list was empty?
    // Only clear tracked by default; use clear_all for untracked.
    write_list(&[])?;
    Ok(serde_json::json!({
        "removed": removed,
        "failed": failed,
        "bytes_freed": bytes,
        "tracked_before": list.len()
    }))
}

/// Clear ALL /tmp/hyprfast-*.png (even untracked) — use with --all
pub fn clear_all() -> Result<serde_json::Value> {
    let tracked = clear()?;
    let mut extra = 0usize;
    let mut extra_bytes: u64 = 0;
    if let Ok(entries) = std::fs::read_dir("/tmp") {
        for e in entries.flatten() {
            let p = e.path();
            if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
                if name.starts_with("hyprfast-") && (name.ends_with(".png") || name.ends_with(".jpg") || name.ends_with(".jpeg")) {
                    if p.exists() {
                        if let Ok(m) = std::fs::metadata(&p) { extra_bytes += m.len(); }
                        if std::fs::remove_file(&p).is_ok() { extra += 1; }
                    }
                }
            }
        }
    }
    let mut v = tracked;
    if let Some(o) = v.as_object_mut() {
        o.insert("extra_removed".into(), serde_json::json!(extra));
        o.insert("extra_bytes".into(), serde_json::json!(extra_bytes));
    }
    Ok(v)
}

pub fn status() -> serde_json::Value {
    let list = read_list();
    let mut total_bytes: u64 = 0;
    let mut existing = 0usize;
    for p in &list {
        if let Ok(m) = std::fs::metadata(p) { total_bytes += m.len(); existing += 1; }
    }
    serde_json::json!({
        "session_file": session_path().to_string_lossy().to_string(),
        "tracked": list.len(),
        "existing": existing,
        "total_bytes": total_bytes,
        "files": list
    })
}
