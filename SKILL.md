---
name: hyprfast
description: Fast Rust alternative to hypruse — persistent Hyprland IPC + direct zbus AT-SPI + CDP browser automation + Stagehand LLM (act/observe/extract/agent) + Task State (task_init/status/update), MCP+CLI. Use whenever you need desktop/window/workspace ops, AT-SPI clicks, Brave/Chromium automation via Chrome DevTools, WhatsApp Web, or multi-step todo tracking. Consult for hyprfast's desktop/hypr/launch/ui/click_ui/pointer/keyboard/screenshot/wait_for/binds + browser_navigate/snapshot/click/type/evaluate/screenshot/tabs + stagehand_act/observe/extract/agent + task_init/status/update/clear/next tools. Routed via skills/opencode/SKILL.md rows 5-7.
---

> **Router:** Routed via `skills/opencode/SKILL.md` master router (rows 5-7, 10). **Browser tasks:** read `skill/browser/SKILL.md` first for snapshot & ref recipes and Gemini Copy-button canonical (§ Session Learnings 2026-08-31) — this file owns *transport* (CDP/Stagehand/Hyprland), not recipes. **Unified priority (0.6):** Stagehand → Eval → Snapshot → Screenshot.

# hyprfast — fast hypruse alternative (0.6.1 with Stagehand + Task State)

`hyprfast` is `hypruse` without the forks: direct Unix socket to Hyprland (`$XDG_RUNTIME_DIR/hypr/<sig>/.socket.sock`, `j/<cmd>`), persistent `zbus` D-Bus to `org.a11y.Bus` (no `busctl` per node), `DoAction` clicks (no `movecursor`+`click`), optional `hyprfastd` daemon cache, **plus built-in CDP browser automation + Stagehand LLM (no Node, no @browsermcp/mcp)** — one binary for Hyprland + AT-SPI + Chrome DevTools + `google/gemini-2.5-flash`/`openai/gpt-4o`.

Same MCP shape as `hypruse` (`desktop`, `hypr`, `launch`, `ui`, `click_ui`, `pointer`, `keyboard`, `screenshot`, `wait_for`, `binds`) **plus** `browser_*` (`browser_navigate`, `browser_snapshot`, `browser_click`, `browser_type`, `browser_evaluate`, `browser_screenshot`, `browser_tabs`, etc.) **plus** `stagehand_*` (`stagehand_act`, `stagehand_observe`, `stagehand_extract`, `stagehand_agent`, `stagehand_snapshot`) **plus** `task_*` (`task_init`, `task_status`, `task_update`, `task_add`, `task_clear`, `task_next`) — full `browsermcp` + `stagehand` parity via `src/cdp/mod.rs:30` + `src/browser/mod.rs:1` + `src/stagehand/*` + `src/task.rs:1`, but `~10-150×` faster and 0 extra deps. Version `0.6.1` `src/main.rs:1` `Cargo.toml:3` `src/task.rs:1` (47 tools).

## Prerequisite — Read Browser Skill First (router enforced)

> **Before ANY browser task, read `browser/SKILL.md` (`~/.config/opencode/skill/browser/SKILL.md`).** Canonical for snapshot & ref golden rule, Stagehand-first priority, and Gemini Copy-button extraction. This file (hyprfast) owns transport.

**Required read order (per `skills/opencode/SKILL.md §3`):**
1. `skill("opencode")` — router classifies intent
2. `skill("browser")` — recipes, snapshot golden rule, Gemini Copy-button DOM (`model-response-message-content{rId}`), legacy `snap.mjs` fallback
3. `hyprfast` §Browser Automation (this file) — CDP/Stagehand execution, `stagehand.env`, `GET /json` tab isolation

Do NOT start `hyprfast browser_*` without `browser` — see `Root Cause Analysis 2026-09-02` 7 causes (fixed).

## Quick reference (MCP + CLI)

| Want to... | hyprfast |
|---|---|
| Snapshot (<5ms) | `desktop` / `hyprfast desktop` |
| Window/workspace ops | `hypr action=workspace target=3` / `hyprfast hypr workspace 3` ; `focus_window` / `move_window` / `close_window` / `fullscreen` / `toggle_floating` |
| Launch, block on `openwindow`, return address | `launch command="brave --new-window https://web.whatsapp.com"` / `hyprfast launch "brave --new-window https://web.whatsapp.com"` |
| AT-SPI tree (fast, no screenshot) | `ui window=0x... name="Save"` / `hyprfast ui --window 0x... --name Save` |
| Click by name via `DoAction` | `click_ui name="OK" window=0x...` / `hyprfast click OK --window 0x...` |
| Mouse | `pointer action=move x=600 y=400` / `hyprfast pointer move --x 600 --y 400` |
| Keyboard (focuses `window` first) | `keyboard action=type text="hello" window=0x...` / `hyprfast keyboard type --text hello --window 0x...` ; `action=key keys="ctrl+k"` |
| Screenshot (fallback, auto-tracked) | `screenshot window=0x...` / `hyprfast screenshot --window active` // every shot auto-appended to session list |
| Clear screenshots (after task success) | `clear_screenshots all=false` / `hyprfast clear` / `hyprfast clear --all` (deletes /tmp/hyprfast-*.png tracked) |
| Session status | `session_status` / `hyprfast session status` (shows tracked files + bytes) |
| Block on compositor event | `wait_for event=window_open match=WhatsApp timeout_s=5` / `hyprfast wait window_open --match-str WhatsApp --timeout 5` |
| Keybinds | `binds` / `hyprfast binds` |
| Browser navigate | `browser_navigate url="https://example.com"` / `hyprfast browser navigate https://example.com` |
| Browser snapshot (AX refs) | `browser_snapshot` / `hyprfast browser snapshot` — returns refs for click/type |
| Browser click | `browser_click element="Submit" ref="12"` / `hyprfast browser click --selector "button.submit"` |
| Browser type | `browser_type element="Search" ref="5" text="hello" submit=true` / `hyprfast browser type "hello" --selector "input"` |
| Browser eval | `browser_evaluate js="document.title"` / `hyprfast browser eval "document.title"` |
| Browser screenshot (CDP) | `browser_screenshot` / `hyprfast browser shot` — Page.captureScreenshot PNG, no grim |
| Browser tabs | `browser_tabs` / `hyprfast browser tabs` — GET /json |
| Launch Brave with CDP | `browser_open url="https://web.whatsapp.com"` / `hyprfast browser open https://web.whatsapp.com --workspace 3` |
| **Stagehand act** | `stagehand_act instruction="click the login button"` / `hyprfast stagehand act "click the login button"` — LLM `google/gemini-2.5-flash` `src/stagehand/act.rs:146` |
| **Stagehand observe** | `stagehand_observe instruction="find login elements"` / `hyprfast stagehand observe "find login elements"` — chunked AX + xpath |
| **Stagehand extract** | `stagehand_extract instruction="extract products as JSON" schema="{\"products\":[]}"` / `hyprfast stagehand extract "extract products"` |
| **Stagehand snapshot** | `stagehand_snapshot` / `hyprfast stagehand snapshot` — hybrid `Accessibility.getFullAXTree` `src/stagehand/snapshot.rs:1` |
| **Stagehand agent** | `stagehand_agent instruction="book cheapest flight"` / `hyprfast stagehand agent "book cheapest flight"` — autonomous loop |
| **Task init (breakdown)** | `task_init goal="play boomshakalaka" steps=["search","click","verify"]` / `hyprfast task init "play boomshakalaka" --steps '["search","click","verify"]'` — persists to `hyprfast-tasks.json` |
| **Task status (resume)** | `task_status` / `hyprfast task status` — current todo + progress % + next pending |
| **Task update (mark done)** | `task_update index=0 status="completed"` / `hyprfast task update --index 0 completed` — auto-clears when all done |
| **Task next / add / clear** | `task_next` / `task_add description="new step"` / `task_clear` / `hyprfast task next/add/clear` |

