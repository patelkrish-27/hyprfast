mod hypr;
mod a11y;
mod input;
mod screenshot;
mod events;
mod daemon;
mod session;

use clap::{Parser, Subcommand};
use anyhow::Result;

#[derive(Parser)]
#[command(name="hyprfast", version, about="Fast hypruse alternative - persistent IPC, direct AT-SPI")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
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
    Mcp,
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
            let rule = workspace.map(|w| format!("[workspace {} silent] ", w)).unwrap_or_default();
            hypr::dispatch("exec", &format!("{}{}", rule, command))?;
            std::thread::sleep(std::time::Duration::from_millis(500));
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
        {"name":"launch","description":"Launch app via Hyprland exec","inputSchema":{"type":"object","properties":{"command":{"type":"string"},"workspace":{"type":"string"}}}},
        {"name":"ui","description":"AT-SPI accessible tree (fast, no screenshot)","inputSchema":{"type":"object","properties":{"window":{"type":"string"},"name":{"type":"string"}}}},
        {"name":"click_ui","description":"Click by accessible name via DoAction (no pointer, no screenshot)","inputSchema":{"type":"object","properties":{"name":{"type":"string"},"window":{"type":"string"},"mark":{"type":"integer"}}}},
        {"name":"pointer","description":"Mouse: move|click|drag|scroll at global logical coords","inputSchema":{"type":"object","properties":{"action":{"type":"string"},"x":{"type":"number"},"y":{"type":"number"},"button":{"type":"string"},"to_x":{"type":"number"},"to_y":{"type":"number"},"scroll_dy":{"type":"number"},"scroll_dx":{"type":"number"}}}},
        {"name":"keyboard","description":"Keyboard: type (text) or key (combo like ctrl+t). window focuses first.","inputSchema":{"type":"object","properties":{"action":{"type":"string"},"text":{"type":"string"},"keys":{"type":"string"},"window":{"type":"string"}}}},
        {"name":"screenshot","description":"Capture via grim: window, region, or monitor. Returns file path + meta. Auto-tracked for session clear.","inputSchema":{"type":"object","properties":{"window":{"type":"string"},"region":{"type":"string"},"scale":{"type":"number"}}}},
        {"name":"wait_for","description":"Block on Hyprland events: window_open/window_close/workspace/title_change/layer_open/layer_close","inputSchema":{"type":"object","properties":{"event":{"type":"string"},"match":{"type":"string"},"timeout_s":{"type":"number"}}}},
        {"name":"binds","description":"List Hyprland keybinds","inputSchema":{"type":"object","properties":{}}},
        {"name":"clear_screenshots","description":"Clear tracked screenshots (/tmp/hyprfast-*.png) after successful task — deletes files recorded in session and resets list. Use all=true to also delete untracked leftovers.","inputSchema":{"type":"object","properties":{"all":{"type":"boolean"}}}},
        {"name":"session_status","description":"Show screenshot session status (tracked files, bytes, session file path)","inputSchema":{"type":"object","properties":{}}}
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
            let rule = if ws.is_empty() { "".to_string() } else { format!("[workspace {} silent] ", ws) };
            hypr::dispatch("exec", &format!("{}{}", rule, cmd))?;
            std::thread::sleep(std::time::Duration::from_millis(400));
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
        _ => anyhow::bail!("unknown tool {}", name),
    }
}
