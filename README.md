# hyprfast

**hypruse is slow because it forks.** `hyprfast` fixes that.
| hypruse 0.9.4 (Python) | hyprfast 0.9.0 (Rust) | speedup |
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
Agent --(MCP stdio)--> hyprfast 0.9.0 --(Unix socket)--> Hyprland
                             |
                             +--(zbus D-Bus)--> AT-SPI a11y bus (no busctl)
                             +--(HTTP+WS :9222)-> Brave/Chromium CDP (no Node)  — single connect_async (src/browser_runtime/connection.rs:364)
                                 |-- Page/DOM/Accessibility.getFullAXTree/Runtime/Network/Input/Target
                                 +-- Hint overlay Vimium-primary (src/hint/mod.rs, assets/hint.js) — DOM scan + labeled overlay A S D F + shadow piercing + [draggable], parallel batch via hint_act/hint_batch (1 snapshot + batched LLM + parallel dispatch)
                                 +-- Stagehand hybrid AX (src/stagehand/snapshot + a11y/*) + LLM → act/observe/extract/agent (fallback after hint)
                                 +-- Vision grounding (src/ground.rs:1) — screenshot JPEG 0.5x + Gemini Flash → {x,y}, last resort only if hint count==0
                                 +-- Resolution order per action: 1) hint-key Vimium-primary (hint_snapshot + heuristic/LLM batch → hint_click/type, parallel) → 2) a11y/AX CDP → 3) vision (ground + OS pointer, kept if nothing works)
                                 +-- Stagehand metrics (src/stagehand/instrumentation.rs:1) records tier distribution a11y/hint/vision per act/observe/agent step
                                 +-- Excalidraw lightning (src/excalidraw/mod.rs:1) — excalidrawAPI.updateScene via React Fiber, batch ~120ms/50 els, templates flowchart/sequence/microservices/aws/network/er/custom, fit bbox zoom 0.7
                             +--(grim)--------> screenshot only as fallback (desktop/browser_shot)
                             +--(uinput/wtype)-> input only when DoAction unavailable or vision fallback
                             +--(Stagehand runtime) 68 MCP tools incl. stagehand_* + hint_* (snapshot/click/type/act/batch/clear) + ground + task_* + excalidraw_* (draw/diagram/export/fit)
                             +--(Task State) src/task.rs persistent todo $XDG_RUNTIME_DIR/hyprfast-tasks.json
                             +--(Excalidraw) src/excalidraw/mod.rs + docs/excalidraw-capture.md (80 shortcuts, dual canvas, export modal, full element spec)
```

**Key design choices:**

1. **No forks.** Hyprland IPC is `UnixStream` `j/<cmd>` / `dispatch <cmd>` directly, not `hyprctl` binary. AT-SPI is `zbus::Connection` to `org.a11y.Bus`, not `busctl` shell.
2. **DoAction > pointer.** `click_ui` calls `org.a11y.atspi.Action.DoAction(0)` on the accessible, no `movecursor`+`click`. Works even when window is not visible / on other workspace.
3. **MCP + CLI.** Same binary serves `hyprfast desktop|hypr|launch|ui|click` for humans and `hyprfast mcp` for agents (opencode, Claude). Drop-in replacement for `hypruse` tool names.
4. **Screenshot is fallback.** `desktop` + `ui` return structured JSON (few hundred tokens). `screenshot` only when app exposes no a11y tree (terminals, canvas).
5. **Vimium-primary + vision last resort (v0.8).** Every `stagehand_act` / `hint_act` resolves via **1) hint-key Vimium-primary** (`hint_snapshot` DOM scan + shadow piercing + `[draggable]` → heuristic exact text/role, else batched LLM → `hint_click`/`hint_type`, parallel `hint_batch` one snapshot + one LLM for N steps) → on no match → **2) a11y/AX CDP** → only if hint finds 0 candidates (canvas/WebGL/custom-drawn) → **3) vision grounding** (`ground` screenshot + Gemini Flash → OS pointer, kept if nothing works). `stagehand_metrics` shows `tier: {a11y, hint, vision}` distribution per step.

## Usage

```bash
cargo install --path .          # or cargo build --release
hyprfast desktop                # instant snapshot, same shape as hypruse desktop()
hyprfast hypr workspace 3
hyprfast hypr focus 0x55a6953facd0
hyprfast launch "foot" --workspace 2
hyprfast ui --name "Save"
hyprfast click "Save"

# Astra-like visual grounding (v0.7 — works on canvas/draw/color-pickers, no AX tree needed)
hyprfast ground "the Brave address bar URL field"
hyprfast act-fast "the Login button" --action click
hyprfast act-fast "the search input" --action type --text "hello"
hyprfast act-batch '[{"instruction":"search input","action":"click"},{"instruction":"first result","action":"click"}]'
# GROUND_MODEL=gemini-3.5-flash-lite for ~2x speed (default gemini-2.5-flash, key from ~/.config/hyprfast/stagehand.env)

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

Tools exposed (68): `desktop`, `hypr`, `launch` (auto `--remote-debugging-port` for browsers), `ui`, `click_ui`, `pointer`, `keyboard`, `screenshot`, `wait_for`, `binds` + **CDP browser** `browser_navigate`, `browser_snapshot`, `browser_click`, `browser_hover`, `browser_type`, `browser_select_option`, `browser_press_key`, `browser_wait`, `browser_evaluate`, `browser_screenshot`, `browser_tabs`, `browser_console`, `browser_go_back/forward`, `browser_open`, `browser_execute_plan` — full `@browsermcp/mcp` parity without Node + **Stagehand** `stagehand_act`, `stagehand_observe`, `stagehand_extract`, `stagehand_agent`, `stagehand_snapshot`, `stagehand_cache`, `stagehand_metrics` (tier `a11y`/`hint`/`vision`), `stagehand_batch`, `stagehand_webmcp` + `context_pages`, `context_cookies`, `clipboard_*` — full `browserbase/stagehand` port without Node + **Ground/Vision** `ground`, `act_fast`, `act_batch` (last-resort tier, `src/ground.rs:1`) + **Hint Vimium-primary** `hint_snapshot`, `hint_click`, `hint_type`, `hint_act`, `hint_batch`, `hint_clear` (`assets/hint.js`, `src/hint/mod.rs:1`, parallel, `[draggable]` + shadow piercing) + **Task State** `task_init/status/update/add/clear/next` (`src/task.rs:1`) + **Excalidraw Lightning** `excalidraw_open/get_scene/clear/draw/draw_batch/update_scene/diagram/export/save/view/fit` (`src/excalidraw/mod.rs:1`) — 11 tools for whiteboard diagrams/architecture. Order `hint (Vimium-primary, parallel) → a11y → vision (kept if nothing works)`.

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
- [x] v0.7: Astra-like visual grounding — `ground` (screenshot JPEG 0.5x + Gemini Flash vision → `{x,y}`, `src/ground.rs:1`), fused `act_fast`/`act_batch` (ground+click/type in ONE MCP call, OS pointer = trusted gesture for canvas/draw/color-pickers), CDP ws_url 2s cache (`src/cdp/mod.rs:63`), 50 MCP tools
- [x] v0.8: Chrome DevTools MCP backend + Hint-key Vimium-primary (parallel) — **DevTools MCP** `npx chrome-devtools-mcp@latest` stdio proxy (`src/devtools_mcp/process.rs:1`, `proxy.rs:1`), all `browser_*` tools route through proxy when available with 2s DevTools target/session cache, legacy `BrowserRuntime` daemon remains as fallback, `--workspace` launch flag and single `connect_async` invariant preserved (`src/browser_runtime/connection.rs:364`) + **Hint Vimium-primary** `hint_snapshot`/`hint_click`/`hint_type`/`hint_act`/`hint_batch`/`hint_clear` (`assets/hint.js`, `src/hint/mod.rs:1`), DOM scan + labeled overlay + shadow piercing + `[draggable]` + parallel `hint_batch` (1 snapshot + batched LLM + `thread::scope` parallel dispatch, CDP concurrent per rule 29, per-target queue via server), **order** `hint (Vimium-primary, parallel) → a11y → vision (kept if nothing works, `src/ground.rs:1` last resort)` (`src/stagehand/act.rs:1` hint_act fast path before AX, `src/stagehand/observe.rs:1`), heuristic exact-text fast-path + batched LLM (`src/hint/mod.rs:resolve`), `stagehand_metrics` tier `a11y`/`hint`/`vision` + per-step logging in `src/stagehand/agent.rs:1`, 57 MCP tools (`Cargo.toml:3` `0.8.0-dev`)
- [x] v0.9: Excalidraw Lightning — **Excalidraw** whiteboard automation (`src/excalidraw/mod.rs:1`) deep capture (80 shortcuts, dual canvas `1882×858`, export `Background|Dark|EmbedScene|Scale`, full `ExcalidrawElement` spec) + **lightning injection** via live `excalidrawAPI.updateScene` (React Fiber `__reactFiber*` BFS `memoizedProps.excalidrawAPI`, `src/excalidraw/mod.rs:42`) `~120ms/50` vs `~3s` pointer drag (**25×**), 11 tools `excalidraw_open/get_scene/clear/draw/draw_batch/update_scene/diagram/export/save/view/fit` (`src/main.rs:1` + MCP `handle_tool`), templates `flowchart|sequence|microservices|architecture|aws|3tier|network|er|custom` auto-layout + `fit` bbox `zoom 0.7`, 68 MCP tools (`Cargo.toml:3` `0.9.0-dev`) — any architecture/diagram/complex drawing in one call (`docs/excalidraw-capture.md:1`)