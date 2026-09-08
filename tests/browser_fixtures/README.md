# Browser fixtures (Phase 0B)

Real HTML/JS pages loaded into the real Chromium/Brave in later phases.
Never mocks: each fixture is a genuine webpage exercising what its name
says. Per-fixture table below is the map **Phase 16 uses to confirm
coverage** — each row names the later phase(s) and DoD that exercise it
and why the fixture exists.

Harness: `serve.py` (see "Serving" below). Fixtures work over both
`file://` and `http://127.0.0.1:<port>/`, except where noted.

## Fixture → phase/DoD map

| Fixture | Later phase / DoD | Why it exists |
|---|---|---|
| `basic.html` | Phase 1 (transport smoke: evaluate against a live target); Phase 4 (lifecycle sanity); Phase 15 (CLI/MCP compat baseline) | Minimal baseline: heading, paragraph, one real button (`#ok-button`) that writes `clicked` into `#status`. Proves the runtime can attach, evaluate, and click at all before fancier fixtures matter. |
| `iframe.html` | Phase 6 DoD (iframe resolution via correct frame execution context); Phase 9 (`FrameTreeVersionAtLeast` test: "wait for this iframe to finish navigating"); failure-injection #4 (iframe navigates during resolution → snapshot re-check) | Embeds a real `<iframe id="inner-frame">` (srcdoc) containing a real button (`#iframe-button`) that writes `iframe-clicked` into `#iframe-status`. Cross-frame `ElementRef.frame_id` resolution target. |
| `shadow_dom.html` | Phase 6 DoD (shadow DOM: walk real shadow roots via `DOM.describeNode` / `getFlattenedDocument(pierce=true)`); Phase 10 (recovery-tier frame/shadow traversal) | Open shadow root on `#shadow-host` with a real button (`#shadow-button`) inside; click writes `shadow-clicked` into outer `#status`. Closed roots are intentionally NOT covered (open-only per plan). |
| `navigation.html` | Phase 4 DoD (delayed-navigation timing proves completion is event-driven via `wait_for_lifecycle`, not a ~400ms sleep); Phase 8 (NAVIGATE requires CDP success + lifecycle event + `location.href`); Phase 9 (`NavigationComplete`/`Lifecycle` conditions) | Delayed navigation two ways: plain link (`#delayed-link` → `instant.html`) and a JS 1500ms timer button (`#delayed-nav-button`). Prefer the `/slow` harness endpoint for server-side delay; this file covers client-side delayed navigation. |
| `disabled_button.html` | Phase 8 DoD ("Disabled: click disabled button → `ElementNotInteractable` in `PreconditionCheck`, before `Dispatching`") | Genuinely disabled button (`#disabled-button[disabled]`); clicking must fail pre-action per contract rule 10, never as a post-hoc verification failure. `#enable-button` allows positive-path tests. |
| `ambiguous_buttons.html` | Phase 6 DoD (ambiguity: equally-valid candidates ⇒ `AmbiguousElement`, never click-first); failure-injection #20 (ambiguity appearing only after a recovery-tier fallback); final acceptance (ambiguous resolution rejects) | Two buttons (`#submit-a`, `#submit-b`) with the exact same accessible name/label "Submit". Resolver must return both candidates, not guess. |
| `mutation.html` | Phase 8 DoD (snapshot staleness: resolve → mutate → dispatch ⇒ `StaleElementRef`, no click on substituted node); Phase 10 DoD (stale ref → same semantic identity, new `backendNodeId` → recovery; ambiguity-after-recovery); Phase 12 DoD (thousands of real mutations, add/remove/change/`invalidated_refs` correctness + before/after perf); failure-injection #5 (DOM mutates between resolution and dispatch) | Mutates DOM on demand: `#mutate-button` replaces `#target-button` with a same-id/new-identity node (bumps `data-version`), `#add-button` appends nodes, `#remove-button` detaches target, `#attr-button` modifies an attribute. Covers all four Phase 12 mutation classes. |
| `popup.html` | Phase 5 DoD (popup two-target test: `targetCreated`/`attachedToTarget` trace, one WebSocket throughout, `switch_target` never creates a WebSocket); failure-injection #6 (popup opens during a click → still one WebSocket); final acceptance (open popup → second target → switch) | Real `window.open('basic.html', ...)` popup via `#open-popup`; `#status` records `popup-opened` vs `popup-blocked` (headless automation must allow popups for this fixture). Event-driven discovery target — never `/json` polling. |
| `search_form.html` | Phase 7 DoD (navigate → type search → submit → `wait_for` URL → extract; verify real URL and DOM state) | Real GET form (`#search-form`, `#search-input[name=q]`, `#search-submit`); after submit the page reflects `?q=` into `#search-results` and back into the input, so type→submit→URL→extract round-trips. |
| `analytics_button.html` | Phase 8 DoD ("False-cascade: analytics button → `Inconclusive`; next evaluate step still runs") | Button (`#analytics-button`) whose click fires an analytics beacon (`sendBeacon` + `window.__analyticsEvents`) with deliberately NO DOM mutation, attribute/class change, navigation, or dialog. Verification must be `Inconclusive` per contract rule 9 — not failure, must NOT stop the plan. |
| `broken_login.html` | Phase 8 DoD ("Genuine contradiction: broken login → click → expected navigation absent → `Contradicted` → later steps `Skipped`") | Login form (`#login-form`, `#username`, `#password`, `#login-submit`) whose submit is `preventDefault`ed and only writes an inline error into `#login-status` — never navigates. The negative counterpart to `analytics_button.html`: here an expected effect is missing ⇒ `Contradicted`, plan stops. |
| `instant.html` | Phase 4 DoD ("instant fixture × 50 runs, no missed lifecycle event") | Loads instantly: static HTML, no scripts, no timers. Marker `#ready` (`ready`) proves lifecycle-event detection (`Page.lifecycleEvent`) works without races on trivially-fast loads. Target of `navigation.html` links. |
| `contenteditable.html` | Phase 8 DoD (TYPE/FILL always read back `value`/`textContent`, mismatch ⇒ `Contradicted`); Phase 15 (compat across input classes) | Real `contenteditable` region (`#editor[role=textbox]`) plus `#read-button` that copies its `textContent` into `#status` for read-back verification of non-`<input>` editing. |
| `select.html` | Phase 8 DoD ("SELECT reads back selected values") | Real `<select id="choice">` with three options (alpha/beta/gamma); submit writes `selected: <value>` into `#status` so the executor can read back and compare selected values. |
| `crash_test.html` | Phase 11 DoD (all three `restart_on_crash` scenarios + generation-rejection asserts); Phase 8 DoD ("Unknown action: kill browser after `Dispatching` begins" — renderer hang covers the timeout side); failure-injection #1–#3 (browser closes during evaluate / after click dispatch / tab closes during resolution), #13 (SIGTERM mid-action), #14–#16 (crash with policy off / on+launched / on+attached) | Crash-scenario helpers: `#crash-button` navigates to `chrome://crash` for a real renderer crash (http-served); `#hang-button` spins the renderer 10s so dispatch-timeout ⇒ `Unknown` (rule 31) is exercisable without killing the process; `#close-button` closes the page for target-destroyed paths. |

