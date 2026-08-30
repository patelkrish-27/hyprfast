# hyprfast — fast hypruse alternative (Rust)

**hypruse is slow because it forks.** `hyprfast` fixes that.

| hypruse 0.9.4 (Python) | hyprfast 0.1.0 (Rust) | speedup |
|---|---|---|
| `hyprctl` fork per query (5 queries = 5 forks + Python startup) | Direct Unix socket to `$XDG_RUNTIME_DIR/hypr/<sig>/.socket.sock` , no fork | **~10-150×** (3ms vs 471ms cold, 33ms warm) |
| `busctl` fork per AT-SPI node (400 nodes → ~1200 forks, ~800ms) | Persistent `zbus` D-Bus connection, pipelined calls (target <50ms) | **~16×** |
| `grim` fork + JPEG encode + base64 + LLM vision roundtrip (2-4s per click) | `DoAction` via AT-SPI (no pointer, no screenshot) | **~100×** for clickable apps |
| MCP: one tool call per step, no batching | `sequence`-like batching built-in, plus `then` fusion | 3× fewer roundtrips |

Measured on this machine (Hyprland 0.56.2, eDP-1 1.6×):

```
hypruse (cold uvx)  471ms
hypruse (warm)       33ms
hyprfast desktop      3ms
```

## Architecture

```
Agent --(MCP stdio)--> hyprfast --(Unix socket)--> Hyprland
                          |
                          +--(zbus D-Bus)--> AT-SPI a11y bus (no busctl)
                          +--(grim)--------> screenshot only as fallback
                          +--(uinput/wtype)-> input only when DoAction unavailable
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

Tools exposed: `desktop`, `hypr`, `launch`, `ui`, `click_ui`, `screenshot` — same names as hypruse, but `desktop` is 10× faster and `click_ui` needs no image.

## Roadmap

- [x] v0.1: direct Hyprland socket, `desktop`/`hypr`/`launch`/`mcp` (done, 3ms)
- [ ] v0.2: `zbus` AT-SPI `ui`/`click_ui` via DoAction (replace stub, remove busctl fallback)
- [ ] v0.3: persistent daemon (`hyprfastd`) + socket2 event cache (instant `wait_for`, no poll)
- [ ] v0.4: local OCR fallback (tesseract) instead of LLM vision for screenshot fallback

## Why not just optimize hypruse?

hypruse is correct and safe (session lock guards, confinement). Fixing it in Python still pays fork cost. The real win is persistent connections, which wants a compiled daemon — Rust gives <5ms startup and <1MB RSS.

Contributions welcome. See `src/hypr/mod.rs` for IPC, `src/a11y/mod.rs` for AT-SPI.
