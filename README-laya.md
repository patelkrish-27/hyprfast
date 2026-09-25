# hyprfast MCP for Laya (`convaiinnovations/laya`)

Use `hyprfast` (68 MCP tools) as the hands and `laya` as the System-1 decider.
Laya never generates text — it answers typed `choice/score/noul` questions over a `state` in ~33ms.

> Model: https://huggingface.co/convaiinnovations/laya
> Rule: **never give Laya 68 options at once.** It collapses at `>20 options` (Banking77 0.425). Use 2 passes: `category (8)` → `command (3-16)`.

## 1. Architecture

```
Lucy state {goal, focused, last_obs} 
  -> Laya pass-1: choice category (8 options)
  -> Laya pass-2: choice command (per-category dict)
  -> hyprfast MCP tools/call <command>
  -> observe -> repeat
```

`hyprfast mcp` (stdio) owns all execution. Laya only routes.

## 2. Prerequisites

```bash
cargo install --path .   # provides `hyprfast mcp` (68 tools)
pip install laya transformers torch
# Brave for browser tools:
hyprfast browser open https://example.com --workspace 3
```

MCP config (`~/.config/opencode/opencode.json`):

```json
{ "mcp": { "hyprfast": { "type": "local", "command": ["hyprfast", "mcp"], "enabled": true } } }
```

## 3. Pass-1: categories (always this, 8 options)

```python
from laya import Router
router = Router(preload=True)  # english + multilingual resident, <1ms switch

state = {
  "goal": "click login button on example.com",
  "focused": "brave-browser example.com",
  "last_obs": "snapshot 12 refs, no login ref",
  "last_action": "browser_snapshot"
}

q_cat = {"category": {"type": "choice",
  "instructions": "Which tool group handles the next step?",
  "criteria": {
    "core-desktop": "window workspace launch wait handling",
    "perceive": "read screen snapshot tabs without changing",
    "native-act": "click type native app mouse keyboard",
    "browser-act": "deterministic browser click type navigate eval",
    "smart-llm": "language intent click extract agent fallback",
    "fast-ground": "hint labels vision coords canvas no AX",
    "task-memory": "multi-step plan track resume cleanup",
    "draw": "excalidraw whiteboard architecture diagrams only"
  }}}

cat = router.predict(state, q_cat)["answers"]["category"]
# gate on confidence, NOT act_probability (act_probability ~1.0 always, AUROC 0.30)
# if confidence < 0.70: re-perceive (screenshot/snapshot) and retry
```

| category | when | tools |
|---|---|---|
| `core-desktop` | bootstrap, window mgmt | `desktop, hypr, launch, binds, wait_for` |
| `perceive` | read-only, what is on screen | `ui, screenshot, browser_snapshot, browser_tabs, browser_console, context_pages, session_status` |
| `native-act` | native GTK/dialogs | `click_ui, pointer, keyboard` |
| `browser-act` | deterministic CDP, selector known | `browser_navigate, browser_open, browser_go_back, browser_go_forward, browser_click, browser_hover, browser_type, browser_select_option, browser_press_key, browser_wait, browser_evaluate, browser_screenshot, browser_execute_plan, clipboard_write, clipboard_read, cookies_set` |
| `smart-llm` | intent, self-heal (default for web) | `stagehand_act, stagehand_observe, stagehand_extract, stagehand_agent, stagehand_snapshot, stagehand_cache, stagehand_metrics, stagehand_batch, stagehand_webmcp` |
| `fast-ground` | Vimium hints / vision, AX empty | `hint_snapshot, hint_click, hint_type, hint_act, hint_batch, hint_clear, ground, act_fast, act_batch` |
| `task-memory` | >=2 steps, resume | `task_init, task_status, task_update, task_next, task_add, task_clear, clear_screenshots` |
| `draw` | diagrams only | `excalidraw_open, excalidraw_get_scene, excalidraw_clear, excalidraw_draw, excalidraw_draw_batch, excalidraw_update_scene, excalidraw_diagram, excalidraw_export, excalidraw_save, excalidraw_view, excalidraw_fit` |

## 4. Pass-2: command (per-category, short criteria!)

Keep each criterion to 5-12 words or you blow `head_max_len` (192 EN / 256 ML). Full mapping in `src/main.rs:550-621`.

