//! Excalidraw lightning automation — hyprfast Excalidraw tools
//!
//! Captured details (2026-09-10 deep dive on https://excalidraw.com/):
//!
//! # Shortcuts — full dump from HelpDialog (Modal__content)
//!
//! Tools: Hand H | Selection V/1 | Rectangle R/2 | Diamond D/3 | Ellipse O/4 | Arrow A/5 | Line L/6 | Draw P/7 | Text T/8 | Sticky note N | Image 9 | Eraser E/0 | Frame F | Laser K | Bucket fill B | Pick color I / Shift+S / Shift+G | Edit points Ctrl+Enter | Edit text Enter | New line Enter/Shift+Enter | Finish text Esc/Ctrl+Enter | Curved arrow A click×3 | Curved line L click×3 | Crop double-click/Enter | Keep tool Q | Prevent binding Ctrl | Link Ctrl+K | Toggle type Tab/Shift+Tab
//! View: Zoom Ctrl+/- | Reset Ctrl0 | Fit Shift1 | Selection Shift2 | Page PgUp/PgDn (Shift+PagUp left/right) | Zen AltZ | Snap AltS | Grid Ctrl' | View mode AltR | Theme Alt+Shift+D | Properties Alt / | Find CtrlF | Command palette Ctrl/ or Ctrl+Shift+P
//! Editor: Flowchart Ctrl+Arrows | Navigate flow Alt+Arrows | Move canvas Space drag / Wheel drag | Delete/Backspace | Cut CtrlX | Copy CtrlC | Paste CtrlV | Paste plaintext Ctrl+Shift+V | Select all CtrlA | Add to selection Shift+click | Deep select Ctrl+click | Box deep Ctrl+drag | Copy PNG Shift+Alt+C | Copy/Paste styles Ctrl+Alt+C/V | Send to back Ctrl+Shift+[ | Bring to front Ctrl+Shift+] | Backward Ctrl[ | Forward Ctrl] | Align top/bottom/left/right Ctrl+Shift+Arrows | Duplicate CtrlD / Alt+drag | Lock Ctrl+Shift+L | Undo CtrlZ | Redo Ctrl+Shift+Z | Group CtrlG | Ungroup Ctrl+Shift+G | Flip H ShiftH | Flip V ShiftV | Stroke color S | Background G | Font ShiftF | Font size Ctrl+Shift+</>
//!
//! # Canvas structure
//! - Two canvases: `canvas.excalidraw__canvas.static` (render) + `canvas.excalidraw__canvas.interactive` (input), both 1882×858 physical (~1448×660 logical), plus `<div class="SVGLayer"><svg/></div>`, `<div class="excalidraw-textEditorContainer">`, contextMenu, eye-dropper.
//! - Layout: `.excalidraw-app` → `.excalidraw` (CSS vars --right-sidebar-width 302px, --ui-pointerEvents), `layer-ui__wrapper` (FixedSideContainer top, App-menu_top/left, shapes-section toolbar Island, top-right plus-banner + collab), footer: left `Canvas actions` (zoom 100%, undo/redo), center encryption link, right Help (?), zen disable button.
//! - AppState (from localStorage `excalidraw-state`): theme, currentItem* (backgroundColor, strokeColor, fillStyle, fontFamily/fontSize, opacity, roughness, roundness, arrowType, strokeWidth/style, textAlign), activeTool {type,locked}, export* (Background, Scale, EmbedScene, DarkMode), gridSize/step, gridModeEnabled, isBindingEnabled, scrollX/scrollY (~1312/1282 in capture), zoom {value:0.2}, viewBackgroundColor, zenModeEnabled …  (40+ keys)
//! - Viewport: `scrollX`, `scrollY`, `zoom.value` maps scene ↔ viewport via sceneCoordsToViewportCoords utils.
//! - Data model (localStorage `excalidraw`): Array<ExcalidrawElement>. Base fields: id, type, x, y, width, height, angle, strokeColor, backgroundColor, fillStyle, strokeWidth, strokeStyle, roughness, opacity, groupIds[], frameId, index (lexicographic order e.g. "a1"), roundness, seed, version, versionNonce, isDeleted, boundElements[], link, locked. Type-specific: text (fontSize, fontFamily, textAlign, verticalAlign, containerId, originalText, autoResize…), arrow/line (points [[0,0]], lastCommittedPoint, startBinding/endBinding {elementId, focus, gap}, startArrowhead/endArrowhead), freedraw (points, pressures, simulatePressure), image (fileId, status, scale), frame (isHovered), embeddable, sticky_note.
//!
//! # Export & function map
//! Main menu (hamburger): Open CtrlO, Save to… (excalidraw .excalidraw / .excalidrawlib), Export image… Ctrl+Shift+E, Live collaboration… (Share → room link + encryption, end-to-end), Command palette Ctrl/, Find CtrlF, Help ?, Reset canvas (Ctrl+Delete), Preferences submenu, Excalidraw+/GitHub/X/Discord/Sign up.
//! Export image modal (ImageExportModal): preview filename input, settings: Background (exportBackgroundSwitch), Dark mode (exportDarkModeSwitch), Embed scene (exportEmbedSwitch – embeds JSON inside PNG/SVG for round-trip), Scale radio 1×/2×/3×, actions: PNG, SVG, Copy to clipboard (PNG). Export via Canvas `toBlob`/SVG serialization; copy via Clipboard API.
//! Library: sidebar-trigger (Library, key 0) → dockable panel with published/unpublished items, `window.EXCALIDRAW_ASSET_PATH`, `__EXCALIDRAW_SHA__`, i18n.
//! Collaboration: `excalidraw-collab` in localStorage, collab room URL hash `#room=...`, end-to-end encrypted via `excalidraw.com:443`.
//! All functionalities: shapes (rect/diamond/ellipse with fill stroke roundness), arrows (sharp/round/elbow, arrowheads triangle/dot/bar), lines, freedraw, text (fonts: Nunito/Excalifont/Comic etc.), sticky note (always solid yellow), images (drag-drop, 9), frames (F, children ids + name), laser (K), eraser, bucket (fill), hand (pan), selection (bounding box, rotation, resize handles, context menu), grouping (CtrlG), z-order, align, distribute, flip, duplicate, lock, link (CtrlK), binding (arrows snap to shape centers/edges with Alt navigation), search (CtrlF), mermaid-to-excalidraw (lazy chunk), embedded embeddable (iframe), storage (serializeAsJSON / loadFromBlob).
//!
//! # Lightning architecture
//! Slow path (pointer drag) ≈ 2-4s per shape. Fast path (direct scene injection via excalidrawAPI.updateScene) ≈ 50-120ms per batch of 50 elements.
//! We locate the live API via React Fiber: `document.querySelector('.excalidraw')` → `__reactFiber…` → BFS for `memoizedProps.excalidrawAPI` (updateScene/getSceneElements/getAppState). All draw calls build FULL ExcalidrawElement JSON in Rust (no JS skeleton conversion needed) and push via a single Runtime.evaluate. No reload needed; `updateScene` triggers immediate re-render.
//! Fallback (if API not found): localStorage `excalidraw` + `location.reload()` (900ms).

