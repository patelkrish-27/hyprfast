#![recursion_limit = "512"]
#![allow(clippy::all, clippy::pedantic, clippy::nursery, dead_code, unused_imports, unused_variables, unused_assignments)]
mod hypr;
mod a11y;
mod input;
mod screenshot;
mod events;
mod daemon;
mod session;
mod task;
mod cdp;
mod browser;
mod ground;
mod browser_runtime;
mod devtools_mcp;
mod hint;
mod excalidraw;
mod decider;
mod perception;

use clap::{Parser, Subcommand};
use anyhow::Result;

#[derive(Parser)]
#[command(name="hyprfast", version, about="Fast hypruse alternative - persistent IPC, direct AT-SPI + CDP browser automation")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum BrowserCmd {
    /// Navigate to URL (creates tab if needed)
    Navigate { url: String, #[arg(long)] target: Option<String> },
    /// Go back
    Back,
    /// Go forward
    Forward,
    /// Capture accessibility snapshot (CDP)
    Snapshot,
    /// Click element by selector or ref
    Click { #[arg(long)] selector: Option<String>, #[arg(long)] r#ref: Option<String>, #[arg(long)] element: Option<String> },
    /// Hover element
    Hover { #[arg(long)] selector: Option<String>, #[arg(long)] r#ref: Option<String> },
    /// Type text into element
    Type { text: String, #[arg(long)] selector: Option<String>, #[arg(long)] r#ref: Option<String>, #[arg(long)] submit: bool },
    /// Fill selector with text (alias)
    Fill { selector: String, text: String },
    /// Select option in dropdown
    Select { #[arg(long)] selector: Option<String>, #[arg(long)] r#ref: Option<String>, values: Vec<String> },
    /// Press key (Enter, Escape, Tab, ArrowLeft etc)
    Press { key: String },
    /// Evaluate JavaScript
    Eval { js: String },
    /// Screenshot via CDP (browser tab) - faster than grim for browser
    Shot { #[arg(long)] output: Option<String> },
    /// List tabs/targets
    Tabs,
    /// Get console logs
    Console,
    /// Wait N seconds
    Wait { #[arg(default_value="1")] secs: f64 },
    /// Launch browser with remote-debugging-port (workspace optional)
    Open { url: String, #[arg(long)] workspace: Option<String> },
}

#[derive(Subcommand)]
enum BrowserRuntimeCmd {
    /// Start the persistent browser-runtime daemon (owns the single CDP connection)
    Start,
    /// Stop the browser-runtime daemon cleanly (drains, closes WS, removes socket)
    Stop,
    /// Show daemon status: lifecycle state, browser, targets, counters, socket mode
    Status,
}

#[derive(Subcommand)]
enum TaskCmd {
    /// Initialize task list: breakdown goal into steps
    Init { goal: String, #[arg(long)] steps: String },
    /// Add a step to current task list
    Add { description: String },
    /// Update step status: pending|in_progress|completed|failed|skipped
    Update { #[arg(long)] index: Option<usize>, #[arg(long)] id: Option<usize>, status: String },
    /// Show current task status & progress
    Status,
    /// Clear all tasks (manual)
    Clear,
    /// Get next pending step
    Next,
    /// List - alias for Status
    List,
}

#[derive(Subcommand)]
enum ExcalidrawCmd {
    /// Ensure https://excalidraw.com is open (creates tab if needed)
    Open { #[arg(default_value="https://excalidraw.com/")] url: String },
    /// Get current scene (elements + appState)
    GetScene,
    /// Clear canvas (remove all elements)
    Clear,
    /// Update scene: JSON {elements:[...], mode: append|replace, commitToHistory:bool}
    UpdateScene { json: String },
    /// Draw single primitive: type rectangle|ellipse|diamond|arrow|line|text|freedraw|frame|stickynote — JSON opts {x,y,width,height,x2,y2,text,label,strokeColor,backgroundColor,fillStyle,strokeWidth,points,name,children}
    Draw { json: String },
    /// Batch draw: JSON array of primitives [{type,x,y,width,height,...}]
    DrawBatch { json: String },
    /// High-level diagram: kind flowchart|sequence|microservices|architecture|aws|3tier|network|er|custom — params JSON
    Diagram { kind: String, #[arg(default_value="{}")] params: String },
    /// Export: opts JSON {format: png|svg|clipboard, background:bool, dark:bool, embedScene:bool, scale:1|2|3}
    Export { #[arg(default_value="{\"format\":\"png\"}")] opts: String },
    /// Save scene as .excalidraw JSON to path (triggers download)
    Save { #[arg(default_value="/tmp/excalidraw-scene.excalidraw")] path: String },
    /// Viewport: get or set {scrollX,scrollY,zoom:{value},viewBackgroundColor}
    View { #[arg(default_value="")] json: String },
    /// Center viewport on content (zoom to fit)
    Fit,
}

#[derive(Subcommand)]
enum DeciderCmd {
    /// Unified Decider-2B: single or multiple questions, options, image (file/b64/screenshot), context (covers decide, batch, choose, classify, detect)
    Decide {
        /// Single question or query (positional)
        #[arg(value_name = "QUESTION")]
        question: Option<String>,
        /// Alternative explicit -q/--question flag
        #[arg(long, short = 'q')]
        q: Option<String>,
        /// Options: comma-separated ("A, B, C") or JSON array ('["A", "B"]')
        #[arg(long)]
        options: Option<String>,
        /// Multiple questions: JSON array of objects ([{"question": "...", "options": [...]}, ...])
        #[arg(long)]
        questions: Option<String>,
        /// Context or state description
        #[arg(long)]
        context: Option<String>,
        /// Image: file path (/tmp/shot.png), base64 string, or data URI
        #[arg(long)]
        image: Option<String>,
        /// Auto-capture screenshot of desktop or browser
        #[arg(long, default_value_t = false)]
        screenshot: bool,
        /// Target browser tab index/name or window
        #[arg(long)]
        target: Option<String>,
        /// Sampling temperature
        #[arg(long)]
        temperature: Option<f32>,
    },
    /// Multiple questions same context/screenshot (or requests array for concurrent batch)
    Batch { #[arg(long)] context: Option<String>, #[arg(long)] questions: Option<String>, #[arg(long)] requests: Option<String>, #[arg(long)] image: Option<String> },
    /// Semantic target resolver: query -> DOM/AX/hints -> candidate filtering -> Decider if ambiguous
    Find { query: String, #[arg(long)] target: Option<String>, #[arg(long)] use_vision: bool, #[arg(long)] image: Option<String> },
    /// Choose: question + options[] up to 255, numeric IDs internally when visual candidates
    Choose { question: String, #[arg(long)] options: String, #[arg(long)] context: Option<String>, #[arg(long)] image: Option<String>, #[arg(long)] target: Option<String>, #[arg(long)] use_vision: bool },
    /// State classification from explicit options, image optional
    Classify { question: String, #[arg(long)] options: String, #[arg(long)] image: Option<String>, #[arg(long)] context: Option<String> },
    /// Presence/absence YES/NO/UNCERTAIN
    Detect { query: String, #[arg(long)] context: Option<String>, #[arg(long)] image: Option<String>, #[arg(long)] use_vision: bool },
    /// Which candidate/entity
    Identify { query: String, #[arg(long)] candidates: Option<String>, #[arg(long)] target: Option<String>, #[arg(long)] use_vision: bool },
    /// Visual target: description+image+candidate rects/metadata -> selected candidate ID/confidence/probs/rect
    VisualTarget { description: String, #[arg(long)] candidates: String, #[arg(long)] image: Option<String>, #[arg(long)] target: Option<String> },
    /// Verify (DOM first, Decider visual only when necessary)
    Verify { query: String },
    /// Verify element (candidate)
    VerifyElement { #[arg(long)] candidate: Option<String>, #[arg(long)] selector: Option<String> },
    /// Verify action succeeded
    VerifyAction { query: String, #[arg(long)] expected: Option<String> },
    /// Wait until predicate via DOM/AX polling + visual fallback
    WaitUntil { query: String, #[arg(long, default_value="5000")] timeout_ms: u64, #[arg(long, default_value="200")] interval_ms: u64 },
    /// Observe state: classify current UI state from explicit options
    ObserveState { query: String, #[arg(long)] options: Option<String>, #[arg(long)] image: Option<String> },
    /// Hint resolve: hint_snapshot -> Decider -> hint_click (single instruction)
    HintResolve { instruction: String, #[arg(long)] target: Option<String>, #[arg(long)] vision: bool },
    /// Hint resolve batch: multiple instructions same snapshot/screenshot
    HintResolveBatch { #[arg(long)] instructions: String, #[arg(long)] target: Option<String>, #[arg(long)] vision: bool },
    /// Key identify for visual/virtual keyboards
    KeyIdentify { key: String, #[arg(long)] target: Option<String>, #[arg(long)] rect: Option<String>, #[arg(long)] vision: bool },
    /// Composite: find + click
    FindAndClick { query: String, #[arg(long)] target: Option<String>, #[arg(long)] use_vision: bool },
    /// Composite: find + type
    FindAndType { query: String, text: String, #[arg(long)] target: Option<String>, #[arg(long)] use_vision: bool },
}

#[derive(Subcommand)]
enum Commands {
    /// Instant desktop snapshot (no screenshot, <5ms) — windows, workspaces, monitors
    Desktop,
    /// Hyprland IPC: workspace/focus_window/move_window/close_window/fullscreen/toggle_floating
    Hypr { #[arg(help="Action: workspace|focus_window|move_window|close_window|fullscreen|toggle_floating")] action: String, #[arg(default_value="", help="Target window address or workspace name")] target: String, #[arg(default_value="", help="Workspace name (for move_window)")] workspace: String },
    /// Launch app via Hyprland exec (auto-adds --remote-debugging-port=9222 for browsers)
    Launch { #[arg(help="Command to launch, e.g. 'brave --new-window https://example.com'")] command: String, #[arg(long, help="Hyprland workspace to launch on")] workspace: Option<String> },
    /// AT-SPI accessible tree (fast, no screenshot) — list elements by window/name filter
    Ui { #[arg(long, help="Filter by window address or title substring")] window: Option<String>, #[arg(long, default_value="", help="Filter by accessible name substring (empty = all)")] name: String },
    /// Click AT-SPI element by accessible name via DoAction (no pointer, no screenshot)
    Click { #[arg(help="Accessible name to click")] name: String, #[arg(long, help="Window address/title to scope search")] window: Option<String> },
    /// Mouse: move|click|drag|scroll at global logical coords
    Pointer { #[arg(help="Action: move|click|drag|scroll")] action: String, #[arg(long, help="X coordinate")] x: Option<f64>, #[arg(long, help="Y coordinate")] y: Option<f64>, #[arg(long, default_value="left", help="Mouse button: left|right|middle")] button: String, #[arg(long, help="Destination X (for drag)")] to_x: Option<f64>, #[arg(long, help="Destination Y (for drag)")] to_y: Option<f64>, #[arg(long, default_value="0", help="Scroll delta Y (for scroll)")] dy: f64, #[arg(long, default_value="0", help="Scroll delta X (for scroll)")] dx: f64 },
    /// Keyboard: type (text) or key (combo like ctrl+t) — optionally focuses window first
    Keyboard { #[arg(help="Action: type|key")] action: String, #[arg(long, default_value="", help="Text to type (for action=type)")] text: String, #[arg(long, default_value="", help="Key combo like ctrl+t, Enter, Escape (for action=key)")] keys: String, #[arg(long, help="Window address to focus first")] window: Option<String> },
    /// Capture via grim: window, region, or monitor. Returns file path + meta (auto-tracked for session clear)
    Screenshot { #[arg(long, help="Window address/title to capture")] window: Option<String>, #[arg(long, help="Region WxH+X+Y or monitor name")] region: Option<String> },
    /// Block on Hyprland events: window_open/window_close/workspace/title_change/layer_open/layer_close
    Wait { #[arg(help="Event: window_open|window_close|workspace|title_change|layer_open|layer_close")] event: String, #[arg(long, default_value="", help="Substring to match on window title/workspace")] match_str: String, #[arg(long, default_value="5", help="Timeout seconds")] timeout: f64 },
    /// List Hyprland keybinds
    Binds,
    /// (legacy) Hypr IPC daemon — start/stop persistent helper (see browser-runtime for CDP daemon)
    Daemon { #[arg(long, help="Stop daemon and remove socket")] stop: bool },
    /// Clear tracked screenshots (/tmp/hyprfast-*.png) — deletes session files; --all for untracked leftovers
    Clear { #[arg(long, help="Also delete untracked /tmp/hyprfast-*.png leftovers")] all: bool },
    /// Screenshot session: status/clear/list tracked files
    Session { #[arg(help="Action: status|clear|clear_all|list")] action: String },
    /// Task state: init/add/update/status/next/clear — multi-step todo tracking
    Task { #[command(subcommand)] cmd: TaskCmd },
    /// CDP browser automation: navigate/click/type/eval/tabs/snapshot (via persistent BrowserRuntime)
    Browser { #[command(subcommand)] cmd: BrowserCmd },
    /// Persistent browser-runtime daemon (single CDP WS) — start/stop/status
    BrowserRuntime { #[command(subcommand)] cmd: BrowserRuntimeCmd },
    /// Internal: daemon serve loop (spawned detached by `browser-runtime start`)
    #[command(hide = true)]
    BrowserRuntimeInternalServe,
    /// Fast visual grounding: screenshot + Gemini Flash -> {x,y}
    Ground { instruction: String, #[arg(long)] window: Option<String>, #[arg(long)] region: Option<String> },
    /// Fused ground+click/type in one call (Astra-like, no N LLM turns)
    ActFast { instruction: String, #[arg(long, default_value="click")] action: String, #[arg(long, default_value="")] text: String, #[arg(long)] window: Option<String> },
    /// Batch fused steps: JSON array [{instruction,action,text}]
    ActBatch { steps: String, #[arg(long)] window: Option<String> },
    /// Hint-key overlay: scan DOM for clickable elements and show labels (add --target to pick tab without focusing)
    HintSnapshot { #[arg(long, help="Target tab: 1-based index, targetId, or url/title substring (e.g. '1', 'excalidraw', 'google.com')")] target: Option<String> },
    /// Click element by hint label (e.g. hyprfast hint-click A)
    HintClick { label: String, #[arg(long, help="Target tab: 1-based index, targetId, or url/title substring")] target: Option<String> },
    /// Type text into element by hint label (e.g. hyprfast hint-type A "hello")
    HintType { label: String, text: String, #[arg(long, help="Target tab: 1-based index, targetId, or url/title substring")] target: Option<String> },
    /// Vimium-primary: snapshot+resolve+click/type in one call (heuristic→LLM, vision last resort)
    HintAct { instruction: String, #[arg(long, default_value="click")] action: String, #[arg(long, default_value="")] text: String, #[arg(long, help="Target tab: 1-based index, targetId, or url/title substring")] target: Option<String> },
    /// Vimium-primary parallel batch: one snapshot + batched LLM + parallel dispatches
    HintBatch { steps: String, #[arg(long, help="Target tab: 1-based index, targetId, or url/title substring")] target: Option<String> },
    /// Clear hint overlay
    HintClear { #[arg(long, help="Target tab: 1-based index, targetId, or url/title substring")] target: Option<String> },
    /// Excalidraw automation: open/scene/draw/diagram/export/view/fit
    Excalidraw { #[command(subcommand)] cmd: ExcalidrawCmd },
    /// Semantic perception + Decider-2B (vision 10, text 255, reuse perception resolver)
    Decider { #[command(subcommand)] cmd: DeciderCmd },
    /// Unified Decider-2B: single or multiple questions, options, image (file/b64/screenshot), context (covers decide, batch, choose, classify, detect)
    Decide {
        /// Single question or query (positional)
        #[arg(value_name = "QUESTION")]
        question: Option<String>,
        /// Alternative explicit -q/--question flag
        #[arg(long, short = 'q')]
        q: Option<String>,
        /// Options: comma-separated ("A, B, C") or JSON array ('["A", "B"]')
        #[arg(long)]
        options: Option<String>,
        /// Multiple questions: JSON array of objects ([{"question": "...", "options": [...]}, ...])
        #[arg(long)]
        questions: Option<String>,
        /// Context or state description
        #[arg(long)]
        context: Option<String>,
        /// Image: file path (/tmp/shot.png), base64 string, or data URI
        #[arg(long)]
        image: Option<String>,
        /// Auto-capture screenshot of desktop or browser
        #[arg(long, default_value_t = false)]
        screenshot: bool,
        /// Target browser tab index/name or window
        #[arg(long)]
        target: Option<String>,
        /// Sampling temperature
        #[arg(long)]
        temperature: Option<f32>,
    },
    /// Laya Pass-1: list 8 tool categories (category_name + description)
    Categories,
    /// Laya Pass-2: show category + its commands with descriptions
    Category { #[arg(help="Category name: core-desktop|perceive|native-act|browser-act|fast-ground|task-memory|draw|perception")] category: String },
    /// Run as MCP server (stdio JSON-RPC) — exposes all tools to LLM clients (default when no command given)
    Mcp,
}

fn ensure_browser_args(cmd: &str) -> String {
    // Auto-inject --remote-debugging-port=9222 if launching brave/chromium and missing
    if (cmd.contains("brave") || cmd.contains("chromium") || cmd.contains("google-chrome") || cmd.contains("chrome")) && !cmd.contains("remote-debugging-port") {
        // insert after binary
        if cmd.starts_with("brave ") { return cmd.replacen("brave ", "brave --remote-debugging-port=9222 --force-renderer-accessibility ", 1); }
        if cmd.starts_with("chromium ") { return cmd.replacen("chromium ", "chromium --remote-debugging-port=9222 --force-renderer-accessibility ", 1); }
        // fallback: append
        return format!("{} --remote-debugging-port=9222", cmd);
    }
    cmd.to_string()
}

// Chromium is single-instance per profile: launching against the default profile
// forwards the URL to the running browser and exits, so the debugging port never
// opens. Always use an isolated --user-data-dir and verify readiness by polling.

// Mirrors the isVisible() rule in assets/hint.js: an element counts as actionable
// when it is laid out, styled visible and inside the viewport. `load`/readyState is
// not a usable gate — a cold YouTube profile flips readyState to "complete" seconds
// before it paints anything clickable.
const PAGE_INTERACTIVE_JS: &str = r#"(function(){
  var sel = 'a, button, input, select, textarea, [role="button"], [role="link"], [role="textbox"], [role="combobox"], [role="checkbox"], [role="radio"], [role="tab"], [role="menuitem"], [onclick], [contenteditable], [draggable="true"], summary, [tabindex]:not([tabindex="-1"])';
  var els = document.querySelectorAll(sel);
  for (var i = 0; i < els.length; i++) {
    var el = els[i], r = el.getBoundingClientRect();
    if (r.width <= 0 || r.height <= 0) continue;
    var s = getComputedStyle(el);
    if (s.visibility === 'hidden' || s.display === 'none' || s.opacity === '0' || s.pointerEvents === 'none') continue;
    if (r.bottom < 0 || r.right < 0 || r.top > window.innerHeight || r.left > window.innerWidth) continue;
    if (el.hidden) continue;
    return true;
  }
  return false;
})()"#;

fn env_millis(var_name: &str, default_ms: u64) -> u64 {
    std::env::var(var_name).ok().and_then(|v| v.parse().ok()).unwrap_or(default_ms)
}

/// Bounded poll for "the page a user just asked us to open is actionable".
/// Requires a run of consecutive hits because heavy pages (YouTube boots, then
/// reloads itself with `&themeRefresh=1`) go interactive and blank out again a
/// moment later; returning on the first hit hands the caller a half-loaded DOM.
/// Returns elapsed ms; a page that never settles is reported, not fatal.
fn wait_page_interactive(budget_ms: u64) -> (bool, u64) {
    const STABLE_POLLS: u32 = 3;
    let started = std::time::Instant::now();
    let deadline = started + std::time::Duration::from_millis(budget_ms);
    let mut streak = 0;
    loop {
        if cdp::evaluate(PAGE_INTERACTIVE_JS, false).ok().and_then(|v| v.as_bool()) == Some(true) {
            streak += 1;
            if streak >= STABLE_POLLS { return (true, started.elapsed().as_millis() as u64); }
        } else {
            streak = 0;
        }
        if std::time::Instant::now() >= deadline { return (false, started.elapsed().as_millis() as u64); }
        std::thread::sleep(std::time::Duration::from_millis(400));
    }
}

fn browser_open_url(url: &str, workspace: &str) -> Result<Value> {
    if url.is_empty() { anyhow::bail!("browser_open needs url"); }
    if cdp::probe() {
        let navigated = browser::navigate(url, None).is_ok();
        let (interactive, waited_ms) = wait_page_interactive(env_millis("HYPRFAST_PAGE_READY_TIMEOUT_MS", 2500));
        return Ok(serde_json::json!({
            "launched": url,
            "browser": "reused",
            "cdp_base_url": cdp::base_url(),
            "navigated": navigated,
            "interactive": interactive,
            "page_ready_ms": waited_ms,
            "cdp": cdp::endpoint_version().unwrap_or(serde_json::json!({})),
            "target": open_target(url),
        }));
    }
    let port = cdp::port();
    let profile = std::env::temp_dir().join(format!("hyprfast-brave-{}", port));
    let _ = std::fs::create_dir_all(&profile);
    let cmd = format!(
        "brave --remote-debugging-port={} --force-renderer-accessibility --remote-allow-origins=* --no-first-run --no-default-browser-check --user-data-dir={} --new-window {}",
        port, profile.display(), url
    );
    let rule = if workspace.is_empty() { String::new() } else { format!("[workspace {} silent] ", workspace) };
    let started = std::time::Instant::now();
    hypr::dispatch("exec", &format!("{}{}", rule, cmd))?;
    let timeout_ms = env_millis("HYPRFAST_CDP_READY_TIMEOUT_MS", 8000);
    let deadline = started + std::time::Duration::from_millis(timeout_ms);
    let mut ready = false;
    while std::time::Instant::now() < deadline {
        if cdp::probe() { ready = true; break; }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    if !ready {
        anyhow::bail!(
            "browser_open: no CDP endpoint at {}/json/version {}ms after launching brave with --user-data-dir={} (port {}). \
             A Brave already running on the default profile swallows the launch request unless it was started with --remote-debugging-port; \
             retry, or launch it yourself with the same isolated profile.",
            cdp::base_url(), timeout_ms, profile.display(), port
        );
    }
    let cdp_ready_ms = started.elapsed().as_millis() as u64;
    let (interactive, page_ms) = wait_page_interactive(env_millis("HYPRFAST_PAGE_READY_TIMEOUT_MS", 20000));
    Ok(serde_json::json!({
        "launched": url,
        "browser": "launched",
        "cdp_base_url": cdp::base_url(),
        "port": port,
        "user_data_dir": profile.display().to_string(),
        "cdp_ready_ms": cdp_ready_ms,
        "interactive": interactive,
        "page_ready_ms": page_ms,
        "cdp": cdp::endpoint_version().unwrap_or(serde_json::json!({})),
        "target": open_target(url),
    }))
}

fn open_target(url: &str) -> Value {
    let pages = match cdp::targets_http() {
        Ok(p) => p,
        Err(_) => return Value::Null,
    };
    pages.iter().find(|t| t.typ == "page" && t.url.starts_with(url))
        .or_else(|| pages.iter().rev().find(|t| t.typ == "page" && t.url != "about:blank"))
        .or_else(|| pages.iter().rev().find(|t| t.typ == "page"))
        .map(|t| serde_json::json!({"id": t.id, "url": t.url, "title": t.title}))
        .unwrap_or(Value::Null)
}

// --- Laya 2-pass helpers (README-laya.md) ---

fn laya_categories_value() -> serde_json::Value {
    serde_json::json!({
        "total": 8,
        "categories": [
            {"category_name": "core-desktop", "description": "Hyprland desktop — snapshot, hypr, launch, binds, wait_for"},
            {"category_name": "perceive", "description": "Perceive — read-only: ui, screenshot, browser_snapshot, tabs, console, session_status"},
            {"category_name": "native-act", "description": "Native act — click_ui, pointer, keyboard"},
            {"category_name": "browser-act", "description": "Browser act — deterministic CDP: navigate, click, type, eval, etc."},
            {"category_name": "fast-ground", "description": "Fast ground — hint_* + ground/act_fast (vision fallback for canvas/WebGL)"},
            {"category_name": "task-memory", "description": "Task memory — task_* + clear_screenshots"},
            {"category_name": "draw", "description": "Draw — excalidraw whiteboard / architecture diagrams"},
            {"category_name": "perception", "description": "Perception — Decider-2B semantic: decide/decider_batch/find/choose/classify/detect/identify/visual_target/verify/* + hint_resolve (vision 10, text 255)"}
        ]
    })
}

fn laya_category_value(name: &str) -> anyhow::Result<serde_json::Value> {
    let key = name.trim().to_lowercase().replace('_', "-");
    let v = match key.as_str() {
        "core-desktop" | "core" | "desktop" => serde_json::json!({
            "category_name": "core-desktop",
            "description": "Hyprland desktop — snapshot, hypr, launch, binds, wait_for",
            "total_commands": 5,
            "commands": [
                {"command_name": "desktop", "description": "Instant desktop snapshot (no screenshot, <5ms)"},
                {"command_name": "hypr", "description": "Window/workspace ops: workspace/focus_window/move_window/close_window/fullscreen/toggle_floating"},
                {"command_name": "launch", "description": "Launch app via Hyprland exec (auto-adds --remote-debugging-port=9222 for browsers)"},
                {"command_name": "binds", "description": "List Hyprland keybinds"},
                {"command_name": "wait_for", "description": "Block on Hyprland events: window_open/window_close/workspace/title_change/layer_open/layer_close"}
            ]
        }),
        "perceive" => serde_json::json!({
            "category_name": "perceive",
            "description": "Perceive — read-only: ui, screenshot, browser_snapshot, tabs, console, session_status",
            "total_commands": 6,
            "commands": [
                {"command_name": "ui", "description": "AT-SPI accessible tree (fast, no screenshot)"},
                {"command_name": "screenshot", "description": "Capture via grim: window, region, or monitor. Returns file path + meta"},
                {"command_name": "browser_snapshot", "description": "CDP: capture accessibility snapshot (AX tree via Accessibility.getFullAXTree)"},
                {"command_name": "browser_tabs", "description": "CDP: list browser tabs/targets (GET /json)"},
                {"command_name": "browser_console", "description": "CDP: get console logs (Console.enable)"},
                {"command_name": "session_status", "description": "Show screenshot session status (tracked files, bytes)"}
            ]
        }),
        "native-act" | "native" => serde_json::json!({
            "category_name": "native-act",
            "description": "Native act — click_ui, pointer, keyboard",
            "total_commands": 3,
            "commands": [
                {"command_name": "click_ui", "description": "Click by accessible name via DoAction (no pointer, no screenshot)"},
                {"command_name": "pointer", "description": "Mouse: move|click|drag|scroll at global logical coords"},
                {"command_name": "keyboard", "description": "Keyboard: type (text) or key (combo like ctrl+t). window focuses first."}
            ]
        }),
        "browser-act" | "browser" => serde_json::json!({
            "category_name": "browser-act",
            "description": "Browser act — deterministic CDP: navigate, click, type, eval, etc.",
            "total_commands": 13,
            "commands": [
                {"command_name": "browser_navigate", "description": "CDP: navigate browser tab to URL (auto-discovers ws://9222)"},
                {"command_name": "browser_open", "description": "Hypr+CDP: launch Brave with --remote-debugging-port=9222"},
                {"command_name": "browser_go_back", "description": "CDP: go back (history.back)"},
                {"command_name": "browser_go_forward", "description": "CDP: go forward (history.forward)"},
                {"command_name": "browser_click", "description": "CDP: click element. Use ref from snapshot or CSS selector"},
                {"command_name": "browser_hover", "description": "CDP: hover element"},
                {"command_name": "browser_type", "description": "CDP: type text into editable element (ref from snapshot)"},
                {"command_name": "browser_select_option", "description": "CDP: select option in dropdown"},
                {"command_name": "browser_press_key", "description": "CDP: press key (Enter, Escape, ArrowLeft, etc) via Input.dispatchKeyEvent"},
                {"command_name": "browser_wait", "description": "CDP: wait N seconds (browser)"},
                {"command_name": "browser_evaluate", "description": "CDP: evaluate JavaScript in page (Runtime.evaluate)"},
                {"command_name": "browser_screenshot", "description": "CDP: capture browser tab screenshot via Page.captureScreenshot (PNG, no grim)"},
                {"command_name": "browser_execute_plan", "description": "Structured execution plan: navigate→click/type/select/press/hover/wait/eval/extract"}
            ]
        }),
        "fast-ground" | "fast" | "ground" | "hint" => serde_json::json!({
            "category_name": "fast-ground",
            "description": "Fast ground — hint_* + ground/act_fast (vision fallback for canvas/WebGL) — now targetable: --target 1|url|title",
            "total_commands": 9,
            "commands": [
                {"command_name": "hint_snapshot", "description": "Hint-key overlay: scan DOM for clickable elements and show labels — add --target 1|excalidraw|targetId to pick tab without focusing"},
                {"command_name": "hint_click", "description": "Click element by hint label (e.g. hyprfast hint-click A) — add --target to pick tab"},
                {"command_name": "hint_type", "description": "Type text into element by hint label — add --target to pick tab"},
                {"command_name": "hint_act", "description": "Vimium-primary: snapshot+resolve+click/type in one call (heuristic→LLM, vision last resort) — add --target"},
                {"command_name": "hint_batch", "description": "Vimium-primary parallel batch: one snapshot + batched LLM + parallel dispatches — add --target"},
                {"command_name": "hint_clear", "description": "Clear hint overlay — add --target to clear specific tab"},
                {"command_name": "ground", "description": "Fast visual grounding: screenshot + Gemini Flash -> {x,y}"},
                {"command_name": "act_fast", "description": "Fused ground+click/type in one call (Astra-like, no N LLM turns)"},
                {"command_name": "act_batch", "description": "Batch fused steps: JSON array [{instruction,action,text}]"}
            ]
        }),
        "task-memory" | "task" | "memory" => serde_json::json!({
            "category_name": "task-memory",
            "description": "Task memory — task_* + clear_screenshots",
            "total_commands": 7,
            "commands": [
                {"command_name": "task_init", "description": "Task state: init todo list for multi-step action"},
                {"command_name": "task_status", "description": "Task state: show current todo list, progress % and next pending step"},
                {"command_name": "task_update", "description": "Task state: update step status (pending|in_progress|completed|failed|skipped)"},
                {"command_name": "task_next", "description": "Task state: get next pending step"},
                {"command_name": "task_add", "description": "Task state: add a new step to current task list"},
                {"command_name": "task_clear", "description": "Task state: manually clear current task list"},
                {"command_name": "clear_screenshots", "description": "Clear tracked screenshots (/tmp/hyprfast-*.png) — use all=true for leftovers"}
            ]
        }),
        "draw" | "excalidraw" => serde_json::json!({
            "category_name": "draw",
            "description": "Draw — excalidraw whiteboard / architecture diagrams",
            "total_commands": 11,
            "commands": [
                {"command_name": "excalidraw_open", "description": "Excalidraw: ensure https://excalidraw.com is open"},
                {"command_name": "excalidraw_get_scene", "description": "Excalidraw: get current scene elements + appState (counts, bbox)"},
                {"command_name": "excalidraw_clear", "description": "Excalidraw: clear canvas (remove all elements)"},
                {"command_name": "excalidraw_draw", "description": "Excalidraw lightning draw single primitive: {type: rectangle|ellipse|...}"},
                {"command_name": "excalidraw_draw_batch", "description": "Excalidraw lightning batch draw: array of primitives"},
                {"command_name": "excalidraw_update_scene", "description": "Excalidraw: update scene elements directly — {elements:[...], mode: append|replace}"},
                {"command_name": "excalidraw_diagram", "description": "Excalidraw lightning diagrams: kind flowchart|sequence|microservices|..."},
                {"command_name": "excalidraw_export", "description": "Excalidraw export: {format: png|svg|clipboard, background...}"},
                {"command_name": "excalidraw_save", "description": "Excalidraw: trigger Save to file (.excalidraw JSON)"},
                {"command_name": "excalidraw_view", "description": "Excalidraw viewport: get or set {scrollX,scrollY,zoom:{value}...}"},
                {"command_name": "excalidraw_fit", "description": "Excalidraw: center viewport on content (zoom to fit)"}
            ]
        }),
        "perception" | "semantic" | "decider" => serde_json::json!({
            "category_name": "perception",
            "description": "Perception — Decider-2B semantic: decide/decider_batch/find/choose/classify/detect/identify/visual_target/verify/* + hint_resolve (vision 10, text 255)",
            "total_commands": 20,
            "commands": [
                {"command_name": "decide", "description": "Unified Decider-2B: single/multiple questions, options, image (file/b64/screenshot), context (covers decide, batch, choose, classify, detect)"},
                {"command_name": "decider_batch", "description": "Multiple questions same context/screenshot (or requests[] for concurrent batch, bounded 4)"},
                {"command_name": "find", "description": "Semantic target resolver: query -> DOM/AX/hints -> candidate filtering -> Decider if ambiguous -> resolved candidate metadata"},
                {"command_name": "choose", "description": "Choose: question+options[] up to 255, numeric IDs internally when visual candidates, return selected + confidence + probs + runner_up/margin"},
                {"command_name": "classify", "description": "State classification from explicit options, image optional"},
                {"command_name": "detect", "description": "Presence/absence YES/NO/UNCERTAIN"},
                {"command_name": "identify", "description": "Which candidate/entity — identify among candidates"},
                {"command_name": "visual_target", "description": "Visual target: description+image+candidate rects/metadata -> selected candidate ID/confidence/probs/rect (vision 10 budget)"},
                {"command_name": "verify", "description": "Verify (DOM first, Decider visual only when necessary) -> success/failure/uncertain + confidence"},
                {"command_name": "verify_element", "description": "Verify element (candidate/selector) present/visible"},
                {"command_name": "verify_action", "description": "Verify action succeeded (query + expected text)"},
                {"command_name": "wait_until", "description": "Wait until predicate via DOM/AX polling + visual fallback, timeout/interval"},
                {"command_name": "observe_state", "description": "Observe state: classify current UI state from explicit options"},
                {"command_name": "hint_resolve", "description": "Hint resolve: hint_snapshot -> Decider -> hint_click (single instruction, target aware, fallback deterministic)"},
                {"command_name": "hint_resolve_batch", "description": "Hint resolve batch: multiple instructions same snapshot/screenshot, batched Decider (max 12)"},
                {"command_name": "key_identify", "description": "Key identify for visual/virtual keyboards (same pipeline, keyboard candidates)"},
                {"command_name": "find_and_click", "description": "Composite: find + click (semantic find then hint/browser click)"},
                {"command_name": "find_and_type", "description": "Composite: find + type (semantic find then hint/browser type)"},
                {"command_name": "decider_metrics", "description": "Decider metrics snapshot"},
                {"command_name": "decider_health", "description": "Decider health check"}
            ]
        }),
        _ => anyhow::bail!("unknown category '{key}': use one of core-desktop|perceive|native-act|browser-act|fast-ground|task-memory|draw|perception (see `hyprfast categories`)"),
    };
    Ok(v)
}

fn build_decide_args(
    question: Option<String>,
    q_flag: Option<String>,
    options: Option<String>,
    questions: Option<String>,
    context: Option<String>,
    image: Option<String>,
    screenshot: bool,
    target: Option<String>,
    temperature: Option<f32>,
) -> Value {
    let mut args = serde_json::json!({});
    if let Some(c) = context { args["context"] = Value::String(c); }
    if let Some(img) = image { args["image"] = Value::String(img); }
    if screenshot { args["screenshot"] = Value::Bool(true); }
    if let Some(t) = target { args["target"] = Value::String(t); }
    if let Some(temp) = temperature { args["temperature"] = serde_json::json!(temp); }

    let q_effective = question.or(q_flag);

    if let Some(qs_str) = questions {
        let trimmed = qs_str.trim();
        if let Ok(v) = serde_json::from_str::<Value>(trimmed) {
            args["questions"] = v;
        } else {
            args["questions"] = Value::String(qs_str);
        }
    }

    if let Some(q) = q_effective {
        args["question"] = Value::String(q);
    }

    if let Some(opts) = options {
        let trimmed = opts.trim();
        if let Ok(v) = serde_json::from_str::<Value>(trimmed) {
            args["options"] = v;
        } else {
            args["options"] = Value::String(opts);
        }
    }

    args
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Some(Commands::Desktop) => { println!("{}", serde_json::to_string_pretty(&hypr::snapshot()?)?); }
        Some(Commands::Hypr { action, target, workspace }) => {
            let ws = if workspace.is_empty() && !target.is_empty() && action=="workspace" { target.clone() } else { workspace.clone() };
            let (d, arg) = match action.as_str() {
                "workspace" => ("workspace", ws),
                "focus" | "focus_window" => ("focuswindow", format!("address:{}", target)),
                "move" | "move_window" => ("movetoworkspacesilent", format!("{},address:{}", workspace, target)),
                "close" | "close_window" => ("closewindow", format!("address:{}", target)),
                "fullscreen" => ("fullscreen", "0".to_string()),
                "toggle_floating" => ("togglefloating", if target.is_empty() { "".into() } else { format!("address:{}", target)}),
                _ => (action.as_str(), target),
            };
            let out = hypr::dispatch(d, &arg)?;
            println!("{} -> {}", d, out);
        }
        Some(Commands::Launch { command, workspace }) => {
            let cmd = ensure_browser_args(&command);
            let rule = workspace.map(|w| format!("[workspace {} silent] ", w)).unwrap_or_default();
            hypr::dispatch("exec", &format!("{}{}", rule, cmd))?;
            std::thread::sleep(std::time::Duration::from_millis(600));
            // try to show cdp status hint
            if cmd.contains("remote-debugging-port") {
                match cdp::version() {
                    Ok(v) => eprintln!("CDP ready: {}", v.get("Browser").and_then(|x| x.as_str()).unwrap_or("ok")),
                    Err(e) => eprintln!("CDP not yet ready (browser starting): {}", e),
                }
            }
            println!("{}", serde_json::to_string_pretty(&hypr::snapshot()?)?);
        }
        Some(Commands::Ui { window, name }) => {
            let v = a11y::list_elements(&window.unwrap_or_default(), &name)?;
            println!("{}", serde_json::to_string_pretty(&v)?);
        }
        Some(Commands::Click { name, window }) => {
            let v = a11y::click_by_name(&window.unwrap_or_default(), &name)?;
            println!("{}", serde_json::to_string_pretty(&v)?);
        }
        Some(Commands::Pointer { action, x, y, button, to_x, to_y, dy, dx }) => {
            match action.as_str() {
                "move" => {
                    let (xx, yy) = (x.ok_or_else(|| anyhow::anyhow!("pointer move requires --x and --y, e.g. hyprfast pointer move --x 100 --y 100"))?, y.ok_or_else(|| anyhow::anyhow!("pointer move requires --x and --y"))?);
                    input::move_cursor(xx, yy)?; println!("moved");
                }
                "click" => { input::click(x, y, &button, false)?; println!("clicked"); }
                "drag" => {
                    let (xx, yy, txx, tyy) = (x.ok_or_else(|| anyhow::anyhow!("pointer drag requires --x --y --to-x --to-y"))?, y.ok_or_else(|| anyhow::anyhow!("pointer drag requires --x --y --to-x --to-y"))?, to_x.ok_or_else(|| anyhow::anyhow!("pointer drag requires --to-x"))?, to_y.ok_or_else(|| anyhow::anyhow!("pointer drag requires --to-y"))?);
                    input::drag(xx, yy, txx, tyy, &button)?; println!("dragged");
                }
                "scroll" => { input::scroll(dy, dx, x, y)?; println!("scrolled"); }
                _ => anyhow::bail!("unknown pointer action '{}': use move|click|drag|scroll (see --help)", action),
            }
        }
        Some(Commands::Keyboard { action, text, keys, window }) => {
            if let Some(w)=window { if !w.is_empty() { hypr::dispatch("focuswindow", &format!("address:{}", w))?; std::thread::sleep(std::time::Duration::from_millis(50)); } }
            match action.as_str() {
                "type" => input::type_text(&text)?,
                "key" => input::key_combo(&keys)?,
                _ => anyhow::bail!("unknown keyboard action"),
            }
            println!("{} ok", action);
        }
        Some(Commands::Screenshot { window, region }) => {
            let (data, meta) = screenshot::capture(&window.unwrap_or_default(), &region.unwrap_or_default(), 0.0)?;
            let path = format!("/tmp/hyprfast-{}.png", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis());
            std::fs::write(&path, &data)?;
            let _ = session::record(&path);
            println!("saved {} {:?}", path, meta);
        }
        Some(Commands::Wait { event, match_str, timeout }) => {
            let v = events::wait_for(&event, &match_str, timeout)?;
            println!("{}", serde_json::to_string_pretty(&v)?);
        }
        Some(Commands::Binds) => {
            println!("{}", serde_json::to_string_pretty(&hypr::binds()?)?);
        }
        Some(Commands::Daemon { stop }) => {
            if stop {
                let p = daemon::daemon_path();
                if p.exists() { std::fs::remove_file(&p)?; println!("stopped daemon {}", p.display()); } else { println!("daemon not running"); }
            } else {
                println!("starting hyprfastd on {}", daemon::daemon_path().display());
                let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
                rt.block_on(daemon::run_daemon())?;
            }
        }
        Some(Commands::Clear { all }) => {
            let v = if all { session::clear_all()? } else { session::clear()? };
            println!("{}", serde_json::to_string_pretty(&v)?);
        }
        Some(Commands::Session { action }) => {
            match action.as_str() {
                "status" => println!("{}", serde_json::to_string_pretty(&session::status())?),
                "clear" => println!("{}", serde_json::to_string_pretty(&session::clear()?)?),
                "clear_all" => println!("{}", serde_json::to_string_pretty(&session::clear_all()?)?),
                "list" => println!("{}", serde_json::to_string_pretty(&serde_json::json!({"files": session::list()}))?),
                _ => println!("{}", serde_json::to_string_pretty(&session::status())?),
            }
        }
        Some(Commands::Task { cmd }) => {
            let res = match cmd {
                TaskCmd::Init { goal, steps } => {
                    let parsed = task::parse_steps_arg(&steps);
                    task::init(&goal, parsed)?
                },
                TaskCmd::Add { description } => task::add(&description)?,
                TaskCmd::Update { index, id, status } => task::update(index, id, &status)?,
                TaskCmd::Status => task::status(),
                TaskCmd::Clear => task::clear()?,
                TaskCmd::Next => task::next_pending()?,
                TaskCmd::List => task::status(),
            };
            println!("{}", serde_json::to_string_pretty(&res)?);
        }
        Some(Commands::BrowserRuntime { cmd }) => {
            match cmd {
                BrowserRuntimeCmd::Start => {
                    match browser_runtime::server::start_daemon_detached() {
                        Ok(()) => {
                            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
                            match rt.block_on(browser_runtime::client::status_once()) {
                                Ok(s) => println!("{}", serde_json::to_string_pretty(&s)?),
                                Err(e) => println!("{}", serde_json::to_string_pretty(&serde_json::json!({"started": true, "socket": browser_runtime::server::browser_socket_path(), "status_error": e.to_string()}))?),
                            }
                        }
                        Err(e) => anyhow::bail!("browser-runtime start: {e}"),
                    }
                }
                BrowserRuntimeCmd::Stop => {
                    match browser_runtime::server::stop_daemon_sync() {
                        Ok(()) => println!("{}", serde_json::to_string_pretty(&serde_json::json!({"stopped": true}))?),
                        Err(browser_runtime::error::RuntimeError::RuntimeDead(_)) => println!("daemon not running"),
                        Err(e) => anyhow::bail!("browser-runtime stop: {e}"),
                    }
                }
                BrowserRuntimeCmd::Status => {
                    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
                    match rt.block_on(browser_runtime::client::status_once()) {
                        Ok(s) => println!("{}", serde_json::to_string_pretty(&s)?),
                        Err(e) => anyhow::bail!("browser-runtime status: {e} (is the daemon running? try `hyprfast browser-runtime start`)"),
                    }
                }
            }
        }
        Some(Commands::BrowserRuntimeInternalServe) => {
            let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
            rt.block_on(browser_runtime::server::serve(browser_runtime::server::ServeOptions::defaults()))?;
        }
        Some(Commands::Browser { cmd }) => {
            let res = match cmd {
                BrowserCmd::Navigate { url, target } => browser::navigate(&url, target.as_deref())?,
                BrowserCmd::Back => browser::go_back()?,
                BrowserCmd::Forward => browser::go_forward()?,
                BrowserCmd::Snapshot => browser::snapshot(60)?,
                BrowserCmd::Click { selector, r#ref, element } => {
                    let sel = selector.or(element).unwrap_or_default();
                    let rf = r#ref.unwrap_or_default();
                    let target = if !rf.is_empty() { rf } else { sel };
                    if target.is_empty() { anyhow::bail!("click needs --ref or --selector"); }
                    // Phase 3: route via persistent BrowserRuntime (no per-call WS)
                    if target.chars().all(|c| c.is_ascii_digit()) {
                        browser::click_by_ref(&target, "")?
                    } else {
                        browser::click_by_selector(&target)?
                    }
                },
                BrowserCmd::Hover { selector, r#ref } => {
                    let s = selector.or(r#ref).unwrap_or_default();
                    browser::hover_by_ref(&s, Some(&s))?
                },
                BrowserCmd::Type { text, selector, r#ref, submit } => {
                    let sel = selector.or(r#ref).map(|s| s.clone());
                    browser::type_text(&sel.clone().unwrap_or_default(), &text, submit, sel.as_deref())?
                },
                BrowserCmd::Fill { selector, text } => browser::fill(&selector, &text)?,
                BrowserCmd::Select { selector, r#ref, values } => {
                    let s = selector.or(r#ref).unwrap_or_default();
                    browser::select_option(&s, &values)?
                },
                BrowserCmd::Press { key } => browser::press_key(&key)?,
                BrowserCmd::Eval { js } => {
                    // Phase 2: the daemon owns the persistent CDP connection.
                    // Degraded direct mode only when no daemon is reachable.
                    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
                    match rt.block_on(browser_runtime::client::try_evaluate_via_daemon(&js)) {
                        Some(Ok(v)) => v,
                        Some(Err(e)) => anyhow::bail!("browser-runtime daemon error: {e}"),
                        None => {
                            eprintln!("warning: browser-runtime daemon unavailable; using degraded direct mode (`hyprfast browser-runtime start` for the persistent path)");
                            browser::evaluate_js(&js)?
                        }
                    }
                },
                BrowserCmd::Shot { output } => {
                    let (data, meta) = browser::screenshot_cdp()?;
                    let path = output.unwrap_or_else(|| format!("/tmp/hyprfast-browser-{}.png", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis()));
                    std::fs::write(&path, &data)?;
                    let _ = session::record(&path);
                    serde_json::json!({"path": path, "meta": meta})
                },
                BrowserCmd::Tabs => browser::tabs()?,
                BrowserCmd::Console => browser::console_logs()?,
                BrowserCmd::Wait { secs } => browser::wait(secs)?,
                BrowserCmd::Open { url, workspace } => {
                    browser_open_url(&url, workspace.as_deref().unwrap_or(""))?
                },
            };
            println!("{}", serde_json::to_string_pretty(&res)?);
        }
        Some(Commands::Ground { instruction, window, region }) => {
            let v = ground::ground(&instruction, &window.unwrap_or_default(), &region.unwrap_or_default())?;
            println!("{}", serde_json::to_string_pretty(&v)?);
        }
        Some(Commands::ActFast { instruction, action, text, window }) => {
            let v = ground::act_fast(&instruction, &action, &text, &window.unwrap_or_default())?;
            println!("{}", serde_json::to_string_pretty(&v)?);
        }
        Some(Commands::ActBatch { steps, window }) => {
            let v: Value = serde_json::from_str(&steps).unwrap_or(Value::Null);
            let out = ground::act_batch(&v, &window.unwrap_or_default())?;
            println!("{}", serde_json::to_string_pretty(&out)?);
        }
        Some(Commands::HintSnapshot { target }) => {
            let v = hint::hint_snapshot_with_target(target.as_deref())?;
            println!("{}", serde_json::to_string_pretty(&v)?);
        }
        Some(Commands::HintClick { label, target }) => {
            let v = hint::hint_click_with_target(&label, target.as_deref())?;
            println!("{}", serde_json::to_string_pretty(&v)?);
        }
        Some(Commands::HintType { label, text, target }) => {
            let v = hint::hint_type_with_target(&label, &text, target.as_deref())?;
            println!("{}", serde_json::to_string_pretty(&v)?);
        }
        Some(Commands::HintAct { instruction, action, text, target }) => {
            let v = hint::hint_act_with_target(&instruction, &action, &text, target.as_deref())?;
            println!("{}", serde_json::to_string_pretty(&v)?);
        }
        Some(Commands::HintBatch { steps, target }) => {
            let v: Value = serde_json::from_str(&steps).unwrap_or(Value::Null);
            let arr = if let Some(a) = v.as_array() { a.clone() } else if let Some(o) = v.get("steps").and_then(|x| x.as_array()) { o.clone() } else { vec![v] };
            let out = hint::hint_batch_with_target(&arr.iter().cloned().collect::<Vec<_>>(), target.as_deref())?;
            println!("{}", serde_json::to_string_pretty(&out)?);
        }
        Some(Commands::HintClear { target }) => {
            let v = hint::hint_clear_with_target(target.as_deref())?;
            println!("{}", serde_json::to_string_pretty(&v)?);
        }
        Some(Commands::Excalidraw { cmd }) => {
            let res = match cmd {
                ExcalidrawCmd::Open { url } => excalidraw::ensure_open(Some(&url))?,
                ExcalidrawCmd::GetScene => excalidraw::get_scene()?,
                ExcalidrawCmd::Clear => excalidraw::clear_scene()?,
                ExcalidrawCmd::UpdateScene { json } => {
                    let v: Value = serde_json::from_str(&json).unwrap_or(serde_json::json!({"elements":[]}));
                    let els = v.get("elements").and_then(|x| x.as_array()).cloned().unwrap_or_else(|| v.as_array().cloned().unwrap_or_default());
                    let mode = v.get("mode").cloned().unwrap_or(serde_json::json!("append"));
                    excalidraw::update_scene(els, &serde_json::json!({"mode": mode}))?
                },
                ExcalidrawCmd::Draw { json } => {
                    let v: Value = serde_json::from_str(&json).unwrap_or(Value::Null);
                    excalidraw::draw_primitive(&v)?
                },
                ExcalidrawCmd::DrawBatch { json } => {
                    let v: Value = serde_json::from_str(&json).unwrap_or(Value::Null);
                    let arr = if let Some(a)=v.as_array() { a.clone() } else if let Some(a)=v.get("elements").and_then(|x| x.as_array()) { a.clone() } else { vec![v] };
                    excalidraw::draw_batch(&arr)?
                },
                ExcalidrawCmd::Diagram { kind, params } => {
                    let p: Value = serde_json::from_str(&params).unwrap_or(serde_json::json!({}));
                    excalidraw::build_diagram(&kind, &p)?
                },
                ExcalidrawCmd::Export { opts } => {
                    let v: Value = serde_json::from_str(&opts).unwrap_or(serde_json::json!({}));
                    excalidraw::export_image(&v)?
                },
                ExcalidrawCmd::Save { path } => excalidraw::save_scene_file(Some(&path))?,
                ExcalidrawCmd::View { json } => {
                    if json.trim().is_empty() { excalidraw::get_view()? } else {
                        let v: Value = serde_json::from_str(&json).unwrap_or(serde_json::json!({}));
                        excalidraw::set_view(&v)?
                    }
                },
                ExcalidrawCmd::Fit => excalidraw::scroll_to_content()?,
            };
            println!("{}", serde_json::to_string_pretty(&res)?);
        }
        Some(Commands::Decide { question, q, options, questions, context, image, screenshot, target, temperature }) => {
            let args = build_decide_args(question, q, options, questions, context, image, screenshot, target, temperature);
            let res = decider::tools::decide(args)?;
            println!("{}", serde_json::to_string_pretty(&res)?);
        }
        Some(Commands::Decider { cmd }) => {
            let res = match cmd {
                DeciderCmd::Decide { question, q, options, questions, context, image, screenshot, target, temperature } => {
                    let args = build_decide_args(question, q, options, questions, context, image, screenshot, target, temperature);
                    decider::tools::decide(args)?
                },
                DeciderCmd::Batch { context, questions, requests, image } => {
                    let mut args = Value::Null;
                    if let Some(reqs) = requests {
                        let v: Value = serde_json::from_str(&reqs).unwrap_or(Value::Null);
                        args = serde_json::json!({"requests": v});
                    } else if let Some(qs) = questions {
                        let v: Value = serde_json::from_str(&qs).unwrap_or(Value::Null);
                        let ctx = context.unwrap_or_default();
                        args = serde_json::json!({"context": ctx, "questions": v});
                        if let Some(img) = image { args["image"] = Value::String(img); }
                    } else {
                        anyhow::bail!("decider batch needs --questions or --requests JSON");
                    }
                    decider::tools::decider_batch(args)?
                },
                DeciderCmd::Find { query, target, use_vision, image } => {
                    let mut args = serde_json::json!({"query": query, "use_vision": use_vision});
                    if let Some(t) = target { args["target"] = Value::String(t); }
                    if let Some(img) = image { args["image"] = Value::String(img); }
                    decider::tools::find(args)?
                },
                DeciderCmd::Choose { question, options, context, image, target, use_vision } => {
                    let opts: Value = serde_json::from_str(&options).unwrap_or(Value::Array(vec![]));
                    let mut args = serde_json::json!({"question": question, "options": opts, "use_vision": use_vision});
                    if let Some(c) = context { args["context"] = Value::String(c); }
                    if let Some(img) = image { args["image"] = Value::String(img); }
                    if let Some(t) = target { args["target"] = Value::String(t); }
                    decider::tools::choose(args)?
                },
                DeciderCmd::Classify { question, options, image, context } => {
                    let opts: Value = serde_json::from_str(&options).unwrap_or(Value::Array(vec![]));
                    let mut args = serde_json::json!({"question": question, "options": opts});
                    if let Some(img) = image { args["image"] = Value::String(img); }
                    if let Some(c) = context { args["context"] = Value::String(c); }
                    decider::tools::classify(args)?
                },
                DeciderCmd::Detect { query, context, image, use_vision } => {
                    let mut args = serde_json::json!({"query": query, "use_vision": use_vision});
                    if let Some(c) = context { args["context"] = Value::String(c); }
                    if let Some(img) = image { args["image"] = Value::String(img); }
                    decider::tools::detect(args)?
                },
                DeciderCmd::Identify { query, candidates, target, use_vision } => {
                    let mut args = serde_json::json!({"query": query, "use_vision": use_vision});
                    if let Some(c) = candidates { let v: Value = serde_json::from_str(&c).unwrap_or(Value::Null); args["candidates"] = v; }
                    if let Some(t) = target { args["target"] = Value::String(t); }
                    decider::tools::identify(args)?
                },
                DeciderCmd::VisualTarget { description, candidates, image, target } => {
                    let cands: Value = serde_json::from_str(&candidates).unwrap_or(Value::Null);
                    let mut args = serde_json::json!({"description": description, "candidates": cands});
                    if let Some(img) = image { args["image"] = Value::String(img); }
                    if let Some(t) = target { args["target"] = Value::String(t); }
                    decider::tools::visual_target(args)?
                },
                DeciderCmd::Verify { query } => decider::tools::verify(serde_json::json!({"query": query}))?,
                DeciderCmd::VerifyElement { candidate, selector } => {
                    let mut args = serde_json::json!({});
                    if let Some(c) = candidate { let v: Value = serde_json::from_str(&c).unwrap_or(Value::Null); args["candidate"] = v; }
                    if let Some(s) = selector { args["selector"] = Value::String(s); }
                    decider::tools::verify_element(args)?
                },
                DeciderCmd::VerifyAction { query, expected } => {
                    let mut args = serde_json::json!({"query": query});
                    if let Some(e) = expected { args["expected"] = Value::String(e); }
                    decider::tools::verify_action(args)?
                },
                DeciderCmd::WaitUntil { query, timeout_ms, interval_ms } => decider::tools::wait_until(serde_json::json!({"query": query, "timeout_ms": timeout_ms, "interval_ms": interval_ms}))?,
                DeciderCmd::ObserveState { query, options, image } => {
                    let mut args = serde_json::json!({"query": query});
                    if let Some(o) = options { let v: Value = serde_json::from_str(&o).unwrap_or(Value::Null); args["options"] = v; }
                    if let Some(img) = image { args["image"] = Value::String(img); }
                    decider::tools::observe_state(args)?
                },
                DeciderCmd::HintResolve { instruction, target, vision } => decider::tools::hint_resolve(serde_json::json!({"instruction": instruction, "target": target, "use_vision": vision}))?,
                DeciderCmd::HintResolveBatch { instructions, target, vision } => {
                    let v: Value = serde_json::from_str(&instructions).unwrap_or(Value::Null);
                    let arr = if let Some(a)=v.as_array() { a.clone() } else { vec![v] };
                    let instrs: Vec<String> = arr.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect();
                    let mut args = serde_json::json!({"instructions": instrs, "use_vision": vision});
                    if let Some(t) = target { args["target"] = Value::String(t); }
                    decider::tools::hint_resolve_batch(args)?
                },
                DeciderCmd::KeyIdentify { key, target, rect, vision } => {
                    let mut args = serde_json::json!({"key": key, "use_vision": vision});
                    if let Some(t) = target { args["target"] = Value::String(t); }
                    if let Some(r) = rect { let v: Value = serde_json::from_str(&r).unwrap_or(Value::Null); args["rect"] = v; }
                    decider::tools::key_identify(args)?
                },
                DeciderCmd::FindAndClick { query, target, use_vision } => {
                    let mut args = serde_json::json!({"query": query, "use_vision": use_vision});
                    if let Some(t) = target { args["target"] = Value::String(t); }
                    decider::tools::find_and_click(args)?
                },
                DeciderCmd::FindAndType { query, text, target, use_vision } => {
                    let mut args = serde_json::json!({"query": query, "text": text, "use_vision": use_vision});
                    if let Some(t) = target { args["target"] = Value::String(t); }
                    decider::tools::find_and_type(args)?
                },
            };
            println!("{}", serde_json::to_string_pretty(&res)?);
        }
        Some(Commands::Categories) => {
            println!("{}", serde_json::to_string_pretty(&laya_categories_value())?);
        }
        Some(Commands::Category { category }) => {
            let v = laya_category_value(&category)?;
            println!("{}", serde_json::to_string_pretty(&v)?);
        }
        Some(Commands::Mcp) | None => { run_mcp()?; }
    }
    Ok(())
}

fn run_mcp() -> Result<()> {
    use std::io::{BufRead, Write};
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    let reader = std::io::BufReader::new(stdin.lock());
    let tools = serde_json::json!([
        {"name":"desktop","description":"Instant desktop snapshot (no screenshot, <5ms)","inputSchema":{"type":"object","properties":{}}},
        {"name":"hypr","description":"Window/workspace ops: workspace/focus_window/move_window/close_window/fullscreen/toggle_floating","inputSchema":{"type":"object","properties":{"action":{"type":"string"},"target":{"type":"string"},"workspace":{"type":"string"}}}},
        {"name":"launch","description":"Launch app via Hyprland exec (auto-adds --remote-debugging-port=9222 for browsers)","inputSchema":{"type":"object","properties":{"command":{"type":"string"},"workspace":{"type":"string"}}}},
        {"name":"ui","description":"AT-SPI accessible tree (fast, no screenshot)","inputSchema":{"type":"object","properties":{"window":{"type":"string"},"name":{"type":"string"}}}},
        {"name":"click_ui","description":"Click by accessible name via DoAction (no pointer, no screenshot)","inputSchema":{"type":"object","properties":{"name":{"type":"string"},"window":{"type":"string"},"mark":{"type":"integer"}}}},
        {"name":"pointer","description":"Mouse: move|click|drag|scroll at global logical coords","inputSchema":{"type":"object","properties":{"action":{"type":"string"},"x":{"type":"number"},"y":{"type":"number"},"button":{"type":"string"},"to_x":{"type":"number"},"to_y":{"type":"number"},"scroll_dy":{"type":"number"},"scroll_dx":{"type":"number"}}}},
        {"name":"keyboard","description":"Keyboard: type (text) or key (combo like ctrl+t). window focuses first.","inputSchema":{"type":"object","properties":{"action":{"type":"string"},"text":{"type":"string"},"keys":{"type":"string"},"window":{"type":"string"}}}},
        {"name":"screenshot","description":"Capture via grim: window, region, or monitor. Returns file path + meta. Auto-tracked for session clear.","inputSchema":{"type":"object","properties":{"window":{"type":"string"},"region":{"type":"string"},"scale":{"type":"number"}}}},
        {"name":"wait_for","description":"Block on Hyprland events: window_open/window_close/workspace/title_change/layer_open/layer_close","inputSchema":{"type":"object","properties":{"event":{"type":"string"},"match":{"type":"string"},"timeout_s":{"type":"number"}}}},
        {"name":"binds","description":"List Hyprland keybinds","inputSchema":{"type":"object","properties":{}}},
        {"name":"clear_screenshots","description":"Clear tracked screenshots (/tmp/hyprfast-*.png) after successful task — deletes files recorded in session and resets list. Use all=true to also delete untracked leftovers.","inputSchema":{"type":"object","properties":{"all":{"type":"boolean"}}}},
        {"name":"session_status","description":"Show screenshot session status (tracked files, bytes, session file path)","inputSchema":{"type":"object","properties":{}}},
        {"name":"task_init","description":"Task state: init todo list for multi-step action — AI breakdowns goal into steps, starts tracking. Auto-clears when all completed.","inputSchema":{"type":"object","properties":{"goal":{"type":"string","description":"Overall goal e.g. 'play boomshakalaka on youtube'"},"steps":{"type":"array","items":{"type":"string"},"description":"Ordered steps e.g. ['search youtube','click first video','verify playing']"}},"required":["goal","steps"]}},
        {"name":"task_status","description":"Task state: show current todo list, progress % and next pending step. Use to resume after failure/timeout.","inputSchema":{"type":"object","properties":{}}},
        {"name":"task_update","description":"Task state: update step status (pending|in_progress|completed|failed|skipped). Auto-clears list when all completed.","inputSchema":{"type":"object","properties":{"index":{"type":"integer","description":"0-based step index"},"id":{"type":"integer","description":"1-based step id (alternative to index)"},"status":{"type":"string","description":"pending|in_progress|completed|failed|skipped"}},"required":["status"]}},
        {"name":"task_add","description":"Task state: add a new step to current task list","inputSchema":{"type":"object","properties":{"description":{"type":"string"}},"required":["description"]}},
        {"name":"task_clear","description":"Task state: manually clear current task list","inputSchema":{"type":"object","properties":{}}},
        {"name":"task_next","description":"Task state: get next pending step (use to decide what to do next)","inputSchema":{"type":"object","properties":{}}},
        // Chrome DevTools / browsermcp parity (hyprfast 0.5)
        {"name":"browser_navigate","description":"CDP: navigate browser tab to URL (auto-discovers ws://9222, creates tab if needed)","inputSchema":{"type":"object","properties":{"url":{"type":"string","description":"URL to navigate to"},"target":{"type":"string","description":"optional tab URL/title substring to target"}},"required":["url"]}},
        {"name":"browser_snapshot","description":"CDP: capture accessibility snapshot (AX tree via Accessibility.getFullAXTree, fallback to JS). Returns refs for click/type.","inputSchema":{"type":"object","properties":{}}},
        {"name":"browser_click","description":"CDP: click element. Use ref from snapshot or CSS selector via element.","inputSchema":{"type":"object","properties":{"element":{"type":"string","description":"Human-readable element description"},"ref":{"type":"string","description":"Exact target element reference from snapshot (backendNodeId or selector)"}},"required":["element","ref"]}},
        {"name":"browser_hover","description":"CDP: hover element","inputSchema":{"type":"object","properties":{"element":{"type":"string"},"ref":{"type":"string"}},"required":["element","ref"]}},
        {"name":"browser_type","description":"CDP: type text into editable element (ref from snapshot)","inputSchema":{"type":"object","properties":{"element":{"type":"string"},"ref":{"type":"string"},"text":{"type":"string"},"submit":{"type":"boolean"}},"required":["element","ref","text","submit"]}},
        {"name":"browser_select_option","description":"CDP: select option in dropdown","inputSchema":{"type":"object","properties":{"element":{"type":"string"},"ref":{"type":"string"},"values":{"type":"array","items":{"type":"string"}}},"required":["element","ref","values"]}},
        {"name":"browser_press_key","description":"CDP: press key (Enter, Escape, ArrowLeft, a, etc) via Input.dispatchKeyEvent","inputSchema":{"type":"object","properties":{"key":{"type":"string"}},"required":["key"]}},
        {"name":"browser_wait","description":"CDP: wait N seconds (browser)","inputSchema":{"type":"object","properties":{"time":{"type":"number"}},"required":["time"]}},
        {"name":"browser_evaluate","description":"CDP: evaluate JavaScript in page (Runtime.evaluate)","inputSchema":{"type":"object","properties":{"js":{"type":"string","description":"JavaScript expression"},"expression":{"type":"string"}},"required":[]}},
        {"name":"browser_screenshot","description":"CDP: capture browser tab screenshot via Page.captureScreenshot (PNG, no grim, no compositor)","inputSchema":{"type":"object","properties":{}}},
        {"name":"browser_tabs","description":"CDP: list browser tabs/targets (GET /json)","inputSchema":{"type":"object","properties":{}}},
        {"name":"browser_console","description":"CDP: get console logs (Console.enable)","inputSchema":{"type":"object","properties":{}}},
        {"name":"browser_go_back","description":"CDP: go back (history.back)","inputSchema":{"type":"object","properties":{}}},
        {"name":"browser_go_forward","description":"CDP: go forward (history.forward)","inputSchema":{"type":"object","properties":{}}},
        {"name":"browser_open","description":"Hypr+CDP: launch Brave with --remote-debugging-port=9222 on workspace and navigate","inputSchema":{"type":"object","properties":{"url":{"type":"string"},"workspace":{"type":"string"}},"required":["url"]}},
        {"name":"browser_execute_plan","description":"Run a structured multi-step browser plan in one call: {steps:[{action: navigate|click|type|select|press|hover|wait|eval|extract, ...}]}. Cheaper and more reliable than N separate browser_* calls.","inputSchema":{"type":"object","properties":{"steps":{"type":"array","items":{"type":"object","properties":{"action":{"type":"string"},"url":{"type":"string"},"selector":{"type":"string"},"ref":{"type":"string"},"text":{"type":"string"},"key":{"type":"string"},"js":{"type":"string"},"time":{"type":"number"}},"required":["action"]},"description":"Ordered plan steps"},"plan":{"type":"object","description":"Alternative: wrap the plan as {steps:[...]}"},"target":{"type":"string"}},"required":[]}},

        {"name":"decide","description":"Unified Decider-2B: single or multiple questions, options (or auto-detect yes/no/uncertain), optional image (path/base64/data URI/auto-screenshot), context. Single command covering decide, batch, choose, classify, and detect.","inputSchema":{"type":"object","properties":{"context":{"type":"string","description":"Context/state for the decision"},"state":{"type":"string"},"questions":{"type":"array","items":{"type":"object","properties":{"question":{"type":"string"},"options":{"type":"array","items":{"type":"string"}}}},"description":"Questions: [{question, options: string[]}] 1..255 options each"},"question":{"type":"string","description":"Single question or query"},"query":{"type":"string","description":"Alternative to question"},"options":{"description":"Options array or comma-separated string (defaults to ['yes', 'no', 'uncertain'] if omitted for presence detection)"},"image":{"type":"string","description":"Image file path, base64, or data URI"},"screenshot":{"type":"boolean","description":"Auto-capture screenshot of browser or desktop"},"use_vision":{"type":"boolean","description":"Auto-capture screenshot if true"},"target":{"type":"string","description":"Target browser tab index or window name"},"temperature":{"type":"number"}},"required":[]}},
        {"name":"decider_batch","description":"Multiple questions same context/screenshot (or requests[] for concurrent batch, bounded 4). Reuses single screenshot + deterministic fallback.","inputSchema":{"type":"object","properties":{"context":{"type":"string"},"questions":{"type":"array"},"requests":{"type":"array","description":"Alternative: [{context, questions, image}] for concurrent batch (max 12)"},"image":{"type":"string"},"temperature":{"type":"number"}},"required":[]}},
        {"name":"find","description":"Semantic target resolver: query -> DOM/AX/hints -> candidate filtering -> Decider if ambiguous -> resolved candidate metadata (target-aware, image pipeline reused)","inputSchema":{"type":"object","properties":{"query":{"type":"string","description":"Natural query e.g. 'Submit button'"},"instruction":{"type":"string"},"description":{"type":"string"},"target":{"type":"string","description":"Target tab: 1-based index, targetId, or url/title substring"},"use_vision":{"type":"boolean"},"vision":{"type":"boolean"},"image":{"type":"string"},"context":{"type":"string"},"viewport":{"type":"object"}},"required":["query"]}},
        {"name":"choose","description":"Choose: question+options[] up to 255, numeric IDs internally when visual candidates, return selected + confidence + probs + runner_up/margin","inputSchema":{"type":"object","properties":{"question":{"type":"string"},"options":{"type":"array","items":{"type":"string"}},"context":{"type":"string"},"image":{"type":"string"},"target":{"type":"string"},"use_vision":{"type":"boolean"},"vision":{"type":"boolean"},"candidates":{"type":"array"}},"required":["question","options"]}},
        {"name":"classify","description":"State classification from explicit options, image optional (choose alias for state)","inputSchema":{"type":"object","properties":{"question":{"type":"string"},"query":{"type":"string"},"options":{"type":"array","items":{"type":"string"}},"context":{"type":"string"},"image":{"type":"string"}},"required":["question"]}},
        {"name":"detect","description":"Presence/absence YES/NO/UNCERTAIN (Decider yes/no/uncertain + DOM fallback)","inputSchema":{"type":"object","properties":{"query":{"type":"string"},"question":{"type":"string"},"context":{"type":"string"},"image":{"type":"string"},"use_vision":{"type":"boolean"}},"required":["query"]}},
        {"name":"identify","description":"Which candidate/entity — identify among candidates (query + hints/candidates, vision 10/text 255)","inputSchema":{"type":"object","properties":{"query":{"type":"string"},"candidates":{"type":"array"},"hints":{"type":"array"},"target":{"type":"string"},"use_vision":{"type":"boolean"}},"required":["query"]}},
        {"name":"visual_target","description":"Visual target: description+image+candidate rects/metadata -> selected candidate ID/confidence/probs/rect (vision 10 budget, annotated screenshot)","inputSchema":{"type":"object","properties":{"description":{"type":"string"},"query":{"type":"string"},"candidates":{"type":"array","description":"[{id,label,tag,role,name,rect:{x,y,width,height},selector}]"},"hints":{"type":"array"},"image":{"type":"string","description":"base64/data URI; captured via browser/monitor pipeline if omitted"},"target":{"type":"string"}},"required":["description","candidates"]}},
        {"name":"verify","description":"Verify (DOM first, Decider visual only when necessary) -> success/failure/uncertain + confidence","inputSchema":{"type":"object","properties":{"query":{"type":"string"}},"required":["query"]}},
        {"name":"verify_element","description":"Verify element (candidate/selector) present/visible (DOM + visual fallback)","inputSchema":{"type":"object","properties":{"candidate":{"type":"object"},"selector":{"type":"string"}},"required":[]}},
        {"name":"verify_action","description":"Verify action succeeded (query + expected text)","inputSchema":{"type":"object","properties":{"query":{"type":"string"},"expected":{"type":"string"}},"required":["query"]}},
        {"name":"wait_until","description":"Wait until predicate via DOM/AX polling + visual fallback, timeout/interval","inputSchema":{"type":"object","properties":{"query":{"type":"string"},"timeout_ms":{"type":"integer"},"timeout":{"type":"integer"},"interval_ms":{"type":"integer"},"interval":{"type":"integer"}},"required":["query"]}},
        {"name":"observe_state","description":"Observe state: classify current UI state from explicit options (image optional)","inputSchema":{"type":"object","properties":{"query":{"type":"string"},"options":{"type":"array","items":{"type":"string"}},"states":{"type":"array","items":{"type":"string"}},"context":{"type":"string"},"image":{"type":"string"},"target":{"type":"string"}},"required":["query"]}},
        // ---- Hint pipeline (fast-ground) ----
        // hint_act is the PRIMARY browser-interaction tool: it takes one natural-language
        // instruction and resolves the element itself (heuristic -> Decider-2B -> vision).
        // A planner that executes BLIND should prefer it over browser_click/browser_type,
        // which require a ref/selector obtained from an observation it will never make.
        {"name":"hint_act","description":"PRIMARY browser interaction. Give ONE natural-language instruction; it snapshots clickable elements, resolves the target itself (heuristic -> Decider-2B -> vision last resort), then clicks or types. Use this instead of browser_click/browser_type when you have no snapshot ref. Returns tier, label, success.","inputSchema":{"type":"object","properties":{"instruction":{"type":"string","description":"e.g. 'click the first video result' or 'type despacito into the search box'"},"action":{"type":"string","description":"click|type (default click)"},"text":{"type":"string","description":"Text to type when action=type; omit to extract it from the instruction"},"target":{"type":"string","description":"Tab selector: 1-based index, targetId, or url/title substring"}},"required":["instruction"]}},
        {"name":"hint_batch","description":"Batch hint actions: ONE snapshot, resolve all instructions, dispatch in parallel (max 12 steps). Each step {instruction, action, text}. Cheaper than repeated hint_act.","inputSchema":{"type":"object","properties":{"steps":{"type":"array","description":"[{instruction, action:click|type, text}]"},"target":{"type":"string"}},"required":["steps"]}},
        {"name":"hint_snapshot","description":"List all clickable/typeable elements on the page as letter keys (A, S, D, ... AA, AS). Use with hint_click/hint_type. Prefer hint_act, which does snapshot+resolve+act in one call.","inputSchema":{"type":"object","properties":{"target":{"type":"string"}},"required":[]}},
        {"name":"hint_click","description":"Click the element carrying a hint_snapshot label (e.g. 'A'). Only after hint_snapshot; prefer hint_act which skips the manual label step.","inputSchema":{"type":"object","properties":{"label":{"type":"string","description":"Hint label from hint_snapshot, e.g. 'A' or 'AS'"},"target":{"type":"string"}},"required":["label"]}},
        {"name":"hint_type","description":"Type text into the element carrying a hint_snapshot label. Only after hint_snapshot; prefer hint_act with action=type.","inputSchema":{"type":"object","properties":{"label":{"type":"string","description":"Hint label from hint_snapshot"},"text":{"type":"string"},"target":{"type":"string"}},"required":["label","text"]}},
        {"name":"hint_clear","description":"Remove the hint key overlay from the page (labels stay in the DOM).","inputSchema":{"type":"object","properties":{"target":{"type":"string"}},"required":[]}},
        {"name":"ground","description":"Visual grounding fallback: screenshot + Decider candidate ranking -> {x,y} global logical coords, for canvas/WebGL pages with no DOM hints. Then click with pointer.","inputSchema":{"type":"object","properties":{"instruction":{"type":"string"},"window":{"type":"string"},"region":{"type":"string"}},"required":["instruction"]}},
        {"name":"act_fast","description":"Fused ground+click/type in one call (no extra round trip). Use when hint_act cannot resolve because the page has no DOM hints.","inputSchema":{"type":"object","properties":{"instruction":{"type":"string"},"action":{"type":"string","description":"click|type (default click)"},"text":{"type":"string"},"window":{"type":"string"}},"required":["instruction"]}},
        {"name":"act_batch","description":"Batch fused steps: JSON array [{instruction, action, text}] resolved and dispatched together.","inputSchema":{"type":"object","properties":{"steps":{"type":"array"},"window":{"type":"string"}},"required":[]}},
        {"name":"hint_resolve","description":"Hint resolve: hint_snapshot -> Decider -> hint_click (single instruction, target-aware, deterministic fallback, vision optional)","inputSchema":{"type":"object","properties":{"instruction":{"type":"string"},"query":{"type":"string"},"target":{"type":"string"},"use_vision":{"type":"boolean"},"vision":{"type":"boolean"}},"required":["instruction"]}},
        {"name":"hint_resolve_batch","description":"Hint resolve batch: multiple instructions same snapshot/screenshot, batched Decider (max 12, bounded 4)","inputSchema":{"type":"object","properties":{"instructions":{"type":"array","items":{"type":"string"}},"steps":{"type":"array"},"target":{"type":"string"},"use_vision":{"type":"boolean"},"vision":{"type":"boolean"}},"required":["instructions"]}},
        {"name":"key_identify","description":"Key identify for visual/virtual keyboards (same pipeline, keyboard candidates, target+rect aware)","inputSchema":{"type":"object","properties":{"key":{"type":"string"},"query":{"type":"string"},"target":{"type":"string"},"rect":{"type":"object"},"keyboard_rect":{"type":"object"},"use_vision":{"type":"boolean"}},"required":["key"]}},
        {"name":"find_and_click","description":"Composite: find + click (semantic find then hint/browser click, target-aware)","inputSchema":{"type":"object","properties":{"query":{"type":"string"},"instruction":{"type":"string"},"target":{"type":"string"},"use_vision":{"type":"boolean"}},"required":["query"]}},
        {"name":"find_and_type","description":"Composite: find + type (semantic find then hint/browser type, target-aware)","inputSchema":{"type":"object","properties":{"query":{"type":"string"},"instruction":{"type":"string"},"text":{"type":"string"},"target":{"type":"string"},"use_vision":{"type":"boolean"}},"required":["query","text"]}},
        {"name":"decider_health","description":"Decider health check (GET /health, fallback /v1/health)","inputSchema":{"type":"object","properties":{}}},
        {"name":"decider_metrics","description":"Decider metrics snapshot (request/success/error/timeout/latency)","inputSchema":{"type":"object","properties":{}}},
        {"name":"categories","description":"Laya Pass-1: list 8 tool categories as {category_name, description} with total count","inputSchema":{"type":"object","properties":{}}},
        {"name":"category","description":"Laya Pass-2: show category {category_name, description, total_commands, commands:[{command_name, description}]}. category: core-desktop|perceive|native-act|browser-act|fast-ground|task-memory|draw|perception","inputSchema":{"type":"object","properties":{"category":{"type":"string","description":"Category name: core-desktop|perceive|native-act|browser-act|fast-ground|task-memory|draw|perception"}},"required":["category"]}},
        {"name":"excalidraw_fit","description":"Excalidraw: center viewport on content (zoom to fit) — auto-fits bbox","inputSchema":{"type":"object","properties":{}}}
    ]);
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() { continue; }
        let msg: Value = match serde_json::from_str(&line) { Ok(v)=>v, Err(_)=>continue };
        let id = msg.get("id").cloned().unwrap_or(Value::Null);
        let method = msg.get("method").and_then(|v| v.as_str()).unwrap_or("");
        let params = msg.get("params").cloned().unwrap_or(Value::Null);
        let result = match method {
            "initialize" => serde_json::json!({"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"serverInfo":{"name":"hyprfast","version":env!("CARGO_PKG_VERSION")}}),
            "notifications/initialized" => continue,
            "tools/list" => serde_json::json!({"tools": tools}),
            "tools/call" => {
                let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let args = params.get("arguments").cloned().unwrap_or(Value::Null);
                match handle_tool(name, args) {
                    Ok(v) => serde_json::json!({"content":[{"type":"text","text": v.to_string()}]}),
                    Err(e) => serde_json::json!({"content":[{"type":"text","text": format!("error: {}", e)}],"isError": true}),
                }
            },
            _ => serde_json::json!({}),
        };
        let resp = serde_json::json!({"jsonrpc":"2.0","id": id, "result": result});
        writeln!(stdout, "{}", resp.to_string())?;
        stdout.flush()?;
    }
    Ok(())
}
use serde_json::Value;
fn handle_tool(name: &str, args: Value) -> Result<Value> {
    match name {
        "desktop" => Ok(hypr::snapshot()?),
        "hypr" => {
            let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("");
            let target = args.get("target").and_then(|v| v.as_str()).unwrap_or("");
            let workspace = args.get("workspace").and_then(|v| v.as_str()).unwrap_or("");
            // Mirror CLI: `action=workspace target=N` works when workspace is omitted
            let ws = if workspace.is_empty() && !target.is_empty() && action == "workspace" {
                target.to_string()
            } else {
                workspace.to_string()
            };
            let (d, arg) = match action {
                "workspace" => ("workspace", ws),
                "focus_window" => ("focuswindow", format!("address:{}", target)),
                "move_window" => ("movetoworkspacesilent", format!("{},address:{}", workspace, target)),
                "close_window" => ("closewindow", format!("address:{}", target)),
                "fullscreen" => ("fullscreen", "0".into()),
                "toggle_floating" => ("togglefloating", if target.is_empty() { "".into() } else { format!("address:{}", target)}),
                _ => (action, target.to_string()),
            };
            let out = hypr::dispatch(d, &arg)?;
            Ok(serde_json::json!({"result": out, "snapshot": hypr::snapshot()?}))
        },
        "launch" => {
            let cmd = args.get("command").and_then(|v| v.as_str()).unwrap_or("");
            let ws = args.get("workspace").and_then(|v| v.as_str()).unwrap_or("");
            let cmd2 = ensure_browser_args(cmd);
            let rule = if ws.is_empty() { "".to_string() } else { format!("[workspace {} silent] ", ws) };
            hypr::dispatch("exec", &format!("{}{}", rule, cmd2))?;
            std::thread::sleep(std::time::Duration::from_millis(600));
            Ok(hypr::snapshot()?)
        },
        "ui" => {
            let w = args.get("window").and_then(|v| v.as_str()).unwrap_or("");
            let n = args.get("name").and_then(|v| v.as_str()).unwrap_or("");
            a11y::list_elements(w, n)
        },
        "click_ui" => {
            let w = args.get("window").and_then(|v| v.as_str()).unwrap_or("");
            let n = args.get("name").and_then(|v| v.as_str()).unwrap_or("");
            if !n.is_empty() { a11y::click_by_name(w, n) } else { anyhow::bail!("click_ui needs name or mark") }
        },
        "pointer" => {
            let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("click");
            let x = args.get("x").and_then(|v| v.as_f64());
            let y = args.get("y").and_then(|v| v.as_f64());
            let button = args.get("button").and_then(|v| v.as_str()).unwrap_or("left");
            let to_x = args.get("to_x").and_then(|v| v.as_f64());
            let to_y = args.get("to_y").and_then(|v| v.as_f64());
            let dy = args.get("scroll_dy").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let dx = args.get("scroll_dx").and_then(|v| v.as_f64()).unwrap_or(0.0);
            match action {
                "move" => { input::move_cursor(x.unwrap(), y.unwrap())?; Ok(serde_json::json!({"result":"moved"})) },
                "click" => { input::click(x, y, button, false)?; Ok(serde_json::json!({"result":"clicked","at":[x,y]})) },
                "drag" => { input::drag(x.unwrap(), y.unwrap(), to_x.unwrap(), to_y.unwrap(), button)?; Ok(serde_json::json!({"result":"dragged"})) },
                "scroll" => { input::scroll(dy, dx, x, y)?; Ok(serde_json::json!({"result":"scrolled"})) },
                _ => anyhow::bail!("unknown pointer action"),
            }
        },
        "keyboard" => {
            let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("type");
            let text = args.get("text").and_then(|v| v.as_str()).unwrap_or("");
            let keys = args.get("keys").and_then(|v| v.as_str()).unwrap_or("");
            let w = args.get("window").and_then(|v| v.as_str()).unwrap_or("");
            if !w.is_empty() { hypr::dispatch("focuswindow", &format!("address:{}", w))?; std::thread::sleep(std::time::Duration::from_millis(50)); }
            match action {
                "type" => { input::type_text(text)?; Ok(serde_json::json!({"typed": text.len()})) },
                "key" => { input::key_combo(keys)?; Ok(serde_json::json!({"pressed": keys})) },
                _ => anyhow::bail!("unknown keyboard action"),
            }
        },
        "screenshot" => {
            let w = args.get("window").and_then(|v| v.as_str()).unwrap_or("");
            let r = args.get("region").and_then(|v| v.as_str()).unwrap_or("");
            let scale = args.get("scale").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let (data, meta) = screenshot::capture(w, r, scale)?;
            let path = format!("/tmp/hyprfast-{}.png", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis());
            std::fs::write(&path, &data)?;
            let _ = session::record(&path);
            Ok(serde_json::json!({"path": path, "meta": meta}))
        },
        "clear_screenshots" => {
            let all = args.get("all").and_then(|v| v.as_bool()).unwrap_or(false);
            let v = if all { session::clear_all()? } else { session::clear()? };
            Ok(v)
        },
        "session_status" => {
            Ok(session::status())
        },
        "wait_for" => {
            let e = args.get("event").and_then(|v| v.as_str()).unwrap_or("window_open");
            let m = args.get("match").and_then(|v| v.as_str()).unwrap_or("");
            let t = args.get("timeout_s").and_then(|v| v.as_f64()).unwrap_or(5.0);
            events::wait_for(e, m, t)
        },
        "binds" => hypr::binds(),
        // ----- Browser CDP tools (hyprfast 0.5) -----
        "browser_navigate" => {
            let url = args.get("url").and_then(|v| v.as_str()).unwrap_or("");
            let target = args.get("target").and_then(|v| v.as_str());
            browser::navigate(url, target)
        },
        "browser_snapshot" => browser::snapshot(60),
        "browser_click" => {
            let element = args.get("element").and_then(|v| v.as_str()).unwrap_or("");
            let r#ref = args.get("ref").and_then(|v| v.as_str()).unwrap_or("");
            let sel = if !r#ref.is_empty() { r#ref } else { element };
            if sel.is_empty() { anyhow::bail!("browser_click needs ref or element selector"); }
            // Phase 3: backend path now via persistent BrowserRuntime (no per-call WS)
            if r#ref.chars().all(|c| c.is_ascii_digit()) && !r#ref.is_empty() {
                let res = browser::click_by_ref(r#ref, "");
                if res.is_ok() { return res; }
            }
            browser::click_by_selector(sel)
        },
        "browser_hover" => {
            let element = args.get("element").and_then(|v| v.as_str()).unwrap_or("");
            let r#ref = args.get("ref").and_then(|v| v.as_str()).unwrap_or("");
            let sel = if !r#ref.is_empty() { r#ref } else { element };
            browser::hover_by_ref(sel, Some(sel))
        },
        "browser_type" => {
            let element = args.get("element").and_then(|v| v.as_str()).unwrap_or("");
            let r#ref = args.get("ref").and_then(|v| v.as_str()).unwrap_or("");
            let text = args.get("text").and_then(|v| v.as_str()).unwrap_or("");
            let submit = args.get("submit").and_then(|v| v.as_bool()).unwrap_or(false);
            let sel = if !r#ref.is_empty() { Some(r#ref.to_string()) } else if !element.is_empty() { Some(element.to_string()) } else { None };
            browser::type_text(r#ref, text, submit, sel.as_deref())
        },
        "browser_select_option" => {
            let r#ref = args.get("ref").and_then(|v| v.as_str()).unwrap_or("");
            let element = args.get("element").and_then(|v| v.as_str()).unwrap_or("");
            let sel = if !r#ref.is_empty() { r#ref } else { element };
            let vals = args.get("values").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect::<Vec<_>>()).unwrap_or_default();
            browser::select_option(sel, &vals)
        },
        "browser_press_key" => {
            let key = args.get("key").and_then(|v| v.as_str()).unwrap_or("");
            browser::press_key(key)
        },
        "browser_wait" => {
            let t = args.get("time").and_then(|v| v.as_f64()).unwrap_or(1.0);
            browser::wait(t)
        },
        "browser_evaluate" => {
            let js = args.get("js").or_else(|| args.get("expression")).and_then(|v| v.as_str()).unwrap_or("");
            browser::evaluate_js(js)
        },
        "browser_screenshot" => {
            let (data, meta) = browser::screenshot_cdp()?;
            let path = format!("/tmp/hyprfast-browser-{}.png", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis());
            std::fs::write(&path, &data)?;
            let _ = session::record(&path);
            Ok(serde_json::json!({"path": path, "meta": meta}))
        },
        "browser_tabs" => browser::tabs(),
        "browser_console" => browser::console_logs(),
        "browser_go_back" => browser::go_back(),
        "browser_go_forward" => browser::go_forward(),
        "browser_open" => {
            let url = args.get("url").and_then(|v| v.as_str()).unwrap_or("");
            let ws = args.get("workspace").and_then(|v| v.as_str()).unwrap_or("");
            browser_open_url(url, ws)
        },

        "task_init" => {
            let goal = args.get("goal").and_then(|v| v.as_str()).unwrap_or("");
            if goal.is_empty() { anyhow::bail!("task_init needs goal"); }
            let steps = args.get("steps").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect::<Vec<_>>()).unwrap_or_default();
            if steps.is_empty() { anyhow::bail!("task_init needs steps array"); }
            task::init(goal, steps)
        },
        "task_status" => Ok(task::status()),
        "task_update" => {
            let status = args.get("status").and_then(|v| v.as_str()).unwrap_or("");
            if status.is_empty() { anyhow::bail!("task_update needs status"); }
            let index = args.get("index").and_then(|v| v.as_u64()).map(|v| v as usize);
            let id = args.get("id").and_then(|v| v.as_u64()).map(|v| v as usize);
            task::update(index, id, status)
        },
        "task_add" => {
            let desc = args.get("description").and_then(|v| v.as_str()).unwrap_or("");
            if desc.is_empty() { anyhow::bail!("task_add needs description"); }
            task::add(desc)
        },
        "task_clear" => task::clear(),
        "task_next" => task::next_pending(),
        "ground" => {
            let instruction = args.get("instruction").and_then(|v| v.as_str()).unwrap_or("");
            let window = args.get("window").and_then(|v| v.as_str()).unwrap_or("");
            let region = args.get("region").and_then(|v| v.as_str()).unwrap_or("");
            ground::ground(instruction, window, region)
        },
        "act_fast" => {
            let instruction = args.get("instruction").and_then(|v| v.as_str()).unwrap_or("");
            let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("click");
            let text = args.get("text").and_then(|v| v.as_str()).unwrap_or("");
            let window = args.get("window").and_then(|v| v.as_str()).unwrap_or("");
            ground::act_fast(instruction, action, text, window)
        },
        "act_batch" => {
            let steps = args.get("steps").cloned().unwrap_or(Value::Null);
            let window = args.get("window").and_then(|v| v.as_str()).unwrap_or("");
            ground::act_batch(&steps, window)
        },
        "browser_execute_plan" => {
            // Phase 7 additive plan execution — additive, single-action tools preserved.
            // Accept either {plan:{steps:[...]}} or {steps:[...]} or bare array {plan:[...]}
            let plan_val = args.get("plan").cloned()
                .or_else(|| args.get("steps").cloned().map(|v| serde_json::json!({"steps": v})))
                .unwrap_or(args.clone());
            // If args itself looks like a plan object (has steps or type), forward as-is
            let v = if plan_val.get("steps").is_some() || plan_val.is_array() { plan_val } else if args.get("steps").is_some() { serde_json::json!({"steps": args.get("steps").unwrap()}) } else { args.clone() };
            crate::browser_runtime::client::execute_plan_sync(v)
        },
        "hint_snapshot" => {
            let target = args.get("target").and_then(|v| v.as_str());
            hint::hint_snapshot_with_target(target)
        },
        "hint_click" => {
            let label = args.get("label").and_then(|v| v.as_str()).unwrap_or("");
            let target = args.get("target").and_then(|v| v.as_str());
            hint::hint_click_with_target(label, target)
        },
        "hint_type" => {
            let label = args.get("label").and_then(|v| v.as_str()).unwrap_or("");
            let text = args.get("text").and_then(|v| v.as_str()).unwrap_or("");
            let target = args.get("target").and_then(|v| v.as_str());
            hint::hint_type_with_target(label, text, target)
        },
        "hint_act" => {
            let instruction = args.get("instruction").and_then(|v| v.as_str()).unwrap_or("");
            let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("click");
            let text = args.get("text").and_then(|v| v.as_str()).unwrap_or("");
            let target = args.get("target").and_then(|v| v.as_str());
            hint::hint_act_with_target(instruction, action, text, target)
        },
        "hint_batch" => {
            let steps = args.get("steps").and_then(|v| v.as_array()).cloned().unwrap_or_else(|| args.as_array().cloned().unwrap_or_default());
            let target = args.get("target").and_then(|v| v.as_str());
            hint::hint_batch_with_target(&steps, target)
        },
        "hint_clear" => {
            let target = args.get("target").and_then(|v| v.as_str());
            hint::hint_clear_with_target(target)
        },
        // ---- Excalidraw lightning tools ----
        "excalidraw_open" => {
            let url = args.get("url").and_then(|v| v.as_str()).unwrap_or("https://excalidraw.com/");
            excalidraw::ensure_open(Some(url))
        },
        "excalidraw_get_scene" => excalidraw::get_scene(),
        "excalidraw_clear" => excalidraw::clear_scene(),
        "excalidraw_draw" => {
            // Support both flat args and json string arg
            let v = if args.get("json").is_some() {
                let s = args.get("json").and_then(|v| v.as_str()).unwrap_or("{}");
                serde_json::from_str(s).unwrap_or(args.clone())
            } else if args.get("type").is_some() { args.clone() } else { args.clone() };
            // also accept single object wrapped
            excalidraw::draw_primitive(&v)
        },
        "excalidraw_draw_batch" => {
            let arr = if let Some(s) = args.get("json").and_then(|v| v.as_str()) {
                serde_json::from_str::<Value>(s).ok().and_then(|v| v.as_array().cloned()).unwrap_or_default()
            } else if let Some(a) = args.get("elements").and_then(|v| v.as_array()) { a.clone() }
            else if let Some(a) = args.as_array() { a.clone() }
            else { vec![args.clone()] };
            excalidraw::draw_batch(&arr)
        },
        "excalidraw_update_scene" => {
            let (els, mode) = if let Some(s)=args.get("json").and_then(|v| v.as_str()) {
                let v: Value = serde_json::from_str(s).unwrap_or(serde_json::json!({}));
                (v.get("elements").and_then(|x| x.as_array()).cloned().unwrap_or_default(), v.get("mode").and_then(|x| x.as_str()).unwrap_or("append").to_string())
            } else {
                (args.get("elements").and_then(|v| v.as_array()).cloned().unwrap_or_default(), args.get("mode").and_then(|v| v.as_str()).unwrap_or("append").to_string())
            };
            excalidraw::update_scene(els, &serde_json::json!({"mode": mode}))
        },
        "excalidraw_diagram" => {
            let kind = args.get("kind").and_then(|v| v.as_str()).unwrap_or("flowchart");
            let params = if let Some(s)=args.get("json").and_then(|v| v.as_str()) { serde_json::from_str(s).unwrap_or(serde_json::json!({})) }
            else if args.get("params").is_some() { args.get("params").cloned().unwrap() }
            else {
                let mut p=serde_json::json!({});
                if let Some(t)=args.get("title").and_then(|v| v.as_str()) { p["title"]=serde_json::json!(t); }
                // pass through any other keys as params
                for k in ["services","databases","participants","messages","nodes","entities","steps","elements"] {
                    if let Some(v)=args.get(k) { p[k]=v.clone(); }
                }
                p
            };
            excalidraw::build_diagram(kind, &params)
        },
        "excalidraw_export" => {
            let opts = if args.get("json").is_some() {
                let s=args.get("json").and_then(|v| v.as_str()).unwrap_or("{}");
                serde_json::from_str(s).unwrap_or(serde_json::json!({}))
            } else { args.clone() };
            excalidraw::export_image(&opts)
        },
        "excalidraw_save" => {
            let p = args.get("path").and_then(|v| v.as_str()).unwrap_or("/tmp/excalidraw-scene.excalidraw");
            excalidraw::save_scene_file(Some(p))
        },
        "excalidraw_view" => {
            if args.get("json").is_some() || args.get("scrollX").is_some() || args.get("scrollY").is_some() || args.get("zoom").is_some() {
                let v = if let Some(s)=args.get("json").and_then(|v| v.as_str()) { serde_json::from_str(s).unwrap_or(serde_json::json!({})) } else { args.clone() };
                if v.as_object().map(|o| o.is_empty()).unwrap_or(true) { excalidraw::get_view() } else { excalidraw::set_view(&v) }
            } else { excalidraw::get_view() }
        },
        "excalidraw_fit" => excalidraw::scroll_to_content(),
        // ----- Perception / Decider-2B semantic tools (reuse perception resolver, single image pipeline, target-aware, bounded 4, deterministic fallback) -----
        "decide" => decider::tools::decide(args),
        "decider_batch" => decider::tools::decider_batch(args),
        "find" => decider::tools::find(args),
        "choose" => decider::tools::choose(args),
        "classify" => decider::tools::classify(args),
        "detect" => decider::tools::detect(args),
        "identify" => decider::tools::identify(args),
        "visual_target" => decider::tools::visual_target(args),
        "verify" => decider::tools::verify(args),
        "verify_element" => decider::tools::verify_element(args),
        "verify_action" => decider::tools::verify_action(args),
        "wait_until" => decider::tools::wait_until(args),
        "observe_state" => decider::tools::observe_state(args),
        "hint_resolve" => decider::tools::hint_resolve(args),
        "hint_resolve_batch" => decider::tools::hint_resolve_batch(args),
        "key_identify" => decider::tools::key_identify(args),
        "find_and_click" => decider::tools::find_and_click(args),
        "find_and_type" => decider::tools::find_and_type(args),
        "decider_health" => {
            let cfg = decider::config::DeciderConfig::from_env();
            let client = decider::client::DeciderClient::new(cfg)?;
            Ok(client.health_sync()?)
        },
        "decider_metrics" => Ok(decider::client::metrics().snapshot()),
        "categories" => Ok(laya_categories_value()),
        "category" => {
            let cat = args.get("category").or_else(|| args.get("name")).and_then(|v| v.as_str()).unwrap_or("");
            if cat.is_empty() { anyhow::bail!("category needs category name: core-desktop|perceive|native-act|browser-act|fast-ground|task-memory|draw|perception"); }
            // returns {category_name, description, total_commands, commands:[{command_name, description}]}
            laya_category_value(cat)
        },
        _ => anyhow::bail!("unknown tool {}", name),
    }
}
