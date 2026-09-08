# BrowserRuntime Migration Report (Phase 0C)

Source of truth: `docs/hyprfast-browserruntime-plan-v3.md`.
Rules: `AGENTS.md`.
Method: re-derived in-session via `rg`/reads of `src/cdp/`, `src/browser/`,
`src/stagehand/`, `src/task.rs`, `src/daemon.rs`, `src/main.rs`.
No `src/` production code was modified to produce this report.

Target directory layout (from the plan, § "Final directory architecture"):

```
src/browser_runtime/
  mod.rs, connection.rs, error.rs,
  server.rs, client.rs, state.rs,
  events.rs, dom_state.rs,
  targets.rs, frames.rs, element_index.rs, dom_diff.rs,
  plan.rs, executor.rs, action.rs, action_journal.rs,
  wait.rs, vision.rs, trace.rs, metrics.rs
```

Capability classes (plan rule 32, Phase 2 wire model, Phase 8 journal):
`runtime_evaluate | cookies | clipboard | file_upload | download | navigation | none`.

---

## 0. Phase 0A answers required by the plan (re-derived)

- **MCP tool names beginning with `browser_` / `stagehand_`** (exact, from
  `src/main.rs:407-431` tool table + `src/main.rs:562-703` handlers): **24 total.**
  - `browser_*` (15): `browser_navigate`, `browser_snapshot`, `browser_click`,
    `browser_hover`, `browser_type`, `browser_select_option`, `browser_press_key`,
    `browser_wait`, `browser_evaluate`, `browser_screenshot`, `browser_tabs`,
    `browser_console`, `browser_go_back`, `browser_go_forward`, `browser_open`.
  - `stagehand_*` (9): `stagehand_act`, `stagehand_observe`, `stagehand_extract`,
    `stagehand_agent`, `stagehand_snapshot`, `stagehand_cache`, `stagehand_metrics`,
    `stagehand_batch`, `stagehand_webmcp`.
  - Adjacent stagehand-owned tools **not** matching the prefix but in scope for
    Phase 15 reconciliation: `context_pages`, `context_cookies`, `cookies_set`,
    `clipboard_write`, `clipboard_read` (`src/main.rs:432-436`, handlers
    `src/main.rs:704-711`).
- **`$XDG_RUNTIME_DIR` socket + permission bits:** there is **no**
  `hyprfast-browser.sock` today. The only Unix socket is the Hyprland-daemon
  socket `hyprfastd.sock` (`src/daemon.rs:14-17`), bound at
  `src/daemon.rs:125-130` with a blind `remove_file` + `UnixListener::bind`
  and **no `chmod(0600)`, no directory-ownership check**. Invariant I20 /
  rule 23 work lands in **Phase 2**.
- **Browser launch / profile strategy today:** `src/main.rs:128-138`
  (`ensure_browser_args`) injects `--remote-debugging-port=9222
  --force-renderer-accessibility`; `BrowserCmd::Open` (`src/main.rs:301-307`)
  and `browser_open` (`src/main.rs:632-640`) launch
  `brave --remote-debugging-port=9222 --force-renderer-accessibility --new-window <url>`
  with **no `--user-data-dir` at all**. There is therefore no ephemeral-vs-production
  profile distinction; rule 22 tagging lands in **Phase 8**, restart policy +
  launch-parameter ownership in **Phase 11**.
- **`target_id` / `session_id` stability assumption:** **yes, assumed stable.**
  `src/cdp/mod.rs:67-80` caches a page `ws_url` for 2s; `get_ws_url_async`
  (`src/cdp/mod.rs:82-103`) picks "last page" with no generation concept;
  backendNodeIds are treated as durable refs across calls; nothing tracks
  connection/target generations. The §3 identity model (`TargetRef` /
  `SessionRef` + `target_generation` / `connection_generation` /
  `frame_tree_version`) lands in **Phases 5/6/11**.
- **CDP event handling today:** none. `rg` for `Target.targetCreated |
  Target.targetDestroyed | Target.attachedToTarget | Target.detachedFromTarget |
  Page.lifecycleEvent | DOM.getDocument | DOM.describeNode | DOM.querySelector |
  DOM.getBoxModel` in `src/` returns **no production consumers** — only a comment
  at `src/browser/mod.rs:137` and doc text in `src/stagehand/snapshot.rs:2`.
  Events land in **Phase 4** (dispatcher + DomState), targets in **Phase 5**.
- **Fixtures present (Phase 0B):** `tests/browser_fixtures/` contains
  `ambiguous_buttons.html`, `analytics_button.html`, `basic.html`,
  `broken_login.html`, `contenteditable.html`, `crash_test.html`,
  `disabled_button.html`, `iframe.html`, `instant.html`, `mutation.html`,
  `navigation.html`, `popup.html`, `search_form.html`, `select.html`,
  `shadow_dom.html`, plus `README.md` and `serve.py`.

---

## 1. Call-site → target-module map

Every row is a real call site found by this session's inspection.
"Keep as thin adapter" means Phase 3/15 leaves the public function/tool shape
and routes its transport through `BrowserRuntimeClient`.

