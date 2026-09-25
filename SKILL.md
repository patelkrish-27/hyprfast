---
name: hyprfast
description: Fast Rust alternative to hypruse — persistent Hyprland IPC + direct zbus AT-SPI + CDP browser automation + Stagehand LLM (act/observe/extract/agent) + Task State (task_init/status/update) + Excalidraw lightning (excalidraw_draw/diagram/export), MCP+CLI. Use whenever you need desktop/window/workspace ops, AT-SPI clicks, Brave/Chromium automation via Chrome DevTools, WhatsApp Web, multi-step todo tracking, or Excalidraw whiteboard diagrams/architecture. Consult for hyprfast's desktop/hypr/launch/ui/click_ui/pointer/keyboard/screenshot/wait_for/binds + browser_navigate/snapshot/click/type/evaluate/screenshot/tabs + stagehand_act/observe/extract/agent + task_init/status/update/clear/next + excalidraw_draw/diagram/export/fit tools. Routed via skills/opencode/SKILL.md rows 5-7.
---

> **Router:** Read `skill/browser/SKILL.md` first for browser recipes (snapshot/ref rules, Gemini Copy-button DOM). This file owns *transport* (CDP/Stagehand/Hyprland), not recipes.

# hyprfast (0.9.0-dev)

Single Rust binary replacing `hypruse` + `@browsermcp/mcp`: direct Unix-socket Hyprland IPC (`$XDG_RUNTIME_DIR/hypr/<sig>/.socket.sock`), persistent `zbus` AT-SPI (no `busctl` per node), CDP browser automation, Stagehand LLM automation (`google/gemini-2.5-flash`), and Excalidraw whiteboard control. 68 MCP tools total (`desktop`, `hypr`, `launch`, `ui`, `click_ui`, `pointer`, `keyboard`, `screenshot`, `wait_for`, `binds`, `browser_*`, `stagehand_*`, `task_*`, `excalidraw_*`).

## Quick reference

Two calling conventions — do not mix them. **MCP** (tool calls): `key=value` named params. **CLI** (terminal): subcommand + `--flag value`.

