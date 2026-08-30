//! Direct zbus AT-SPI — no busctl forks.
//! Mirrors hypruse/a11y.py but via persistent zbus::Connection.
//! Target <50ms for 400 nodes (vs ~800ms for busctl forks).

use anyhow::{Result, bail, Context};
use serde_json::Value;
use std::collections::HashSet;
use std::sync::OnceLock;
use zbus::{Connection, Proxy, zvariant::OwnedObjectPath};

const COORD_WINDOW: u32 = 1;
const STATE_ENABLED: usize = 8;
const STATE_SENSITIVE: usize = 24;
const STATE_SHOWING: usize = 25;
const STATE_VISIBLE: usize = 30;
const STATE_FOCUSED: usize = 12;
const STATE_CHECKED: usize = 4;
const STATE_PRESSED: usize = 20;

const ACTIONABLE_ROLES: &[i32] = &[7,8,11,33,35,37,40,43,44,45,51,52,62,79,88];
const CHECKABLE_ROLES: &[i32] = &[7,8,44,45,62];
const TEXT_ROLES: &[i32] = &[61,79];
const VALUE_ROLES: &[i32] = &[51,52];
const MAX_TEXT: i32 = 200;
const EXTENT_SANITY: i32 = 20000;

static ACTIONABLE_SET: OnceLock<HashSet<i32>> = OnceLock::new();
static CHECKABLE_SET: OnceLock<HashSet<i32>> = OnceLock::new();
static TEXT_SET: OnceLock<HashSet<i32>> = OnceLock::new();
static VALUE_SET: OnceLock<HashSet<i32>> = OnceLock::new();
static A11Y_ADDR_CACHE: OnceLock<String> = OnceLock::new();
static A11Y_CONN_CACHE: OnceLock<Connection> = OnceLock::new();
static GLOBAL_RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

fn global_rt() -> &'static tokio::runtime::Runtime {
    GLOBAL_RT.get_or_init(|| tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("rt"))
}
fn actionable_set() -> &'static HashSet<i32> { ACTIONABLE_SET.get_or_init(|| ACTIONABLE_ROLES.iter().cloned().collect()) }
fn checkable_set() -> &'static HashSet<i32> { CHECKABLE_SET.get_or_init(|| CHECKABLE_ROLES.iter().cloned().collect()) }
fn text_set() -> &'static HashSet<i32> { TEXT_SET.get_or_init(|| TEXT_ROLES.iter().cloned().collect()) }
fn value_set() -> &'static HashSet<i32> { VALUE_SET.get_or_init(|| VALUE_ROLES.iter().cloned().collect()) }

async fn a11y_bus_address() -> Result<String> {
    if let Some(cached) = A11Y_ADDR_CACHE.get() { return Ok(cached.clone()); }
    let conn = Connection::session().await.context("session bus")?;
    let proxy = Proxy::new(&conn, "org.a11y.Bus", "/org/a11y/bus", "org.a11y.Bus").await?;
    let addr: String = proxy.call("GetAddress", &()).await.context("GetAddress")?;
    if addr.is_empty() { bail!("a11y bus reported no address"); }
    let _ = A11Y_ADDR_CACHE.set(addr.clone());
    Ok(addr)
}

async fn a11y_connection() -> Result<Connection> {
    if let Some(conn) = A11Y_CONN_CACHE.get() { return Ok(conn.clone()); }
    let addr = a11y_bus_address().await?;
    let conn = zbus::connection::Builder::address(addr.as_str())?.build().await.context("a11y bus connect")?;
    let _ = A11Y_CONN_CACHE.set(conn.clone());
    Ok(conn)
}

async fn get_children(conn: &Connection, svc: &str, path: &str) -> Result<Vec<(String, String)>> {
    let proxy = Proxy::new(conn, svc, path, "org.a11y.atspi.Accessible").await?;
    let children: Vec<(String, OwnedObjectPath)> = proxy.call("GetChildren", &()).await.unwrap_or_default();
    Ok(children.into_iter().map(|(s,p)| (s, p.to_string())).collect())
}

async fn get_name(conn: &Connection, svc: &str, path: &str) -> String {
    let proxy = match Proxy::new(conn, svc, path, "org.a11y.atspi.Accessible").await {
        Ok(p) => p,
        Err(_) => return String::new(),
    };
    proxy.get_property::<String>("Name").await.unwrap_or_default()
}

