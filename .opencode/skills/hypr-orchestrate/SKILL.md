---
name: hypr-orchestrate
description: "Orchestrate N opencode sessions inside ONE Hyprland window (tmux splits) via hyprfast — create, split, drive, and verify workers in parallel. Use when user says 'open N opencode sessions' or wants laptop control via hyprfast, not direct file writes."
---

# hypr-orchestrate — one window, N opencode sessions (hyprfast + tmux)

You have superpowers (`hyprfast` desktop/hypr/launch/ui/keyboard + `tmux`) but must not bypass them with direct `Write`/`Task`. This skill enforces **one Hyprland window, N tmux panes, each running `opencode`** — visible side-by-side, not scattered across workspaces.

## When to use

- User says: `open 2 opencode sessions`, `open two opencode sessions and orchestrate them to build X`, `use hyprfast to control my laptop`
- Any `N >=2` multi-agent task where workers must be visible together

> Rule: NEVER create N separate foot windows on N workspaces. ALWAYS one `foot` window with `tmux` splits (`tmux split-window -h` / `-v`). That is what user means by "same window not different different".

## Architecture

```
Hyprland workspace 3/4 (one tiled window, 1200x629)
└─ foot (class=foot, title=opencode-orch)
   └─ tmux session=opencode-orch
      ├─ pane 0 (left)  → opencode --port 0  (Engine / Worker A)  CWD=/home/krish/Projects/<slug>
      └─ pane 1 (right) → opencode --port 0  (UI / Worker B)      CWD=/home/krish/Projects/<slug>
Coordinator = this main opencode session (ws1) — drives workers via hyprfast + tmux send-keys, never writes src/* itself.
```

Why tmux not Hyprland split: AT-SPI `ui` sees only one foot window — hyprfast cannot target pane by `window=0x...` alone. `tmux send-keys -t opencode-orch:0.0` targets panes precisely; `hyprfast_hypr focus_window` + `hyprfast_keyboard` focuses the container window first, then tmux routes.

## Quick reference

| Want to... | Command |
|---|---|
| Snapshot before action | `hyprfast_desktop` (or `desktop` MCP) |
| Launch ONE container window | `hyprfast_launch command="foot -e tmux new-session -A -s opencode-orch" workspace=4` |
| Wait for window | `hyprfast_wait_for event=window_open match=opencode-orch timeout_s=5` or `desktop` poll |
| Verify single window | `hyprctl clients -j` → exactly 1 foot where `title` contains `opencode-orch` on target workspace |
| Create splits | `tmux has-session -t opencode-orch && tmux split-window -h -t opencode-orch:0` then `tmux select-layout -t opencode-orch even-horizontal` |
| Launch opencode in pane | `tmux send-keys -t opencode-orch:0.0 'cd /home/krish/Projects/<slug> && exec opencode' Enter` and `:0.1` for second |
| Focus container for typing | `hyprfast_hypr action=focus_window target=0x...` (from desktop) |
| Send prompt to pane 0 | `tmux send-keys -t opencode-orch:0.0 'Build engine...' Enter`  (preferred) OR `hyprfast_keyboard action=type text="..." window=0x...` after `tmux select-pane -t 0.0` |
| Check pane is opencode TUI | `tmux capture-pane -p -t opencode-orch:0.0 | head -20` should contain `opencode` prompt |
| Kill & restart cleanly | `tmux kill-session -t opencode-orch; pkill -f opencode-orch` then relaunch |

## Standard flow (generic, no Sudoku hard-code)

### Phase 0 — Provision single-window container

```bash
# 1. Clean stale orchestrated windows (keep user's original ws1 opencode)
tmux kill-session -t opencode-orch 2>/dev/null; true
# kill old multi-window strays ws3/4/5 if they exist (only if they are opencode-sudoku-*):
hyprctl clients -j | jq -r '.[] | select(.workspace.id==4 or .workspace.id==5) | .address' | xargs -I{} hyprctl dispatch closewindow address:{}
# 2. Launch ONE foot with tmux
hyprfast launch "foot -e tmux new-session -A -s opencode-orch" --workspace 4
hyprfast wait window_open --match-str opencode-orch --timeout 5
hyprfast_desktop  # confirm 1 foot on ws4, not 2
# 3. Split into N panes (N=2 example)
tmux split-window -h -t opencode-orch:0
tmux select-layout -t opencode-orch even-horizontal
# For N=3: second split -v on pane 0, layout tiled
```

### Phase 1 — Start opencode in each pane

