//! Fast input: native Rust — no python forks.
//! Pointer: hypr movecursor (global logical) + Wayland zwlr_virtual_pointer_unstable_v1 (no ydotool)
//! Keyboard: wtype via zwp_virtual_keyboard_v1, Rust combo parser
//! Mirrors hypruse/input.py + wire.py but pure Rust (<2ms vs 60-80ms)

use anyhow::{Result, bail, Context};
use std::process::Command;
use std::collections::HashMap;
use std::os::unix::net::UnixStream;
use std::io::{Read, Write};
use std::time::{Duration, Instant};

pub fn move_cursor(x: f64, y: f64) -> Result<()> {
    crate::hypr::dispatch("movecursor", &format!("{} {}", x.round() as i32, y.round() as i32))?;
    Ok(())
}

// ---------- Wayland virtual pointer (port of hypruse/wire.py) ----------

const DISPLAY_ID: u32 = 1;
const REQ_SYNC: u32 = 0;
const REQ_GET_REGISTRY: u32 = 1;
const MGR_CREATE_POINTER: u32 = 0;
const MGR_DESTROY: u32 = 1;
const PTR_BUTTON: u32 = 2;
const PTR_AXIS: u32 = 3;
const PTR_FRAME: u32 = 4;
const PTR_AXIS_SOURCE: u32 = 5;
const PTR_AXIS_DISCRETE: u32 = 7;
const PTR_DESTROY: u32 = 8;
const MANAGER_INTERFACE: &str = "zwlr_virtual_pointer_manager_v1";
const BUTTONS: &[(&str, u32)] = &[("left", 0x110), ("right", 0x111), ("middle", 0x112), ("back", 0x113), ("forward", 0x114)];
const PRESSED: u32 = 1;
const RELEASED: u32 = 0;
const AXIS_VERTICAL: u32 = 0;
const AXIS_HORIZONTAL: u32 = 1;
const AXIS_SOURCE_WHEEL: u32 = 0;

fn wl_string(s: &str) -> Vec<u8> {
    let mut raw = s.as_bytes().to_vec();
    raw.push(0);
    let pad = (4 - raw.len() % 4) % 4;
    let mut out = (raw.len() as u32).to_le_bytes().to_vec();
    out.extend_from_slice(&raw);
    out.extend(std::iter::repeat(0).take(pad));
    out
}
fn to_fixed(v: f64) -> i32 { (v * 256.0).round() as i32 }
fn encode_msg(obj: u32, opcode: u32, body: &[u8]) -> Vec<u8> {
    let size = 8 + body.len();
    let mut out = Vec::with_capacity(size);
    out.extend_from_slice(&obj.to_le_bytes());
    out.extend_from_slice(&(((size as u32) << 16 | opcode).to_le_bytes()));
    out.extend_from_slice(body);
    out
}
fn parse_events(buf: &[u8]) -> (Vec<(u32,u32,Vec<u8>)>, Vec<u8>) {
    let mut events = Vec::new();
    let mut off = 0usize;
    while buf.len() >= off + 8 {
        let obj = u32::from_le_bytes([buf[off],buf[off+1],buf[off+2],buf[off+3]]);
        let sizeop = u32::from_le_bytes([buf[off+4],buf[off+5],buf[off+6],buf[off+7]]);
        let size = (sizeop >> 16) as usize;
        let opcode = sizeop & 0xFFFF;
        if size < 8 || buf.len() < off + size { break; }
        events.push((obj, opcode, buf[off+8..off+size].to_vec()));
        off += size;
    }
    (events, buf[off..].to_vec())
}
fn parse_global(body: &[u8]) -> Option<(u32,String,u32)> {
    if body.len() < 12 { return None; }
    let name = u32::from_le_bytes([body[0],body[1],body[2],body[3]]);
    let slen = u32::from_le_bytes([body[4],body[5],body[6],body[7]]) as usize;
    if body.len() < 8 + slen + 4 { return None; }
    let iface = String::from_utf8_lossy(&body[8..8+slen-1]).to_string();
    let pad = (4 - slen % 4) % 4;
    let ver = u32::from_le_bytes([body[8+slen+pad],body[8+slen+pad+1],body[8+slen+pad+2],body[8+slen+pad+3]]);
    Some((name, iface, ver))
}