### 1.1 `src/cdp/mod.rs` (transport — dissolved into `connection.rs` / `targets.rs` / `error.rs`)

| Call site | Today | Target module | Phase | Notes |
|---|---|---|---|---|
| `connect_async` ×2 (`src/cdp/mod.rs:135`, `:162`) | per-call page WebSocket | `connection.rs` (sole `connect_async`, browser-level `/json/version`, flat session) | Phase 1 moves; Phase 3 deletes the old paths | Invariants I1/I2/I4/I24/I25 |
| `cdp_call_async` / `cdp_call` (`:134-158`) | single call, fixed `id=1`, 8s/5s timeouts, ignores events | `connection.rs` (`call`, `call_with_timeout`, pending map, reorder buffer) | Phase 1 | Timeout categories refined Phase 8 (§5) |
| `cdp_batch_async` / `cdp_batch` (`:161-186`) | serial calls on a throwaway WS | `connection.rs` (concurrent multiplexed calls) | Phase 1 | |
| `get_ws_url_async` / `get_ws_url` + `WS_CACHE` (`:67-114`) | `/json` poll + 2s last-page cache | `targets.rs` (event-driven `TargetRecord` set, `TargetRef` generations) | Phase 5 | Cache deleted in Phase 3/5 |
| `list_targets_async` / `list_targets` / `list_targets_filtered_async` (`:30-53`) | HTTP GET `/json` per call | `targets.rs` (`list_targets`) served from owned state | Phase 5 | CLI `browser_tabs` reads this |
| `version` (`:55-63`) | GET `/json/version` per call | `connection.rs` (`Browser.getVersion` at startup → diagnostics) | Phase 1 | Surfaced in `status` Phase 2 |
| `new_page_async` (`:116-122`) | PUT `/json/new` | `targets.rs` (`attach_target` / `Target.createTarget` via session) | Phase 5 | |
| `call` / `call_on` (`:190-197`) | discovery + one-shot call | `client.rs` request router | Phase 3 | Tag capability class (Phase 2 model) at migration time |
| `evaluate` (`:200-206`) | discovery + `Runtime.evaluate`, bails on `exceptionDetails` | `executor.rs` + `action.rs` (evaluate action w/ `Verified/Contradicted/Inconclusive`) | Phase 3 moves transport; Phase 8 fixes semantics | Capability `runtime_evaluate` |
| `ensure_enabled` (`:209-215`) | fire-and-forget `Page/DOM/Runtime.enable` | `events.rs` + `dom_state.rs` (per-session enable on connect/reconnect) | Phase 4 | |

### 1.2 `src/browser/mod.rs` (feature layer — thin adapters after Phase 3)

| Call site | Today | Target module | Phase | Notes |
|---|---|---|---|---|
| `ws_for_target` (`:9-11`) | discovery helper | deleted; `client.rs` + `targets.rs` | Phase 3 | |
| `navigate` (`:14-23`) | `Page.enable` + `Page.navigate` + 400ms sleep | `executor.rs` (navigate action) + `wait.rs` (lifecycle condition) + `dom_state.rs` (`wait_for_lifecycle`) | Phase 3 moves; Phase 4 replaces sleep; Phase 9 unifies waiter | Capability `navigation` |
| `go_back` / `go_forward` (`:25-34`) | JS `history.back/forward` via `evaluate` | `executor.rs` (navigation actions, verified via lifecycle + `location.href`) | Phase 8 | Capability `navigation` |
| `snapshot` / `build_snapshot_from_ax` (`:38-80`) | `Accessibility.getFullAXTree`, truncate 80/60, `ref = backendId string` | `element_index.rs` (unified DOM+AX index, versioned `ElementRef`) | Phase 3 moves; Phase 6 rebuilds | Read-only (`none`) |
| `snapshot_via_js` (`:82-112`) | `Runtime.evaluate` DOM walk, `ref = TAG:count` | `element_index.rs` (fallback only; real `backendNodeId`s) | Phase 6 | Fabricated-ref bug B4 |
| `click_by_ref` (`:116-133`) | backend/selector/description guess incl. `:contains()` | `element_index.rs` (resolution order) + `executor.rs` (dispatch + verify) | Phase 3 moves; Phase 6 fixes resolution | Bugs B1/B5 |
| `click_by_backend` (`:135-147`) | `DOM.resolveNode` → `Runtime.callFunctionOn(click)` | `executor.rs` (Resolved → PreconditionCheck → Dispatching → Verifying) | Phase 8 | |
| `click_by_selector` (`:149-153`) | string-built JS `querySelector.click()` | `element_index.rs` + `executor.rs` (structured `DOM.querySelector`/`resolveNode`) | Phase 6 | Bug B2 |
| `hover_by_ref` (`:155-160`) | string-built JS mouseover | `executor.rs` | Phase 6/8 | |
| `type_text` (`:164-190`) | string-built JS fill incl. dead `[data-ref]` selector | `executor.rs` (TYPE/FILL with read-back; mismatch = `Contradicted`) | Phase 6/8 | Bugs B2/B5 |
| `fill` (`:193-197`) | string-built JS | `executor.rs` | Phase 6/8 | Bug B2 |
| `select_option` (`:200-213`) | string-built JS select | `executor.rs` (SELECT reads back selected values) | Phase 8 | |
| `press_key` / `map_key` / `key_code` (`:216-251`) | `Input.dispatchKeyEvent` + JS submit fallback | `executor.rs` (press action) | Phase 8 | Largely reusable logic |
| `evaluate_js` (`:254-257`) | `evaluate(expr, await=true)` | `action.rs` (`runtime_evaluate` capability) + journal | Phase 3 moves; Phase 8 tags | |
| `screenshot_cdp` (`:260-268`) | `Page.captureScreenshot` → bytes | `vision.rs` (last-resort path; DOM-first enforced) | Phase 13 | |
| `tabs` (`:271-275`) | `list_targets` → JSON | `targets.rs` | Phase 5 | |
| `console_logs` (`:281-288`) | `Console.enable` then placeholder note | `events.rs` (Log subscription) → `trace.rs` (Phase 14 surface) | Phase 4/14 | Bug B9 |
| `wait` (`:291-294`) | `thread::sleep` | `wait.rs` (condition engine; unconditional sleep only as explicit wait action) | Phase 9 | Bug B3 |
| `enable_domains` (`:299-305`) | fire-and-forget enables | `events.rs` / `connection.rs` startup | Phase 4 | |