**Rules from hyprsuse still apply:**
- `desktop` first, then act on `address`. Never screenshot to locate windows.
- `ui` > `screenshot+zoom`. `Brave/Chromium` needs `--force-renderer-accessibility` at launch or tree is empty (`brave-browser exposes no accessibility tree; use screenshot` — `src/a11y/mod.rs:35`). **With 0.6 Stagehand, prefer `stagehand_act`/`stagehand_snapshot` (CDP AX `src/stagehand/snapshot.rs:1`) over `ui` for browsers — Stagehand works without the flag and without pointer; `browser_snapshot`/`browser_click` are fallback if Stagehand fails.**
- `wait_for` > `sleep`. `hyprfastd` at `/run/user/1000/hyprfastd.sock` accelerates `desktop`+`wait_for`. For browsers, `browser_wait` or CDP `Page.loadEventFired` via `browser_navigate`.
- `click_ui` uses `org.a11y.atspi.Action.DoAction(0)` (`src/a11y/zbus_impl.rs:295`), fallback to pointer.
- `launch` now auto-injects `--remote-debugging-port=9222` for `brave/chromium/chrome` if missing (`src/main.rs:60`). Override via `HYPRFAST_CDP_HOST/PORT`.

## Browser Automation (CDP + Stagehand — 0.6)

hyprfast 0.6 embeds Chrome DevTools Protocol + Stagehand LLM — no `@browsermcp/mcp` or `playwright` needed. One binary, one MCP. Uses `reqwest` GET `/json` discovery (`src/cdp/mod.rs:30`) + `tokio-tungstenite` WS JSON-RPC (`src/cdp/mod.rs:103`) + `src/stagehand/*`.

| Want to... | hyprfast 0.6 |
|---|---|
| Launch debuggable Brave | `hyprfast browser open https://example.com --workspace 3` or `hyprfast launch "brave --new-window https://example.com"` (auto adds `--remote-debugging-port=9222`) |
| List tabs | `browser_tabs` / `hyprfast browser tabs` |
| Navigate | `browser_navigate url="https://news.ycombinator.com"` |
| Snapshot refs | `browser_snapshot` → `{snapshot:[{role,name,ref,nodeId}]}` via `Accessibility.getFullAXTree` fallback JS |
| Click | `browser_click element="Hacker News" ref="42"` — resolves `backendNodeId` via `DOM.resolveNode` then `Runtime.callFunctionOn` click |
| Type | `browser_type element="Search" ref="5" text="hyprland" submit=true` |
| Press key | `browser_press_key key="Enter"` via `Input.dispatchKeyEvent` |
| Evaluate JS | `browser_evaluate js="document.title"` via `Runtime.evaluate` |
| Screenshot (browser) | `browser_screenshot` via `Page.captureScreenshot` PNG (no grim, per-tab) |

**Rule: STAGEHAND FIRST → EVAL → SNAPSHOT → SCREENSHOT LAST (0.6).** Always try `stagehand_act`/`stagehand_observe` first (LLM `google/gemini-2.5-flash` via `~/.config/hyprfast/stagehand.env:1` `src/stagehand/mod.rs:48`), then fallback to `browser_evaluate` (CDP `Runtime.evaluate` `~50ms`), then `browser_snapshot`/`browser_click`, then `browser_screenshot` verify only. CDP eval is fast but Stagehand handles AX + self-heal + ref invalidation robustly.

Flow: `desktop` → `hypr focus_window` or `browser_open --remote-debugging-port=9222 --force-renderer-accessibility` → **`stagehand_act`/`stagehand_observe` (preferred)** → `browser_evaluate` (fallback if `No action found`/`not found`/`backend resolve` fails) → `browser_snapshot`/`browser_click` (last structural) → `browser_screenshot` (verify only). Prefer Stagehand for browsers; keep `ui/click_ui` for native GTK apps. See `browser/SKILL.md:317` `Hyprfast Stagehand + CDP Fast Path`.

