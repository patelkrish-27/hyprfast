mod hypr;
mod a11y;
mod input;
mod screenshot;
mod events;
mod daemon;
mod session;
mod cdp;
mod browser;

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
enum Commands {
    Desktop,
    Hypr { action: String, #[arg(default_value="")] target: String, #[arg(default_value="")] workspace: String },
    Launch { command: String, #[arg(long)] workspace: Option<String> },
    Ui { #[arg(long)] window: Option<String>, #[arg(long, default_value="")] name: String },
    Click { name: String, #[arg(long)] window: Option<String> },
    Pointer { action: String, #[arg(long)] x: Option<f64>, #[arg(long)] y: Option<f64>, #[arg(long, default_value="left")] button: String, #[arg(long)] to_x: Option<f64>, #[arg(long)] to_y: Option<f64>, #[arg(long, default_value="0")] dy: f64, #[arg(long, default_value="0")] dx: f64 },
    Keyboard { action: String, #[arg(long, default_value="")] text: String, #[arg(long, default_value="")] keys: String, #[arg(long)] window: Option<String> },
    Screenshot { #[arg(long)] window: Option<String>, #[arg(long)] region: Option<String> },
    Wait { event: String, #[arg(long, default_value="")] match_str: String, #[arg(long, default_value="5")] timeout: f64 },
    Binds,
    Daemon { #[arg(long)] stop: bool },
    Clear { #[arg(long)] all: bool },
    Session { action: String },
    Browser { #[command(subcommand)] cmd: BrowserCmd },
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
                "move" => { input::move_cursor(x.unwrap(), y.unwrap())?; println!("moved"); }
                "click" => { input::click(x, y, &button, false)?; println!("clicked"); }
                "drag" => { input::drag(x.unwrap(), y.unwrap(), to_x.unwrap(), to_y.unwrap(), &button)?; println!("dragged"); }
                "scroll" => { input::scroll(dy, dx, x, y)?; println!("scrolled"); }
                _ => anyhow::bail!("unknown pointer action"),
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
                    if target.chars().all(|c| c.is_ascii_digit()) {
                        // backendNodeId path: resolve then click via Runtime
                        let ws = cdp::get_ws_url(None)?;
                        let bid: i64 = target.parse().unwrap_or(0);
                        let resolved = cdp::cdp_call(&ws, "DOM.resolveNode", serde_json::json!({"backendNodeId": bid}))?;
                        if let Some(obj) = resolved.get("object").and_then(|o| o.get("objectId")).and_then(|v| v.as_str()) {
                            let _ = cdp::cdp_call(&ws, "Runtime.callFunctionOn", serde_json::json!({"objectId": obj, "functionDeclaration": "function(){this.click(); return this.tagName;}", "returnByValue": true}))?;
                            serde_json::json!({"clicked": true, "ref": target, "via":"CDP-backend"})
                        } else {
                            browser::click_by_selector(&target)?
                        }
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
                BrowserCmd::Eval { js } => browser::evaluate_js(&js)?,
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
                    let cmd = format!("brave --remote-debugging-port=9222 --force-renderer-accessibility --new-window {}", url);
                    let rule = workspace.map(|w| format!("[workspace {} silent] ", w)).unwrap_or_default();
                    hypr::dispatch("exec", &format!("{}{}", rule, cmd))?;
                    std::thread::sleep(std::time::Duration::from_millis(800));
                    serde_json::json!({"launched": url, "cdp": cdp::version().unwrap_or(serde_json::json!({}))})
                },
            };
            println!("{}", serde_json::to_string_pretty(&res)?);
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
        {"name":"browser_open","description":"Hypr+CDP: launch Brave with --remote-debugging-port=9222 on workspace and navigate","inputSchema":{"type":"object","properties":{"url":{"type":"string"},"workspace":{"type":"string"}},"required":["url"]}}
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
            let (d, arg) = match action {
                "workspace" => ("workspace", workspace.to_string()),
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
            // Prefer JS selector click; if ref looks like backend id, try backend
            if r#ref.chars().all(|c| c.is_ascii_digit()) && !r#ref.is_empty() {
                // backend id path
                let ws = cdp::get_ws_url(None)?;
                let backend: i64 = r#ref.parse().unwrap_or(0);
                if backend!=0 {
                    let resolved = cdp::cdp_call(&ws, "DOM.resolveNode", serde_json::json!({"backendNodeId": backend}))?;
                    if let Some(obj) = resolved.get("object").and_then(|o| o.get("objectId")).and_then(|v| v.as_str()) {
                        let _ = cdp::cdp_call(&ws, "Runtime.callFunctionOn", serde_json::json!({"objectId": obj, "functionDeclaration": "function(){this.click(); return true;}", "returnByValue": true}))?;
                        return Ok(serde_json::json!({"clicked": true, "ref": r#ref, "via":"CDP"}));
                    }
                }
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
            let cmd = format!("brave --remote-debugging-port=9222 --force-renderer-accessibility --new-window {}", url);
            let rule = if ws.is_empty() { "".to_string() } else { format!("[workspace {} silent] ", ws) };
            hypr::dispatch("exec", &format!("{}{}", rule, cmd))?;
            std::thread::sleep(std::time::Duration::from_millis(800));
            Ok(serde_json::json!({"launched": url}))
        },
        _ => anyhow::bail!("unknown tool {}", name),
    }
}