### 1.3 `src/stagehand/` (LLM + snapshot + adapters)

| Call site | Today | Target module | Phase | Notes |
|---|---|---|---|---|
| `snapshot.rs::capture_hybrid` (`:22-55`) | AX-first + JS fallback | `element_index.rs` (index) + `dom_diff.rs` (diff) | Phase 6 (index), Phase 12 (incremental) | Bug B4 (fake encIds in fallback `:95-140`) |
| `snapshot.rs::build_tree_from_ax` / `trim_tree` (`:57-93`) | flat lines, char truncate | `element_index.rs` + `trace.rs`/`metrics.rs` budgets | Phase 6 | |
| `snapshot.rs::parse_enc_id` (`:143-148`) | `ord-backend` parse | `element_index.rs` (`ElementRef` §3 shape) | Phase 6 | |
| `snapshot.rs::diff_trees` (`:151-156`) | added-lines diff | `dom_diff.rs` (atomic `DomDiff`) | Phase 12 | `a11y/tree_format.rs` diff merges here too |
| `act.rs::execute_action` (`:65-187`) per-method arms | resolve-by-backend + string JS + first-match fallbacks | `executor.rs` + `action.rs` + `element_index.rs` | Phase 3 moves; Phase 6 (resolution) / Phase 8 (dispatch/verify/journal) | Bugs B2/B5/B14 |
| `act.rs::act` / `replay_cached` (`:189-328`) incl. 500ms two-step sleep (`:283`) | LLM → action → self-heal → dropdown second inference | `executor.rs` (state machine) + `plan.rs` (two-step as plan) + `wait.rs` (replace sleep) | Phase 8/9; plan shape Phase 7 | Bug B3 |
| `observe.rs::observe` (`:10-63`) | chunked LLM over tree + xpath enrich | `executor.rs` (read-only path) + `element_index.rs` | Phase 15 compat; resolution Phase 6 | |
| `extract.rs::extract` (`:8-46`) | LLM over tree, warn-only schema check | `executor.rs` (EXTRACT validates schema fields) | Phase 8/15 | |
| `agent.rs::execute` (`:8-50`) incl. blind `sleep` wait arm (`:33`) | act/extract/navigate loop | `plan.rs` + `executor.rs` (plan-step deadlines §5) + `wait.rs` | Phase 7/8/9 | Bug B3 |
| `batch.rs::experimental_batch` (`:5-10`) | `Runtime.evaluate(callbackSource)`, ignores `input` + `_timeout_ms` | `plan.rs` + `executor.rs` (deadline composition) | Phase 7/8 | Bug B8 |
| `clipboard.rs::write/read/clear` (`:3-5`) | `Runtime.evaluate` clipboard JS | `executor.rs` (`clipboard` capability) | Phase 8 | |
| `context.rs::new_page/pages/add_init_script` (`:6-8`) | one-shot CDP | `targets.rs` (pages), `executor.rs` (init-script nav hook) | Phase 5/8 | `set_extra_http_headers`/`normalize_domain_policy` (`:9-10`) no-ops → Phase 8/15 (B18) |
| `cookies.rs::get/set` (`:8-9`) | `Storage.getCookies` / per-cookie `setCookies` | `executor.rs` (`cookies` capability) | Phase 8 | Bug B19 |
| `cookies.rs::filter/normalize/to_cdp/matches` (`:4-7`) | identity/`true` stubs | `executor.rs` or deleted | Phase 8/15 | Bug B19 |
| `file_upload.rs::set_input_files` (`:7-10`) | dispatches `change`, never sets files | `executor.rs` (`file_upload` capability via `DOM.setFileInputFiles`) | Phase 8 | Bug B7 |
| `locator.rs::LocatorHandle::*` (`:8-14`) + `locator_for` (`:15`) | string-JS click; `count()=1`, `is_visible()=true` constants | `element_index.rs` (locators resolve via index) + `executor.rs` (visibility precondition) | Phase 6/8 | Bugs B2/B6/B12 |
| `page.rs::goto/reload/evaluate` (`:5-7`) | one-shot CDP, `page_id` ignored | `targets.rs` + `executor.rs` | Phase 5/8 | Bug B6 |
| `page.rs::screenshot` (`:8`) | `Ok(vec![])` stub | `vision.rs` | Phase 13 | Bug B10 |
| `page.rs::wait_for_load_state` (`:9`) | `Ok(())` no-op | `wait.rs` (`NavigationComplete`/`Lifecycle`) | Phase 9 | Bug B11 |
| `webmcp.rs::list/invoke_tools` (`:4-12`) | `Runtime.evaluate(__webmcp*)`, `page_id` ignored | `executor.rs` (`runtime_evaluate` capability) | Phase 8/15 | Bug B6 |
| `a11y/xpath.rs` (`:42-112`) | `resolveNode` → `callFunctionOn(NODE_TO_XPATH_JS)` chain builder | `element_index.rs` (frame-aware resolution) | Phase 6 | Reusable JS (`NODE_TO_XPATH_JS` `:6-40`) |
| `a11y/sessions.rs` (`:3-4`), `active_element.rs:3`, `coordinate.rs:3`, `dom_tree.rs`, `a11y_tree.rs`, `focus.rs` | `None`/`Null`/`false` stubs | `targets.rs` (sessions), `frames.rs` (owner/frame tail), `element_index.rs`, `vision.rs` (coordinate) | Phase 5/6/10/13 | Bug B13 |
| `frame.rs::Frame/FrameRegistry/FrameLocator` (`:6-21`) | in-memory maps, `evaluate`/`screenshot` stubs | `frames.rs` (`frame_tree_version`, execution contexts) | Phase 5/6 | |
| `deep_locator.rs` (`:5-9`) | echo-selector / `None` stubs | `element_index.rs` (iframe/shadow traversal) + Phase 10 ladder | Phase 6/10 | Bug B13 |
| `llm.rs`, `prompt.rs`, `protocol.rs`, `cache.rs`, `instrumentation.rs` | LLM wire, prompts, schemas, disk cache, token metrics | **stay**; consumed by `executor.rs`/`plan.rs`/`trace.rs`/`metrics.rs` | N/A (supporting) | `cache status/clear` + `metrics snapshot` surface via Phase 14/15 |