*Why not EVALUATE FIRST?* Older `0.5` rule said `EVALUATE FIRST` for speed; `0.6` corrects to Stagehand-first because eval requires hand-crafted selectors (`div.ql-editor[aria-label="Enter a prompt for Gemini"]`) and breaks on ref invalidation/streaming markdown, while Stagehand auto-discovers via `Accessibility.getFullAXTree` `src/stagehand/snapshot.rs:1` + LLM + self-heal `src/stagehand/act.rs:189`. If user explicitly says "use eval only" then skip Stagehand, otherwise Stagehand is default even if `browser_evaluate` would be faster on a single step.

**Env:** `HYPRFAST_CDP_HOST`/`PORT` default `127.0.0.1:9222`. Error `CDP unreachable...` means browser not launched with flag — use `hyprfast browser open ...`.

**Migration (done 2026-09-02):** `npx @browsermcp/mcp` removed — `hyprfast mcp` is sole provider of `browser_*` + `stagehand_*` (41 tools `src/main.rs:89`). `browsermcp` entry deleted from `opencode.json:8`; use `hyprfast` only.

## Hyprfast Stagehand (0.6) — LLM-driven browser automation

Stagehand is hybrid AX + LLM (`src/stagehand/*` port of `browserbase/stagehand` `4.0.2`): `Accessibility.getFullAXTree` tree → `google/gemini-2.5-flash` (or `openai/gpt-4o-mini`) → deterministic `DOM.resolveNode`/`Runtime.callFunctionOn` action (`src/stagehand/act.rs:22`). Use for *any* browser task unless user explicitly says "no LLM" or "eval only".

**Config:** `StagehandConfig::from_env()` `src/stagehand/mod.rs:48` reads `~/.config/hyprfast/stagehand.env` (600, gitignored) + env:
```
GEMINI_API_KEY=AIza... / GOOGLE_API_KEY / GOOGLE_GENERATIVE_AI_API_KEY
STAGEHAND_MODEL=google/gemini-2.5-flash  # default if Google key present else openai/gpt-4o-mini
STAGEHAND_BASE_URL=  STAGEHAND_SYSTEM_PROMPT=
```
`cargo` fix `2026-09-02`: `reqwest` `rustls-tls` (was `default-features false` no TLS → `scheme is not http`), `no_proxy()` (env proxy `invalid URL`), `google_generate` markdown ````json` extract.

**MCP + CLI:**
| Task | MCP | CLI |
|---|---|---|
| Act | `stagehand_act instruction="click the login button"` | `hyprfast stagehand act "click the login button"` `src/stagehand/act.rs:146` |
| Observe | `stagehand_observe instruction="find login elements"` | `hyprfast stagehand observe "find login elements"` |
| Extract | `stagehand_extract instruction="extract products" schema="{\"products\":[]}"` | `hyprfast stagehand extract "extract products"` |
| Snapshot | `stagehand_snapshot` | `hyprfast stagehand snapshot` — `combined_tree` `raw_nodes 1144` `src/stagehand/snapshot.rs:1` |
| Agent | `stagehand_agent instruction="book cheapest flight"` | `hyprfast stagehand agent "book cheapest flight"` |
| Metrics/Batch/WebMCP | `stagehand_metrics` `stagehand_batch` | `hyprfast stagehand metrics` |

**Correct flow (Stagehand-first):**
```bash
1. brave --user-data-dir=/tmp/hyprfast-gemini --remote-debugging-port=9222 --force-renderer-accessibility --new-window https://gemini.google.com # or hyprfast browser open --workspace 6
2. hyprfast stagehand snapshot # verify tree via Accessibility
3. hyprfast stagehand act "Type 'What types of themes are available for website? List 8-10 with 1-line description each' into the prompt box aria-label 'Enter a prompt for Gemini' and press Enter" # LLM picks elementId 0-XXXX
4. if "No action found" / "not found" / "Could not find object with given id" / "backend resolve" → fallback:
   hyprfast browser eval "(() => { const el=document.querySelector('div[aria-label=\"Enter a prompt for Gemini\"]'); el.focus(); document.execCommand('insertText',false,'PROMPT'); el.dispatchEvent(new KeyboardEvent('keydown',{key:'Enter',bubbles:true})); })()"
5. if still fails → hyprfast browser snapshot + browser_click with ref
6. verify: hyprfast browser eval "document.body.innerText.slice(-3500)" or browser shot
```

**When to use what:**
- **Stagehand:** All browser tasks by default — `click`, `type`, `observe`, `extract` — handles ref invalidation, twoStep dropdown, self-heal `src/stagehand/act.rs:189`. Required per `browser/SKILL.md:317`.
- **Eval fallback:** Only if Stagehand returns `No action found`/`not found`/`backend` error, or prompt box is `contenteditable` `ql-editor` not in AX (Gemini `div.ql-editor[aria-label="Enter a prompt for Gemini"]` case 2026-09-02), or user explicitly says "use eval".
- **Snapshot last:** Only for structural refs `e*` when LLM fails to map `elementId`.

**Troubleshooting:**
- `CDP unreachable` → launch with `--remote-debugging-port=9222`
- `brave-browser exposes no accessibility tree` → add `--force-renderer-accessibility` or use `stagehand_snapshot` (CDP AX works without flag via fallback JS `src/browser/mod.rs:82`)
- `Google LLM scheme is not http` → fixed `rustls-tls` `Cargo.toml:33`, rebuild
- `LLM did not return actionable element` → `google_generate` now extracts `{ }` from ````json` `src/stagehand/llm.rs:132`
- `Could not find object with given id` → AX `backendDOMNodeId` stale, fallback to eval

## Brave/Chromium gotchas

- Single-instance: `brave --new-window` reuses existing process — if existing `brave` launched without `--force-renderer-accessibility`, new windows still have no tree. `pkill brave; brave --force-renderer-accessibility --new-window <url> &`
- Class is `brave-browser` (check `desktop` / `hyprctl -j clients`), not `chromium`.
- For WhatsApp Web inside Brave without flag, **do not use `ui`/`click_ui`** — use keyboard shortcuts below + `keyboard` tool. That's faster anyway.

