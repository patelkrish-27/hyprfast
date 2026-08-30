//! Fast input: keyboard via wtype, pointer via hypr movecursor + virtual-pointer
//! hypruse does: movecursor (Hyprland) + VirtualPointer wire (Wayland) + wtype
//! We do the same but without Python fork: direct Command calls.
//! For pointer wire we shell to python helper (one fork) until Rust wire client lands.

use anyhow::{Result, bail};
use std::process::Command;

pub fn move_cursor(x: f64, y: f64) -> Result<()> {
    // Hyprland 0.56: use lua dsp cursor move
    crate::hypr::eval_lua(&format!("return hl.dispatch(hl.dsp.cursor.move({{x={}, y={}}}))", x.round() as i32, y.round() as i32))?;
    Ok(())
}

pub fn click(x: Option<f64>, y: Option<f64>, button: &str, double: bool) -> Result<()> {
    if let (Some(x), Some(y)) = (x, y) {
        move_cursor(x, y)?;
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    // Use python wire helper for now (single fork, same as hypruse)
    let double_flag = if double { "true" } else { "false" };
    let xs = x.map(|v| v.to_string()).unwrap_or("None".to_string());
    let ys = y.map(|v| v.to_string()).unwrap_or("None".to_string());
    let code = format!("from hypruse.input import click; click({}, {}, button={:?}, double={})", xs, ys, button, double_flag);
    let out = Command::new("python3").args(["-c", &code]).output()?;
    if !out.status.success() {
        bail!("click failed: {}", String::from_utf8_lossy(&out.stderr));
    }
    Ok(())
}

pub fn drag(x1: f64, y1: f64, x2: f64, y2: f64, button: &str) -> Result<()> {
    let code = format!("from hypruse.input import drag; drag({},{},{},{}, button={:?})", x1, y1, x2, y2, button);
    let out = Command::new("python3").args(["-c", &code]).output()?;
    if !out.status.success() { bail!("drag failed: {}", String::from_utf8_lossy(&out.stderr)); }
    Ok(())
}

pub fn scroll(dy: f64, dx: f64, x: Option<f64>, y: Option<f64>) -> Result<()> {
    if let (Some(x), Some(y)) = (x, y) { move_cursor(x, y)?; }
    let xs = x.map(|v| v.to_string()).unwrap_or("None".to_string());
    let ys = y.map(|v| v.to_string()).unwrap_or("None".to_string());
    let code = format!("from hypruse.input import scroll; scroll(dy={}, dx={}, x={}, y={})", dy, dx, xs, ys);
    let out = Command::new("python3").args(["-c", &code]).output()?;
    if !out.status.success() { bail!("scroll failed: {}", String::from_utf8_lossy(&out.stderr)); }
    Ok(())
}

pub fn type_text(text: &str) -> Result<()> {
    // wtype via stdin (unicode-safe)
    let mut cmd = Command::new("wtype");
    cmd.arg("-");
    let mut child = cmd.stdin(std::process::Stdio::piped()).spawn()?;
    use std::io::Write;
    child.stdin.take().unwrap().write_all(text.as_bytes())?;
    let out = child.wait_with_output()?;
    if !out.status.success() { bail!("wtype failed: {}", String::from_utf8_lossy(&out.stderr)); }
    Ok(())
}

pub fn key_combo(combo: &str) -> Result<()> {
    // Delegate to hypruse parser (handles aliases, mods) via python wtype path
    let code = format!("from hypruse.input import key_combo; key_combo({:?})", combo);
    let out = Command::new("python3").args(["-c", &code]).output()?;
    if !out.status.success() { bail!("key_combo failed: {}", String::from_utf8_lossy(&out.stderr)); }
    Ok(())
}