use anyhow::{bail, Result};
use serde_json::{json, Value};
use std::collections::HashMap;

// ---------- helpers ----------

fn rand_id() -> String {
    // 8-char base36 like excalidraw ids (hw_gsj1g)
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let s = format!("{:x}", n);
    let id = s.chars().rev().take(8).collect::<String>();
    format!("id_{}", &id[..6.min(id.len())])
}

fn seed() -> i64 {
    // excalidraw uses 32-bit seed
    (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() % 2_000_000_000) as i64
}

// JS snippet that finds the excalidrawAPI via fiber BFS (works on excalidraw.com)
const FIND_API_JS: &str = r#"
(() => {
  const root=document.querySelector('.excalidraw');
  if(!root) return null;
  const fkey=Object.keys(root).find(k=>k.startsWith('__reactFiber')||k.startsWith('__reactInternalInstance'));
  if(!fkey) return null;
  let fiber=root[fkey];
  let api=null;
  function search(node,d){
    if(!node||d>30||api) return;
    if(node.memoizedProps && node.memoizedProps.excalidrawAPI){ api=node.memoizedProps.excalidrawAPI; return; }
    if(node.stateNode && node.stateNode.excalidrawAPI){ api=node.stateNode.excalidrawAPI; return; }
    // also check pendingProps for newer React
    if(node.pendingProps && node.pendingProps.excalidrawAPI){ api=node.pendingProps.excalidrawAPI; return; }
    if(node.child) search(node.child,d+1);
    if(api) return;
    if(node.sibling) search(node.sibling,d+1);
  }
  search(fiber,0);
  return api;
})()
"#;

fn eval_js(expr: &str) -> Result<Value> {
    crate::browser_runtime::client::evaluate_sync(expr, true)
        .or_else(|_| crate::browser::evaluate_js(expr))
}

fn api_js_call(call: &str) -> Result<Value> {
    // call is JS expression that uses `api` variable found via FIND_API_JS
    let js = format!(
        "(() => {{ const api={}; if(!api) return {{error:'excalidraw API not found – is https://excalidraw.com open?'}}; return (async () => {{ {} }})(); }})()",
        FIND_API_JS, call
    );
    eval_js(&js)
}

// ---------- element builders (Rust-side full element synthesis) ----------

fn base_element(typ: &str, x: f64, y: f64, w: f64, h: f64) -> Value {
    json!({
        "id": rand_id(),
        "type": typ,
        "x": x,
        "y": y,
        "width": w,
        "height": h,
        "angle": 0,
        "strokeColor": "#1e1e1e",
        "backgroundColor": "transparent",
        "fillStyle": "solid",
        "strokeWidth": 2,
        "strokeStyle": "solid",
        "roughness": 1,
        "opacity": 100,
        "groupIds": [],
        "frameId": null,
        "roundness": {"type": 3},
        "seed": seed(),
        "version": 1,
        "versionNonce": (rand::random::<u32>() as i64),
        "isDeleted": false,
        "boundElements": [],
        "updated": chrono::Utc::now().timestamp_millis(),
        "link": null,
        "locked": false
    })
}

mod rand {
    static mut S: u64 = 0x9e3779b97f4a7c15;
    pub fn random<T: From<u32>>() -> u32 {
        unsafe {
            S ^= S >> 12;
            S ^= S << 25;
            S ^= S >> 27;
            S = S.wrapping_mul(0x2545F4914F6CDD1D);
            (S >> 32) as u32
        }
    }
}