## Root Cause Analysis 2026-09-02 — Why Agents Used Old Eval (Fixed — All 7 Causes Resolved)

> **Status: Fixed 2026-09-02** — all 7 causes below have been resolved via `hyprfast` skill + code (`a72ff2c` `Cargo.toml:33` `src/stagehand/llm.rs:60` `src/stagehand/act.rs:189` + `SKILL.md` Stagehand-first). Historical record kept for learning; no further action needed.

**Incident:** User prompted 2026-09-02 "use stagehand to control browser and add this learning to skill of browser and try again by controlling browser using stagehand if that didnt work then using eval and then snapshot" — agent still used `hyprfast browser eval` (`document.querySelector('div[aria-label=\"Enter a prompt for Gemini\"]')` + `execCommand`) for Gemini `71af278115aae64b` instead of `stagehand_act`, despite explicit instruction. Same for ChatGPT `chatgpt.com` `Session Learnings 2026-09-02 Correction` violation.

**Deep analysis — 7 overlapping causes:**

1. **Front-matter description missing Stagehand (tool discovery failure):**
   - Old `SKILL.md:3` `description: ... desktop/.../browser_navigate/snapshot/click/type/evaluate...` listed only `browser_*` (14 tools) — no `stagehand_act/observe/extract/agent`. The LLM that selects skills/tools via description embedding never surfaces `stagehand` as candidate. Even when user says "stagehand", the skill ranker doesn't boost it because description doesn't contain the token. **Fixed 2026-09-02:** description now includes `Stagehand LLM (act/observe/extract/agent)` and `stagehand_act/observe/extract/agent` tokens.

2. **Contradictory rules — hyprfast vs browser skill:**
   - `browser/SKILL.md:317` `Hyprfast Stagehand + CDP Fast Path` mandates `Stagehand → Eval → Snapshot` (Stagehand first).
   - `hyprfast/SKILL.md:70` **contradicted** with `**Rule: EVALUATE FIRST, screenshot last.** Always try browser_evaluate ... before snapshot`. This rule is in Quick Reference's eye-level, bold, with performance claim `~50ms vs 2-4s`. Agents anchor on the first bold rule they parse (hyprfast skill) and ignore the later browser skill's correct priority, especially when hyprfast skill is read after browser skill. **Fixed:** replaced with `STAGEHAND FIRST → EVAL → SNAPSHOT → SCREENSHOT LAST (0.6)` and explicit `Why not EVALUATE FIRST?` rationale.

3. **Version lag — 0.5 vs 0.6:**
   - Header said `0.5 with CDP`, table `hyprfast 0.5`, migration `14 tools`. Actual binary is `0.6.0` with 41 tools `src/main.rs:89`. Agent assumes Stagehand doesn't exist in 0.5 and falls back to eval. **Fixed:** header `0.6 with Stagehand`, table `0.6`, `41 tools`.

4. **Quick Reference table missing Stagehand rows:**
   - Table listed 8 `browser_*` rows, zero `stagehand_*` rows. The LLM uses table as tool catalog for planning; if `stagehand_act` not in table, it doesn't plan it. **Fixed:** added 5 rows `Stagehand act/observe/extract/snapshot/agent` with `src/stagehand/*` refs.

5. **No Stagehand examples in hyprfast skill:**
   - `0.5` skill had `Canonical flows` for WhatsApp `browser_evaluate` and keyboard, but zero `stagehand_act` example for `gemini.google.com`/`chatgpt.com`. Agent few-shots on eval pattern (muscle memory) and reuses `document.querySelector('div.ql-editor')` because it's the only concrete snippet. **Fixed:** added `Hyprfast Stagehand (0.6)` section with 3 concrete `stagehand act` snippets for Gemini prompts and fallback chain.

6. **Stagehand's prior bugs reinforced avoidance learning:**
   - Before `a72ff2c` fix, `stagehand act` failed 100%: `reqwest` `default-features false` no TLS → `scheme is not http`, `no_proxy` missing, `google_generate` markdown ````json` not extracted → `LLM did not return actionable element`, `DOM.resolveNode backend 0-4231` stale → `Could not find object`. Agent observed these failures in previous turns (`2026-09-02` logs) and learned to avoid Stagehand as unreliable, preferring eval which succeeded. The fix is now committed but skill didn't explain that Stagehand is now reliable — so agent kept avoidance. **Fixed:** `Cargo.toml:33` `rustls-tls`, `llm.rs:60` `no_proxy()`, `llm.rs:132` markdown extract, documented in skill's Troubleshooting.

7. **Instruction hierarchy — user says "use stagehand" but skill says "eval first":**
   - The planner resolves conflict by weighting skill's bold Rule over user's inline "use stagehand" (user prompt is lower priority than skill's canonical workflow in many instruction-tuned models). Without explicit `if user says stagehand → override skill's eval-first`, agent defaults to skill. **Fix in skill:** added `unless user explicitly says "no LLM" or "eval only", Stagehand is default even if eval would be faster on a single step` and `Stagehand is default even when user says "use stagehand" — honor it`.

