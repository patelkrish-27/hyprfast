//! Screenshots via grim (wlroots screencopy)
//! Mirrors hypruse screenshot.py but without Python overhead.
//! Captures via `grim -g "x,y WxH" -` and returns (bytes, meta).

use anyhow::{Result, bail};
use std::process::Command;
use serde_json::Value;

pub fn capture(window: &str, region: &str, scale: f64) -> Result<(Vec<u8>, Value)> {
    // Resolve target geometry via hypr query (like hypruse)
    let mut args: Vec<String> = Vec::new();
    let mut meta = serde_json::json!({"coords": "global = geometry[:2] + image_pixel / scale"});
    let mut geom = [0,0,0,0];
    let mut base_scale = 1.0;

    if !window.is_empty() && !region.is_empty() { bail!("pass window OR region, not both"); }
    if scale != 0.0 && !(0.1..=1.0).contains(&scale) { bail!("scale out of range 0.1-1.0"); }

    if !region.is_empty() {
        // parse x,y,WxH
        let (x,y,w,h) = parse_region(region)?;
        geom = [x,y,w,h];
        args = vec!["-g".into(), format!("{x},{y} {w}x{h}")];
        meta["target"] = Value::String("region".into());
        meta["geometry"] = serde_json::json!([x,y,w,h]);
        // scale for rect: max scale among monitors intersecting rect
        base_scale = scale_for_rect(x,y,w,h)?;
    } else if !window.is_empty() {
        let addr = if window=="active" {
            crate::hypr::query_json("activewindow")?.get("address").and_then(|v| v.as_str()).map(|s| s.to_string()).unwrap_or_default()
        } else { window.to_string() };
        if addr.is_empty() { bail!("no active window; pass address from desktop()"); }
        let clients = crate::hypr::query_json("clients")?;
        let c = clients.as_array().and_then(|a| a.iter().find(|c| c.get("address").and_then(|v| v.as_str())==Some(&addr))).ok_or_else(|| anyhow::anyhow!("window {} not found", addr))?;
        let at = c.get("at").and_then(|v| v.as_array()).ok_or_else(|| anyhow::anyhow!("no at"))?;
        let sz = c.get("size").and_then(|v| v.as_array()).ok_or_else(|| anyhow::anyhow!("no size"))?;
        let x = at[0].as_i64().unwrap_or(0) as i32;
        let y = at[1].as_i64().unwrap_or(0) as i32;
        let w = sz[0].as_i64().unwrap_or(0) as i32;
        let h = sz[1].as_i64().unwrap_or(0) as i32;
        geom = [x,y,w,h];
        args = vec!["-g".into(), format!("{x},{y} {w}x{h}")];
        meta["target"] = Value::String("window".into());
        meta["window"] = Value::String(addr);
        meta["class"] = c.get("class").cloned().unwrap_or(Value::String("".into()));
        meta["geometry"] = serde_json::json!([x,y,w,h]);
        base_scale = scale_for_rect(x,y,w,h)?;
    } else {
        // focused monitor
        let mons = crate::hypr::query_json("monitors")?;
        let mon = mons.as_array().and_then(|a| a.iter().find(|m| m.get("focused").and_then(|v| v.as_bool())==Some(true))).or_else(|| mons.as_array().and_then(|a| a.first())).ok_or_else(|| anyhow::anyhow!("no monitors"))?;
        let name = mon.get("name").and_then(|v| v.as_str()).unwrap_or("");
        args = vec!["-o".into(), name.to_string()];
        let x = mon.get("x").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
        let y = mon.get("y").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
        let w = (mon.get("width").and_then(|v| v.as_f64()).unwrap_or(0.0) / mon.get("scale").and_then(|v| v.as_f64()).unwrap_or(1.0)).round() as i32;
        let h = (mon.get("height").and_then(|v| v.as_f64()).unwrap_or(0.0) / mon.get("scale").and_then(|v| v.as_f64()).unwrap_or(1.0)).round() as i32;
        meta["target"] = Value::String("monitor".into());
        meta["monitor"] = Value::String(name.to_string());
        meta["geometry"] = serde_json::json!([x,y,w,h]);
        base_scale = mon.get("scale").and_then(|v| v.as_f64()).unwrap_or(1.0);
    }

    // Default to JPEG q85 for 3x smaller transfer vs PNG (vision bottleneck)
    // Keep PNG only if caller explicitly wants it via env HYPRFAST_PNG=1
    let use_png = std::env::var("HYPRFAST_PNG").is_ok();
    let mut cmd_args = Vec::new();
    if !use_png {
        cmd_args.push("-t".to_string());
        cmd_args.push("jpeg".to_string());
        cmd_args.push("-q".to_string());
        cmd_args.push("85".to_string());
    }
    // Apply explicit scale if given (grim -s)
    if scale != 0.0 && (scale-1.0).abs()>1e-6 {
        cmd_args.push("-s".to_string());
        cmd_args.push(format!("{}",(base_scale*scale)));
        base_scale *= scale;
    }
    cmd_args.extend(args);
    cmd_args.push("-".to_string()); // stdout

    let out = Command::new("grim").args(&cmd_args).output()?;
    if !out.status.success() || out.stdout.is_empty() {
        bail!("grim failed: {}", String::from_utf8_lossy(&out.stderr));
    }
    let (iw, ih) = image_size(&out.stdout)?;
    meta["image"] = serde_json::json!([iw, ih]);
    meta["format"] = Value::String("jpeg".into()); // grim default without -t is png, but we treat as png
    // Detect format by magic
    if out.stdout.starts_with(&[0xFF,0xD8]) { meta["format"] = Value::String("jpeg".into()); }
    else if out.stdout.starts_with(&[0x89,0x50,0x4E,0x47]) { meta["format"] = Value::String("png".into()); }
    meta["scale"] = serde_json::json!(base_scale);
    meta["geometry"] = serde_json::json!(geom);
    Ok((out.stdout, meta))
}