### 1.4 `src/main.rs` / `src/daemon.rs` / `src/task.rs`

| Call site | Today | Target module | Phase | Notes |
|---|---|---|---|---|
| CLI `BrowserCmd` enum + dispatch (`:26-59`, `:250-310`) incl. inline backend-click (`:261-274`) | per-command direct calls | `client.rs` router; inline CDP deleted in favor of `browser::click_by_ref` path | Phase 2 (router) / Phase 3 (migration) / Phase 15 (compat) | |
| CLI `StagehandCmd` dispatch (`:62-81`, `:311-346`) | direct `stagehand::*` calls | `client.rs` router | Phase 2/15 | `browser_execute_plan` added Phase 7 |
| MCP tool table (`:387-440`) + `handle_tool` (`:469-754`) incl. inline backend-click (`:574-587`) | direct calls, no handshake | `server.rs` (handshake + capability-tagged requests) + `client.rs` | Phase 2 (handshake/router), Phase 15 (compat) | |
| `ensure_browser_args` (`:128-138`), `Launch` (`:158-171`), `Open` (`:301-307`), `browser_open` (`:632-640`) + 600/800ms sleeps | launch w/o user-data-dir | Phase 11 owns launch/restart params; Phase 8 tags `production_profile` | Phase 8/11 | Bugs B3/B24 |
| `src/daemon.rs` hyprfastd (`:14-187`) | Hyprland-only socket, no perms, blind unlink | `server.rs` + `state.rs` (new `hyprfast-browser.sock`, 0600, ownership check, lifecycle table, stale-vs-live socket logic) | Phase 2 | Coexists with hyprfastd; never merged silently |
| `src/task.rs` persistence (`:29-62`, `:64-259`) | `$XDG_RUNTIME_DIR/hyprfast-tasks.json` todo state | **reused as-is** — Phase 11 integrates, does not fork | Phase 11 | Explicit plan constraint: no second task system |

---

## 2. Deferred-bug list (Phase 3 defers against this — precise, with fixing phase)

> Transport bugs (T-series) are fixed by Phases 1–3. Resolution/execution bugs
> (B-series) are **not** fixed in Phase 3 even when visible during migration —
> Phase 3's report must reconcile each one back to this list.

### T-series (transport — Phases 1/3, not deferred past migration)

- **T1 — per-operation WebSockets.** `src/cdp/mod.rs:135`, `:162` (`connect_async`
  ×2); every `cdp_call`/`cdp_batch` opens + closes a WS. → **Phase 1**
  (one browser-level WS), **Phase 3** (delete old paths; `grep connect_async`
  clean). Invariants I1/I2/I24.