| Want to... | MCP tool call | CLI |
|---|---|---|
| Snapshot (<5ms) | `desktop` | `hyprfast desktop` |
| Window/workspace ops | `hypr action=workspace target=3` (also `focus_window`/`move_window`/`close_window`/`fullscreen`/`toggle_floating`) | `hyprfast hypr workspace 3` |
| Launch (blocks on `openwindow`, returns address) | `launch command="brave --new-window <url>"` | `hyprfast launch "brave --new-window <url>"` — auto-injects `--remote-debugging-port=9222` for brave/chromium/chrome |
| AT-SPI tree | `ui window=0x... name="Save"` | `hyprfast ui --window 0x... --name Save` |
| Click by name via `DoAction` | `click_ui name="OK" window=0x...` | `hyprfast click OK --window 0x...` |
| Mouse | `pointer action=move x=600 y=400` | `hyprfast pointer move --x 600 --y 400` |
| Keyboard (focuses window first) | `keyboard action=type text="hello" window=0x...` (or `action=key keys="ctrl+k"`) | `hyprfast keyboard type --text hello --window 0x...` |
| Screenshot (fallback only, auto-tracked) | `screenshot window=0x...` | `hyprfast screenshot --window active` |
| Clear tracked screenshots | `clear_screenshots all=false` (or `all=true` to sweep untracked leftovers) | `hyprfast clear` (or `hyprfast clear --all`) |
| Session status | `session_status` | `hyprfast session status` |
| Block on compositor event | `wait_for event=window_open match=WhatsApp timeout_s=5` | `hyprfast wait window_open --match-str WhatsApp --timeout 5` |
| Keybinds | `binds` | `hyprfast binds` |
| Browser navigate | `browser_navigate url="https://example.com"` | `hyprfast browser navigate https://example.com` |
| Browser snapshot (AX refs) | `browser_snapshot` | `hyprfast browser snapshot` |
| Browser click | `browser_click element="Submit" ref="12"` | `hyprfast browser click --selector "button.submit"` |
| Browser type | `browser_type element="Search" ref="5" text="hello" submit=true` | `hyprfast browser type "hello" --selector "input"` |
| Browser eval | `browser_evaluate js="document.title"` | `hyprfast browser eval "document.title"` |
| Browser screenshot (CDP) | `browser_screenshot` | `hyprfast browser shot` |
| Browser tabs | `browser_tabs` | `hyprfast browser tabs` |
| Launch Brave with CDP | `browser_open url="https://..."` | `hyprfast browser open https://... --workspace 3` |
| Stagehand act | `stagehand_act instruction="click the login button"` | `hyprfast stagehand act "click the login button"` |
| Stagehand observe | `stagehand_observe instruction="find login elements"` | `hyprfast stagehand observe "find login elements"` |
| Stagehand extract | `stagehand_extract instruction="extract products" schema="{\"products\":[]}"` | `hyprfast stagehand extract "extract products"` |
| Stagehand snapshot | `stagehand_snapshot` | `hyprfast stagehand snapshot` |
| Stagehand agent | `stagehand_agent instruction="book cheapest flight"` | `hyprfast stagehand agent "book cheapest flight"` |
| Task init | `task_init goal="play boomshakalaka" steps=["search","click","verify"]` | `hyprfast task init "play boomshakalaka" --steps '["search","click","verify"]'` |
| Task status | `task_status` | `hyprfast task status` |
| Task update | `task_update index=0 status="completed"` | `hyprfast task update --index 0 completed` (or `--id 1`) |
| Task next/add/clear | `task_next` / `task_add description="new step"` / `task_clear` | `hyprfast task next` / `hyprfast task add "new step"` / `hyprfast task clear` |
| Excalidraw open | `excalidraw_open url="https://excalidraw.com/"` | `hyprfast excalidraw open` |
| Excalidraw draw | `excalidraw_draw type="rectangle" x=100 y=100 width=200 height=80 label="Backend"` | `hyprfast excalidraw draw '{"type":"rectangle","x":100,"y":100,"width":200,"height":80,"label":"Backend"}'` |
| Excalidraw batch | `excalidraw_draw_batch elements=[{...},{...}]` | `hyprfast excalidraw draw-batch '[{"type":"rectangle",...}]'` |
| Excalidraw diagram | `excalidraw_diagram kind="microservices" params={title,services,databases}` | `hyprfast excalidraw diagram microservices '{"services":["API Gateway","Auth"],"databases":["Postgres"]}'` |
| Excalidraw scene | `excalidraw_get_scene` / `excalidraw_clear` / `excalidraw_update_scene elements=[...] mode=append\|replace` | `hyprfast excalidraw get-scene` |
| Excalidraw export | `excalidraw_export format="png" scale=1 background=true` | `hyprfast excalidraw export '{"format":"png"}'` |
| Excalidraw view/fit | `excalidraw_view json="{\"scrollX\":0}"` / `excalidraw_fit` | `hyprfast excalidraw fit` |

**Rules:**
- `desktop` first, act on `address`. Never screenshot to locate windows.
- `ui` > `screenshot+zoom` for native apps. Brave/Chromium needs `--force-renderer-accessibility` or its AT-SPI tree is empty — for browsers, prefer `stagehand_act`/`stagehand_snapshot` (works without the flag) over `ui`.
- `wait_for` > `sleep`.
- `click_ui` uses `DoAction(0)`, falls back to pointer.
- Prefer `address:` selector over `class:` regex; re-verify `desktop` after any focus change (`address` goes stale on window close).

## Browser Automation (CDP + Stagehand)

No Node/`@browsermcp/mcp`/Playwright needed — `hyprfast` is the sole `browser_*`/`stagehand_*` provider (uses `GET /json` discovery + WS JSON-RPC).

**Priority: Stagehand → Eval fallback → Snapshot → Screenshot (verify only).**
Always try `stagehand_act`/`stagehand_observe` first — it handles AX discovery, self-heal, and ref invalidation without hand-crafted selectors. Fall back to `browser_evaluate` only on an explicit `No action found` / `not found` / `backend resolve` error, or when the target is a `contenteditable` editor not exposed in the AX tree. Use `browser_snapshot`/`browser_click` only if both fail. `browser_screenshot` is verification-only, never for locating elements. This priority holds even if the user says "use stagehand" or if eval would be faster for a single step — only skip Stagehand if the user explicitly says "no LLM" or "eval only".

