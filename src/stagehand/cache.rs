//! Cache service — port of packages/extension/services/cacheService.ts
//! Stagehand caches act/observe/extract results keyed by instruction + xpathMap hash
//! Hyprfast uses file-based cache at ~/.cache/hyprfast/stagehand-cache.json

use anyhow::Result;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::PathBuf;

fn cache_path() -> PathBuf {
    let base = std::env::var("XDG_CACHE_HOME").unwrap_or_else(|_| format!("{}/.cache", std::env::var("HOME").unwrap_or_else(|_| "/tmp".into())));
    PathBuf::from(base).join("hyprfast").join("stagehand-cache.json")
}

fn load_cache() -> HashMap<String, Value> {
    let p = cache_path();
    if let Ok(s) = std::fs::read_to_string(&p) {
        if let Ok(m) = serde_json::from_str(&s) { return m; }
    }
    HashMap::new()
}
fn save_cache(m: &HashMap<String, Value>) -> Result<()> {
    let p = cache_path();
    if let Some(parent) = p.parent() { std::fs::create_dir_all(parent)?; }
    std::fs::write(p, serde_json::to_string_pretty(m)?)?;
    Ok(())
}

fn cache_key(method: &str, instruction: &str, url: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    format!("{}|{}|{}", method, instruction, url).hash(&mut h);
    format!("{}-{:x}", method, h.finish())
}

pub fn get_cached(method: &str, instruction: &str, url: &str) -> Option<Value> {
    let m = load_cache();
    m.get(&cache_key(method, instruction, url)).cloned()
}
pub fn set_cached(method: &str, instruction: &str, url: &str, value: Value) -> Result<()> {
    let mut m = load_cache();
    m.insert(cache_key(method, instruction, url), value);
    // keep last 500 entries
    if m.len()>500 {
        let keys: Vec<String> = m.keys().cloned().collect();
        for k in keys.iter().take(m.len()-500) { m.remove(k); }
    }
    save_cache(&m)
}
pub fn clear_cache() -> Result<Value> {
    let p = cache_path();
    let existed = p.exists();
    if existed { std::fs::remove_file(&p)?; }
    Ok(json!({"cleared": existed, "path": p.to_string_lossy().to_string()}))
}
pub fn cache_status() -> Value {
    let p = cache_path();
    let m = load_cache();
    json!({"path": p.to_string_lossy().to_string(), "entries": m.len(), "exists": p.exists()})
}