- **T2 — `/json` discovery per call.** `src/cdp/mod.rs:30-53`, `:82-114`
  (plus 2s `WS_CACHE` `:67-80`). → **Phase 5** (event-driven targets);
  cache removed in **Phase 3/5**.
- **T3 — no multiplexing.** Fixed `id = 1` (`src/cdp/mod.rs:136`), no
  `sessionId` on any request, no `flatten`/`setAutoAttach` anywhere. → **Phase 1**
  (rule 34 fail-closed). Invariants I4/I25.
- **T4 — no event consumption.** Zero subscribers for `Target.*` /
  `Page.lifecycleEvent` / `DOM.*` (grep-clean in `src/`). → **Phase 4/5**.
- **T5 — unstructured errors.** `anyhow::bail!` everywhere
  (e.g. `src/cdp/mod.rs:141`, `:173`; `src/browser/mod.rs:15`, `:78`). →
  **Phase 1** (`error.rs`: `ConnectionFailed/RuntimeDead/Timeout/CdpError/
  InvalidResponse/UnsupportedBrowserProtocol`) + Phase 2 state errors
  (`NotReady/Reconnecting/ShuttingDown/ProtocolMismatch`).

### B-series (deferred — Phase 3 must NOT fix these)

- **B1 — invalid `:contains()` CSS.** `src/browser/mod.rs:129` generates
  `button:contains("...")` (not a CSS selector; `querySelector` throws). →
  **Phase 6** (eliminate `:contains()` entirely; AX/text tiers instead).
  Fixture: `ambiguous_buttons.html` / `search_form.html`.
- **B2 — string-concatenated JS with user input.** `src/browser/mod.rs:150`
  (click), `:157` (hover), `:167-187` (type), `:194` (fill), `:201-210`
  (select), `:229` (key fallback); `src/stagehand/act.rs:113` (fill decl),
  `:119` (activeElement type), `:135` (select decl), `:147` (scroll pct),
  `:182` (`querySelector('*')?.click()`), `:232`, `:255` (text-search
  fallbacks); `src/stagehand/locator.rs:8`; `src/stagehand/file_upload.rs:9`;
  `src/stagehand/clipboard.rs:3`; `src/stagehand/batch.rs:9`;
  `src/stagehand/webmcp.rs:11`; `src/stagehand/page.rs:7`. → **Phase 6**
  (`DOM.querySelector` / `DOM.resolveNode` / `Runtime.callFunctionOn` with
  structured params, never `format!` JS).
- **B3 — blind sleeps for browser sync.** `src/browser/mod.rs:21` (400ms after
  `Page.navigate`), `src/browser/mod.rs:292` (`wait`), `src/main.rs:305`,
  `:638` (800ms after open), `src/main.rs:162`, `:494` (600ms launch),
  `src/stagehand/act.rs:283` (500ms two-step), `src/stagehand/agent.rs:33`
  (wait arm). → navigation/lifecycle sleeps → **Phase 4**
  (`wait_for_lifecycle`: check→subscribe→re-check→await), generalized **
  Phase 9** (single condition engine incl. `FrameTreeVersionAtLeast`).
  (The explicit user-facing wait action itself survives as a `wait.rs`
  condition.)
- **B4 — fabricated encIds.** `src/stagehand/snapshot.rs:124-125`
  (`"0-"+(10000+count)`); comment admits faking (`:124`). These collide with
  real `backendDOMNodeId`s and poison `parse_enc_id` resolution. →
  **Phase 6** (real backend IDs only). Fixture: `basic.html`.
- **B5 — dead selectors.** `src/browser/mod.rs:165` (`[data-ref="..."]` never
  written to the DOM); `src/stagehand/act.rs:99`
  (`[data-stagehand-id='...']` never written). → **Phase 6** (resolution
  order has no such tier; remove).
- **B6 — ignored `page_id` / target params.** `src/stagehand/page.rs:5`
  (`goto`), `:7` (`evaluate`); `src/stagehand/webmcp.rs:4`, `:9`;
  `src/stagehand/locator.rs:15` (`locator_for` stores but `click` `:8`
  re-discovers); `src/stagehand/frame.rs:8-9` stubs;
  `src/stagehand/a11y/active_element.rs:3`, `coordinate.rs:3`,
  `sessions.rs:3-4`, `deep_locator.rs:9`. All ops hit "last page". →
  **Phase 5** (`TargetRef`/`SessionRef` routing; switching never opens a WS).
  Fixture: `popup.html`.
- **B7 — no-op file upload.** `src/stagehand/file_upload.rs:7-10` only
  dispatches a `change` event; `files` is explicitly ignored
  (`let _ = files`). → **Phase 8** (`file_upload` capability via
  `DOM.setFileInputFiles` + verification + journal).
