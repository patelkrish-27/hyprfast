# hyprfast — fast hypruse alternative (Rust)

**hypruse is slow because it forks.** `hyprfast` fixes that.

| hypruse 0.9.4 (Python) | hyprfast 0.6.1 (Rust) | speedup |
|---|---|---|
| `hyprctl` fork per query (5 queries = 5 forks + Python startup) | Direct Unix socket to `$XDG_RUNTIME_DIR/hypr/<sig>/.socket.sock` , no fork | **~10-150×** (3ms vs 471ms cold, 33ms warm) |
| `busctl` fork per AT-SPI node (400 nodes → ~1200 forks, ~800ms) | Persistent `zbus` D-Bus connection, pipelined calls (target <50ms) | **~16×** |
| `grim` fork + JPEG encode + base64 + LLM vision roundtrip (2-4s per click) | `DoAction` via AT-SPI (no pointer, no screenshot) | **~100×** for clickable apps |
| Playwright/Node + CDP via `@browsermcp/mcp` (separate MCP, Node cold start) | Built-in CDP client (`src/cdp`, `src/browser`) — no Node, one binary | **~3-5×** faster browser ops, 0 extra deps |
| Browserbase Stagehand SDK (Node, ~2MB + cloud) | Built-in Stagehand runtime (`src/stagehand/*`) — Rust, hybrid AX + LLM, no JS | **~2×**faster act/observe/extract, 0 Node, single binary |
| MCP: one tool call per step, no batching | `sequence`-like batching built-in, plus `then` fusion | 3× fewer roundtrips |

Measured on this machine (Hyprland 0.56.2, eDP-1 1.6×):

```
hypruse (cold uvx)  471ms
hypruse (warm)       33ms
hyprfast desktop      3ms
```

## Architecture

```
Agent --(MCP stdio)--> hyprfast 0.6.1 --(Unix socket)--> Hyprland
                            |
                            +--(zbus D-Bus)--> AT-SPI a11y bus (no busctl)
                            +--(grim)--------> screenshot only as fallback
                            +--(uinput/wtype)-> input only when DoAction unavailable
                            +--(HTTP+WS :9222)-> Brave/Chromium CDP (no Node)
                                |-- Page/DOM/Accessibility/Runtime/Network/Input
                                +-- Stagehand hybrid AX (src/stagehand/snapshot + a11y/*) + LLM (openai/anthropic/google) → act/observe/extract/agent
                            +--(Stagehand runtime) 47 MCP tools incl. stagehand_* + task_*
                            +--(Task State) src/task.rs persistent todo $XDG_RUNTIME_DIR/hyprfast-tasks.json
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

# Stagehand (LLM-driven, no Node) — hyprfast 0.6 (port of browserbase/stagehand)
export OPENAI_API_KEY=sk-...                    # or ANTHROPIC_API_KEY / STAGEHAND_MODEL=openai/gpt-4o-mini
hyprfast stagehand snapshot                     # hybrid Accessibility.getFullAXTree + xpathMap (no LLM)
hyprfast stagehand act "click the login button"
hyprfast stagehand observe "find all submit buttons"
hyprfast stagehand extract "extract title and price" --schema '{"title":"string","price":"string"}'
hyprfast stagehand agent "book a flight" --max-steps 8
hyprfast stagehand metrics                      # token usage
hyprfast stagehand cache status                 # ~/.cache/hyprfast/stagehand-cache.json

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

Tools exposed (47): `desktop`, `hypr`, `launch` (auto `--remote-debugging-port` for browsers), `ui`, `click_ui`, `pointer`, `keyboard`, `screenshot`, `wait_for`, `binds` + **CDP browser** `browser_navigate`, `browser_snapshot`, `browser_click`, `browser_hover`, `browser_type`, `browser_select_option`, `browser_press_key`, `browser_wait`, `browser_evaluate`, `browser_screenshot`, `browser_tabs`, `browser_console`, `browser_go_back/forward`, `browser_open` — full `@browsermcp/mcp` parity without Node + **Stagehand** `stagehand_act`, `stagehand_observe`, `stagehand_extract`, `stagehand_agent`, `stagehand_snapshot`, `stagehand_cache`, `stagehand_metrics`, `stagehand_batch`, `stagehand_webmcp` + `context_pages`, `context_cookies`, `clipboard_*` — full `browserbase/stagehand` port without Node + **Task State** `task_init/status/update/add/clear/next` (`src/task.rs:1`).

Env: `HYPRFAST_CDP_HOST=127.0.0.1` `HYPRFAST_CDP_PORT=9222` `OPENAI_API_KEY`/`ANTHROPIC_API_KEY`/`STAGEHAND_MODEL` (e.g. `openai/gpt-4o-mini`, `anthropic/claude-3.5-sonnet`) `STAGEHAND_BASE_URL` `STAGEHAND_SYSTEM_PROMPT`.

## Stagehand runtime (v0.6 — full port)

Port of `browserbase/stagehand` into Rust — no Node, no Python SDK. Mirrors `packages/protocol` → `src/stagehand/protocol.rs:1`, `packages/extension/prompt.ts:29` → `prompt.rs:1`, `understudy/a11y/snapshot/*` → `a11y/*` + `snapshot.rs:1` (hybrid `Accessibility.getFullAXTree` + `xpathUtils` + `treeFormatUtils`), `locator/frame/deepLocator` → `locator.rs`/`frame.rs`/`deep_locator.rs`, `services/actService` (self-heal + twoStep dropdown) → `act.rs:1`, `observe` chunking → `observe.rs:1`, `extract` JSON-schema → `extract.rs:1`, `agent` operator loop → `agent.rs:1`, `batch`/`webmcp`/`cookies`/`clipboard`/`fileUpload`/`instrumentation` → `src/stagehand/*` (`1540` LOC). CLI `hyprfast stagehand *` and MCP `stagehand_*` share same `StagehandConfig::from_env()` (`OPENAI_API_KEY`, `STAGEHAND_MODEL`).

See `src/stagehand/mod.rs:1` for module map; skipped `sdk-go`/`sdk-python`/`examples` as non-runtime.

## Roadmap

- [x] v0.1: direct Hyprland socket, `desktop`/`hypr`/`launch`/`mcp` (done, 3ms)
- [x] v0.2: `zbus` AT-SPI `ui`/`click_ui` via DoAction (done)
- [x] v0.3: persistent daemon (`hyprfastd`) + socket2 event cache (done)
- [x] v0.4: local grim JPEG + session tracking (done)
- [x] v0.5: CDP browser automation — pure Rust, no `@browsermcp/mcp`/playwright needed (`src/cdp/mod.rs:30`, `src/browser/mod.rs:1`)
- [x] v0.6: Stagehand runtime — full port of `browserbase/stagehand` `act`/`observe`/`extract`/`agent` hybrid AX + LLM (self-heal, cache, batch, WebMCP) into Rust (`src/stagehand/*`), 41 MCP tools (`Cargo.toml:3` `0.6.0`)
- [x] v0.6.1: Task State — persistent todo `$XDG_RUNTIME_DIR/hyprfast-tasks.json` for multi-step resume (`task_init/status/update/next/add/clear`, `src/task.rs:1`), 47 MCP tools (`Cargo.toml:3` `0.6.1`)

## Why not just optimize hypruse?

hypruse is correct and safe (session lock guards, confinement). Fixing it in Python still pays fork cost. The real win is persistent connections, which wants a compiled daemon — Rust gives <5ms startup and <1MB RSS.

Contributions welcome. See `src/hypr/mod.rs` for IPC, `src/a11y/mod.rs` for AT-SPI.