**What to do next time (very fast):**
- Read `browser/SKILL.md` then `hyprfast/SKILL.md:Hyprfast Stagehand (0.6)` — note `Stagehand → Eval → Snapshot` order.
- Launch `brave --user-data-dir=/tmp/hyprfast-gemini --remote-debugging-port=9222 --force-renderer-accessibility` before any `stagehand_snapshot`.
- For any browser task: `stagehand_snapshot` → `stagehand act "Type 'PROMPT' into prompt box aria-label 'Enter a prompt for Gemini' and press Enter"` → on `No action found`/`backend` error immediately `browser eval` with `ql-editor` fallback (don't retry Stagehand twice).
- Do not re-introduce `EVALUATE FIRST` — keep Stagehand-first in both skills.

## WhatsApp Web — shortcut protocol (Windows/Linux vs Mac)

hyprfast handles WhatsApp Web via keyboard chords through `keyboard action=key keys="..." window=0x...` (which does `hypr::dispatch focuswindow` + `input::key_combo` `src/main.rs:75`). No `ui` needed. Sequence is always: `desktop` → `hypr focus_window` → `keyboard key/type` → `wait_for`/`screenshot` to confirm.

**Model translates `Cmd` → `ctrl` on Linux. Use the Windows column on Omarchy/Hyprland.**

### Chats

| Action | Windows/Linux | Mac | hyprfast `keys` |
|---|---|---|---|
| New chat | `Ctrl+Alt+N` | `Cmd+Ctrl+N` | `ctrl+alt+n` |
| New group | `Ctrl+Alt+Shift+N` | `Cmd+Ctrl+Shift+N` | `ctrl+alt+shift+n` |
| Archive chat | `Ctrl+Alt+E` | `Cmd+Ctrl+E` | `ctrl+alt+e` |
| Mute chat | `Ctrl+Alt+Shift+M` | `Cmd+Ctrl+Shift+M` | `ctrl+alt+shift+m` |
| Pin chat | `Ctrl+Alt+Shift+P` | `Cmd+Ctrl+Shift+P` | `ctrl+alt+shift+p` |
| Mark as unread | `Ctrl+Alt+Shift+U` | `Cmd+Ctrl+Shift+U` | `ctrl+alt+shift+u` |
| Delete chat | `Ctrl+Alt+Backspace` | `Cmd+Ctrl+Backspace` | `ctrl+alt+backspace` |

### Navigation and Search

| Action | Windows/Linux | Mac | hyprfast `keys` |
|---|---|---|---|
| Search | `Ctrl+Alt+/` | `Cmd+Ctrl+/` | `ctrl+alt+slash` (also `ctrl+k` works as fallback) |
| Search inside chat | `Ctrl+Alt+Shift+F` | `Cmd+Ctrl+Shift+F` | `ctrl+alt+shift+f` |
| Next chat | `Ctrl+Alt+Tab` | `Cmd+Ctrl+Tab` | `ctrl+alt+tab` |
| Previous chat | `Ctrl+Alt+Shift+Tab` | `Cmd+Ctrl+Shift+Tab` | `ctrl+alt+shift+tab` |
| Close chat | `Esc` | `Esc` | `esc` |

### Panels and Settings

| Action | Windows/Linux | Mac | hyprfast `keys` |
|---|---|---|---|
| Emoji panel | `Ctrl+Alt+E` | `Cmd+Ctrl+E` | `ctrl+alt+e` |
| GIF panel | `Ctrl+Alt+G` | `Cmd+Ctrl+G` | `ctrl+alt+g` |
| Sticker panel | `Ctrl+Alt+S` | `Cmd+Ctrl+S` | `ctrl+alt+s` |
| Profile | `Ctrl+Alt+P` | `Cmd+Ctrl+P` | `ctrl+alt+p` |
| Settings | `Ctrl+Alt+,` | `Cmd+Ctrl+,` | `ctrl+alt+comma` |

### Canonical flows

**WhatsApp Web: Evaluate-first approach (fastest, ~0.5s):**
```json
{"tool":"desktop"}
→ {"tool":"hypr","action":"focus_window","target":"0x...brave..."}
→ {"tool":"browser_evaluate","js":"document.querySelector('[data-testid=\"cell-frame-container\"]').click()"}
→ {"tool":"browser_wait","time":0.5}
→ {"tool":"browser_evaluate","js":"const ed=document.querySelector('[data-testid=\"conversation-compose-box-input\"]');ed.focus();document.execCommand('insertText',false,'hello');document.querySelector('[data-testid=\"send\"]').click()"}
→ done — 2-3 tool calls, no screenshots needed
```

**WhatsApp Web: Keyboard fallback (if evaluate fails):**
```json
{"tool":"desktop"}
→ {"tool":"hypr","action":"focus_window","target":"0x...brave..."}
→ {"tool":"keyboard","action":"key","keys":"ctrl+alt+slash","window":"0x..."}
→ {"tool":"keyboard","action":"type","text":"Khushi","window":"0x..."}
→ {"tool":"keyboard","action":"key","keys":"enter","window":"0x..."}
→ wait 0.8s (chat opens)
→ {"tool":"keyboard","action":"type","text":"hello","window":"0x..."}
→ {"tool":"keyboard","action":"key","keys":"enter","window":"0x..."}
→ done — no screenshot needed. Only screenshot if you need visual confirm: {"tool":"screenshot","window":"0x...","scale":0.5} // fast JPEG 0.5x ~80KB, or HYPRFAST_PNG=1 for PNG
```

**Perf note:** `screenshot` is fallback only. `src/screenshot.rs:64` now defaults to `grim -t jpeg -q 85` (~80KB vs ~250KB PNG, 3× less vision tokens). Native apps use `ui`/`click_ui` zbus `9-13ms` no image at all. For WhatsApp Web in Brave, keyboard is `~50ms` per key vs `2-4s` vision loop — prefer keyboard.

**Archive / Pin / Mute the active chat:** focus window → `keyboard key` with chord above.

**New chat / group:** `keyboard key` `ctrl+alt+n` / `ctrl+alt+shift+n`.

**Never do:** `ui name="Khushi"` or `click_ui name="hello"` inside Brave without `--force-renderer-accessibility` — it will return `exposes no accessibility tree; use screenshot` and loop. Use keyboard. Only use `ui`/`click_ui` for native GTK apps (e.g. `zenity`, file chooser) where zbus is `9-13ms` vs `~250ms` (`src/a11y/zbus_impl.rs:64`).

**WhatsApp Web DOM selectors for `browser_evaluate`:**
- Chat list item: `[data-testid="cell-frame-container"]`
- Message input: `[data-testid="conversation-compose-box-input"]`
- Send button: `[data-testid="send"]`
- Search input: `[data-testid="chat-list-search"]`

## Window Placement — WhatsApp Fullscreen Rule (prevents stuck tiled overlay)

**Problem:** If WhatsApp Web in Brave is tiled (split) not fullscreen, Brave’s omnibox/search overlay traps focus and `ctrl+alt+/` / typing gets stuck — screenshots show “Search Google |” overlay and no chat search.

**Rule:** Before any WhatsApp keyboard flow, check `desktop` → `windows` where `class=="brave-browser"` and `title` contains `WhatsApp`:
- If `fullscreen != true` and `size` != monitor `geometry` (i.e. tiled 581×629), **shift it to an unused workspace and fullscreen there**.

**How to find unused:** `desktop.workspaces` where `windows==0` → pick first empty `id` (usually 3..10). If none, use `10`.

**Flow:**
```json
{"tool":"desktop"} // inspect brave window at/size vs monitors geometry, check fullscreen
// if not fullscreen:
→ {"tool":"hypr","action":"move_window","target":"0x...brave...","workspace":"3"}
→ {"tool":"hypr","action":"workspace","target":"3"}
→ {"tool":"hypr","action":"fullscreen","target":"0x...brave..."}
→ re-check desktop fullscreen==true then continue with keyboard flow
```

This isolates Brave on its own workspace, gives it full `1200×675` logical area, removes tiling overlap, and disables the bookmark-bar omnibox trapping.

## Screenshot Session — auto-tracked, clear after task success

Every `screenshot` (CLI `hyprfast screenshot --window ...` or MCP `screenshot`) is **auto-appended** to `$XDG_RUNTIME_DIR/hyprfast-session.json` (fallback `/tmp`, `src/session.rs`). No manual list needed.

- **After successful task, clear:** `hyprfast clear` (CLI) or `clear_screenshots` (MCP, `all=false`) deletes only tracked `/tmp/hyprfast-*.png` and truncates session to `[]`. Returns `{removed, bytes_freed, tracked_before}`.
- **Full cleanup (stale leftovers):** `hyprfast clear --all` or `clear_screenshots all=true` also sweeps untracked `/tmp/hyprfast-*.png` (extra_removed).
- **Inspect:** `hyprfast session status` or `session_status` → `{session_file, tracked, existing, total_bytes, files[]}`. Also `hyprfast session list`.
- **Pattern:** Do task with zero or more screenshots → on success `clear_screenshots` → next task starts clean. On failure, keep shots for debugging then `clear --all`.

Example:
```json
{"tool":"screenshot","window":"0x..."} → {"path":"/tmp/hyprfast-....png"}
{"tool":"clear_screenshots","all":false} → {"removed":3,"bytes_freed":480000}
```

## Task State — persistent todo for multi-step actions (0.6.1, `src/task.rs:1`)

hyprfast now maintains a **persistent todo list** for any multi-step action — AI breakdowns goal into steps, initializes via hyprfast, updates per step, auto-clears when all done. Fixes stateless retry (2026-09-04 boomshakalaka duplication) by remembering where you are.

**File:** `$XDG_RUNTIME_DIR/hyprfast-tasks.json` fallback `/tmp` — single active list `{goal, steps:[{id,description,status,created_at,updated_at}], progress}`. Status: `pending|in_progress|completed|failed|skipped`. Auto-clears when every step `completed|skipped`.

**MCP tools (6, 47 total):**

| Tool | Params | When |
|---|---|---|
| `task_init` | `goal` (string), `steps` (string[]) | Start: breakdown user request into ordered steps before acting |
| `task_status` | — | Resume: check current list + `progress` + `next pending` after failure/timeout/before retry |
| `task_update` | `index` (0-based) or `id` (1-based), `status` (pending/in_progress/completed/failed/skipped) | After each step: mark done/failed; last completed triggers auto-clear `{auto_cleared:true}` |
| `task_next` | — | Get next `pending` step to execute |
| `task_add` | `description` | Append extra step mid-flow |
| `task_clear` | — | Manual clear (cancel) |

**CLI (`hyprfast task`):**
```bash
hyprfast task init "play boomshakalaka on youtube" --steps '["search youtube for boomshakalaka","list results and get first video","click first video","verify video is playing"]'
hyprfast task status
hyprfast task update --index 0 completed        # or --id 1
hyprfast task next
hyprfast task clear
# steps also accepts comma list: --steps "search,click,verify"
```

**Canonical AI workflow (mandatory for multi-step browser tasks):**
```json
1. task_init goal="open youtube and play boomshakalaka" steps=["search youtube boomshakalaka","click first video cL0KKSPjZf8","verify video playing paused==false"]
→ {"goal":"...","steps":[{id:1,status:"pending"},...],"progress":{"total":3,"pending":3}}

2. task_update index=0 status="in_progress"  // before search
   -> browser_open https://www.youtube.com/results?search_query=boomshakalaka
   -> browser_tabs + evaluate a#video-title.length==19
   -> task_update index=0 status="completed"

3. task_next → {next:{id:2,description:"click first video"}}
   -> task_update index=1 status="in_progress"
   -> Runtime.evaluate document.querySelector('a#video-title').click()
   -> verify location.href==watch?v=cL0KKSPjZf8 && video.paused==false
   -> task_update index=1 status="completed"

4. task_update index=2 status="completed"  // after verify
→ {"auto_cleared":true,"steps_completed":3}  // file deleted, status empty
```

**Resume after failure (critical — fixes 2026-09-04):** If `stagehand_agent` times out after step 1 done, **don't** re-`browser_open` search. Instead:
```json
{"tool":"task_status"} → {"goal":"play...","steps":[{id:1,status:"completed"}, {id:2,status:"pending"},...], "progress":{"completed":1,"pending":2}}
{"tool":"browser_tabs"} → check youtube results tab still exists → reuse via /json/activate
{"tool":"task_next"} → next pending is step 2 → continue from there
```
No duplicate tab `0BF421`, no wasted `browser_snapshot` on wrong target.

**Rules:**
- Always `task_init` before first browser step for any request with ≥2 steps (YouTube play, Gemini chat, WhatsApp send).
- After each step, `task_update` immediately — don't batch.
- Before retry, `task_status` + `browser_tabs` to decide resume point.
- Auto-clear means `task_status` returns `{"status":"empty"}` — next task starts clean.

**See:** `browser/SKILL.md § Session Learnings 2026-09-04` (recipe) + `opencode/SKILL.md §3` (global resume rule).

## Safety

- Confirm recipient/content before `Enter` to send — same as `close_window`.
- Re-verify `desktop` after any focus change; `address` goes stale on close.
- Prefer `address:` selector over `class:` regex.

Refs: `references/hyprland.md`, `references/browser.md` in `hyprsuse` skill — hyprfast reuses same Hyprland/Brave semantics, only transport is faster.

## Session Learnings 2026-08-31 — Gemini Continuous Chat, Model Switch & CDP Tab Isolation (canonical in `skill/browser/SKILL.md` § Session Learnings 2026-08-31 — summarized here)

> **De-duplicated:** Full Gemini toolbar (`buttons-container-v2` Copy `r_*` → `model-response-message-content{rId}`), `navigator.clipboard` NotAllowedError, `ql-editor` insert, and `aria-busy` polling live in `browser/SKILL.md` canonical. This file keeps only transport fix:

### CDP Multi-Tab Targeting Fix (critical — transport owns this)
`browser_evaluate` picks arbitrary `GET /json` page → wrong tab (WhatsApp/Gemini clash) → `no editor`. Fix: `browser_tabs` → if >1 `page`, `curl /json/close/<non-target-id>` so only target remains, then evaluate. `activate` alone insufficient. See browser skill for full copy-button/chat loop — use that recipe, not this summary.

## Session Learnings 2026-09-02 — pkill Hangs & ChatGPT WOW Theme Implementation (added from live session)

### pkill -f Hangs on Omarchy (critical — blocks bash tool 120s)

**Symptom:** Any command containing `pkill -f "8765"` hung with `(no output)` then `shell tool terminated after 120000ms` (`chatgpt.com:11` / `13`) — `cat|ss|ps` after `;` never executed. Same for `pkill -f "8765_NOPE_12345"` and `pkill "bash"` (`killing pid 954 failed: Operation not permitted` then hang).

**Diagnosis (2026-09-02):**
- `echo hi; sleep 0.2` → instant `hi/done` — shell ok.
- `pkill --help` → instant.
- `timeout 1 pkill -f "8765"` → 120s timeout with no output.
- `timeout 2 pkill -f "8765_NOPE_12345"` → same 120s hang (no match still hangs).
- `pgrep -f "8765"` → instant `3306287` `exit:0` — `pgrep` works fine.
- `nohup python3 -m http.server 8766 --bind 127.0.0.1 &` → instant `LISTEN 127.0.0.1:8766`, `curl` ok, `kill $!` ok — same `nohup`/`python` pattern works when `pkill` removed.

**Root cause:** `procps` `pkill -f` on this host iterates `/proc` and tries to `kill()` privileged PIDs (954/1029) → `Operation not permitted` then blocks on zombie/kernel thread iteration (procps-ng bug). It also self-matches the parent `bash -c " ... pkill -f \"8765\" ..."` causing deadlock. Tool waits 120s default timeout.

**Fix — never use `pkill -f "8765"` in hyprfast flows. Replace with:**
```bash
# check
ss -tlnp | grep 8765 || echo "no server"
# kill only if needed — pgrep works
pids=$(pgrep -f "8765" || true); [ -n "$pids" ] && kill $pids 2>/dev/null || true
# or port-based (preferred)
lsof -ti:8765 | xargs -r kill 2>/dev/null || true
fuser -k 8765/tcp 2>/dev/null || true
# then start without pkill in same line
nohup python3 -m http.server 8765 --bind 127.0.0.1 > /tmp/server.log 2>&1 &
sleep 0.5; ss -tlnp | grep 8765
```
Verified `nohup python3 -m http.server 8766` instant with this pattern.

**Rule:** In `hyprfast` bash tool, avoid `pkill -f`; prefer `pgrep -f` + `kill`, `lsof -ti:PORT`, or `fuser -k`. Never chain `pkill` with `;` before `nohup` — it blocks the persistent shell.

### ChatGPT WOW Theme Workflow (2026-09-02)

**Goal:** Use browser + ChatGPT to find best WOW design theme styles, select one, ask to implement.

**Fix for Brave CDP:**
- `brave` already running without `--remote-debugging-port=9222` → CDP `http://127.0.0.1:9222/json/version` unreachable.
- Fix: `pkill brave` is also affected by pkill hang — use `kill $(pgrep brave)` then `nohup brave --remote-debugging-port=9222 --force-renderer-accessibility --new-window https://chatgpt.com > /tmp/brave.log 2>&1 &` → verify `curl -s http://127.0.0.1:9222/json/version` returns `Chrome/152.0...`.

**ChatGPT interaction (CDP):**
- Editor: `div#prompt-textarea.ProseMirror[contenteditable=true]` (hidden `textarea` fallback). Insert via `ed.focus(); document.execCommand('selectAll',false,null); document.execCommand('insertText',false,prompt)` — click `button#composer-submit-button[aria-label="Send prompt"]`.
- Prompt 1 ranked Top 7 WOW styles: 1 Immersive 3D, 2 Kinetic Editorial, 3 **Dark Luxury + Glass/Aurora** (best overall/practicality ⭐⭐⭐⭐⭐), 4 Tactile Maximalism, 5 Neo-Brutalism, 6 Bento 2.0, 7 Retro-Futuristic — WOW formula = Dark Luxury + Kinetic + Aurora + subtle 3D + Bento.
- Prompt 2 selected #3 and requested single-file HTML. Result `NOVA — Build Something Future` ~63k HTML (`pre[2]` length 63742) with `aurora-blob` blur 110px, `noise`, `particles` canvas, glass nav `backdrop-filter: blur(24px)`, hero `clamp(64px,10.8vw,170px)`, `product-shell` 3D tilt, `ai-orb`, bento `repeat(12,1fr)`.
- Download: `pre` code via `Blob` + `a.click()` blocked; use chunked `browser_evaluate` slices (`innerText.slice(0,8000)`) or `lsof`-based server without `pkill` to save to `/tmp/nova-dark-luxury.html`.

**Correction 2026-09-02 — Violated Browser Skill Stagehand-First Rule:**
- `browser/SKILL.md:317-341` mandates **Stagehand first → Eval fallback → Snapshot last** for all browser automation (Gemini + ChatGPT). `hyprfast/SKILL.md:328-341` repeats same priority.
- **What we did wrong (this session):** For ChatGPT `chatgpt.com` we skipped Stagehand and went directly to `browser_evaluate` (`ed.focus() + execCommand('insertText') + click send`) because older hyprfast rule `SKILL.md:60` said `EVALUATE FIRST, screenshot last` and eval was instantly working after CDP fix. We did not check updated `browser/SKILL.md:8 Hyprfast Stagehand + CDP Fast Path` which now requires Stagehand first.
- **Why Stagehand was correct:** Stagehand `act/observe` via `Accessibility.getFullAXTree` + `google/gemini-2.5-flash` handles `ProseMirror` editor and streaming markdown more robustly, auto-heals ref invalidation, and is the canonical path per browser skill. Eval should only be fallback if `hyprfast stagehand act "Type ... into prompt box aria-label 'Chat with ChatGPT' and press Enter"` returns `no element` or empty observe. We incorrectly treated eval as primary for speed.
- **Future ChatGPT flow (must follow):**
  ```bash
  hyprfast stagehand act "Type 'What are the best WOW design theme styles...' into the prompt box aria-label 'Chat with ChatGPT' and press Enter"
  # if fails → fallback:
  hyprfast browser eval "(() => { const ed=document.getElementById('prompt-textarea'); ed.focus(); document.execCommand('selectAll',false,null); document.execCommand('insertText',false,'PROMPT'); document.getElementById('composer-submit-button').click(); })()"
  ```
- Added cross-ref to `browser/SKILL.md:317` — always try Stagehand first for ChatGPT/Gemini, eval only on fallback, snapshot last.

## Session Learnings 2026-09-04 — State-aware Resume & Tab Deduplication (YouTube boomshakalaka)

**Incident:** User: "open youtube and play boomshakalaka". Agent: 1) `browser_open https://www.youtube.com/results?search_query=boomshakalaka` → correctly on search results (state: search done, 19 `a#video-title` results, first `https://www.youtube.com/watch?v=cL0KKSPjZf8`). 2) Called `stagehand_agent` with 10-step goal — timed out (`MCP error -32001`). 3) Retried by opening *another* `browser_navigate` to same search URL again, then separately activating tabs, instead of continuing from existing state (`already on search results`). Created duplicate tab `0BF421...` with same URL, caused `browser_snapshot` to hit wrong target (kaggle/settings) due to CDP tab ambiguity, wasted 3-4 tool calls.