- **B8 — ignored batch timeout + input.** `src/stagehand/batch.rs:5`
  (`_timeout_ms` unused; `input` unused — callback runs raw). →
  **Phase 7** (plan modeling) + **Phase 8** (§5 deadline composition:
  resolution/dispatch/verification + plan-step ceiling; dispatch-timeout on
  state-changing op ⇒ `Unknown`).
- **B9 — placeholder console logs.** `src/browser/mod.rs:281-288`
  (`Console.enable`, then reads nonexistent `window._hyprfast_console`,
  returns a "for now returning empty" note). → **Phase 4** (Log-domain event
  subscription) surfaced **Phase 14** (diagnostics). Invariant I14-adjacent.
- **B10 — stub screenshots.** `src/stagehand/page.rs:8` (`Ok(vec![])`),
  `src/stagehand/frame.rs:9` (`Ok(vec![])`). → **Phase 13** (vision
  last-resort only; `VisualTarget` never silently becomes `ElementRef`).
  Fixture: canvas case (Phase 13 DoD).
- **B11 — `wait_for_load_state` no-op.** `src/stagehand/page.rs:9`
  (`Ok(())`). → **Phase 9** (`NavigationComplete`/`Lifecycle` conditions).
  Fixture: `navigation.html` (+ 0B HTTP harness).
- **B12 — locator constants.** `src/stagehand/locator.rs:10`
  (`count() → 1`), `:11` (`is_visible() → true`). → **Phase 6** (real
  resolution/count) + **Phase 8** (interactability as pre-action failure,
  rule 10). Fixture: `disabled_button.html`.
- **B13 — a11y/frame/session stubs.** `active_element.rs:3`,
  `coordinate.rs:3`, `dom_tree.rs` (all fns `false`/`Null`/`None`),
  `a11y_tree.rs` (`Null`/`false`), `focus.rs` (echo), `sessions.rs:3-4`
  (`None`), `deep_locator.rs:8-9` (echo/`None`), `frame.rs:14-18`
  (map-only registry, `seed_from_frame_tree` empty). → **Phase 6** (index +
  XPath/frame-tail resolution), **Phase 10** (recovery ladder consumes them),
  coordinate→ **Phase 13** (vision-only). Fixtures: `iframe.html`,
  `shadow_dom.html`.
- **B14 — ambiguous-action violations (rule 7).** `src/stagehand/act.rs:100`
  (`querySelectorAll('*')[0]?.click()` fallback), `:182`
  (`querySelector('*')?.click()`), `:232`, `:255` (first text-match wins). →
  **Phase 6** (`AmbiguousElement`, never first-match) + **Phase 8**
  (explicit terminal states). Fixture: `ambiguous_buttons.html`.
- **B15 — no verification semantics.** Type/fill never read back
  (`src/browser/mod.rs:164-197` return `typed` count only); click returns
  `clicked:true` unconditionally (`:149-153`, `:135-147` ignores `clicked`
  value); `cdp::evaluate` bails on any `exceptionDetails`
  (`src/cdp/mod.rs:204`). No `Verified/Contradicted/Inconclusive`
  distinction anywhere. → **Phase 8** (rule 9 + §2 state machine).
  Fixtures: `analytics_button.html` (Inconclusive), `broken_login.html`
  (Contradicted), `disabled_button.html` (precondition).
- **B16 — snapshot truncation without versions.** `src/browser/mod.rs:45`
  (`take(max_nodes)`), `:59`, `:76` (`take(80)`/60-cap);
  `src/stagehand/snapshot.rs:62` (`take(300)`), `:80` (150-line cap),
  char-truncate `:87-93`. No `dom_version`/`frame_tree_version`/
  `target_generation` carried on any ref. → **Phase 4** (generations) +
  **Phase 6** (versioned `ElementRef` §3/§6) + **Phase 12** (incremental
  updates instead of re-truncate). Fixture: `mutation.html`.
- **B17 — no stale-ref / TOCTOU protection.** Refs are bare strings/ints
  (`backendId` in `browser_click`, `encId` in `act`) with no
  created-against versions and no dispatch-time re-check. → **Phase 6**
  (versioned refs) + **Phase 8** (§6 snapshot re-check → `StaleElementRef`,
  never silent substitution). Fixture: `mutation.html`.
- **B18 — no-op context/policy ops.** `src/stagehand/context.rs:9`
  (`set_extra_http_headers → Ok(())`), `:10` (`normalize_domain_policy`
  echo); `add_init_script` (`:8`) fires without session tracking. →
  **Phase 8** (capability-tagged, journaled) / **Phase 15** (compat decision).
- **B19 — cookie helpers are identity stubs + wrong call shape.**
  `src/stagehand/cookies.rs:4-7` (`filter_cookies` echo, `matches → true`);
  `:9` sends `{"cookies":[c]}` per cookie instead of one `Storage.setCookies`
  with proper params. → **Phase 8** (`cookies` capability, verified).
- **B20 — select/type success is assumed.** `select_option`
  (`src/browser/mod.rs:200-213`) returns `selected: vals` (the *requested*
  values, not read-back); `press_key` (`:216-231`) ignores CDP errors
  (`let _ =`). → **Phase 8** (SELECT reads back; dispatch errors are
  `Failed`, dispatch timeouts on state-changing ops are `Unknown` per
  rule 31). Fixture: `select.html`, `contenteditable.html`.