static REGION_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
fn parse_region(s: &str) -> Result<(i32,i32,i32,i32)> {
    let s = s.trim();
    let re = REGION_RE.get_or_init(|| regex::Regex::new(r"^\s*(-?\d+)\s*,\s*(-?\d+)\s*[, ]\s*(\d+)\s*x\s*(\d+)\s*$").unwrap());
    let caps = re.captures(s).ok_or_else(|| anyhow::anyhow!("bad region {}", s))?;
    Ok((caps[1].parse()?, caps[2].parse()?, caps[3].parse()?, caps[4].parse()?))
}

fn scale_for_rect(x:i32,y:i32,w:i32,h:i32) -> Result<f64> {
    let mons = crate::hypr::query_json("monitors")?;
    let mut scales = Vec::new();
    for m in mons.as_array().unwrap_or(&vec![]) {
        let mx = m.get("x").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
        let my = m.get("y").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
        let mw = (m.get("width").and_then(|v| v.as_f64()).unwrap_or(0.0)/ m.get("scale").and_then(|v| v.as_f64()).unwrap_or(1.0)).round() as i32;
        let mh = (m.get("height").and_then(|v| v.as_f64()).unwrap_or(0.0)/ m.get("scale").and_then(|v| v.as_f64()).unwrap_or(1.0)).round() as i32;
        if x < mx+mw && mx < x+w && y < my+mh && my < y+h {
            scales.push(m.get("scale").and_then(|v| v.as_f64()).unwrap_or(1.0));
        }
    }
    Ok(scales.into_iter().fold(1.0f64, |a,b| a.max(b)))
}

fn image_size(data: &[u8]) -> Result<(u32,u32)> {
    if data.starts_with(&[0x89,0x50,0x4E,0x47,0x0D,0x0A,0x1A,0x0A]) {
        let w = u32::from_be_bytes([data[16],data[17],data[18],data[19]]);
        let h = u32::from_be_bytes([data[20],data[21],data[22],data[23]]);
        return Ok((w,h));
    }
    if data.starts_with(&[0xFF,0xD8]) {
        let mut i=2usize;
        while i+9 < data.len() {
            if data[i]!=0xFF { i+=1; continue; }
            let m=data[i+1];
            if m==0xD8||m==0xD9||(0xD0..=0xD7).contains(&m){ i+=2; continue; }
            let seg = u16::from_be_bytes([data[i+2],data[i+3]]) as usize;
            if [0xC0,0xC1,0xC2,0xC3,0xC5,0xC6,0xC7,0xC9,0xCA,0xCB,0xCD,0xCE,0xCF].contains(&m) {
                let h = u16::from_be_bytes([data[i+5],data[i+6]]) as u32;
                let w = u16::from_be_bytes([data[i+7],data[i+8]]) as u32;
                return Ok((w,h));
            }
            i+=2+seg;
        }
    }
    Ok((0,0))
}