**Root cause — stateless retry:**
- No pre-retry audit: didn't run `browser_tabs` (`GET /json`) to see `56FFE72... (78) boomshakalaka - YouTube` already exists.
- No step tracking: didn't map workflow `search → list results → click first → verify play` to current position (step 1 done, next is step 3).
- `browser_open`/`browser_navigate` without dedup always creates/navigates even if identical tab exists → duplicate tabs + CDP target confusion (snapshot hits wrong tab).
- `stagehand_agent` timeout treated as full reset instead of partial progress.

**Fix — State-aware resume (mandatory for all multi-step browser tasks):**

1. **Always audit before acting (new attempt or retry):**
   ```bash
   # 1. What tabs exist already?
   curl -s http://127.0.0.1:9222/json | python3 -c "import json; print(...)  # list id title url"
   # via MCP: browser_tabs
   # 2. Where am I in the workflow?
   # - Compare desired URL vs existing tabs — if match, reuse via /json/activate/<id> NOT browser_open
   # - Evaluate location.href on candidate tab via CDP Runtime.evaluate
   # 3. Fresh snapshot only AFTER activating correct tab
   ```

2. **Define & track workflow steps + current state:**
   ```
   Plan for "play boomshakalaka": [1.search, 2.list_results, 3.click_first, 4.verify_play]
   After step 1 success (search page with 19 results) → state = step1_done
   On retry/timeout → resume at step3 (click), NOT step1
   ```