Flow: `desktop` → `hypr focus_window` or `browser_open url=... workspace=N` (auto-adds CDP flags) → `stagehand_act`/`stagehand_observe` → `browser_evaluate` (fallback) → `browser_snapshot`/`browser_click` (last structural) → `browser_screenshot` (verify).

**Env:** `HYPRFAST_CDP_HOST`/`PORT` default `127.0.0.1:9222`. `CDP unreachable` means the browser wasn't launched with `--remote-debugging-port=9222` — use `browser_open`, not a bare `brave`/`chromium` launch.

**Multi-tab targeting:** `browser_evaluate`/`browser_snapshot` can hit the wrong tab when >1 page is open. Before acting, call `browser_tabs`; if the target tab already exists, activate it (`/json/activate/<id>`) instead of opening a new one, and close stray duplicate tabs pointed at the same URL.

## Stagehand (LLM-driven browser automation)

Hybrid AX + LLM: `Accessibility.getFullAXTree` → `google/gemini-2.5-flash` (or `openai/gpt-4o-mini`) → deterministic `DOM.resolveNode`/`Runtime.callFunctionOn` action. Default for all browser tasks.

**Config** — `~/.config/hyprfast/stagehand.env` (mode 600, gitignored) or env vars:
```
GEMINI_API_KEY=... / GOOGLE_API_KEY / GOOGLE_GENERATIVE_AI_API_KEY
STAGEHAND_MODEL=google/gemini-2.5-flash   # default if Google key present, else openai/gpt-4o-mini
STAGEHAND_BASE_URL=   STAGEHAND_SYSTEM_PROMPT=
```

**Canonical flow:**
```text
1. brave --user-data-dir=/tmp/hyprfast-session --remote-debugging-port=9222 --force-renderer-accessibility --new-window <url>
   # or: browser_open url=<url> workspace=N
2. stagehand_snapshot                                   # verify AX tree is populated
3. stagehand_act instruction="Type '<text>' into the prompt box aria-label '<label>' and press Enter"
4. On "No action found"/"not found"/"backend resolve" error → fall back once:
   browser_evaluate js="(() => { const el=document.querySelector('<selector>'); el.focus(); document.execCommand('insertText',false,'<text>'); el.dispatchEvent(new KeyboardEvent('keydown',{key:'Enter',bubbles:true})); })()"
5. If still failing → browser_snapshot + browser_click with ref
6. Verify: browser_evaluate js="document.body.innerText.slice(-3500)" or browser_screenshot
```

**Troubleshooting:**
| Symptom | Fix |
|---|---|
| `CDP unreachable` | Launch with `--remote-debugging-port=9222` (or use `browser_open`) |
| `brave-browser exposes no accessibility tree` | Add `--force-renderer-accessibility`, or use `stagehand_snapshot` (works without the flag) |
| `LLM did not return actionable element` | Retry once; if persistent, fall back to `browser_evaluate` |
| `Could not find object with given id` | AX node is stale — fall back to `browser_evaluate` |

## Brave/Chromium gotchas

- Single-instance: `brave --new-window` reuses the existing process — if that process launched without `--force-renderer-accessibility`, new windows still have no AT-SPI tree. Kill and relaunch with the flag (avoid `pkill -f`, see Known gotchas).
- Window class is `brave-browser`, not `chromium`.
- For WhatsApp Web without the flag, don't use `ui`/`click_ui` — use keyboard shortcuts or `browser_evaluate` instead.

## WhatsApp Web — shortcuts

Via `keyboard action=key keys="..." window=0x...` (focuses window, then sends chord). **Use the Windows/Linux column on Omarchy/Hyprland** (model translates `Cmd`→`ctrl`).