```python
COMMANDS = {
 "core-desktop": {"desktop":"snapshot windows workspaces","hypr":"switch focus move close fullscreen","launch":"start app url workspace","binds":"list keybindings","wait_for":"wait window event"},
 "perceive": {"ui":"native accessibility tree list","screenshot":"grim photo verify fallback","browser_snapshot":"web AX refs for click","browser_tabs":"list tabs urls dedup","browser_console":"console logs errors","context_pages":"targets via CDP","session_status":"tracked shots bytes"},
 "native-act": {"click_ui":"click native by name","pointer":"mouse move click drag scroll","keyboard":"type text or key combo"},
 "browser-act": {"browser_navigate":"go to URL","browser_open":"launch Brave CDP","browser_go_back":"history back","browser_go_forward":"history forward","browser_click":"click by ref selector","browser_hover":"hover element","browser_type":"type into ref submit","browser_select_option":"dropdown select","browser_press_key":"press Enter Escape","browser_wait":"wait seconds","browser_evaluate":"run JS read click","browser_screenshot":"tab PNG","browser_execute_plan":"multi-step plan","clipboard_write":"write clipboard","clipboard_read":"read clipboard","cookies_set":"set cookies"},
 "smart-llm": {"stagehand_act":"do intent click type","stagehand_observe":"find elements xpath","stagehand_extract":"extract JSON","stagehand_agent":"autonomous goal loop","stagehand_snapshot":"hybrid AX tree","stagehand_cache":"cache status clear","stagehand_metrics":"token stats","stagehand_batch":"run JS batch","stagehand_webmcp":"list invoke tools"},
 "fast-ground": {"hint_snapshot":"list clickables A S D","hint_click":"click by label","hint_type":"type by label","hint_act":"auto pick click type","hint_batch":"batch picks parallel","hint_clear":"clear overlay","ground":"vision x y","act_fast":"ground plus click type","act_batch":"batch ground steps"},
 "task-memory": {"task_init":"start goal steps","task_status":"show progress","task_update":"mark step done","task_next":"next pending","task_add":"append step","task_clear":"cancel list","clear_screenshots":"delete PNGs"},
 "draw": {"excalidraw_open":"ensure tab","excalidraw_get_scene":"count elements","excalidraw_clear":"clear canvas","excalidraw_draw":"single shape","excalidraw_draw_batch":"batch shapes","excalidraw_update_scene":"raw append replace","excalidraw_diagram":"auto flowchart sequence microservices","excalidraw_export":"export png svg","excalidraw_save":"save file","excalidraw_view":"viewport zoom","excalidraw_fit":"center fit"}
}

q_cmd = {"command": {"type": "choice",
  "instructions": f"Which {cat} tool executes next?",
  "criteria": COMMANDS[cat]}}
cmd = router.predict(state, q_cmd)["answers"]["command"]["choice"]
# -> execute via MCP: tools/call {name: cmd, arguments: {...}}
```

Use `choice` only. `noul` follows its `false:/true:` labels on EN checkpoint — if you need yes/no, use `choice {A: yes..., B: no...}`.

## 5. Lucy loop

```python
while True:
    cat = decide_category(state)          # pass-1
    cmd = decide_command(state, cat)      # pass-2
    result = mcp_call(cmd, args_from_llm_or_rule)
    state["last_obs"] = summarize(result)[:320]  # keep state <=320 tokens (EN budget 512-192)
    state["last_action"] = cmd
    if done: task_update(completed); clear_screenshots(); break
```

Order: `perceive -> smart-llm -> browser-act (fallback) -> screenshot verify`. `stagehand_*` first for web, `hint_*` if deterministic, `ground` only if `hint_snapshot` returns 0 (canvas/WebGL).

## 6. Limits / tuning

* `max 20 options per question` — already satisfied by 8 + max 16 split. For 50+ options raise `agent.cfg["head_max_len"]=512`.
* Multilingual: `Router` auto-routes non-Latin to `laya-multilingual`. Pass `lang_guess=` if you have LID.
* Calibration: base ECE ~0.21-0.46, refit temperature per (type, option-count) on your logs → ~0.08 before trusting probabilities.
* State budget: EN 512 total (192 head + ~320 state). Summarize snapshots to `role+name+ref` triples, not full AX dump.

Refs: `src/main.rs:550-621` (68 tools), `SKILL.md` (transport priority Stagehand→Eval→Snapshot→Screenshot).