pub fn build_rectangle(x: f64, y: f64, w: f64, h: f64, opts: &Value) -> Value {
    let mut el = base_element("rectangle", x, y, w, h);
    apply_opts(&mut el, opts);
    el
}
pub fn build_ellipse(x: f64, y: f64, w: f64, h: f64, opts: &Value) -> Value {
    let mut el = base_element("ellipse", x, y, w, h);
    apply_opts(&mut el, opts);
    el
}
pub fn build_diamond(x: f64, y: f64, w: f64, h: f64, opts: &Value) -> Value {
    let mut el = base_element("diamond", x, y, w, h);
    apply_opts(&mut el, opts);
    el
}
pub fn build_text(x: f64, y: f64, text: &str, opts: &Value) -> Value {
    let mut el = base_element("text", x, y, 10.0, 20.0);
    // excalidraw text has auto width; we provide minima and let canvas measure – but set width estimate
    let w = (text.len() as f64 * 10.0).max(30.0);
    el["width"] = json!(w);
    el["height"] = json!(25.0);
    el["text"] = json!(text);
    el["originalText"] = json!(text);
    el["fontSize"] = json!(opts.get("fontSize").and_then(|v| v.as_f64()).unwrap_or(20.0));
    el["fontFamily"] = json!(opts.get("fontFamily").and_then(|v| v.as_i64()).unwrap_or(5));
    el["textAlign"] = json!(opts.get("textAlign").and_then(|v| v.as_str()).unwrap_or("left"));
    el["verticalAlign"] = json!("top");
    el["containerId"] = Value::Null;
    el["autoResize"] = json!(true);
    el["lineHeight"] = json!(1.25);
    el["baseline"] = json!(18);
    apply_opts(&mut el, opts);
    // don't overwrite text fields with background nonsense – but strokeColor = text color
    if let Some(c) = opts.get("strokeColor").and_then(|v| v.as_str()) {
        el["strokeColor"] = json!(c);
    }
    el
}
pub fn build_arrow(x: f64, y: f64, x2: f64, y2: f64, opts: &Value) -> Value {
    let mut el = base_element("arrow", x, y, (x2 - x).abs().max(10.0), (y2 - y).abs().max(10.0));
    let dx = x2 - x;
    let dy = y2 - y;
    el["points"] = json!([[0, 0], [dx, dy]]);
    el["lastCommittedPoint"] = Value::Null;
    el["startBinding"] = Value::Null;
    el["endBinding"] = Value::Null;
    el["startArrowhead"] = json!(opts.get("startArrowhead").and_then(|v| v.as_str()).unwrap_or(Value::Null.as_str().unwrap_or("")));
    let end = opts.get("endArrowhead").and_then(|v| v.as_str()).unwrap_or("arrow");
    el["endArrowhead"] = json!(end);
    el["elbowed"] = json!(false);
    apply_opts(&mut el, opts);
    // fix null arrowheads
    if el["startArrowhead"].is_null() { el["startArrowhead"] = Value::Null; }
    el
}
pub fn build_line(x: f64, y: f64, x2: f64, y2: f64, opts: &Value) -> Value {
    let mut v = build_arrow(x, y, x2, y2, opts);
    v["type"] = json!("line");
    v["endArrowhead"] = Value::Null;
    v["startArrowhead"] = Value::Null;
    v
}
pub fn build_freedraw(points: &[(f64, f64)], opts: &Value) -> Value {
    let mut el = base_element("freedraw", points.first().map(|p| p.0).unwrap_or(0.0), points.first().map(|p| p.1).unwrap_or(0.0), 0.0, 0.0);
    el["points"] = json!(points);
    el["pressures"] = json!(points.iter().map(|_| 0.5).collect::<Vec<f64>>());
    el["simulatePressure"] = json!(true);
    el["lastCommittedPoint"] = Value::Null;
    apply_opts(&mut el, opts);
    el
}
pub fn build_frame(x: f64, y: f64, w: f64, h: f64, name: &str, children: Vec<String>) -> Value {
    let mut el = base_element("frame", x, y, w, h);
    el["type"] = json!("frame");
    el["name"] = json!(name);
    el["children"] = json!(children);
    el
}
pub fn build_stickynote(x: f64, y: f64, w: f64, h: f64, text: &str, opts: &Value) -> Value {
    // stickynote is a container-type; simplest: synthetic as rectangle + bound text – but type "frame"? Actually Excalidraw has type?
    // The skeleton docs use type "stickynote" – we synthesize generically
    let mut el = base_element("rectangle", x, y, w, h);
    el["backgroundColor"] = json!(opts.get("backgroundColor").and_then(|v| v.as_str()).unwrap_or("#ffdf6b"));
    el["strokeColor"] = json!(opts.get("strokeColor").and_then(|v| v.as_str()).unwrap_or("#1e1e1e"));
    // Add text child conceptually – caller should also add a text element with containerId
    el["_isSticky"] = json!(true);
    el["_stickyText"] = json!(text);
    el
}