| Action | Keys | Action | Keys |
|---|---|---|---|
| New chat | `ctrl+alt+n` | Search | `ctrl+alt+slash` (or `ctrl+k`) |
| New group | `ctrl+alt+shift+n` | Search in chat | `ctrl+alt+shift+f` |
| Archive chat | `ctrl+alt+e` | Next/prev chat | `ctrl+alt+tab` / `ctrl+alt+shift+tab` |
| Mute chat | `ctrl+alt+shift+m` | Close chat | `esc` |
| Pin chat | `ctrl+alt+shift+p` | Emoji panel | `ctrl+alt+e` |
| Mark unread | `ctrl+alt+shift+u` | GIF panel | `ctrl+alt+g` |
| Delete chat | `ctrl+alt+backspace` | Sticker panel | `ctrl+alt+s` |
| Profile | `ctrl+alt+p` | Settings | `ctrl+alt+comma` |

**DOM selectors (for `browser_evaluate` fallback):**
- Chat list item: `[data-testid="cell-frame-container"]`
- Message input: `[data-testid="conversation-compose-box-input"]`
- Send button: `[data-testid="send"]`
- Search input: `[data-testid="chat-list-search"]`

**Flow A — evaluate (fastest, ~0.5s):**
```text
{"tool":"desktop"} → {"tool":"hypr","action":"focus_window","target":"0x..."}
→ {"tool":"browser_evaluate","js":"document.querySelector('[data-testid=\"cell-frame-container\"]').click()"}
→ {"tool":"browser_evaluate","js":"const ed=document.querySelector('[data-testid=\"conversation-compose-box-input\"]');ed.focus();document.execCommand('insertText',false,'hello');document.querySelector('[data-testid=\"send\"]').click()"}
```

**Flow B — keyboard fallback (if evaluate fails):**
```text
{"tool":"desktop"} → {"tool":"hypr","action":"focus_window","target":"0x..."}
→ {"tool":"keyboard","action":"key","keys":"ctrl+alt+slash","window":"0x..."}
→ {"tool":"keyboard","action":"type","text":"Khushi","window":"0x..."} → {"tool":"keyboard","action":"key","keys":"enter"}
→ wait ~0.8s → {"tool":"keyboard","action":"type","text":"hello"} → {"tool":"keyboard","action":"key","keys":"enter"}
```
Only screenshot for visual confirm (`scale=0.5`, JPEG ~80KB). Keyboard (~50ms/key) beats a vision loop (2-4s) for WhatsApp Web.

**Window placement:** if the Brave/WhatsApp window is tiled (not fullscreen), the omnibox overlay traps keyboard focus. Before any WhatsApp keyboard flow: check `desktop` for `class==brave-browser` + title contains `WhatsApp`; if not fullscreen, move it to an empty workspace (`desktop.workspaces` where `windows==0`, else use `10`) and fullscreen it there before continuing.

## Screenshot session

Every `screenshot` call auto-appends to `$XDG_RUNTIME_DIR/hyprfast-session.json` (fallback `/tmp`).
- After a successful task: `clear_screenshots all=false` — deletes only tracked files, truncates session to `[]`.
- Full cleanup of stale leftovers: `clear_screenshots all=true`.
- Inspect: `session_status`.
- Pattern: screenshot as needed during a task → clear on success. On failure, keep shots for debugging, then `clear --all`.

## Task State — persistent todo for multi-step actions

File: `$XDG_RUNTIME_DIR/hyprfast-tasks.json` (fallback `/tmp`) — single active list `{goal, steps:[{id,description,status}], progress}`. Status: `pending|in_progress|completed|failed|skipped`. Auto-clears when every step is `completed|skipped`.

| Tool | Params | When |
|---|---|---|
| `task_init` | `goal`, `steps[]` | Start: break the request into ordered steps before acting |
| `task_status` | — | Resume: check progress + next pending step after any failure/timeout, before retrying |
| `task_update` | `index` or `id`, `status` | After each step completes/fails; last completion auto-clears |
| `task_next` | — | Get next pending step |
| `task_add` | `description` | Append a step mid-flow |
| `task_clear` | — | Cancel/reset |

**CLI:**
```bash
hyprfast task init "play boomshakalaka on youtube" --steps '["search","click first video","verify playing"]'
hyprfast task status
hyprfast task update --index 0 completed   # or --id 1
hyprfast task next
hyprfast task clear
# --steps also accepts a comma list: --steps "search,click,verify"
```