async fn get_role(conn: &Connection, svc: &str, path: &str) -> i32 {
    let proxy = match Proxy::new(conn, svc, path, "org.a11y.atspi.Accessible").await {
        Ok(p) => p,
        Err(_) => return -1,
    };
    match proxy.call::<&str, (), u32>("GetRole", &()).await {
        Ok(r) => r as i32,
        Err(_) => -1,
    }
}

async fn get_role_name(conn: &Connection, svc: &str, path: &str) -> String {
    let proxy = match Proxy::new(conn, svc, path, "org.a11y.atspi.Accessible").await {
        Ok(p) => p,
        Err(_) => return String::new(),
    };
    let n: String = proxy.call("GetRoleName", &()).await.unwrap_or_default(); n
}

async fn get_state(conn: &Connection, svc: &str, path: &str) -> HashSet<usize> {
    let proxy = match Proxy::new(conn, svc, path, "org.a11y.atspi.Accessible").await {
        Ok(p) => p,
        Err(_) => return HashSet::new(),
    };
    let words: Vec<u32> = proxy.call("GetState", &()).await.unwrap_or_default();
    if words.is_empty() && false { return HashSet::new(); }
    let mut out = HashSet::new();
    for (wi, word) in words.iter().enumerate() {
        for bit in 0..32 {
            if (word >> bit) & 1 == 1 {
                out.insert(wi*32 + bit);
            }
        }
    }
    out
}

async fn get_interfaces(conn: &Connection, svc: &str, path: &str) -> HashSet<String> {
    let proxy = match Proxy::new(conn, svc, path, "org.a11y.atspi.Accessible").await {
        Ok(p) => p,
        Err(_) => return HashSet::new(),
    };
    let ifs: Vec<String> = proxy.call("GetInterfaces", &()).await.unwrap_or_default();
    if ifs.is_empty() { return HashSet::new(); }
    match Ok::<Vec<String>,()>(ifs) {
        Ok(v) => v.into_iter().map(|s| s.rsplit('.').next().unwrap_or(&s).to_string()).collect(),
        Err(_) => HashSet::new(),
    }
}

async fn get_extents(conn: &Connection, svc: &str, path: &str) -> Option<(i32,i32,i32,i32)> {
    let proxy = Proxy::new(conn, svc, path, "org.a11y.atspi.Component").await.ok()?;
    let res: (i32,i32,i32,i32) = proxy.call("GetExtents", &(COORD_WINDOW,)).await.ok()?;
    let (x,y,w,h) = res;
    if w <=0 || h <=0 { return None; }
    if !( -EXTENT_SANITY <= x && x <= EXTENT_SANITY && -EXTENT_SANITY <= y && y <= EXTENT_SANITY) { return None; }
    Some((x,y,w,h))
}