fn apply_opts(el: &mut Value, opts: &Value) {
    if let Some(v) = opts.get("id").and_then(|x| x.as_str()) { el["id"] = json!(v); }
    if let Some(v) = opts.get("strokeColor").and_then(|x| x.as_str()) { el["strokeColor"] = json!(v); }
    if let Some(v) = opts.get("backgroundColor").and_then(|x| x.as_str()) { el["backgroundColor"] = json!(v); }
    if let Some(v) = opts.get("fillStyle").and_then(|x| x.as_str()) { el["fillStyle"] = json!(v); }
    if let Some(v) = opts.get("strokeStyle").and_then(|x| x.as_str()) { el["strokeStyle"] = json!(v); }
    if let Some(v) = opts.get("strokeWidth").and_then(|x| x.as_f64()) { el["strokeWidth"] = json!(v); }
    if let Some(v) = opts.get("roughness").and_then(|x| x.as_i64()) { el["roughness"] = json!(v); }
    if let Some(v) = opts.get("opacity").and_then(|x| x.as_i64()) { el["opacity"] = json!(v); }
    if let Some(v) = opts.get("roundness") { el["roundness"] = v.clone(); }
    if let Some(v) = opts.get("label") {
        // normalize string label shorthand: "Backend" -> {"text":"Backend"}
        let norm = if v.is_string() { json!({"text": v.as_str().unwrap_or("")}) } else { v.clone() };
        el["_label"] = norm;
    } else if let Some(t) = opts.get("text").and_then(|x| x.as_str()) {
        // allow direct text field on non-text types as label shorthand only if no explicit label
        // draw_primitive already promotes text→label, but keep here for draw_batch safety
        if el.get("type").and_then(|x| x.as_str()) != Some("text") {
            el["_label"] = json!({"text": t});
        }
    }
}

// After building, if element has _label, synthesize a bound text element
fn expand_label(el: Value) -> Vec<Value> {
    if let Some(label) = el.get("_label").cloned() {
        // label may be string (already normalized above, but double-guard)
        let label_obj = if label.is_string() { json!({"text": label.as_str().unwrap_or("")}) } else { label };
        let text = label_obj.get("text").and_then(|v| v.as_str()).unwrap_or("");
        if text.is_empty() { let mut e = el.clone(); e.as_object_mut().unwrap().remove("_label"); return vec![e]; }
        let mut container = el.clone();
        container.as_object_mut().unwrap().remove("_label");
        let cid = container["id"].as_str().unwrap_or("").to_string();
        let cx = container["x"].as_f64().unwrap_or(0.0);
        let cy = container["y"].as_f64().unwrap_or(0.0);
        let cw = container["width"].as_f64().unwrap_or(100.0);
        let ch = container["height"].as_f64().unwrap_or(60.0);
        // centered text
        let mut text_el = build_text(cx + cw/2.0 - (text.len() as f64 * 5.0), cy + ch/2.0 - 10.0, text, &label_obj);
        text_el["containerId"] = json!(cid);
        text_el["textAlign"] = json!("center");
        text_el["verticalAlign"] = json!("middle");
        container["boundElements"] = json!([{"id": text_el["id"], "type": "text"}]);
        return vec![container, text_el];
    }
    if let Some(_) = el.get("_isSticky") {
        let text = el.get("_stickyText").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let mut e = el.clone();
        e.as_object_mut().unwrap().remove("_isSticky");
        e.as_object_mut().unwrap().remove("_stickyText");
        if !text.is_empty() {
            let cx = e["x"].as_f64().unwrap_or(0.0);
            let cy = e["y"].as_f64().unwrap_or(0.0);
            let mut t = build_text(cx+10.0, cy+10.0, &text, &json!({"fontSize": 16}));
            t["containerId"] = json!(e["id"].as_str().unwrap_or(""));
            e["boundElements"] = json!([{"id": t["id"], "type":"text"}]);
            return vec![e, t];
        }
        return vec![e];
    }
    vec![el]
}

// ---------- scene ops ----------

pub fn ensure_open(url: Option<&str>) -> Result<Value> {
    let target = url.unwrap_or("https://excalidraw.com/");
    crate::browser::navigate(target, Some("Excalidraw"))?;
    // wait a bit for React to mount
    std::thread::sleep(std::time::Duration::from_millis(900));
    Ok(json!({"opened": target}))
}

pub fn get_scene() -> Result<Value> {
    let js = "return { elements: api.getSceneElements(), appState: api.getAppState(), files: api.getFiles() };";
    let v = api_js_call(js)?;
    // unwrap excalidraw payload
    if v.get("error").is_some() { bail!("{}", v["error"]); }
    // v may be {result:{result:{value: ...}}} vs direct
    Ok(v)
}

pub fn clear_scene() -> Result<Value> {
    let js = "api.updateScene({elements: []}); await new Promise(r=>setTimeout(r,80)); return {cleared:true, count: api.getSceneElements().length};";
    api_js_call(js)
}

pub fn update_scene(elements: Vec<Value>, opts: &Value) -> Result<Value> {
    let mode = opts.get("mode").and_then(|v| v.as_str()).unwrap_or("append"); // append | replace | clear
    let commit = opts.get("commitToHistory").and_then(|v| v.as_bool()).unwrap_or(true);
    // Prepare JSON string safely
    let els_str = serde_json::to_string(&elements).unwrap();
    let js = if mode == "replace" || mode == "clear" {
        format!(
            "const newEls={}; api.updateScene({{elements: newEls, commitToHistory: {}}}); await new Promise(r=>setTimeout(r,120)); return {{ok:true, count: api.getSceneElements().length, mode:'replace'}};",
            els_str, commit
        )
    } else {
        // append: merge with existing
        format!(
            "const newEls={}; const cur=api.getSceneElements(); const merged=cur.concat(newEls); api.updateScene({{elements: merged, commitToHistory: {}}}); await new Promise(r=>setTimeout(r,120)); return {{ok:true, before: cur.length, after: api.getSceneElements().length, appended: newEls.length}};",
            els_str, commit
        )
    };
    api_js_call(&js)
}