struct VirtualPointer {
    sock: UnixStream,
    buf: Vec<u8>,
    next_id: u32,
    registry: u32,
    manager: u32,
    pointer: u32,
    version: u32,
}
impl VirtualPointer {
    fn connect() -> Result<Self> {
        let runtime = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| format!("/run/user/{}", nix::unistd::getuid()));
        let display = std::env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "wayland-0".to_string());
        let path = if display.starts_with('/') { display } else { format!("{}/{}", runtime, display) };
        let sock = UnixStream::connect(&path).with_context(|| format!("connect Wayland {}", path))?;
        sock.set_read_timeout(Some(Duration::from_secs(3)))?;
        sock.set_write_timeout(Some(Duration::from_secs(3)))?;
        let mut vp = VirtualPointer { sock, buf: Vec::new(), next_id: 2, registry: 0, manager: 0, pointer: 0, version: 2 };
        vp.registry = vp.new_id();
        vp.send(DISPLAY_ID, REQ_GET_REGISTRY, &vp.registry.to_le_bytes())?;
        let globals = vp.roundtrip_collect(true)?;
        let mut matched: Option<(u32,u32)> = None;
        for (n,i,v) in globals { if i == MANAGER_INTERFACE { matched = Some((n,v)); break; } }
        let (name, ver) = matched.ok_or_else(|| anyhow::anyhow!("compositor does not advertise {}", MANAGER_INTERFACE))?;
        let version = ver.min(2);
        vp.version = version;
        vp.manager = vp.new_id();
        let mut body = Vec::new();
        body.extend_from_slice(&name.to_le_bytes());
        body.extend_from_slice(&wl_string(MANAGER_INTERFACE));
        body.extend_from_slice(&version.to_le_bytes());
        body.extend_from_slice(&vp.manager.to_le_bytes());
        vp.send(vp.registry, 0, &body)?; // wl_registry.bind
        vp.pointer = vp.new_id();
        let mut b2 = Vec::new();
        b2.extend_from_slice(&0u32.to_le_bytes()); // seat null
        b2.extend_from_slice(&vp.pointer.to_le_bytes());
        vp.send(vp.manager, MGR_CREATE_POINTER, &b2)?;
        vp.roundtrip_collect(false)?;
        Ok(vp)
    }
    fn new_id(&mut self) -> u32 { let id = self.next_id; self.next_id += 1; id }
    fn send(&mut self, obj: u32, opcode: u32, body: &[u8]) -> Result<()> {
        let msg = encode_msg(obj, opcode, body);
        self.sock.write_all(&msg).context("Wayland send")?;
        Ok(())
    }
    fn roundtrip_collect(&mut self, collect: bool) -> Result<Vec<(u32,String,u32)>> {
        let cb = self.new_id();
        self.send(DISPLAY_ID, REQ_SYNC, &cb.to_le_bytes())?;
        let mut globals = Vec::new();
        let mut tmp = [0u8; 65536];
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if Instant::now() > deadline { bail!("Wayland sync timeout"); }
            match self.sock.read(&mut tmp) {
                Ok(0) => bail!("Wayland closed"),
                Ok(n) => {
                    self.buf.extend_from_slice(&tmp[..n]);
                    let (events, remain) = parse_events(&self.buf);
                    self.buf = remain;
                    for (obj, opcode, body) in events {
                        if obj == DISPLAY_ID && opcode == 1 { // error
                            bail!("wl_display error {:?}", body);
                        }
                        if obj == self.registry && opcode == 0 && collect {
                            if let Some(g) = parse_global(&body) { globals.push(g); }
                        }
                        if obj == cb && opcode == 0 { // wl_callback.done
                            return Ok(globals);
                        }
                    }
                },
                Err(e) if e.kind()==std::io::ErrorKind::WouldBlock || e.kind()==std::io::ErrorKind::TimedOut => continue,
                Err(e) => bail!("Wayland read {}", e),
            }
        }
    }
    fn button(&mut self, code: u32, state: u32) -> Result<()> {
        let t = (Instant::now().elapsed().as_millis() & 0xFFFFFFFF) as u32; // approx monotonic
        let mut body = Vec::new();
        body.extend_from_slice(&t.to_le_bytes());
        body.extend_from_slice(&code.to_le_bytes());
        body.extend_from_slice(&state.to_le_bytes());
        self.send(self.pointer, PTR_BUTTON, &body)?;
        self.send(self.pointer, PTR_FRAME, &[])?;
        self.roundtrip_collect(false)?;
        Ok(())
    }
    fn scroll(&mut self, dy: f64, dx: f64) -> Result<()> {
        let t = (Instant::now().elapsed().as_millis() & 0xFFFFFFFF) as u32;
        // axis source wheel
        self.send(self.pointer, PTR_AXIS_SOURCE, &AXIS_SOURCE_WHEEL.to_le_bytes())?;
        for (axis, v) in [(AXIS_VERTICAL, dy), (AXIS_HORIZONTAL, dx)] {
            if v == 0.0 { continue; }
            let val = to_fixed(v * 15.0);
            let notches = v.round() as i32;
            let discrete = self.version >=2 && notches !=0 && (v - notches as f64).abs() < 1e-6;
            if discrete {
                let mut b = Vec::new();
                b.extend_from_slice(&t.to_le_bytes());
                b.extend_from_slice(&axis.to_le_bytes());
                b.extend_from_slice(&val.to_le_bytes());
                b.extend_from_slice(&notches.to_le_bytes());
                self.send(self.pointer, PTR_AXIS_DISCRETE, &b)?;
            } else {
                let mut b = Vec::new();
                b.extend_from_slice(&t.to_le_bytes());
                b.extend_from_slice(&axis.to_le_bytes());
                b.extend_from_slice(&val.to_le_bytes());
                self.send(self.pointer, PTR_AXIS, &b)?;
            }
        }
        self.send(self.pointer, PTR_FRAME, &[])?;
        self.roundtrip_collect(false)?;
        Ok(())
    }
    fn close(mut self) {
        let _ = self.send(self.pointer, PTR_DESTROY, &[]);
        let _ = self.send(self.manager, MGR_DESTROY, &[]);
        let _ = self.roundtrip_collect(false);
    }
}
impl Drop for VirtualPointer { fn drop(&mut self) { let _ = self.sock.shutdown(std::net::Shutdown::Both); } }

