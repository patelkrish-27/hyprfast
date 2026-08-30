---
name: hyprfast
description: Fast Rust alternative to hypruse — persistent Hyprland IPC + direct zbus AT-SPI, MCP+CLI. Use for desktop/window/workspace ops, AT-SPI clicks without screenshots, or WhatsApp Web/Brave automation with keyboard shortcuts.
---

# hyprfast — fast hypruse alternative (Rust)

See installed skill at `~/.config/opencode/skills/hyprfast/SKILL.md` for full reference. Summary:

- `desktop` (<5ms), `hypr`, `launch`, `ui`, `click_ui`, `pointer`, `keyboard`, `screenshot`, `wait_for`, `binds`
- Brave needs `--force-renderer-accessibility` for `ui`; otherwise use keyboard.

## WhatsApp Web shortcuts

Chats: `Ctrl+Alt+N` new chat, `Ctrl+Alt+Shift+N` new group, `Ctrl+Alt+E` archive, `Ctrl+Alt+Shift+M` mute, `Ctrl+Alt+Shift+P` pin, `Ctrl+Alt+Shift+U` unread, `Ctrl+Alt+Backspace` delete.
Nav: `Ctrl+Alt+/` search, `Ctrl+Alt+Shift+F` search inside, `Ctrl+Alt+Tab` next, `Ctrl+Alt+Shift+Tab` prev, `Esc` close.
Panels: `Ctrl+Alt+E` emoji, `G` gif, `S` sticker, `P` profile, `,` settings. Mac: `Cmd+Ctrl+<key>`.

Flow `Khushi -> hello`: `desktop` → `focus_window` → `keyboard key ctrl+alt+slash` → `type Khushi` → `enter` → `type hello` → `enter` → `screenshot` confirm.