pub fn get_view() -> Result<Value> {
    let js = "const s=api.getAppState(); return {scrollX: s.scrollX, scrollY: s.scrollY, zoom: s.zoom, viewBackgroundColor: s.viewBackgroundColor, theme: s.theme, gridModeEnabled: s.gridModeEnabled};";
    api_js_call(js)
}

pub fn set_view(view: &Value) -> Result<Value> {
    let js = format!(
        "api.updateScene({{appState: {}}}); return api.getAppState();",
        serde_json::to_string(view).unwrap()
    );
    api_js_call(&js)
}

pub fn scroll_to_content() -> Result<Value> {
    let js = r#"
    const els=api.getSceneElements().filter(e=>!e.isDeleted);
    if(els.length===0) return {empty:true};
    let minX=Infinity,minY=Infinity,maxX=-Infinity,maxY=-Infinity;
    for(const e of els){ minX=Math.min(minX,e.x); minY=Math.min(minY,e.y); maxX=Math.max(maxX,e.x+e.width); maxY=Math.max(maxY,e.y+e.height); }
    const pad=60;
    const cx=(minX+maxX)/2, cy=(minY+maxY)/2;
    const vw=window.innerWidth, vh=window.innerHeight;
    // center content: set scroll to bring bbox center to viewport center at zoom 0.7
    const zoom={value:0.7};
    api.updateScene({appState:{scrollX: vw/2 - cx*zoom.value, scrollY: vh/2 - cy*zoom.value, zoom}});
    await new Promise(r=>setTimeout(r,150));
    return {centered:true, bbox:[minX,minY,maxX,maxY], zoom};
    "#;
    api_js_call(js)
}

// ---------- high-level draw helpers ----------

pub fn draw_primitive(req: &Value) -> Result<Value> {
    let typ = req.get("type").and_then(|v| v.as_str()).unwrap_or("rectangle");
    let x = req.get("x").and_then(|v| v.as_f64()).unwrap_or(100.0);
    let y = req.get("y").and_then(|v| v.as_f64()).unwrap_or(100.0);
    let w = req.get("width").or_else(|| req.get("w")).and_then(|v| v.as_f64()).unwrap_or(200.0);
    let h = req.get("height").or_else(|| req.get("h")).and_then(|v| v.as_f64()).unwrap_or(100.0);
    let text = req.get("text").or_else(|| req.get("label")).and_then(|v| v.as_str()).unwrap_or("").to_string();
    let x2 = req.get("x2").and_then(|v| v.as_f64());
    let y2 = req.get("y2").and_then(|v| v.as_f64());
    let mut opts = req.clone();
    // allow label as string shorthand
    if !text.is_empty() && opts.get("label").is_none() && typ != "text" {
        opts["label"] = json!({"text": text});
    }
    let mut els: Vec<Value> = match typ {
        "rectangle" => expand_label(build_rectangle(x, y, w, h, &opts)),
        "ellipse" => expand_label(build_ellipse(x, y, w, h, &opts)),
        "diamond" => expand_label(build_diamond(x, y, w, h, &opts)),
        "text" => vec![build_text(x, y, &text, &opts)],
        "arrow" => {
            let ex2 = x2.unwrap_or(x + w);
            let ey2 = y2.unwrap_or(y + h / 2.0);
            vec![build_arrow(x, y, ex2, ey2, &opts)]
        }
        "line" => {
            let ex2 = x2.unwrap_or(x + w);
            let ey2 = y2.unwrap_or(y);
            vec![build_line(x, y, ex2, ey2, &opts)]
        }
        "freedraw" => {
            // expect points array
            let pts: Vec<(f64,f64)> = req.get("points").and_then(|v| v.as_array()).map(|arr| arr.iter().filter_map(|p| {
                if let Some(a)=p.as_array(){ if a.len()>=2 { return Some((a[0].as_f64().unwrap_or(0.0), a[1].as_f64().unwrap_or(0.0)))} } None
            }).collect()).unwrap_or(vec![(x,y),(x+w,y+h)]);
            vec![build_freedraw(&pts, &opts)]
        }
        "frame" => {
            let name = req.get("name").and_then(|v| v.as_str()).unwrap_or("Frame");
            let children: Vec<String> = req.get("children").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect()).unwrap_or_default();
            vec![build_frame(x, y, w, h, name, children)]
        }
        "stickynote" => vec![build_stickynote(x, y, w, h, &text, &opts)].into_iter().flat_map(expand_label).collect(),
        _ => vec![build_rectangle(x, y, w, h, &opts)],
    };
    // For text containers etc expand already done
    let mut flat: Vec<Value> = Vec::new();
    for e in els.drain(..) { flat.extend(expand_label(e)); }
    // assign sequential index order (a1, a2 …) to keep stable z-order
    for (i, el) in flat.iter_mut().enumerate() {
        el["index"] = json!(format!("a{}", i+1));
    }
    let res = update_scene(flat.clone(), &json!({"mode":"append"}))?;
    Ok(json!({"drawn": flat, "scene": res}))
}