## Serving

`file://` alone cannot exercise `Page.lifecycleEvent` timing the way a
deliberately-delayed local HTTP response can, so `serve.py` (stdlib only,
binds `127.0.0.1`, ThreadingHTTPServer) is the controllable HTTP target:

```sh
# Terminal 1: serve fixtures on :8901
python3 tests/browser_fixtures/serve.py --port 8901

# Terminal 2: smoke test (no browser required)
curl -s http://127.0.0.1:8901/healthz
curl -s http://127.0.0.1:8901/basic.html | head -5
time curl -s "http://127.0.0.1:8901/slow?target=navigation.html&delay=2.0" | head -5
```

Endpoints: `/<fixture>.html` (static), `/slow?target=<file>&delay=<sec>`
(sleeps, then serves `<file>`; basename-clamped to this directory, delay
clamped to 0–30s), `/healthz` (`ok`, for wait-for-ready polling).

`file://` usage is fine for fixtures that need no real HTTP navigation
(e.g. `basic.html`, `disabled_button.html`, `ambiguous_buttons.html`,
`shadow_dom.html`, `mutation.html`, `contenteditable.html`, `select.html`,
`analytics_button.html`, `broken_login.html`, `search_form.html` —
note: form-submit URL reflection works under `file://` too, but Phase 7
should prefer HTTP). `navigation.html`, `instant.html` (as a navigation
target), `popup.html` (popup URL resolution), `iframe.html`, and
`crash_test.html` (`chrome://crash` path) should be served over HTTP via
`serve.py` in later phases.
