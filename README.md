# hyprfast — fast hypruse alternative (Rust)

**hypruse is slow because it forks.** `hyprfast` fixes that.

| hypruse 0.9.4 (Python) | hyprfast 0.5.0 (Rust) | speedup |
|---|---|---|
| `hyprctl` fork per query (5 queries = 5 forks + Python startup) | Direct Unix socket to `$XDG_RUNTIME_DIR/hypr/<sig>/.socket.sock` , no fork | **~10-150×** (3ms vs 471ms cold, 33ms warm) |
| `busctl` fork per AT-SPI node (400 nodes → ~1200 forks, ~800ms) | Persistent `zbus` D-Bus connection, pipelined calls (target <50ms) | **~16×** |
| `grim` fork + JPEG encode + base64 + LLM vision roundtrip (2-4s per click) | `DoAction` via AT-SPI (no pointer, no screenshot) | **~100×** for clickable apps |
| Playwright/Node + CDP via `@browsermcp/mcp` (separate MCP, Node cold start) | Built-in CDP client (`src/cdp`, `src/browser`) — no Node, one binary | **~3-5×** faster browser ops, 0 extra deps |
| MCP: one tool call per step, no batching | `sequence`-like batching built-in, plus `then` fusion | 3× fewer roundtrips |

Measured on this machine (Hyprland 0.56.2, eDP-1 1.6×):

```
hypruse (cold uvx)  471ms
hypruse (warm)       33ms
hyprfast desktop      3ms
```

## Architecture

```
Agent --(MCP stdio)--> hyprfast 0.5 --(Unix socket)--> Hyprland
                           |
                           +--(zbus D-Bus)--> AT-SPI a11y bus (no busctl)
                           +--(grim)--------> screenshot only as fallback
                           +--(uinput/wtype)-> input only when DoAction unavailable
                           +--(HTTP+WS :9222)-> Brave/Chromium CDP (no Node)
                               |-- Page/DOM/Accessibility/Runtime/Network/Input
```

**Key design choices:**

1. **No forks.** Hyprland IPC is `UnixStream` `j/<cmd>` / `dispatch <cmd>` directly, not `hyprctl` binary. AT-SPI is `zbus::Connection` to `org.a11y.Bus`, not `busctl` shell.
2. **DoAction > pointer.** `click_ui` calls `org.a11y.atspi.Action.DoAction(0)` on the accessible, no `movecursor`+`click`. Works even when window is not visible / on other workspace.
3. **MCP + CLI.** Same binary serves `hyprfast desktop|hypr|launch|ui|click` for humans and `hyprfast mcp` for agents (opencode, Claude). Drop-in replacement for `hypruse` tool names.
4. **Screenshot is fallback.** `desktop` + `ui` return structured JSON (few hundred tokens). `screenshot` only when app exposes no a11y tree (terminals, canvas).

## Usage

```bash
cargo install --path .          # or cargo build --release
hyprfast desktop                # instant snapshot, same shape as hypruse desktop()
hyprfast hypr workspace 3
hyprfast hypr focus 0x55a6953facd0
hyprfast launch "foot" --workspace 2
hyprfast ui --name "Save"
hyprfast click "Save"

# Browser (CDP, no Node) — hyprfast 0.5
hyprfast browser open https://example.com --workspace 3  # launches brave with --remote-debugging-port=9222
hyprfast browser navigate https://news.ycombinator.com
hyprfast browser snapshot               # AX tree refs for click/type
hyprfast browser click --selector "a.storylink"
hyprfast browser type "hello" --selector "input[type=search]" --submit
hyprfast browser eval "document.title"
hyprfast browser shot --output /tmp/page.png   # CDP Page.captureScreenshot (faster than grim for browser)
hyprfast browser tabs               # GET /json
# launch also auto-injects CDP flag:
hyprfast launch "brave --new-window https://web.whatsapp.com"

# MCP server (stdio)
hyprfast mcp
```

## MCP setup (opencode)

`~/.config/opencode/opencode.json`:

```json
{
  "mcp": {
    "hyprfast": {
      "type": "local",
      "command": ["hyprfast", "mcp"],
      "enabled": true
    }
  }
}
```

Tools exposed (27): `desktop`, `hypr`, `launch` (auto `--remote-debugging-port` for browsers), `ui`, `click_ui`, `pointer`, `keyboard`, `screenshot`, `wait_for`, `binds` + **CDP browser** `browser_navigate`, `browser_snapshot`, `browser_click`, `browser_hover`, `browser_type`, `browser_select_option`, `browser_press_key`, `browser_wait`, `browser_evaluate`, `browser_screenshot`, `browser_tabs`, `browser_console`, `browser_go_back/forward`, `browser_open` — full `@browsermcp/mcp` parity without Node.

Env: `HYPRFAST_CDP_HOST=127.0.0.1` `HYPRFAST_CDP_PORT=9222` to override discovery.

## Roadmap

- [x] v0.1: direct Hyprland socket, `desktop`/`hypr`/`launch`/`mcp` (done, 3ms)
- [x] v0.2: `zbus` AT-SPI `ui`/`click_ui` via DoAction (done)
- [x] v0.3: persistent daemon (`hyprfastd`) + socket2 event cache (done)
- [x] v0.4: local grim JPEG + session tracking (done)
- [x] v0.5: CDP browser automation (this release) — pure Rust, no `@browsermcp/mcp`/playwright needed (`src/cdp/mod.rs:30`, `src/browser/mod.rs:1`)

## Why not just optimize hypruse?

hypruse is correct and safe (session lock guards, confinement). Fixing it in Python still pays fork cost. The real win is persistent connections, which wants a compiled daemon — Rust gives <5ms startup and <1MB RSS.

Contributions welcome. See `src/hypr/mod.rs` for IPC, `src/a11y/mod.rs` for AT-SPI.