```bash
# Pane 0 → Worker A
tmux send-keys -t opencode-orch:0.0 'cd /home/krish/Projects/<slug> && exec opencode --port 0' Enter
sleep 2
# Pane 1 → Worker B
tmux send-keys -t opencode-orch:0.1 'cd /home/krish/Projects/<slug> && exec opencode --port 0' Enter
sleep 3
tmux capture-pane -p -t opencode-orch:0.0 | tail -5
tmux capture-pane -p -t opencode-orch:0.1 | tail -5
```

Verify via `ps aux | grep opencode` → 2 children of tmux (plus coordinator). `hyprctl clients` must still show only ONE foot window.

### Phase 2 — Inject role prompts (via tmux, visible via hyprfast focus)

```bash
# Focus container so typing is visible on screen
ADDR=$(hyprctl clients -j | jq -r '.[] | select(.title=="opencode-orch" or .initialTitle=="opencode-orch") | .address')
hyprfast hypr focus_window $ADDR
# Drive pane 0
tmux select-pane -t opencode-orch:0.0
tmux send-keys -t opencode-orch:0.0 'You are Worker A (ENGINE). Create src/engine.js...' Enter
# Drive pane 1
tmux select-pane -t opencode-orch:0.1
tmux send-keys -t opencode-orch:0.1 'You are Worker B (UI). Create src/style.css...' Enter
```

> Use `tmux send-keys` not coordinator `Write`. Optionally also use `hyprfast_keyboard action=type` after `select-pane` for visual typing, but `tmux` is precise.

### Phase 3 — Poll & orchestrate

- Coordinator polls `tmux capture-pane -p -t opencode-orch:0.0` every 10s + `Read` file existence (read-only check, not write) to know Worker A done.
- If Worker B needs engine API, `tmux send-keys -t opencode-orch:0.1 'Read src/engine.js first' Enter` — workers talk via files, coordinator never merges.
- On hang: `tmux send-keys -t opencode-orch:0.0 C-c` or `Escape` via `hyprfast_keyboard action=key keys=esc window=$ADDR`.

### Phase 4 — Verify

```bash
ls -lh /home/krish/Projects/<slug>/src/
cat /home/krish/Projects/<slug>/src/engine.js | head -5  # written by pane 0, not coordinator
npm run build  # instruct via tmux: send-keys 'npm run build' to pane 1
hyprfast launch "brave --new-tab http://localhost:5173"  # verify browser
```

## Rules & Guardrails

1. **One window**: If you catch yourself launching `foot ...` twice on different workspaces, stop — use `tmux split-window` inside the same `opencode-orch` session.
2. **Empty windows first**: When user says "open sessions" alone, open container + splits with `opencode` waiting at prompt (no pre-typed task) — wait for next instruction.
3. **Never bypass**: Do not use `Task`/`Write` to create `src/*` directly. Workers' `opencode` must do `Write` tool calls (visible in their `tmux capture-pane`).
4. **Visible control**: Every `tmux send-keys` should be preceded by `hyprfast_hypr workspace 4` + `focus_window` so user sees the window focused.
5. **Cleanup**: On task success, `hyprfast_clear_screenshots` if screenshots were taken; keep `opencode-orch` session alive for next task unless user says `close`.
6. **Port isolation**: Each pane gets `opencode --port 0` (random) — do not share port. Coordinator is on 4096.

## Decomposing arbitrary goals

Don't hard-code Sudoku. For `<goal>` `open N sessions to build X`:

- Orchestrator (coordinator) runs 1 LLM thought: split X into N parallel streams (e.g., `sudoku` → `Engine` + `UI`; `blog` → `API` + `Frontend` + `Tests`; `chess` → `Engine` + `Board` + `AI`).
- Generate `N` role prompts with `CWD=/home/krish/Projects/<slugified-goal>` and send each to pane `0..N-1`.
- Slugify: `echo "$goal" | tr '[:upper:]' '[:lower:]' | tr -cs 'a-z0-9' '-'`.

## Example invocations

```
/orchestrate 2 sudoku website game
→ launches 1 foot on ws4, 2 tmux panes, pane0=Engine pane1=UI

/open 2 empty opencode sessions
→ one window, two panes, both at opencode prompt, waiting

/open 3 sessions to build a markdown blog with auth
→ split tiled (3 panes), roles API/Frontend/Auth
```

## References

- `references/tmux-orchestration.md` — tmux pane targeting, capture, layout details
- `hyprfast` SKILL.md — desktop/hypr/launch/ui/pointer/keyboard semantics (reuse, don't re-explain)