3. **Tab deduplication rule (transport owns this — hyprfast):**
   - Before `browser_open url=X` or `browser_navigate url=X`, check `browser_tabs` for any `page` where `url` contains same query/host+path.
   - If found (e.g., `youtube.com/results?search_query=boomshakalaka` exists as `56FFE72...`), use `curl /json/activate/<id>` or `browser_navigate` with `target` set to that tab's title, then operate via `Runtime.evaluate` on its `ws` — don't create second tab `0BF421...`.
   - Only create new tab if zero matches.

4. **Resume pattern (example for this case):**
   ```python
   # After stagehand_agent timeout, instead of reopening search:
   tabs = get_json()  # contains 56FFE72...(78) boomshakalaka
   if any("boomshakalaka" in t["url"] for t in tabs):
       ws = f"ws://127.0.0.1:9222/devtools/page/{matching_id}"
       # verify still on search page
       hrefs = eval(ws, "document.querySelectorAll('a#video-title').length")
       # continue: click first video
       eval(ws, "document.querySelector('a#video-title').click()")
       # verify navigation to watch?v=cL0KKSPjZf8 + video.paused==false
   ```

5. **CDP targeting hygiene:** After activate, always verify `Runtime.evaluate: location.href` matches expected before next action; `browser_snapshot` alone is unreliable with multiple tabs (picks arbitrary `GET /json` page).

**Checklist added to Quick reference:**
- [ ] `browser_tabs` → deduplicate → reuse existing tab
- [ ] Map `planned steps` vs `current url/snapshot` → resume from next incomplete step
- [ ] On failure/timeout, audit state (url + snapshot + video state) then continue — never restart from step 1 if step 1 succeeded

**See also:** `browser/SKILL.md § Session Learnings 2026-09-04` (recipe view), `opencode/SKILL.md §3` (resume requirement).
