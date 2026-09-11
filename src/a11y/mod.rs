//! AT-SPI: zbus direct (fast) + busctl fallback
mod zbus_impl;
use anyhow::{Result, bail};
use serde_json::Value;
use std::process::Command;

/// List elements for window address (or active if empty) via zbus direct, fallback to python
pub fn list_elements(window: &str, name: &str) -> Result<Value> {
    // Try zbus fast path first (<50ms)
    if let Ok(v) = zbus_impl::list_elements_zbus_sync(window, name) {
        return Ok(v);
    }
    // Fallback to python busctl bridge (for envs where a11y bus not yet ready)
    let window_arg = window.to_string();
    let name_arg = name.to_string();
    let code = format!(r#"
import json, sys
from hypruse import a11y, hyprctl
try:
    clients = hyprctl.query("clients")
    target = "{window_arg}"
    if not target or target=="active":
        target = (hyprctl.query("activewindow") or {{}}).get("address","")
    if not target:
        print(json.dumps({{"error":"no active window"}})); sys.exit(0)
    client = next((c for c in clients if c.get("address")==target), None)
    if client is None:
        print(json.dumps({{"error": "window not found"}})); sys.exit(0)
    pid = client.get("pid",0)
    title = client.get("title","")
    cls = client.get("class","")
    bus = a11y.connect()
    app = a11y.app_for_pid(bus, pid, title)
    if app is None:
        print(json.dumps({{"error": f"{{cls}} exposes no accessibility tree; use screenshot"}})); sys.exit(0)
    frame = a11y.window_frame(bus, app[0], app[1], title, tuple(client.get("size",[0,0])))
    els, truncated = a11y.find_elements(bus, frame[0], frame[1], name="{name_arg}", actionable=True)
    ax, ay = client["at"]
    aw, ah = client["size"]
    out=[]
    for e in els:
        ex,ey,ew,eh = e["extent"]
        x,y = ax+ex+ew//2, ay+ey+eh//2
        if not (ax <= x < ax+aw and ay <= y < ay+ah):
            continue
        item={{"role":e["role"],"name":e["name"],"x":x,"y":y,"clickable":e["clickable"]}}
        for k in ("value","percent","checked"):
            if k in e: item[k]=e[k]
        out.append(item)
    if not out:
        what = "matching '{name_arg}'" if "{name_arg}" else "actionable"
        tail=" (truncated)" if truncated else ""
        print(json.dumps({{"note": f"no {{what}} elements in {{cls}}{{tail}}", "elements":[]}})); sys.exit(0)
    print(json.dumps({{"elements": out, "truncated": truncated}}))
except Exception as e:
    import traceback
    print(json.dumps({{"error": str(e), "trace": traceback.format_exc()}}))
"#, window_arg=window_arg.replace('"',r#"\""#), name_arg=name_arg.replace('"',r#"\""#));
    let out = Command::new("python3").args(["-c", &code]).output()?;
    if !out.status.success() { bail!("a11y failed: {}", String::from_utf8_lossy(&out.stderr)); }
    let txt = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if txt.is_empty() { return Ok(Value::Array(vec![])); }
    let v: Value = serde_json::from_str(&txt)?;
    if let Some(err)=v.get("error").and_then(|e| e.as_str()) { bail!("{}", err); }
    Ok(v)
}

pub fn click_by_name(window: &str, name: &str) -> Result<Value> {
    if let Ok(mut v) = zbus_impl::click_by_name_zbus_sync(window, name) {
        // Ensure tier is visible for metrics distribution
        if v.is_object() {
            if let Some(obj) = v.as_object_mut() {
                obj.insert("tier".into(), Value::String("a11y".into()));
            }
        }
        crate::stagehand::instrumentation::METRICS.record_tier("a11y");
        return Ok(v);
    }
    let els = list_elements(window, name)?;
    let arr = els.get("elements").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    if arr.is_empty() {
        // 1. a11y found nothing — 2. try hint-key middle tier before falling back to vision pointer
        let hint_attempt = (|| -> Result<Value> {
            let cfg = crate::stagehand::StagehandConfig::from_env();
            let (res, label) = crate::hint::try_hint_tier(name, "click", "", &cfg)?;
            crate::stagehand::instrumentation::METRICS.record_tier("hint");
            Ok(serde_json::json!({"clicked": true, "via": "hint", "label": label, "tier": "hint", "result": res}))
        })();
        if let Ok(v) = hint_attempt {
            return Ok(v);
        }
        if let Some(note)=els.get("note").and_then(|v| v.as_str()) { bail!("{}", note); }
        bail!("no elements matching {:?}", name);
    }
    // Prefer exact match
    let exact: Vec<_> = arr.iter().filter(|e| e.get("name").and_then(|v| v.as_str()).map(|s| s.to_lowercase()==name.to_lowercase()).unwrap_or(false)).collect();
    let pool = if !exact.is_empty() { exact } else { arr.iter().collect::<Vec<_>>() };
    if pool.len()>1 {
        return Ok(serde_json::json!({"ambiguous": true, "candidates": pool, "hint": "use more specific name or window"}));
    }
    let target = pool[0];
    let x = target.get("x").and_then(|v| v.as_i64()).unwrap_or(0) as f64;
    let y = target.get("y").and_then(|v| v.as_i64()).unwrap_or(0) as f64;
    let win = window.to_string();
    let code = format!(r#"
import json
from hypruse import a11y, hyprctl
from hypruse import input as hinput
clients = hyprctl.query("clients")
target="{win}"
if not target or target=="active":
    target=(hyprctl.query("activewindow") or {{}}).get("address","")
c=next((x for x in clients if x.get("address")==target), None)
if c: hyprctl.dispatch("focuswindow", f"address:{{target}}")
import time; time.sleep(0.05)
from hypruse import a11y as A
bus=A.connect()
app=A.app_for_pid(bus, c.get("pid"), c.get("title","")) if c else None
clicked=False
if app:
    frame=A.window_frame(bus, app[0], app[1], c.get("title",""), tuple(c.get("size",[0,0])) if c else None)
    els,_=A.find_elements(bus, frame[0], frame[1], name={name_py}, actionable=True)
    if els:
        exact=[e for e in els if e["name"].lower()=={name_py}.lower()]
        pool=exact or els
        if len(pool)==1:
            clicked=A.do_action(bus, pool[0]["svc"], pool[0]["path"], 0)
if not clicked:
    hinput.click({x}, {y})
print(json.dumps({{"clicked": True, "x": {x}, "y": {y}, "via": "DoAction" if clicked else "pointer"}}))
"#, win=win.replace('"',r#"\""#), name_py=format!("{:?}", name), x=x, y=y);
    let out = Command::new("python3").args(["-c", &code]).output()?;
    if !out.status.success() { bail!("click failed: {}", String::from_utf8_lossy(&out.stderr)); }
    let mut v: Value = serde_json::from_str(&String::from_utf8_lossy(&out.stdout))?;
    let via = v.get("via").and_then(|x| x.as_str()).unwrap_or("");
    if via == "DoAction" {
        crate::stagehand::instrumentation::METRICS.record_tier("a11y");
        if v.is_object() {
            if let Some(obj) = v.as_object_mut() { obj.insert("tier".into(), Value::String("a11y".into())); }
        }
        return Ok(v);
    }
    // DoAction failed -> fell back to pointer (2nd tier would be hint before vision)
    // Try hint middle tier before returning pointer vision fallback
    let hint_attempt = (|| -> Result<Value> {
        let cfg = crate::stagehand::StagehandConfig::from_env();
        let (res, label) = crate::hint::try_hint_tier(name, "click", "", &cfg)?;
        crate::stagehand::instrumentation::METRICS.record_tier("hint");
        Ok(serde_json::json!({"clicked": true, "via": "hint", "label": label, "tier": "hint", "result": res, "fallback_from": v}))
    })();
    if let Ok(hv) = hint_attempt {
        return Ok(hv);
    }
    // hint also failed -> pointer was already the vision-ish fallback; record as vision for distribution
    crate::stagehand::instrumentation::METRICS.record_tier("vision");
    if v.is_object() {
        if let Some(obj) = v.as_object_mut() { obj.insert("tier".into(), Value::String("vision".into())); }
    }
    Ok(v)
}
