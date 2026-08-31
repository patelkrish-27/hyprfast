# tmux orchestration reference — one window, N panes

## Why tmux inside foot

- `foot` is single Hyprland window (class=foot) — Hyprland can only focus by `address` (`0x...`). Two separate foot windows = two addresses = "different different" which user rejected.
- `tmux` gives N logical terminals inside ONE address. `hyprfast` controls the container; `tmux` controls panes.
- AT-SPI `ui` sees foot as one tree; tmux panes are not separate a11y nodes — use `tmux capture-pane` not `ui`.

## Session naming

- Fixed name: `opencode-orch` (so `tmux has-session -t opencode-orch` is idempotent).
- All commands use `-t opencode-orch:<window>.<pane>` e.g., `opencode-orch:0.0` left, `0.1` right.

## Layouts

```bash
# 2 panes side-by-side (even-horizontal, used for N=2)
tmux split-window -h -t opencode-orch:0
tmux select-layout -t opencode-orch even-horizontal

# 3 panes: 1 left + 2 stacked right
tmux split-window -h -t opencode-orch:0
tmux split-window -v -t opencode-orch:0.1
tmux select-layout -t opencode-orch tiled

# 4 panes: 2x2 grid
tmux split-window -h -t opencode-orch:0
tmux split-window -v -t opencode-orch:0.0
tmux split-window -v -t opencode-orch:0.2
tmux select-layout -t opencode-orch tiled
```

## Pane control

```bash
# Target syntax
tmux send-keys -t opencode-orch:0.0 'echo hello' Enter
tmux send-keys -t opencode-orch:0.1 'cd /tmp && exec opencode' Enter

# Focus for visual typing (hyprfast will focus container, then tmux selects pane)
tmux select-pane -t opencode-orch:0.0
tmux display-message -p '#{pane_index} #{pane_current_command}'

# Capture TUI output (poll every 5-10s, not tight loop)
tmux capture-pane -p -t opencode-orch:0.0 | tail -40
tmux capture-pane -p -t opencode-orch:0.1 | head -60

# Clear hang
tmux send-keys -t opencode-orch:0.0 C-c
tmux send-keys -t opencode-orch:0.0 Escape

# Resize if font too small
tmux resize-pane -t opencode-orch:0.0 -x 120 -y 30
```

## Hyprfast integration

```bash
# Launch container (one window)
hyprfast launch "foot -e tmux new-session -A -s opencode-orch" --workspace 4

# Get address for focus/click (foot title is opencode-orch only if foot launched with --title, else use class)
hyprctl clients -j | jq -r '.[] | select(.class=="foot") | .address'  # if only one foot
# Better: set title explicitly
hyprfast launch "foot --title=opencode-orch -e tmux new-session -A -s opencode-orch" --workspace 4

# Focus before tmux typing so user sees activity
ADDR=$(hyprctl clients -j | jq -r '.[] | select(.title=="opencode-orch") | .address')
hyprfast hypr focus_window $ADDR
hyprfast hypr workspace 4

# Optional visual typing via hyprfast (focus must be correct)
hyprfast keyboard type --text "hello" --window $ADDR  # goes to active tmux pane only — prefer tmux send-keys for targeting

# Screenshot container to show splits
hyprfast screenshot --window $ADDR --scale 0.6
```

## Verification checklist

- [ ] `hyprctl clients -j | jq '[.[] | select(.class=="foot")] | length'` == 2 (coordinator ws1 + orch ws4) not 3+
- [ ] `tmux list-panes -t opencode-orch -F "#{pane_index} #{pane_current_command} #{pane_active}"`
- [ ] `ps aux | grep opencode` shows 3 processes: coordinator (pts/0) + 2 tmux children
- [ ] `tmux capture-pane` for each pane shows opencode TUI prompt (`◆` or `opencode`), not bare shell

## Cleanup

```bash
tmux kill-session -t opencode-orch
# If foot still open but tmux dead, close window:
ADDR=$(hyprctl clients -j | jq -r '.[] | select(.title=="opencode-orch") | .address')
hyprctl dispatch closewindow address:$ADDR
```