- **B21 — `input` events without trusted-input path.** All typing synthesizes
  `el.value=` + `new Event('input')` from JS (`src/browser/mod.rs:167-187`,
  `src/stagehand/act.rs:113-121`) rather than `Input.insertText` /
  `DOM.setFileInputFiles`-class trusted paths. → **Phase 8** (executor
  dispatch choice + read-back verification).
- **B22 — shadow-DOM / iframe traversal absent.** Snapshot walkers use
  `document.body` `TreeWalker` only (`src/browser/mod.rs:88`,
  `src/stagehand/snapshot.rs:103`); no `pierce=true`, no
  `frameAttached/frameNavigated/frameDetached` handling. → **Phase 6**
  (`DOM.describeNode` / `getFlattenedDocument(pierce=true)`, per-frame
  execution contexts). Fixtures: `iframe.html`, `shadow_dom.html`.
- **B23 — error-shape loss at MCP boundary.** `handle_tool` stringifies all
  failures to `"error: {}"` text (`src/main.rs:457`); CLI paths print
  pretty JSON with no error taxonomy. → **Phase 2** (structured
  `ProtocolMismatch`/state errors) + **Phase 8** (terminal states) +
  **Phase 15** (stable tool contracts).
- **B24 — launch/socket hygiene.** No `--user-data-dir`
  (`src/main.rs:128-138`, `:301-307`, `:632-640`); daemon blind-unlinks a
  possibly-live socket (`src/daemon.rs:126-130`); no 0600 anywhere; no
  handshake; no lifecycle states. → **Phase 2** (0600 + ownership +
  handshake + lifecycle table + stale-vs-live socket), **Phase 8**
  (`production_profile` tagging, rule 22), **Phase 11** (launch params +
  default-off restart, rule 24). Invariants I11/I20/I23/I24/I26.

---

## 3. MCP/CLI preservation matrix (all preserved; router Phase 2, compat Phase 15)

Every `browser_*` / `stagehand_*` tool below exists today (`src/main.rs`
tool table + handler cited) and is preserved. "Router" = Phase 2
(`server.rs`/`client.rs`/`state.rs`: handshake, capability tagging,
per-target serialization). "Compat" = Phase 15 (exact tool-count
reconciliation against this §0 list + real-Chromium exercise of every path).
`browser_execute_plan` (new, additive) arrives in **Phase 7**.

| MCP tool | Defined (table) | Handled | CLI parity | Preserved by | Notes |
|---|---|---|---|---|---|
| `browser_navigate` | `:407` | `:562-566` | `browser navigate` | Router Ph2 → Compat Ph15 | Capability `navigation`; sleep fixed Ph4/9 (B3) |
| `browser_snapshot` | `:408` | `:567` | `browser snapshot` | Router Ph2 → Compat Ph15 | Real index Ph6 (B16/B17) |
| `browser_click` | `:409` | `:568-587` | `browser click` | Router Ph2 → Compat Ph15 | Resolution fixed Ph6 (B1/B2/B5/B14) |
| `browser_hover` | `:410` | `:588-593` | `browser hover` | Router Ph2 → Compat Ph15 | Ph6/8 |
| `browser_type` | `:411` | `:594-601` | `browser type` | Router Ph2 → Compat Ph15 | Read-back Ph8 (B15/B20/B21) |
| `browser_select_option` | `:412` | `:602-608` | `browser select` | Router Ph2 → Compat Ph15 | Read-back Ph8 (B20) |
| `browser_press_key` | `:413` | `:609-612` | `browser press` | Router Ph2 → Compat Ph15 | Ph8 |
| `browser_wait` | `:414` | `:613-616` | `browser wait` | Router Ph2 → engine Ph9 → Compat Ph15 | Condition engine (B3) |
| `browser_evaluate` | `:415` | `:617-620` | `browser eval` | Router Ph2 → Compat Ph15 | Capability `runtime_evaluate`; journaled Ph8 |
| `browser_screenshot` | `:416` | `:621-627` | `browser shot` | Router Ph2 → vision Ph13 → Compat Ph15 | Last-resort only (B10) |
| `browser_tabs` | `:417` | `:628` | `browser tabs` | Targets Ph5 → Compat Ph15 | Event-driven (T2) |
| `browser_console` | `:418` | `:629` | `browser console` | Events Ph4 → trace Ph14 → Compat Ph15 | Placeholder fixed (B9) |
| `browser_go_back` | `:419` | `:630` | `browser back` | Router Ph2 → verified nav Ph8 → Compat Ph15 | |
| `browser_go_forward` | `:420` | `:631` | `browser forward` | Same as go_back | |
| `browser_open` | `:421` | `:632-640` | `browser open` | Launch policy Ph11 → Compat Ph15 | user-data-dir (B24) |
| `stagehand_act` | `:423` | `:642-649` | `stagehand act` | Router Ph2 → executor Ph8 → Compat Ph15 | B2/B14/B15 |
| `stagehand_observe` | `:424` | `:650-656` | `stagehand observe` | Router Ph2 → Compat Ph15 | |
| `stagehand_extract` | `:425` | `:657-665` | `stagehand extract` | Router Ph2 → verified Ph8 → Compat Ph15 | Schema validation (Ph8 DoD) |
| `stagehand_agent` | `:426` | `:666-674` | `stagehand agent` | Plan Ph7 → executor Ph8 → Compat Ph15 | Loop becomes plans |
| `stagehand_snapshot` | `:427` | `:675-678` | `stagehand snapshot` | Index Ph6 → Compat Ph15 | B4/B16 |
| `stagehand_cache` | `:428` | `:679-685` | `stagehand cache` | Router Ph2 → Compat Ph15 | `cache.rs` retained |
| `stagehand_metrics` | `:429` | `:686` | `stagehand metrics` | Metrics Ph14 → Compat Ph15 | `instrumentation.rs` → `metrics.rs` |
| `stagehand_batch` | `:430` | `:687-692` | `stagehand batch` | Plan Ph7 → deadlines Ph8 → Compat Ph15 | Timeout honored (B8) |
| `stagehand_webmcp` | `:431` | `:693-703` | `stagehand webmcp` | Router Ph2 → Compat Ph15 | `page_id` routed Ph5 (B6) |
| `context_pages` | `:432` | `:704` | — (MCP-only) | Targets Ph5 → Compat Ph15 | |
| `context_cookies` | `:433` | `:705` | — (MCP-only) | Executor Ph8 (`cookies`) → Compat Ph15 | B19 |
| `cookies_set` | `:434` | `:706-709` | — (MCP-only) | Executor Ph8 (`cookies`) → Compat Ph15 | B19 |
| `clipboard_write` | `:435` | `:710` | — (MCP-only) | Executor Ph8 (`clipboard`) → Compat Ph15 | |
| `clipboard_read` | `:436` | `:711` | — (MCP-only) | Executor Ph8 (`clipboard`) → Compat Ph15 | |