**Rules:**
- `task_init` before the first browser step for any request with ≥2 steps.
- `task_update` immediately after each step — don't batch.
- On retry after a timeout/failure, call `task_status` first and resume from the next pending step — don't restart from step 1, and don't re-open a tab that already exists (check `browser_tabs` and reuse/activate instead of duplicating).

## Excalidraw — whiteboard diagrams

For `https://excalidraw.com`: draws by injecting scene state directly into `excalidrawAPI` via `Runtime.evaluate` `updateScene` (~120ms for a full diagram) instead of pointer-dragging shapes (~2-4s each). Use for any architecture/flow/sequence/network/ER diagram.

| Tool | Params | When |
|---|---|---|
| `excalidraw_open` | `url?` (default `https://excalidraw.com/`) | Ensure tab before drawing |
| `excalidraw_get_scene` | — | Audit `elements.length` + `appState` |
| `excalidraw_clear` | — | Reset to `[]` |
| `excalidraw_draw` | `type,x,y,width,height,x2,y2,text,label,strokeColor,backgroundColor,fillStyle,strokeWidth,points,name` | Single primitive |
| `excalidraw_draw_batch` | `elements:[{type,...}]` | Batch N primitives in one call |
| `excalidraw_update_scene` | `elements:[...]`, `mode: append\|replace` | Raw scene / custom layout / replaying saved `.excalidraw` JSON |
| `excalidraw_diagram` | `kind: flowchart\|sequence\|microservices\|architecture\|aws\|3tier\|network\|er\|custom`, `params:{title,services,databases,participants,messages,nodes,entities,steps}` | Auto-layout template |
| `excalidraw_export` | `format: png\|svg\|clipboard, background, dark, embedScene, scale` | Export |
| `excalidraw_save` | `path?` | Trigger `.excalidraw` download |
| `excalidraw_view` / `excalidraw_fit` | `json{scrollX,scrollY,zoom}` / — | Viewport control / auto-center on bbox |

**CLI:**
```bash
hyprfast excalidraw open
hyprfast excalidraw clear
hyprfast excalidraw draw '{"type":"rectangle","x":100,"y":100,"width":200,"height":80,"backgroundColor":"#a5d8ff","label":"Backend"}'
hyprfast excalidraw draw-batch '[{"type":"rectangle","x":100,"y":200,"width":180,"height":70,"label":"Service A"},{"type":"arrow","x":280,"y":235,"x2":400,"y2":235}]'
hyprfast excalidraw diagram microservices '{"title":"E-Commerce","services":["API Gateway","Auth","Orders"],"databases":["Postgres"]}'
hyprfast excalidraw get-scene
hyprfast excalidraw export '{"format":"png","scale":1}'
hyprfast excalidraw fit
```

**Canonical flow:**
```text
1. excalidraw_open
2. excalidraw_clear  // skip if appending to existing scene
3. excalidraw_diagram kind="microservices" params={title:"...", services:[...], databases:[...]}
   // or excalidraw_draw_batch for a fully custom layout
4. excalidraw_fit
5. excalidraw_get_scene (verify element count) + browser_screenshot or excalidraw_export
```

## Safety

- Confirm recipient/content before sending (`Enter`) — same caution as `close_window`.
- Re-verify `desktop` after any focus change; a stale `address` will target the wrong window.

## Known gotchas

- **Never `pkill -f`** — hangs indefinitely on this host (tries to signal privileged PIDs and self-matches the invoking shell). Use `pgrep -f X | xargs -r kill`, `lsof -ti:PORT | xargs -r kill`, or `fuser -k PORT/tcp` instead, and never chain it with `;` before starting a persistent process.
- **Don't duplicate browser tabs on retry.** Before `browser_open`/`browser_navigate`, check `browser_tabs` for an existing tab at the same URL and activate it instead of opening a new one — duplicates cause `browser_evaluate`/`browser_snapshot` to target the wrong tab.
- **Don't restart a multi-step task from scratch after a timeout.** Call `task_status` (and `browser_tabs` for browser tasks) to find what's already done, then resume from the next pending step.
- **CDP eval before AT-SPI flag fix:** if `brave-browser exposes no accessibility tree` appears, don't loop on `ui`/`click_ui` — either add `--force-renderer-accessibility` or switch to `stagehand_snapshot`, which works without it.
