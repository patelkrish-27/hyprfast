---
name: hyprfast
description: Fast Rust alternative to hypruse — persistent Hyprland IPC + direct zbus AT-SPI + CDP browser automation, MCP+CLI. Use whenever you need desktop/window/workspace ops, AT-SPI clicks, Brave/Chromium automation via Chrome DevTools, or WhatsApp Web. Consult for hyprfast's desktop/hypr/launch/ui/click_ui/pointer/keyboard/screenshot/wait_for/binds + browser_navigate/snapshot/click/type/evaluate/screenshot/tabs tools.
---

# hyprfast — fast hypruse alternative (0.5 with CDP)

`hyprfast` is `hypruse` without the forks: direct Unix socket to Hyprland (`$XDG_RUNTIME_DIR/hypr/<sig>/.socket.sock`, `j/<cmd>`), persistent `zbus` D-Bus to `org.a11y.Bus` (no `busctl` per node), `DoAction` clicks (no `movecursor`+`click`), optional `hyprfastd` daemon cache, **plus built-in CDP browser automation (no Node, no @browsermcp/mcp)** — one binary for Hyprland + AT-SPI + Chrome DevTools.

Same MCP shape as `hypruse` (`desktop`, `hypr`, `launch`, `ui`, `click_ui`, `pointer`, `keyboard`, `screenshot`, `wait_for`, `binds`) **plus** `browser_*` (`browser_navigate`, `browser_snapshot`, `browser_click`, `browser_type`, `browser_evaluate`, `browser_screenshot`, `browser_tabs`, etc.) — full `browsermcp` parity via `src/cdp/mod.rs:30` + `src/browser/mod.rs:1`, but `~10-150×` faster and 0 extra deps.

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

**Rules from hyprsuse still apply:**
- `desktop` first, then act on `address`. Never screenshot to locate windows.
- `ui` > `screenshot+zoom`. `Brave/Chromium` needs `--force-renderer-accessibility` at launch or tree is empty (`brave-browser exposes no accessibility tree; use screenshot` — `src/a11y/mod.rs:35`). **With 0.5 CDP, prefer `browser_snapshot`/`browser_click` over `ui` for browsers — CDP AX works without the flag and without pointer.**
- `wait_for` > `sleep`. `hyprfastd` at `/run/user/1000/hyprfastd.sock` accelerates `desktop`+`wait_for`. For browsers, `browser_wait` or CDP `Page.loadEventFired` via `browser_navigate`.
- `click_ui` uses `org.a11y.atspi.Action.DoAction(0)` (`src/a11y/zbus_impl.rs:295`), fallback to pointer.
- `launch` now auto-injects `--remote-debugging-port=9222` for `brave/chromium/chrome` if missing (`src/main.rs:60`). Override via `HYPRFAST_CDP_HOST/PORT`.

## Browser Automation (CDP — new in 0.5)

hyprfast 0.5 embeds Chrome DevTools Protocol — no `@browsermcp/mcp` or `playwright` needed. One binary, one MCP. Uses `reqwest` GET `/json` discovery (`src/cdp/mod.rs:30`) + `tokio-tungstenite` WS JSON-RPC (`src/cdp/mod.rs:103`).

| Want to... | hyprfast 0.5 |
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

Flow: `desktop` → `hypr focus_window` or `browser_open` → `browser_snapshot` → `browser_click/type` → `browser_screenshot` verify. Prefer CDP for browsers; keep `ui/click_ui` for native GTK apps.

**Env:** `HYPRFAST_CDP_HOST`/`PORT` default `127.0.0.1:9222`. Error `CDP unreachable...` means browser not launched with flag — use `hyprfast browser open ...`.

**Migration:** Replace `npx @browsermcp/mcp` with `hyprfast mcp` — already includes `browser_*` tools parity (14 tools). Disable `browsermcp` in `opencode.json` after 0.5.

## Brave/Chromium gotchas

- Single-instance: `brave --new-window` reuses existing process — if existing `brave` launched without `--force-renderer-accessibility`, new windows still have no tree. `pkill brave; brave --force-renderer-accessibility --new-window <url> &`
- Class is `brave-browser` (check `desktop` / `hyprctl -j clients`), not `chromium`.
- For WhatsApp Web inside Brave without flag, **do not use `ui`/`click_ui`** — use keyboard shortcuts below + `keyboard` tool. That's faster anyway.

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

**Send `hello` to `Khushi` (Web) — no screenshot needed (keyboard-only, ~0.8s):**
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

## Safety

- Confirm recipient/content before `Enter` to send — same as `close_window`.
- Re-verify `desktop` after any focus change; `address` goes stale on close.
- Prefer `address:` selector over `class:` regex.

Refs: `references/hyprland.md`, `references/browser.md` in `hyprsuse` skill — hyprfast reuses same Hyprland/Brave semantics, only transport is faster.