Out of scope for the `browser_*`/`stagehand_*` count but explicitly **not**
removed: `task_*`, `ground`/`act_fast`/`act_batch`, desktop/hypr/AT-SPI/pointer/
keyboard/screenshot/wait_for/binds/session tools — Phase 15 audits against the
narrower 24-tool list above per the plan; the rest keep working via the same
daemon coexistence rule as hyprfastd.

---

## 4. Migration order (for Phase 3's use)

1. Transport first (Phase 1): `connection.rs` + `error.rs`; prove 1 WS,
   2 targets/sessions, 100 multiplexed calls, ordering test, kill test.
2. Daemon + router (Phase 2): `server.rs`/`client.rs`/`state.rs`; handshake,
   0600, lifecycle table, capability tagging on the wire model.
3. Mechanical migration (Phase 3): `src/browser/mod.rs` → `client.rs` calls;
   then `act/batch/clipboard/context/cookies/file_upload/locator/page/
   snapshot/webmcp/a11y/xpath.rs` in that order (highest fan-out first);
   tag capability classes inline; reconcile every B-bug encountered here
   against §2 without fixing.
4. State (Phases 4–6): events → targets → index; B-bugs fixed in Phase 6.
5. Trust (Phases 7–9): plans → executor/journal → waits; B7/B8/B11/B15/B20
   fixed here.
6. Robustness + compat (Phases 10–15): ladder → crash policy → incremental
   diffs → vision → tracing → full tool reconciliation.

## 5. Reusable implementation worth keeping (not bugs)

- `NODE_TO_XPATH_JS` (`src/stagehand/a11y/xpath.rs:6-40`) + `prefix/normalize/
  join/build_child` helpers (`:72-112`) — lift into `element_index.rs`.
- Key map (`src/browser/mod.rs:233-251`) — lift into `action.rs`/`executor.rs`.
- Hybrid snapshot shape (`combined_tree` + `xpathMap`, `snapshot.rs:10-18`,
  `build_tree_from_ax` `:57-85`) — evolves into the versioned index, not
  discarded.
- Prompt/LLM/protocol/cache/instrumentation layer (`prompt.rs`, `llm.rs`,
  `protocol.rs`, `cache.rs`, `instrumentation.rs`, `observe.rs`, `extract.rs`)
  — stays above the runtime; planner/vision escalation only after
  deterministic resolution is exhausted (rule 18).
- `src/task.rs` persistence — integrated by Phase 11, never reimplemented.
- Fixture set + `serve.py` harness — exercised Phases 4/6/8/9/12/16 per §5
  fixture notes.

## 6. Known open questions for later phases (not decisions)

- Exact `protocol_version` numbering for the Phase 2 handshake (propose in
  Phase 2; reject mismatches per I26).
- Decode-offload threshold tuning around the 2MB default (rule 25; measure
  in Phase 1 ordering test + Phase 14).
- `reorder_buffer_depth` alert thresholds (Phase 1 diagnostics → Phase 14).
- Which `browser_open` launch flags beyond `--user-data-dir` become part of
  the recorded identical-restart set (Phase 11).