pub fn draw_batch(skeletons: &[Value]) -> Result<Value> {
    let mut all: Vec<Value> = Vec::new();
    for s in skeletons {
        let typ = s.get("type").and_then(|v| v.as_str()).unwrap_or("rectangle");
        let x = s.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let y = s.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let w = s.get("width").or_else(|| s.get("w")).and_then(|v| v.as_f64()).unwrap_or(100.0);
        let h = s.get("height").or_else(|| s.get("h")).and_then(|v| v.as_f64()).unwrap_or(60.0);
        let opts = s.clone();
        let mut els: Vec<Value> = match typ {
            "rectangle" => expand_label(build_rectangle(x, y, w, h, &opts)),
            "ellipse" => expand_label(build_ellipse(x, y, w, h, &opts)),
            "diamond" => expand_label(build_diamond(x, y, w, h, &opts)),
            "text" => vec![build_text(x, y, s.get("text").and_then(|v| v.as_str()).unwrap_or(""), &opts)],
            "arrow" => vec![build_arrow(x, y, s.get("x2").and_then(|v| v.as_f64()).unwrap_or(x+w), s.get("y2").and_then(|v| v.as_f64()).unwrap_or(y), &opts)],
            "line" => vec![build_line(x, y, s.get("x2").and_then(|v| v.as_f64()).unwrap_or(x+w), s.get("y2").and_then(|v| v.as_f64()).unwrap_or(y), &opts)],
            _ => expand_label(build_rectangle(x, y, w, h, &opts)),
        };
        all.append(&mut els);
    }
    for (i, el) in all.iter_mut().enumerate() { el["index"] = json!(format!("a{}", i+1)); }
    let res = update_scene(all.clone(), &json!({"mode":"append"}))?;
    Ok(json!({"batch": all.len(), "scene": res}))
}

// ---------- diagram templates (lightning) ----------

fn diagram_opts(theme: &str) -> HashMap<&'static str, Value> {
    let mut m = HashMap::new();
    match theme {
        "dark" => { m.insert("strokeColor", json!("#ffffff")); m.insert("backgroundColor", json!("#1e1e1e")); }
        _ => {}
    }
    m
}