fn button_code(name: &str) -> Result<u32> {
    for (k,v) in BUTTONS { if *k==name { return Ok(*v); } }
    bail!("unknown button {:?}; left/right/middle/back/forward", name)
}

pub fn click(x: Option<f64>, y: Option<f64>, button: &str, double: bool) -> Result<()> {
    if let (Some(x), Some(y)) = (x, y) {
        move_cursor(x, y)?;
        std::thread::sleep(Duration::from_millis(20));
    }
    let code = button_code(button)?;
    let mut vp = VirtualPointer::connect()?;
    let presses = if double { 2 } else { 1 };
    for i in 0..presses {
        if i==1 { std::thread::sleep(Duration::from_millis(60)); }
        vp.button(code, PRESSED)?;
        std::thread::sleep(Duration::from_millis(20));
        vp.button(code, RELEASED)?;
    }
    Ok(())
}

pub fn drag(x1: f64, y1: f64, x2: f64, y2: f64, button: &str) -> Result<()> {
    let code = button_code(button)?;
    move_cursor(x1, y1)?;
    std::thread::sleep(Duration::from_millis(30));
    let mut vp = VirtualPointer::connect()?;
    vp.button(code, PRESSED)?;
    let steps = 12;
    for i in 1..=steps {
        let x = x1 + (x2-x1)* i as f64 / steps as f64;
        let y = y1 + (y2-y1)* i as f64 / steps as f64;
        move_cursor(x, y)?;
        std::thread::sleep(Duration::from_millis(15));
    }
    vp.button(code, RELEASED)?;
    Ok(())
}