async fn conn_pid(conn: &Connection, svc: &str) -> Option<u32> {
    let proxy = Proxy::new(conn, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus").await.ok()?;
    let pid: u32 = proxy.call("GetConnectionUnixProcessID", &(svc,)).await.ok()?;
    Some(pid)
}

async fn apps(conn: &Connection) -> Result<Vec<(String,String)>> {
    get_children(conn, "org.a11y.atspi.Registry", "/org/a11y/atspi/accessible/root").await
}

async fn has_frame_named(conn: &Connection, svc: &str, path: &str, title: &str) -> bool {
    let kids = match get_children(conn, svc, path).await { Ok(v)=>v, Err(_)=>return false };
    for (cs, cp) in kids {
        if get_name(conn, &cs, &cp).await == title { return true; }
    }
    false
}

async fn app_for_pid(conn: &Connection, pid: i32, title: &str) -> Option<(String,String)> {
    let registered = apps(conn).await.ok()?;
    for (svc, path) in &registered {
        if let Some(p) = conn_pid(conn, svc).await { if p as i32 == pid { return Some((svc.clone(), path.clone())); } }
    }
    if !title.is_empty() {
        for (svc, path) in registered {
            if has_frame_named(conn, &svc, &path, title).await { return Some((svc, path)); }
        }
    }
    None
}

async fn window_frame(conn: &Connection, app_svc: &str, app_path: &str, title: &str, size: Option<(i32,i32)>) -> (String,String) {
    let frames = match get_children(conn, app_svc, app_path).await { Ok(v)=>v, Err(_)=>return (app_svc.to_string(), app_path.to_string()) };
    if frames.len() <=1 { return (app_svc.to_string(), app_path.to_string()); }
    if !title.is_empty() {
        for (fs, fp) in &frames {
            if get_name(conn, fs, fp).await == title { return (fs.clone(), fp.clone()); }
        }
    }
    if let Some((w,h)) = size {
        for (fs, fp) in &frames {
            if let Some((_,_,ew,eh)) = get_extents(conn, fs, fp).await { if ew==w && eh==h { return (fs.clone(), fp.clone()); } }
        }
    }
    (app_svc.to_string(), app_path.to_string())
}

async fn element_value(conn: &Connection, svc: &str, path: &str, role: i32, states: &HashSet<usize>) -> serde_json::Map<String, Value> {
    let mut out = serde_json::Map::new();
    let checkable = checkable_set();
    let text_roles = text_set();
    let value_roles = value_set();
    if checkable.contains(&role) {
        let checked = states.contains(&STATE_CHECKED) || states.contains(&STATE_PRESSED);
        out.insert("checked".into(), Value::Bool(checked));
        return out;
    }
    if text_roles.contains(&role) {
        let ifs = get_interfaces(conn, svc, path).await;
        if !ifs.contains("Text") { return out; }
        let proxy = match Proxy::new(conn, svc, path, "org.a11y.atspi.Text").await { Ok(p)=>p, Err(_)=>return out };
        let count: i32 = proxy.get_property::<i32>("CharacterCount").await.unwrap_or(0);
        if count <=0 { out.insert("value".into(), Value::String(String::new())); return out; }
        let end = std::cmp::min(count, MAX_TEXT);
        let txt: String = proxy.call("GetText", &(0, end)).await.unwrap_or_default();
        if !txt.is_empty() { let s = txt;
            let trunc = if count > end { "..."} else {""};
            out.insert("value".into(), Value::String(s + trunc));
        }
        return out;
    }
    if value_roles.contains(&role) {
        let ifs = get_interfaces(conn, svc, path).await;
        if !ifs.contains("Value") { return out; }
        let proxy = match Proxy::new(conn, svc, path, "org.a11y.atspi.Value").await { Ok(p)=>p, Err(_)=>return out };
        let cur: f64 = proxy.get_property::<f64>("CurrentValue").await.unwrap_or(0.0);
        let low: f64 = proxy.get_property::<f64>("MinimumValue").await.unwrap_or(0.0);
        let high: f64 = proxy.get_property::<f64>("MaximumValue").await.unwrap_or(0.0);
        out.insert("value".into(), Value::Number(serde_json::Number::from_f64(cur).unwrap_or(serde_json::Number::from(0))));
        if high > low {
            let pct = ((cur - low)/(high-low)*100.0).round() as i64;
            out.insert("percent".into(), Value::Number(pct.into()));
        }
        return out;
    }
    out
}

fn clickable_now(states: &HashSet<usize>) -> bool {
    if !states.contains(&STATE_SHOWING) || !states.contains(&STATE_VISIBLE) { return false; }
    states.contains(&STATE_SENSITIVE) || states.contains(&STATE_ENABLED)
}

async fn find_elements_inner(conn: &Connection, app_svc: &str, app_path: &str, name_filter: &str, actionable: bool) -> (Vec<Value>, bool) {
    let needle = name_filter.to_lowercase();
    let actionable_set = actionable_set();
    let value_bearing: HashSet<i32> = checkable_set().union(text_set()).cloned().collect::<HashSet<_>>().union(value_set()).cloned().collect();
    let mut results = Vec::new();
    let mut stack = vec![(app_svc.to_string(), app_path.to_string())];
    let mut visited = 0;
    let max_nodes = 400;
    let max_results = 60;
    let mut truncated = false;
    while let Some((svc, path)) = stack.pop() {
        if visited >= max_nodes || results.len() >= max_results { truncated = true; break; }
        visited += 1;
        let nm = get_name(conn, &svc, &path).await;
        let kids = get_children(conn, &svc, &path).await.unwrap_or_default();
        for (cs, cp) in kids.iter().rev() { stack.push((cs.clone(), cp.clone())); }
        if !needle.is_empty() && !nm.to_lowercase().contains(&needle) { continue; }
        let role = get_role(conn, &svc, &path).await;
        if actionable && !actionable_set.contains(&role) { continue; }
        if nm.is_empty() && needle.is_empty() && !value_bearing.contains(&role) { continue; }
        let ext = match get_extents(conn, &svc, &path).await { Some(e)=>e, None=> continue };
        let states = get_state(conn, &svc, &path).await;
        let mut item = serde_json::json!({
            "role": get_role_name(conn, &svc, &path).await,
            "name": nm,
            "extent": [ext.0, ext.1, ext.2, ext.3],
            "clickable": clickable_now(&states),
            "svc": svc,
            "path": path
        });
        let vals = element_value(conn, &svc, &path, role, &states).await;
        if let Value::Object(map) = item { let mut m=map; for (k,v) in vals { m.insert(k,v); } item=Value::Object(m); }
        results.push(item);
    }
    if visited >= max_nodes && !stack.is_empty() && results.len() < max_results { truncated = true; }
    (results, truncated)
}

pub async fn list_elements_zbus(window: &str, name: &str) -> Result<Value> {
    // Resolve window via hypr IPC
    let clients: Value = crate::hypr::query_json("clients")?;
    let active: Value = crate::hypr::query_json("activewindow")?;
    let target = if window.is_empty() || window=="active" { active.get("address").and_then(|v| v.as_str()).unwrap_or("").to_string() } else { window.to_string() };
    if target.is_empty() { bail!("no active window"); }
    let client = clients.as_array().and_then(|a| a.iter().find(|c| c.get("address").and_then(|v| v.as_str())==Some(target.as_str()))).ok_or_else(|| anyhow::anyhow!("window {} not found", target))?;
    let pid = client.get("pid").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
    let title = client.get("title").and_then(|v| v.as_str()).unwrap_or("");
    let cls = client.get("class").and_then(|v| v.as_str()).unwrap_or("");
    let size = client.get("size").and_then(|v| v.as_array()).and_then(|a| Some((a[0].as_i64()? as i32, a[1].as_i64()? as i32)));
    let at = client.get("at").and_then(|v| v.as_array()).map(|a| (a[0].as_i64().unwrap_or(0) as i32, a[1].as_i64().unwrap_or(0) as i32)).unwrap_or((0,0));
    let aw = client.get("size").and_then(|v| v.as_array()).map(|a| (a[0].as_i64().unwrap_or(0) as i32, a[1].as_i64().unwrap_or(0) as i32)).unwrap_or((0,0));

    let conn = a11y_connection().await?;
    let app = app_for_pid(&conn, pid, title).await.ok_or_else(|| anyhow::anyhow!("{} exposes no accessibility tree; use screenshot", cls))?;
    let frame = window_frame(&conn, &app.0, &app.1, title, size).await;
    let (els, truncated) = find_elements_inner(&conn, &frame.0, &frame.1, name, true).await;
    // Map window-relative extents to global
    let mut out = Vec::new();
    for e in els {
        let ext = e.get("extent").and_then(|v| v.as_array()).unwrap();
        let ex = ext[0].as_i64().unwrap() as i32;
        let ey = ext[1].as_i64().unwrap() as i32;
        let ew = ext[2].as_i64().unwrap() as i32;
        let eh = ext[3].as_i64().unwrap() as i32;
        let x = at.0 + ex + ew/2;
        let y = at.1 + ey + eh/2;
        if !(at.0 <= x && x < at.0+aw.0 && at.1 <= y && y < at.1+aw.1) { continue; }
        let mut item = serde_json::json!({
            "role": e.get("role").cloned().unwrap_or(Value::String("".into())),
            "name": e.get("name").cloned().unwrap_or(Value::String("".into())),
            "x": x, "y": y,
            "clickable": e.get("clickable").cloned().unwrap_or(Value::Bool(false)),
        });
        for k in ["value","percent","checked"] {
            if let Some(v)=e.get(k) { item[k]=v.clone(); }
        }
        out.push(item);
    }
    if out.is_empty() {
        let what = if name.is_empty() { "actionable".to_string() } else { format!("matching {:?}", name) };
        let tail = if truncated { " (truncated)"} else {""};
        return Ok(serde_json::json!({"note": format!("no {} elements in {}{}", what, cls, tail), "elements": []}));
    }
    Ok(serde_json::json!({"elements": out, "truncated": truncated}))
}

pub async fn click_by_name_zbus(window: &str, name: &str) -> Result<Value> {
    // Single walk: list then DoAction on same elements, no second walk
    let clients: Value = crate::hypr::query_json("clients")?;
    let active: Value = crate::hypr::query_json("activewindow")?;
    let target_addr = if window.is_empty() || window=="active" { active.get("address").and_then(|v| v.as_str()).unwrap_or("").to_string() } else { window.to_string() };
    if target_addr.is_empty() { bail!("no active window"); }
    let client = clients.as_array().and_then(|a| a.iter().find(|c| c.get("address").and_then(|v| v.as_str())==Some(target_addr.as_str()))).ok_or_else(|| anyhow::anyhow!("window {} not found", target_addr))?.clone();
    let pid = client.get("pid").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
    let title = client.get("title").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let cls = client.get("class").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let size = client.get("size").and_then(|v| v.as_array()).and_then(|a| Some((a[0].as_i64()? as i32, a[1].as_i64()? as i32)));
    let at = client.get("at").and_then(|v| v.as_array()).map(|a| (a[0].as_i64().unwrap_or(0) as i32, a[1].as_i64().unwrap_or(0) as i32)).unwrap_or((0,0));
    let aw = client.get("size").and_then(|v| v.as_array()).map(|a| (a[0].as_i64().unwrap_or(0) as i32, a[1].as_i64().unwrap_or(0) as i32)).unwrap_or((0,0));
    let conn = a11y_connection().await?;
    let app = app_for_pid(&conn, pid, &title).await.ok_or_else(|| anyhow::anyhow!("{} exposes no accessibility tree; use screenshot", cls))?;
    let frame = window_frame(&conn, &app.0, &app.1, &title, size).await;
    let (raw_els, truncated) = find_elements_inner(&conn, &frame.0, &frame.1, name, true).await;
    // Map to global coords and keep svc/path for DoAction
    let mut mapped: Vec<(Value,String,String,i64,i64)> = Vec::new();
    for e in raw_els {
        let ext = match e.get("extent").and_then(|v| v.as_array()) { Some(a)=>a, None=>continue };
        let ex = ext[0].as_i64().unwrap() as i32;
        let ey = ext[1].as_i64().unwrap() as i32;
        let ew = ext[2].as_i64().unwrap() as i32;
        let eh = ext[3].as_i64().unwrap() as i32;
        let x = at.0 + ex + ew/2;
        let y = at.1 + ey + eh/2;
        if !(at.0 <= x && x < at.0+aw.0 && at.1 <= y && y < at.1+aw.1) { continue; }
        let svc = e.get("svc").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let path = e.get("path").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let mut item = serde_json::json!({
            "role": e.get("role").cloned().unwrap_or(Value::String("".into())),
            "name": e.get("name").cloned().unwrap_or(Value::String("".into())),
            "x": x, "y": y,
            "clickable": e.get("clickable").cloned().unwrap_or(Value::Bool(false)),
        });
        for k in ["value","percent","checked"] { if let Some(v)=e.get(k) { item[k]=v.clone(); } }
        mapped.push((item, svc, path, x as i64, y as i64));
    }
    if mapped.is_empty() {
        let what = if name.is_empty() { "actionable".to_string() } else { format!("matching {:?}", name) };
        let tail = if truncated { " (truncated)"} else {""};
        bail!("no {} elements in {}{}", what, cls, tail);
    }
    // Ambiguity check: exact name first
    let exact: Vec<usize> = mapped.iter().enumerate().filter(|(_, (item,_,_,_,_))| item.get("name").and_then(|v| v.as_str()).map(|s| s.to_lowercase()==name.to_lowercase()).unwrap_or(false)).map(|(i,_)| i).collect();
    let pool: Vec<usize> = if !exact.is_empty() { exact } else { (0..mapped.len()).collect() };
    if pool.len()>1 {
        let candidates: Vec<Value> = pool.iter().map(|i| mapped[*i].0.clone()).collect();
        return Ok(serde_json::json!({"ambiguous": true, "candidates": candidates, "hint": "use more specific name"}));
    }
    let idx = pool[0];
    let (_target_item, svc, path, x, y) = (&mapped[idx].0, &mapped[idx].1, &mapped[idx].2, mapped[idx].3 as f64, mapped[idx].4 as f64);
    // Focus window via direct dispatch (no hyprctl fork)
    let _ = crate::hypr::dispatch("focuswindow", &format!("address:{}", target_addr));
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    if !svc.is_empty() && !path.is_empty() {
        if let Ok(proxy) = Proxy::new(&conn, svc.as_str(), path.as_str(), "org.a11y.atspi.Action").await {
            let ok: bool = proxy.call("DoAction", &(0,)).await.unwrap_or(false);
            if ok { return Ok(serde_json::json!({"clicked": true, "x": x, "y": y, "via": "DoAction"})); }
        }
    }
    crate::input::click(Some(x), Some(y), "left", false)?;
    Ok(serde_json::json!({"clicked": true, "x": x, "y": y, "via": "pointer"}))
}

// Sync wrappers for non-async callers — reuse global runtime (<1ms vs 3ms)
pub fn list_elements_zbus_sync(window: &str, name: &str) -> Result<Value> {
    global_rt().block_on(list_elements_zbus(window, name))
}
pub fn click_by_name_zbus_sync(window: &str, name: &str) -> Result<Value> {
    global_rt().block_on(click_by_name_zbus(window, name))
}