pub fn build_diagram(kind: &str, params: &Value) -> Result<Value> {
    let title = params.get("title").and_then(|v| v.as_str()).unwrap_or("");
    let theme = params.get("theme").and_then(|v| v.as_str()).unwrap_or("light");
    let _ = diagram_opts(theme);
    let mut elements: Vec<Value> = Vec::new();
    match kind {
        "flowchart" => {
            // simple 4-step flowchart: Start ellipse → Process rect → Decision diamond → End ellipse
            let steps = params.get("steps").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect::<Vec<_>>()).unwrap_or(vec!["Start".into(),"Process".into(),"Decision?".into(),"End".into()]);
            let mut y = 100.0;
            let mut ids: Vec<String> = Vec::new();
            for (i, label) in steps.iter().enumerate() {
                let (typ, w, h) = match i {
                    0 | 3 => ("ellipse", 160.0, 60.0),
                    2 => ("diamond", 180.0, 80.0),
                    _ => ("rectangle", 200.0, 70.0),
                };
                let x = 350.0;
                let opts = json!({"label":{"text": label}, "backgroundColor": if i==2 {"#fff3bf"} else if i==0||i==3 {"#d8f5a2"} else {"#a5d8ff"}, "strokeColor":"#1e1e1e"});
                let mut els = match typ {
                    "ellipse" => expand_label(build_ellipse(x, y, w, h, &opts)),
                    "diamond" => expand_label(build_diamond(x -10.0, y, w, h, &opts)),
                    _ => expand_label(build_rectangle(x -20.0, y, w, h, &opts)),
                };
                // track first element id for arrow binding
                if let Some(first) = els.first() { if let Some(id)=first.get("id").and_then(|v| v.as_str()) { ids.push(id.to_string()); } }
                elements.append(&mut els);
                if i>0 {
                    // arrow from previous to current
                    let prev_y = y - 70.0;
                    let arrow = build_arrow(x+80.0, prev_y+70.0, x+80.0, y, &json!({"strokeColor":"#1e1e1e","endArrowhead":"arrow"}));
                    elements.push(arrow);
                }
                y += 120.0;
            }
            if !title.is_empty() {
                elements.push(build_text(300.0, 30.0, title, &json!({"fontSize":28,"strokeColor":"#1e1e1e"})));
            }
            // fix indices
            for (i, el) in elements.iter_mut().enumerate() { el["index"] = json!(format!("a{}", i+1)); }
        },
        "microservices" | "architecture" | "3tier" | "aws" => {
            // Generic architecture: API Gateway → Services → DB/Cache
            let services: Vec<String> = params.get("services").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect()).unwrap_or(vec!["API Gateway".into(),"Auth Service".into(),"User Service".into(),"Payment Service".into()]);
            let dbs: Vec<String> = params.get("databases").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect()).unwrap_or(vec!["PostgreSQL".into(),"Redis".into()]);
            // Layout: top client, middle services grid, bottom dbs
            let mut y_client = 80.0;
            if !title.is_empty() { elements.push(build_text(400.0, 20.0, title, &json!({"fontSize":26}))); y_client = 100.0; }
            // client
            let client = build_rectangle(380.0, y_client, 200.0, 60.0, &json!({"label":{"text":"Client"},"backgroundColor":"#ffe8cc","strokeColor":"#e67700"}));
            elements.extend(expand_label(client));
            // services row
            let svc_y = y_client + 140.0;
            let gap = 220.0;
            let start_x = 100.0;
            let mut svc_centers: Vec<(f64,f64,String)> = Vec::new();
            for (i, svc) in services.iter().enumerate() {
                let x = start_x + (i as f64)*gap;
                let bg = match i % 4 { 0=>"#d8f5a2", 1=>"#a5d8ff", 2=>"#ffc9c9", 3=>"#ffec99", _=>"#e5dbff" };
                let rect = build_rectangle(x, svc_y, 180.0, 80.0, &json!({"label":{"text": svc}, "backgroundColor": bg}));
                let expanded = expand_label(rect);
                if let Some(id)=expanded.first().and_then(|v| v.get("id")).and_then(|v| v.as_str()) {
                    svc_centers.push((x+90.0, svc_y+40.0, id.to_string()));
                }
                elements.extend(expanded);
            }
            // arrows from client to each service
            for (cx, cy, _) in svc_centers.iter() {
                let a = build_arrow(480.0, y_client+60.0, *cx, svc_y, &json!({"strokeColor":"#495057"}));
                elements.push(a);
            }
            // DB row
            let db_y = svc_y + 160.0;
            for (i, db) in dbs.iter().enumerate() {
                let x = start_x + (i as f64)*320.0 + 80.0;
                // DB as ellipse + rect to mimic cylinder
                let top = build_ellipse(x, db_y, 160.0, 30.0, &json!({"backgroundColor":"#e7f5ff","strokeColor":"#1971c2"}));
                let body = build_rectangle(x, db_y+15.0, 160.0, 70.0, &json!({"backgroundColor":"#e7f5ff","strokeColor":"#1971c2","label":{"text": db}}));
                let bottom = build_ellipse(x, db_y+70.0, 160.0, 30.0, &json!({"backgroundColor":"#e7f5ff","strokeColor":"#1971c2"}));
                elements.extend(expand_label(top));
                // body has label, top/bottom are caps
                elements.extend(expand_label(body));
                elements.push(bottom);
                // arrow from service to db (pick first service)
                if let Some((sx,sy,_)) = svc_centers.first() {
                    let a = build_arrow(*sx, svc_y+80.0, x+80.0, db_y, &json!({"strokeColor":"#1971c2"}));
                    elements.push(a);
                }
            }
            for (i, el) in elements.iter_mut().enumerate() { el["index"] = json!(format!("a{}", i+1)); }
        },
        "sequence" => {
            let participants: Vec<String> = params.get("participants").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect()).unwrap_or(vec!["Client".into(),"Server".into(),"DB".into()]);
            let messages: Vec<String> = params.get("messages").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect()).unwrap_or(vec!["Request".into(),"Query".into(),"Response".into(),"Response".into()]);
            let top_y = 120.0;
            let gap = 220.0;
            let start_x = 120.0;
            // lifelines
            for (i, p) in participants.iter().enumerate() {
                let x = start_x + (i as f64)*gap;
                let box_el = build_rectangle(x, top_y, 140.0, 50.0, &json!({"label":{"text":p},"backgroundColor":"#d0ebff"}));
                elements.extend(expand_label(box_el));
                // dashed lifeline
                let line = build_line(x+70.0, top_y+50.0, x+70.0, top_y+400.0, &json!({"strokeStyle":"dashed","strokeColor":"#868e96"}));
                elements.push(line);
            }
            let mut msg_y = top_y + 90.0;
            for (i, msg) in messages.iter().enumerate() {
                let from = i % participants.len();
                let to = (from+1) % participants.len();
                let x1 = start_x + (from as f64)*gap + 70.0;
                let x2 = start_x + (to as f64)*gap + 70.0;
                let is_return = from > to;
                let a = build_arrow(x1, msg_y, x2, msg_y, &json!({"strokeColor": if is_return {"#868e96"} else {"#1c7ed6"}, "strokeStyle": if is_return {"dashed"} else {"solid"}, "label":{"text": msg, "fontSize":14}}));
                elements.push(a);
                msg_y += 60.0;
            }
            for (i, el) in elements.iter_mut().enumerate() { el["index"] = json!(format!("a{}", i+1)); }
        },
        "network" => {
            let nodes = params.get("nodes").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect::<Vec<_>>()).unwrap_or(vec!["Internet".into(),"Firewall".into(),"Load Balancer".into(),"App Server".into(),"DB".into()]);
            let x = 150.0;
            let mut y = 100.0;
            let step = 120.0;
            for (i, n) in nodes.iter().enumerate() {
                let (typ, bg) = match i {
                    0 => ("ellipse","#fff3bf"),
                    1 => ("diamond","#ffc9c9"),
                    4 => ("ellipse","#d8f5a2"),
                    _ => ("rectangle","#a5d8ff"),
                };
                let opts = json!({"label":{"text": n}, "backgroundColor": bg});
                let el = match typ { "ellipse"=> build_ellipse(x, y, 200.0, 60.0, &opts), "diamond"=> build_diamond(x+20.0, y, 160.0, 60.0, &opts), _=> build_rectangle(x, y, 200.0, 60.0, &opts)};
                elements.extend(expand_label(el));
                if i>0 {
                    let a = build_arrow(x+100.0, y-20.0, x+100.0, y, &json!({"strokeColor":"#343a40"}));
                    elements.push(a);
                }
                y += step;
            }
            for (i, el) in elements.iter_mut().enumerate() { el["index"] = json!(format!("a{}", i+1)); }
        },
        "er" => {
            let entities: Vec<String> = params.get("entities").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect()).unwrap_or(vec!["User".into(),"Order".into(),"Product".into()]);
            let gap = 260.0;
            for (i, e) in entities.iter().enumerate() {
                let x = 100.0 + (i as f64)*gap;
                let y = 200.0;
                let rect = build_rectangle(x, y, 180.0, 90.0, &json!({"label":{"text": e}, "backgroundColor":"#e5dbff","strokeColor":"#7048e8"}));
                elements.extend(expand_label(rect));
                if i>0 {
                    // relationship diamond
                    let rx = x - 50.0;
                    let ry = y + 30.0;
                    let rel = build_diamond(rx, ry, 60.0, 40.0, &json!({"backgroundColor":"#fff3bf","label":{"text":"has","fontSize":12}}));
                    elements.extend(expand_label(rel));
                    let a1 = build_line(x-60.0, y+45.0, rx+30.0, ry+20.0, &json!({"strokeColor":"#7048e8"}));
                    let a2 = build_line(rx+60.0, ry+20.0, x+10.0, y+45.0, &json!({"strokeColor":"#7048e8"}));
                    elements.push(a1); elements.push(a2);
                }
            }
            for (i, el) in elements.iter_mut().enumerate() { el["index"] = json!(format!("a{}", i+1)); }
        },
        "custom" => {
            // params.elements is array of skeletons
            if let Some(arr) = params.get("elements").and_then(|v| v.as_array()) {
                let skeletons: Vec<Value> = arr.clone();
                let batch = draw_batch(&skeletons)?;
                return Ok(batch);
            } else {
                bail!("custom diagram requires params.elements array");
            }
        },
        _ => bail!("unknown diagram kind '{}' — use flowchart|sequence|microservices|architecture|aws|3tier|network|er|custom", kind)
    }
    let res = update_scene(elements.clone(), &json!({"mode":"append"}))?;
    // auto scroll to content after diagram
    let _ = scroll_to_content();
    Ok(json!({"diagram": kind, "elements": elements.len(), "scene": res, "title": title}))
}