pub fn scroll(dy: f64, dx: f64, x: Option<f64>, y: Option<f64>) -> Result<()> {
    if dy==0.0 && dx==0.0 { bail!("scroll needs non-zero dy or dx"); }
    if let (Some(x), Some(y)) = (x,y) { move_cursor(x,y)?; std::thread::sleep(Duration::from_millis(20)); }
    let mut vp = VirtualPointer::connect()?;
    vp.scroll(dy, dx)?;
    Ok(())
}

pub fn type_text(text: &str) -> Result<()> {
    if text.is_empty() { return Ok(()); }
    let mut child = Command::new("wtype").arg("-").stdin(std::process::Stdio::piped()).spawn().context("spawn wtype")?;
    use std::io::Write;
    child.stdin.take().unwrap().write_all(text.as_bytes())?;
    let out = child.wait_with_output()?;
    if !out.status.success() { bail!("wtype failed: {}", String::from_utf8_lossy(&out.stderr)); }
    Ok(())
}

// Rust port of hypruse/input.py parse_combo + combo_to_wtype_args
fn parse_combo(combo: &str) -> Result<(Vec<String>, Option<String>)> {
    let mods_map: HashMap<&str,&str> = [("ctrl","ctrl"),("control","ctrl"),("shift","shift"),("alt","alt"),("super","logo"),("meta","logo"),("win","logo"),("cmd","logo"),("altgr","altgr")].into();
    let aliases: HashMap<&str,&str> = [("enter","Return"),("return","Return"),("esc","Escape"),("escape","Escape"),("tab","Tab"),("space","space"),("backspace","BackSpace"),("delete","Delete"),("del","Delete"),("insert","Insert"),("home","Home"),("end","End"),("pgup","Page_Up"),("pageup","Page_Up"),("pgdn","Page_Down"),("pagedown","Page_Down"),("up","Up"),("down","Down"),("left","Left"),("right","Right")].into();
    let parts: Vec<&str> = combo.split('+').map(|p| p.trim()).filter(|p| !p.is_empty()).collect();
    if parts.is_empty() { bail!("empty key combo"); }
    let mut mods = Vec::new();
    let mut key: Option<String> = None;
    for (i, part) in parts.iter().enumerate() {
        let lower = part.to_lowercase();
        let last = i == parts.len()-1;
        if mods_map.contains_key(lower.as_str()) {
            mods.push(mods_map[lower.as_str()].to_string());
            if last { key = None; }
        } else if !last {
            bail!("unknown modifier {:?} in {:?}", part, combo);
        } else if aliases.contains_key(lower.as_str()) {
            key = Some(aliases[lower.as_str()].to_string());
        } else if part.len()==1 {
            key = Some(part.to_string());
        } else if lower.starts_with('f') && lower[1..].chars().all(|c| c.is_ascii_digit()) {
            key = Some(part.to_uppercase());
        } else {
            key = Some(part.to_string());
        }
    }
    Ok((mods, key))
}

pub fn key_combo(combo: &str) -> Result<()> {
    let (mods, key) = parse_combo(combo)?;
    let mut args: Vec<String> = Vec::new();
    for m in &mods { args.push("-M".into()); args.push(m.clone()); }
    if let Some(k) = key { args.push("-k".into()); args.push(k); }
    for m in mods.iter().rev() { args.push("-m".into()); args.push(m.clone()); }
    if args.is_empty() { bail!("combo resolved to nothing"); }
    let out = Command::new("wtype").args(&args).output().context("spawn wtype")?;
    if !out.status.success() { bail!("wtype key_combo failed: {}", String::from_utf8_lossy(&out.stderr)); }
    Ok(())
}