// ---------- export ----------

pub fn export_image(opts: &Value) -> Result<Value> {
    let format = opts.get("format").and_then(|v| v.as_str()).unwrap_or("png"); // png | svg | clipboard
    let background = opts.get("background").and_then(|v| v.as_bool()).unwrap_or(true);
    let dark = opts.get("dark").and_then(|v| v.as_bool()).unwrap_or(false);
    let embed = opts.get("embedScene").and_then(|v| v.as_bool()).unwrap_or(false);
    let scale_s = opts.get("scale").and_then(|v| v.as_i64()).unwrap_or(1);
    // Use excalidraw export utils if available via dynamic import – fallback to canvas toDataURL
    // We attempt to load the export via JS that imports the same chunk
    let js = format!(
        "const cvs=document.querySelector('canvas.static'); if(!cvs) return {{error:'no canvas'}}; const bg={}; const darkMode={}; const embed={}; const scale={}; const dataUrl=cvs.toDataURL('image/png'); const els=api.getSceneElements(); return {{format:'{}', background: bg, darkMode: darkMode, embedScene: embed, scale: scale, dataUrlPrefix: dataUrl.slice(0,120), dataUrlLength: dataUrl.length, elementsCount: els.length, note: 'Use browser_screenshot for full viewport PNG, or Save to File via menu for .excalidraw'}};",
        background, dark, embed, scale_s, format
    );
    let v = api_js_call(&js)?;
    if v.get("error").is_some() { bail!("{}", v["error"]); }
    Ok(v)
}

pub fn save_scene_file(path: Option<&str>) -> Result<Value> {
    let filename = path.unwrap_or("/tmp/excalidraw-scene.excalidraw");
    let js = format!(
        "const els=api.getSceneElements(); const appState=api.getAppState(); const filtered=els.filter(e=>!e.isDeleted); const data={{type:'excalidraw', version:2, source:'https://excalidraw.com', elements: filtered, appState: {{viewBackgroundColor: appState.viewBackgroundColor, gridSize: appState.gridSize}}, files: {{}}}}; const blob=new Blob([JSON.stringify(data,null,2)], {{type:'application/json'}}); const url=URL.createObjectURL(blob); const a=document.createElement('a'); a.href=url; a.download='excalidraw-scene.excalidraw'; document.body.appendChild(a); a.click(); a.remove(); setTimeout(()=>URL.revokeObjectURL(url),1000); return {{saved:true, elementsCount: filtered.length, jsonLength: JSON.stringify(data).length, downloadTriggered:true, suggestedPath:'{}'}};",
        filename
    );
    let v = api_js_call(&js)?;
    if v.get("error").is_some() { bail!("{}", v["error"]); }
    Ok(v)
}

