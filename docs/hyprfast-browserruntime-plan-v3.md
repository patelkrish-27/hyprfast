# hyprfast BrowserRuntime — Final Agent Implementation Plan (v3)

Supersedes v2. This round closes the gaps a second review found in v2:
state ownership, the runtime lifecycle state machine, transport ordering
under backpressure, concurrency vs. serialization boundaries,
cancellation semantics, timeout/deadline categories, an expanded action
state machine, identity/generation modeling, snapshot consistency, a
capability/protocol-versioned IPC layer, a formal invariants list, and a
real failure-injection test suite. Everything from v2 is preserved;
additions are marked **[V3]**.

Architectural principle, unchanged:

> The BrowserRuntime is the single source of truth for browser state.
> CLI/MCP commands are clients of it. The daemon owns the persistent CDP
> connection. DOM/AX/target/frame state is maintained from real CDP
> events. Actions resolve against that state, execute through CDP, and
> are verified against real browser state.

---

## 0. Non-negotiable agent contract

Paste this whole section to the agent before Phase 0A. Do not summarize
it — agents drop constraints when constraints are summarized.

```
You are implementing a production-grade persistent browser automation
runtime inside the existing hyprfast Rust repository.

This is NOT a prototype, mock, demo, simulated browser, fake CDP
implementation, or static-fixture-driven browser simulator.

The final system must operate against real Chromium/Brave using real CDP.
```

### 1. Inspect before modifying

Before changing any file: read the relevant existing implementation
completely; identify callers; identify existing error handling; identify
existing tests; identify existing CLI/MCP behavior; preserve compatible
behavior unless this phase explicitly changes it. Never implement based
on assumptions.

### 2. Real browser only

Production browser paths must never use fake CDP responses, mocked
browser state, simulated DOM, fake WebSocket responses, static JSON
pretending to be CDP, hardcoded element coordinates, hardcoded target
IDs, hardcoded backend node IDs, or hardcoded session IDs. Tests must
execute against a real Chromium/Brave process. Local HTML fixtures are
allowed and REQUIRED for deterministic testing — real webpages loaded
into the real browser, never mocks.

### 3. One persistent browser-level WebSocket

Exactly ONE persistent WebSocket connection per BrowserRuntime daemon,
made to the browser-level endpoint from `/json/version`. Never connect
to a page WebSocket URL. Never create a WebSocket in an action function.
The only production source location allowed to call `connect_async` is
`src/browser_runtime/connection.rs`. If you find another `connect_async`
in production browser code: **STOP.**

### 4. Multi-target session multiplexing

Multiple tabs/pages MUST share the same browser-level WebSocket via CDP
flat session mode; target-specific requests must carry `sessionId`. The
runtime must never open another WebSocket when switching tabs, opening a
popup, attaching to another target, navigating, resolving an iframe,
entering a shadow DOM, or executing actions.

### 5. Never use fixed sleep for browser synchronization

Forbidden as a substitute for knowing an operation completed:
`std::thread::sleep(...)`, `tokio::time::sleep(...)`. Allowed: bounded
timeout, event-driven synchronization, condition polling against REAL
browser state when no suitable event exists. Never `sleep 400ms` then
assume the page loaded.

### 6. Events are authoritative

Prefer, in order: CDP events, DOM state, Accessibility state, frame
state, target state, runtime state — over screenshots, vision, LLM
interpretation. Screenshots are last-resort only for canvas, WebGL, PDF
viewer, visual-only controls. Never the default element resolver.

### 7. No ambiguous actions

Never guess between multiple elements. Equally-valid candidates ⇒
`AmbiguousElement { candidates: [...] }`. Never click the first
candidate.

### 8. Actions require explicit results

Never silently continue after resolution failure, stale element,
ambiguous element, target disappearance, browser crash, session loss,
contradicted verification, or unknown action outcome.

### 9. Verification semantics

`Verified`, `Contradicted`, `Inconclusive`. An action without an
observable DOM change is not automatically a failure — e.g. an
analytics-only click click can legitimately be `Inconclusive` and must
NOT stop the plan. A mismatched read-back value (`"hello"` expected,
`"hell"` actual) is `Contradicted`.

### 10. Interactability is different from verification

`disabled`/`detached`/`non-interactable`/`readonly` (where relevant)
must produce an explicit **pre-action** failure. Never attempt the
action and call it a verification failure after the fact.

### 11. Action idempotency

Every state-changing action gets a unique `action_id`. If CDP dispatch
succeeds but the connection dies before the result reaches the client,
mark `Unknown` — do not auto-replay. Replaying click/submit/purchase/
delete/send could duplicate the real-world effect.

### 12. DOM state must be versioned

Separate generations: `runtime_generation`, `navigation_generation`,
`dom_version`, `element_index_version`, and **[V3]** `frame_tree_version`
(see §4 Identity Model). Never overload one counter for all invalidation
reasons.

### 13. Stale elements must be explicit

`ElementRef` carries the state versions it was created against. If any
have advanced, return `StaleElementRef` rather than silently resolving
an unrelated node. **[V3]** Full rule in §7 Snapshot Consistency
Contract.

### 14. State-changing actions are serialized

Per target: navigate/click/type/fill/submit/select/scroll(state-
sensitive) are serialized. Read-only ops may run concurrently. No single
global lock across independent tabs. **[V3]** Full concurrency model in
§6.

### 15. Resource cleanup is mandatory

On shutdown: stop accepting requests, cancel writer, cancel reader,
resolve pending CDP requests, unsubscribe listeners, detach sessions,
close WebSocket, remove Unix socket, release resources, clean up browser
process if hyprfast launched it. Never leave callers hanging.

### 16. Crash recovery must be state-aware

A CDP disconnect ≠ a normal request timeout. Track connection, target,
navigation, and DOM generations. Never reuse stale node references after
recovery.

### 17. No blind retries

Retry only operations known to be safe. Never auto-retry an unknown
state-changing action.

### 18. Do not use the LLM for local deterministic problems

Resolve locally: CSS selectors, IDs, names, refs, AX roles, accessible
names, semantic text, DOM relationships, stale refs, frame ownership,
shadow roots. Escalate to planning/vision only when deterministic
resolution is genuinely exhausted.

### 19. Observability is part of the feature

Every action traceable: `action_id`, `target_id`, `session_id`,
`step_index`, `operation`, timestamps, latency, dom versions before/
after, runtime generation, resolution method, verification result,
outcome.

### 20. No claims without execution

Never say "implemented/tested/passed/verified/faster" unless the command
actually ran and real output is attached. If not run: say `NOT RUN`.

### 21. Phase gating

One phase at a time. After completing a phase: build, clippy, unit
tests, integration tests, real Chromium/Brave test, git diff, files
changed, commands, real output, remaining issues. **STOP.** Wait for
explicit instruction before the next phase.

### 22. Non-test-profile safety gate

Any state-changing action against a target whose `user_data_dir` is not
a known ephemeral test profile must be journaled with
`production_profile: true`. Logging/observability requirement now; a
confirmation gate can be layered on later without re-architecting the
journal.

### 23. Unix socket permissions

`$XDG_RUNTIME_DIR/hyprfast-browser.sock` must be created `0600`,
explicitly (not relying on umask/XDG defaults). Verify the containing
directory is owned by the current user on bind.

### 24. Crash-restart ownership policy defaults to OFF

Config key `browser_runtime.restart_on_crash: bool = false`. When
`false`: transition to `Disconnected`, do not auto-restart. When `true`:
restart only a browser hyprfast itself launched (never one it merely
attached to), using identical launch parameters.

### 25. Transport backpressure / large payload handling

Full AX trees, flattened documents, and screenshots can be multi-
megabyte CDP payloads. The reader must not block on large-message
decode (offload above a configurable threshold, default 2MB). `call()`
needs a per-call timeout override for known-large ops. Enforce and fail
cleanly (`InvalidResponse`, never panic) on a max accepted message size.
**[V3]** Offloading decode must not reorder state application — see
rule 28 and §5.

### 26. **[V3]** Authoritative state ownership

`BrowserRuntime` owns all authoritative runtime state and is the only
component permitted to *mutate* connection, target, frame, DOM, AX,
element-index, and generation state. `TargetManager`, `DomState`,
`FrameManager`, and `ElementIndex` are state **modules owned by**
`BrowserRuntime` — not independent authorities with their own copies.
CLI/MCP/action/executor code may request mutations only through
`BrowserRuntime`'s own APIs, never by writing to a manager's internals
directly. If you find a manager or executor holding a state field that
`BrowserRuntime` doesn't also own a reference to: **STOP**, that's a
divergence bug waiting to happen.

### 27. **[V3]** No hidden browser calls

Only `BrowserRuntime`'s CDP transport layer (`connection.rs`) may
communicate with Chromium. `TargetManager`, `DomState`, `FrameManager`,
`ElementIndex`, the resolver, `ActionExecutor`, CLI, MCP, and Stagehand
adapters must never create their own WebSocket, CDP client, browser
connection, or independent browser-state cache. This is rule 3
restated as a whole-system constraint, not just a transport-layer one —
grep for it across every new module added in every phase, not just
Phase 1.

### 28. **[V3]** Transport sequencing survives offloaded decode

A WebSocket frame receives its monotonically increasing transport
`sequence` the moment it is read off the socket — before any decoding,
offloaded or not. Offloaded decode workers (rule 25) may finish out of
order. The dispatcher that applies results to pending requests and
broadcasts events to `EventDispatcher` MUST reassemble decoded frames
into transport-sequence order before dispatch/state-mutation — a small
in-order reorder buffer keyed by `sequence`, draining only when the next
expected sequence is ready. State mutation order must equal wire
arrival order, always, even when a large payload was decoded on a
side task.

### 29. **[V3]** Concurrency vs. serialization, explicitly

- **CDP transport**: concurrent requests allowed, across and within
  targets — this is the entire point of session multiplexing.
- **Action executor**: state-changing operations serialized **per
  target only**; independent targets never block each other.
- **Read-only CDP calls**: concurrent wherever safe.
- **State mutation**: applied through the single state owner
  (`BrowserRuntime`, rule 26) in transport sequence order (rule 28) —
  effectively single-threaded from the state's point of view, regardless
  of how many concurrent CDP calls are in flight.

Do not put a mutex around `BrowserRuntime::call()` itself to achieve
serialization — that defeats multiplexing and conflates transport
concurrency with action ordering, which are different concerns.

### 30. **[V3]** Cancellation semantics

Distinguish: request cancellation, action cancellation, CDP request
timeout, browser disconnect, action completion, action unknown.

Rule: client/request cancellation (e.g. MCP caller disconnects) MUST NOT
automatically cancel an already-**dispatched** state-changing browser
action. Once CDP dispatch begins, the action runs to a terminal state
(`Completed`/`Failed`/`Contradicted`/`Inconclusive`) or `Unknown` — never
silently to `Cancelled`. Cancellation may only stop **pre-dispatch**
work: element resolution, precondition checks, queued-but-not-dispatched
steps in a plan.

### 31. **[V3]** Timeout/deadline semantics, by category

Do not use one giant timeout for an entire action. Define separate
deadlines: CDP transport call timeout, element resolution timeout,
action dispatch timeout, verification timeout, and an overall
request/plan-step deadline that composes the others (not one flat
number reused everywhere). Critically: for a **state-changing**
operation, a **dispatch-timeout** is not the same as `Failed` — CDP may
have received and be executing the command even though the response
didn't arrive in time. Treat dispatch timeout on a state-changing action
as `Unknown`, subject to the same no-auto-replay rule as rule 11.
Read-only operation timeouts may be treated as ordinary `Failed`.

### 32. **[V3]** Capability-oriented request model

Design the internal action/request model so sensitive operation classes
are explicit capabilities, not implicit by method name:
`runtime_evaluate`, `cookies`, `clipboard`, `file_upload`, `download`,
`navigation`. You do not need a full permission-enforcement system in
v1 — but every internal request should be tagged with which capability
class it belongs to, so a permission layer can be added later without
re-plumbing every call site. Record the capability class in the action
journal alongside `production_profile` (rule 22).

### 33. **[V3]** IPC protocol versioning

The Unix socket is a real API surface, not a debug pipe. The first
message on a new client connection must be a handshake:

```
HandshakeRequest  { protocol_version, client_version, requested_capabilities }
HandshakeResponse { protocol_version, runtime_version, capabilities, connection_id }
```

Reject with a structured `ProtocolMismatch` error (not a hang, not a
generic error) if `protocol_version` is incompatible. This is what
prevents a stale CLI binary from sending a daemon requests it can't
parse and getting undefined behavior instead of a clear error.

### 34. **[V3]** Flat-session mode is a hard prerequisite

If `Target.setAutoAttach { flatten: true }` fails, or the runtime cannot
establish the required target/session model at startup:

```
return UnsupportedBrowserProtocol
```

Do NOT fall back to page-level WebSockets. Do NOT silently downgrade to
legacy (non-flat) target handling. Do NOT partially initialize
`BrowserRuntime` in a degraded-but-running state for this specific
failure — it must fail closed, not open, because every other invariant
in this document assumes flat-session multiplexing holds.

---

## 1. Runtime lifecycle state machine **[V3 — formalized]**

```
Starting → Connecting → Connected → Degraded → Disconnected
                                        ↑            │
                                        └─ Reconnecting  (only if configured
                                              │            reconnect is on;
                                              ▼            see rule 24 for the
                                          Connected        browser-process-
                                                            restart case
                                                            specifically —
                                                            this is the CDP-
                                                            connection-level
                                                            reconnect, a
                                                            narrower thing)

Any state → Stopping → Stopped
```

Every state defines exactly what's permitted. An agent must implement
this as an explicit check at the top of every runtime API, not as an
implicit consequence of connection liveness:

| State          | New action calls | Reads (status/diagnostics) | State writes |
|----------------|-------------------|------------------------------|--------------|
| Starting       | reject (`NotReady`) | limited (state only)       | no |
| Connecting     | reject (`NotReady`) | limited (state only)       | no |
| Connected      | yes                | yes                          | yes |
| Degraded       | restricted — read-only actions only, no new state-changing dispatch | yes | no |
| Reconnecting   | reject (`Reconnecting`) | yes (shows generation in progress) | no |
| Disconnected   | reject (`RuntimeDead`) | yes (last-known state)     | no |
| Stopping       | reject (`ShuttingDown`) | limited | no |
| Stopped        | reject (`RuntimeDead`) | no  | no |

`Degraded` is the state used when the connection is alive but something
narrower is broken (e.g. a single target crashed, or `Target.getTargets`
is behaving unexpectedly) — it must not accept new state-changing
dispatch but should still serve reads so the caller can decide what to
do.

---

## 2. Action state machine **[V3 — expanded]**

v2 had `Accepted → Executing → Completed | Failed | Unknown`. That
collapses too many distinct failure points into one `Executing` bucket —
"CDP click call returned an error" and "CDP click succeeded but the
connection died before the response arrived" need to be distinguishable
at the state-machine level, not just in a text field.

```
Accepted
   │
   ▼
Resolving              (element resolution in progress — cancellable, rule 30)
   │
   ▼
Resolved                (or: StaleElementRef / AmbiguousElement / ResolutionFailed → terminal)
   │
   ▼
PreconditionCheck       (interactability — rule 10)
   │
   ▼
Dispatching             (CDP call in flight — NOT cancellable, rule 30)
   │
   ▼
Dispatched               (CDP call acknowledged, or connection died → Unknown, rule 31)
   │
   ▼
Verifying
   │
   ▼
Completed
   │
   ├── Failed            (precondition or resolution failure)
   ├── Contradicted       (verification actively disagrees with expectation)
   ├── Inconclusive        (no observable signal, plan continues)
   └── Unknown             (dispatched, outcome undetermined — never auto-replayed)
```

Only `Resolving` and everything before `Dispatching` begins is
cancellable per rule 30. Once `Dispatching` starts, the action always
reaches a terminal state on its own terms.

---

## 3. Identity model **[V3 — new]**

`target_id` and `session_id` are not eternal identities — Chromium can
in principle reuse an identifier across a crash/restart, and treating a
bare ID as identity risks silently accepting stale state as current.

```
TargetRef  { target_id,  target_generation }
SessionRef { session_id, connection_generation }
```

`target_generation` increments whenever `TargetManager` rediscovers
targets after a reconnect (Phase 11). `connection_generation` increments
whenever the browser-level WebSocket is re-established. A `TargetRef` or
`SessionRef` whose generation doesn't match the runtime's current
generation must be treated as fully invalid, regardless of whether the
raw ID string happens to match something currently live.

**[V3]** Also add `frame_tree_version` (increments on
`Page.frameAttached`/`frameNavigated`/`frameDetached` for any frame, not
just the main frame — iframe navigation/detachment can invalidate frame
assumptions without a full document replacement). `ElementRef` becomes:

```
ElementRef {
    id, backend_node_id, node_id, target_id, target_generation,
    frame_id, frame_tree_version, role, name, tag_name, dom_id, classes,
    selector, text_content, bounding_box, visible, enabled,
    dom_version_created,
}
```

---

## 4. Ordering and event-application model **[V3 — new, ties rules 28/29 together]**

```
WebSocket reader
      │  sequence assigned here, immediately, on every frame
      ▼
Frame sequence assignment
      │
      ▼
Decode (inline for small frames; offloaded task for large frames,
        per rule 25's threshold)
      │
      ▼
Ordered reorder buffer (keyed by sequence, drains only in order)
      │
      ▼
Dispatcher: resolve pending request OR broadcast CdpEvent
      │
      ▼
State mutation (single owner — BrowserRuntime, rule 26)
```

This is what lets Phase 1's backpressure fix (offload large-payload
decode) coexist with Phase 4's "events are applied in the order they
happened" requirement without a race.

---

## 5. Timeout/deadline composition **[V3 — new, formalizes rule 31]**

```
Plan-step deadline (overall, caller-facing)
 ├── resolution deadline     (Resolving state)
 ├── dispatch deadline       (Dispatching state — timeout here ⇒ Unknown, not Failed, for state-changing ops)
 └── verification deadline   (Verifying state)
```

Each sub-deadline is independently configurable; the plan-step deadline
is not simply "sum them and hope" — define it as a hard ceiling that,
if hit, aborts whichever sub-phase is active using that sub-phase's own
failure semantics (so a plan-step deadline hit during `Dispatching`
still produces `Unknown` for a state-changing action, never a bare
`Failed`).

---

## 6. Snapshot consistency contract **[V3 — new, one of the most important additions]**

```
snapshot DOM → resolve button → [page mutates] → dispatch click
```

Rule: every resolver result carries the exact state versions
(`dom_version`, `frame_tree_version`, `target_generation`) it was
resolved against. Immediately before dispatch — not at resolution time,
**at dispatch time** — the executor re-checks those versions against
current runtime state. If any have advanced: return `StaleElementRef`.
Re-resolution is only ever a **new, separate resolution operation**
(back to the `Resolving` state) — never a silent substitution of a
different node that happens to match the same selector. This closes the
TOCTOU gap between "we found the button" and "we clicked something."

---

## 7. Architectural invariants **[V3 — new]**

Mechanically checkable definition of correctness. Every phase's DoD
should be checked against the invariants it touches; Phase 16 checks all
of them.

```
I1.  Exactly one browser-level WebSocket exists per BrowserRuntime.
I2.  No production code outside connection.rs creates a WebSocket.
I3.  No component other than BrowserRuntime's state owner mutates
     authoritative browser state. (rule 26)
I4.  All target-specific CDP requests use sessionId.
I5.  No state-changing action executes concurrently with another
     state-changing action on the same target.
I6.  Read-only CDP calls may execute concurrently.
I7.  No ElementRef can be used after its referenced state generation
     becomes invalid. (§6 Snapshot Consistency)
I8.  No state-changing action is automatically replayed after an
     uncertain (Unknown) outcome.
I9.  Browser events are applied in transport order, even when decode
     was offloaded. (§4)
I10. Client cancellation cannot silently convert an already-dispatched
     state-changing action into a cancelled/failed action. (rule 30)
I11. A disconnected runtime cannot execute browser actions. (§1 table)
I12. A browser target/session from a previous connection generation can
     never be reused after recovery. (§3 Identity Model)
I13. No browser state may be represented by a fake/mock object in
     production.
I14. Every externally visible action has a journal entry.
I15. Every action has an explicit terminal state. (§2)
I16. Verification failure and execution/precondition failure are
     distinct.
I17. Inconclusive verification is not equivalent to contradiction.
I18. Ambiguous resolution never selects an arbitrary candidate.
I19. Production-profile state-changing actions are explicitly tagged.
I20. Runtime IPC socket is owner-only (0600).
I21. Oversized CDP messages fail cleanly without a process panic.
I22. Large-message decoding cannot starve latency-sensitive events, and
     cannot reorder state application relative to wire arrival. (§4)
I23. Browser restart is disabled unless explicitly configured, and never
     restarts a browser hyprfast only attached to.
I24. The daemon never silently creates a second browser connection.
I25. Flat-session initialization failure fails the runtime closed
     (UnsupportedBrowserProtocol), never partially-initialized. (rule 34)
I26. No command is processed on an IPC connection before a successful
     protocol handshake. (rule 33)
```

---

## 8. Final architecture

```
                    ┌─────────────────────────┐
                    │      CLI / MCP          │
                    │ browser commands/tools  │
                    └────────────┬────────────┘
                                 │ Unix socket (0600, owner-only,
                                 │ handshake: protocol_version + capabilities)
                                 ▼
                 ┌───────────────────────────────┐
                 │ BrowserRuntimeServer           │
                 │ lifecycle state machine (§1)   │
                 │ request router                 │
                 │ action queues (per-target)      │
                 │ action journal (capability +    │
                 │   production_profile tagged)    │
                 │ restart_on_crash policy (off)   │
                 └───────────────┬────────────────┘
                                 ▼
                 ┌───────────────────────────────┐
                 │       BrowserRuntime           │
                 │ SOLE authoritative state owner │
                 │ ONE browser-level WebSocket    │
                 │ CDP request router (concurrent)│
                 │ pending request map             │
                 │ sequence → reorder → dispatch    │
                 │   (§4 ordering model)            │
                 └───────────────┬────────────────┘
                                 │ ONE WebSocket
              ┌──────────────────┼──────────────────┐
              ▼                  ▼                  ▼
      Target A (TargetRef)  Target B            Target C
      session-1 (SessionRef) session-2           session-3
              │                  │                  │
              ▼                  ▼                  ▼
          Frame tree          Frame tree          Frame tree
        (frame_tree_version) (frame_tree_version)(frame_tree_version)
              │                  │                  │
              └──────────────────┼──────────────────┘
                                 ▼
                    ┌────────────▼────────────┐
                    │   Event Dispatcher      │
                    │  (ordered per §4)        │
                    └────────────┬────────────┘
          ┌──────────────────────┼────────────────────────┐
          ▼                      ▼                        ▼
     TargetManager*         DomState*                FrameManager*
      *owned modules, not independent authorities (rule 26)
          └──────────────────────┼────────────────────────┘
                                 ▼
                       ElementIndex
             ┌───────────────────┼───────────────────┐
             ▼                   ▼                   ▼
          AX tree              DOM tree           Shadow DOM
             └───────────────────┼───────────────────┘
                                 ▼
              Element Resolution (carries versions, §6)
                                 │
                                 ▼
             Action Executor — state machine per §2,
             serialized per-target (rule 29), dispatch not
             cancellable (rule 30), category-specific
             timeouts (rule 31/§5)
                                 │
                                 ▼
              Snapshot re-check before dispatch (§6)
                                 │
                                 ▼
                   Post-Action Verification
                (tags production_profile + capability)
                                 │
                                 ▼
                         Action Journal
                                 │
                                 ▼
                 Task Persistence (src/task.rs)
```

---

## PHASE 0A — Repository audit **[V3 — split from v2's Phase 0]**

**Goal:** understand the existing system, read-only.

**Agent prompt:**

```
Perform a complete read-only audit of the existing hyprfast repository.

Read completely:
src/cdp/, src/browser/, src/stagehand/ (all submodules: act.rs, batch.rs,
clipboard.rs, context.rs, cookies.rs, file_upload.rs, locator.rs,
page.rs, snapshot.rs, webmcp.rs, a11y/xpath.rs, a11y/sessions.rs),
src/task.rs, src/daemon.rs, src/main.rs

Search the entire repository for: connect_async, get_ws_url, cdp_call,
cdp_call_async, DOM.getDocument, DOM.describeNode, DOM.resolveNode,
DOM.querySelector, DOM.getBoxModel, Accessibility.getFullAXTree,
Runtime.evaluate, Runtime.callFunctionOn, Page.navigate,
Page.lifecycleEvent, Target.targetCreated, Target.targetDestroyed,
Target.attachedToTarget, Target.detachedFromTarget, screenshot,
std::thread::sleep, tokio::time::sleep

Also report:
- the exact set of MCP tool names beginning with browser_ or
  stagehand_ (not desktop/hypr/a11y/task tools — Phase 15 audits
  against this narrower list)
- any existing $XDG_RUNTIME_DIR socket file creation and its current
  permission bits
- any existing browser launch code and its profile/user-data-dir
  strategy today (needed for rule 22 and Phase 11's restart policy)
- whether any code today assumes target_id/session_id are stable across
  a browser restart (needed to validate the identity model in this
  plan's §3 against real current behavior, not just theory)

Produce an architecture table: file, function, responsibility, callers,
browser communication mechanism, target selection mechanism, connection
lifecycle, performance problems, correctness problems, security
problems, reusable implementation, migration destination.

Enumerate every existing browser CLI command, MCP browser tool, browser
action, stagehand operation, CDP method, target discovery mechanism, and
DOM/AX snapshot mechanism.

Do not modify production source. Do not create fixtures yet — that is
Phase 0B.
```

**DoD:** `git status --short` shows no changes at all. `cargo build
--release` and `cargo clippy --all-targets --all-features -- -D
warnings` both pass unchanged (proving nothing was touched).

---

## PHASE 0B — Fixture creation + fixture test harness **[V3 — split from v2's Phase 0]**

**Goal:** deterministic real-HTML fixtures the rest of the plan runs
against, plus a minimal harness to load and serve them.

**Agent prompt:**

```
Create tests/browser_fixtures/ with real HTML/JS fixtures:
basic.html, iframe.html, shadow_dom.html, navigation.html,
disabled_button.html, ambiguous_buttons.html, mutation.html, popup.html,
search_form.html, analytics_button.html, broken_login.html, instant.html,
contenteditable.html, select.html, crash_test.html

These are real webpages, loaded into the real browser in later phases —
never mock CDP responses. Document, per fixture, exactly which later
phase/DoD exercises it and why it exists (this becomes the map Phase 16
uses to confirm coverage).

Add a minimal local-serving harness (e.g. a tiny static file server
bound to localhost, or documented file:// usage where a real HTTP
navigation isn't required) so navigation timing/lifecycle tests in later
phases have a real, controllable network target instead of only
file:// URLs — file:// alone cannot exercise Page.lifecycleEvent timing
the way a deliberately-delayed local HTTP response can.
```

**DoD:** `git status --short` shows only `tests/browser_fixtures/...`
and the harness addition. `cargo build --release` passes.

---

## PHASE 0C — Architecture / migration report **[V3 — split from v2's Phase 0]**

**Goal:** turn 0A's audit into an actionable migration map before any
production code changes.

**Agent prompt:**

```
Using the 0A audit and 0B fixtures, produce a migration report mapping:
- every existing browser/stagehand call site → its target
  browser_runtime/ module (per the directory layout in this plan)
- every known correctness bug found in 0A (e.g. :contains() CSS
  generation, string-concatenated JS, blind sleeps) → the specific
  phase that will fix it (per this plan, most land in Phase 6 or 9) —
  this is the list Phase 3 will explicitly defer against, so it must be
  precise, not vague
- every existing MCP tool → confirmation it will be preserved, and by
  which phase

No source changes in this phase — output is documentation only, saved
under docs/browser_runtime_migration.md or equivalent.
```

**DoD:** `git status --short` shows only the new doc file. Agent's phase
report explicitly lists the deferred-bug list that Phase 3 will need.

---

## PHASE 1 — Browser-level persistent CDP transport

**Goal:** the transport, correct from day one — including backpressure
(rule 25), strict ordering under offloaded decode (rule 28, §4), and a
hard failure mode if flat-session isn't available (rule 34).

**Agent prompt:**

```
Create:
src/browser_runtime/mod.rs
src/browser_runtime/connection.rs
src/browser_runtime/error.rs

BrowserRuntime must connect ONLY to the browser-level WebSocket obtained
from /json/version. Do not connect to a page target WebSocket.

Define:
CdpRequest { id: i64, method: String, params: Value, session_id: Option<String> }
CdpResponse { id: i64, result: Option<Value>, error: Option<CdpError>, session_id: Option<String> }
CdpEvent { method: String, params: Value, session_id: Option<String>, sequence: u64, timestamp: Instant }

Use exactly one WebSocketStream. Split it into a writer task (owns
SplitSink) and a reader task (owns SplitStream).

ORDERING PIPELINE (rule 28, this plan's §4) — implement exactly this
shape, not an ad hoc variant:
  reader reads raw frame → assign `sequence` immediately (before any
  decode) → decode inline for frames under the size threshold, or hand
  off to a decode worker for frames over it (rule 25, default 2MB
  threshold) → all decoded frames (inline or offloaded) pass through an
  in-order reorder buffer keyed by `sequence`, which only drains the
  next expected sequence number → the drained, now-strictly-ordered
  stream is what resolves pending requests and broadcasts CdpEvents.
  This guarantees state mutation order equals wire arrival order even
  when a large payload's decode ran on a side task and finished later
  than a smaller frame that arrived after it.

Maintain Arc<Mutex<HashMap<i64, oneshot::Sender<CdpResponse>>>> and an
AtomicBool for connection liveness. When the stream closes: set
alive=false, resolve every pending request as RuntimeDead, terminate
reader, ensure future calls immediately return RuntimeDead.

Implement:
BrowserRuntime::connect(debugger_ws_url: &str)
BrowserRuntime::call(session_id, method, params)
BrowserRuntime::call_with_timeout(session_id, method, params, timeout)
BrowserRuntime::subscribe()
BrowserRuntime::is_alive()

Concurrency (rule 29): call() must support genuinely concurrent
in-flight requests across and within targets — do not serialize at this
layer. Serialization of state-changing actions happens in the executor
(Phase 8), per target, not here.

Add BrowserRuntimeDiagnostics: connection_id, browser_info,
pending_request_count, event_count, connected_at, attached_session_ids,
reorder_buffer_depth (current + max observed — needed to validate the
ordering pipeline is actually keeping up).

On startup, call Browser.getVersion and store product, revision,
protocol_version.

FLAT-SESSION HARD PREREQUISITE (rule 34): before returning the runtime,
issue Target.setAutoAttach { autoAttach: true, waitForDebuggerOnStart:
false, flatten: true }. If this fails, or the runtime cannot establish
the required target/session model, return UnsupportedBrowserProtocol and
abort initialization entirely. Do NOT fall back to page-level
WebSockets, do NOT downgrade to legacy target handling, do NOT return a
partially-initialized BrowserRuntime. On success, query Target.getTargets
and attach relevant page targets via Target.attachToTarget in flatten
mode. Do not open another WebSocket. Every target-specific request must
carry the correct sessionId.

Errors must distinguish: ConnectionFailed, RuntimeDead, Timeout,
CdpError, InvalidResponse, UnsupportedBrowserProtocol. Do not use
generic anyhow errors where structured runtime errors are required.
```

**DoD**

Run real Brave/Chromium:

```
brave --headless=new \
  --remote-debugging-port=9222 \
  --user-data-dir=/tmp/hyprfast-runtime-test \
  about:blank &
```

Transport test: 2 real targets, 2 real session IDs, 100 real CDP
requests distributed across both targets, one WebSocket, zero failures.

Ordering test: fire a large `Accessibility.getFullAXTree` call
immediately followed by several small `Runtime.evaluate` calls on a
different target; confirm via `reorder_buffer_depth` diagnostics and
event/result timestamps that the small calls' *results* are not
delayed behind the large one's decode, AND that any events emitted
during that window are still delivered in true wire-sequence order (not
reordered by the offload).

```
ss -tnp | grep 9222 | wc -l
```

Expected: `1`. Kill test: `kill -9 <browser_pid>` — every in-flight
request resolves to `RuntimeDead`, never hangs, never resolves
`Timeout` instead.

`UnsupportedBrowserProtocol` test: simulate/force a `setAutoAttach`
failure path (e.g. against a CDP-incompatible stub endpoint used only
for this negative test, not swapped in for the real transport) and
confirm the runtime refuses to return a usable instance.

---

## PHASE 2 — BrowserRuntime daemon + lifecycle state machine + IPC protocol

**Goal:** persistence across CLI invocations, socket locked down (rule
23), and now a versioned handshake (rule 33) implementing the full
lifecycle table from §1.

**Agent prompt:**

```
Read src/daemon.rs completely before implementing this phase.

Create:
src/browser_runtime/server.rs
src/browser_runtime/client.rs
src/browser_runtime/state.rs

Implement BrowserRuntimeServer and BrowserRuntimeClient using
$XDG_RUNTIME_DIR/hyprfast-browser.sock, created 0600 explicitly, with
directory-ownership verified on bind (rule 23) — add a test asserting
the mode bits after bind.

Implement the full lifecycle state machine from this plan's §1:
Starting, Connecting, Connected, Degraded, Reconnecting, Disconnected,
Stopping, Stopped, Failed(String). Every transition explicit. Enforce
the per-state permission table from §1 at the top of every server-side
handler — reject with a structured, state-specific error
(NotReady/Reconnecting/RuntimeDead/ShuttingDown), not a generic error.

IPC PROTOCOL HANDSHAKE (rule 33): the first message on every new client
connection must be:
HandshakeRequest  { protocol_version, client_version, requested_capabilities }
HandshakeResponse { protocol_version, runtime_version, capabilities, connection_id }
Reject any command received before a successful handshake with a
structured error (invariant I26). Reject incompatible protocol_version
with ProtocolMismatch, not a hang.

CAPABILITY TAGGING (rule 32): define the wire request model so each
command is associated with a capability class (runtime_evaluate,
cookies, clipboard, file_upload, download, navigation, or none for
plain DOM/AX reads). You do not need to enforce permissions on these yet
— just make every request carry its capability class so Phase 8's
journal can record it.

The server owns the single BrowserRuntime. CLI processes must NOT own
independent persistent CDP connections.

Read-only requests may execute concurrently. State-changing operations
must be serialized PER TARGET (rule 29) — not globally.

Implement:
hyprfast browser-runtime start
hyprfast browser-runtime stop
hyprfast browser-runtime status

Status must expose: state, browser product/revision/protocol version,
target count, CDP connection count, pending requests, dom_version,
navigation_generation, runtime_generation, frame_tree_version (max
across targets), event_count, socket_mode (octal), restart_on_crash
(config value — behavior lands in Phase 11), reorder_buffer_depth.

Stale Unix socket handling: if the socket exists, determine whether a
live daemon is actually listening; if alive, refuse startup; if stale,
remove it; then bind. Never blindly unlink a potentially-live socket.

Shutdown: stop accepting requests, stop queues, cancel reader/writer,
resolve pending requests, detach sessions, close WebSocket, remove
socket, clean resources. Do not auto-restart the browser here — that's
Phase 11's default-off policy, not Phase 2's concern.

If daemon is unavailable, browser CLI commands fall back to degraded
direct mode with a warning; the fallback must not become the normal
path.
```

**DoD**

```
hyprfast browser-runtime start
for i in $(seq 1 20); do cargo run --release -- browser eval "1+$i" & done
wait
hyprfast browser-runtime status
```

Expected: `state=Connected`, `cdp_connections=1`, `socket_mode=0600`.
Concurrency test proves same-target mutations are ordered, different-
target mutations are not blocked by each other.

Handshake test: connect with a deliberately mismatched
`protocol_version` and confirm a clean `ProtocolMismatch`, not a hang;
confirm a command sent before handshake completes is rejected
(invariant I26).

```
hyprfast browser-runtime stop
test ! -S "$XDG_RUNTIME_DIR/hyprfast-browser.sock"
```

Also test: stale socket, live socket, simultaneous daemon startup,
client disconnect mid-request (see Phase 8 for what this must and must
not do to an already-dispatched action — this phase just needs the
connection-level disconnect to not corrupt server state), daemon
termination, wrong socket permissions rejected/corrected.

---

## PHASE 3 — Migrate existing browser callers

**Goal:** remove per-operation WebSocket creation. **Transport-only —
do not fix resolution bugs here.** Use the exact deferred-bug list
produced in Phase 0C.

**Agent prompt:**

```
Migrate every real browser call site to BrowserRuntimeClient, using the
Phase 0C migration report as the source of truth for what moves where.

Start with src/browser/mod.rs, then act.rs, batch.rs, clipboard.rs,
context.rs, cookies.rs, file_upload.rs, locator.rs, page.rs, snapshot.rs,
webmcp.rs, a11y/xpath.rs.

Preserve functional behavior. Do not rewrite unrelated logic.

SCOPE BOUNDARY: this phase moves how connections are acquired, not how
elements are resolved. Known resolution bugs from Phase 0C (e.g. invalid
:contains() CSS, string-concatenated JS) must NOT be fixed here even
though you'll see them while migrating — cross-check each one you
encounter against the Phase 0C list and confirm it's accounted for
there; do not silently fix it now.

Tag every migrated call site's outgoing request with its capability
class from Phase 2's model (rule 32) as part of the migration, since
that's mechanical and belongs with "how the request is sent," not with
resolution logic.

For each migration: identify existing CDP behavior, replace connection
acquisition, route through BrowserRuntimeClient, preserve method
parameters/errors, run the associated real browser command, compare
behavior.

Only after every caller is migrated may the old per-call connection path
be removed. No connect_async may remain outside
src/browser_runtime/connection.rs.
```

**DoD**

```
grep -rn "connect_async" src/ | grep -v "browser_runtime/connection.rs"
```

Expected: no output.

```
cargo run --release -- browser open https://example.com
cargo run --release -- browser snapshot
cargo run --release -- browser eval "document.title"
```

Verify one persistent connection. Phase report explicitly reconciles
every resolution bug it encountered against the Phase 0C deferred list —
flag any bug found that Phase 0C missed.

---

## PHASE 4 — Event dispatcher + live DOM state

**Goal:** stop repeatedly reconstructing browser state; apply events
through the single state owner (rule 26) in strict order (§4/rule 28).

**Agent prompt:**

```
Create:
src/browser_runtime/events.rs
src/browser_runtime/dom_state.rs

All CDP events must flow through ONE EventDispatcher, itself downstream
of Phase 1's ordering pipeline — do not re-sort or re-buffer events
here; trust the sequence ordering Phase 1 already guarantees, and treat
any out-of-order arrival at this layer as a bug in Phase 1, not
something to work around locally.

Feature modules must not independently consume raw WebSocket messages
(rule 27) — DomState, TargetManager, FrameManager are modules mutated
only via BrowserRuntime's own state-owner API (rule 26), never
independently.

CdpEvent: method, params, session_id, sequence, timestamp.

Implement DomState: runtime_generation, navigation_generation,
dom_version, element_index_version (all AtomicU64), ready_state
(RwLock<String>), current lifecycle state, affected parent node IDs.
frame_tree_version lives on FrameManager (Phase 5) but DomState must
expose read access to it for resolver use (Phase 6).

On DOM.documentUpdated: full invalidation, increment dom_version,
invalidate ElementRefs.

On DOM.childNodeInserted/Removed/attributeModified: increment
dom_version, record affected parentNodeId.

On Page.frameNavigated: update navigation_generation for the main
frame.

On Page.lifecycleEvent: maintain current lifecycle state.

On connect and reconnect: Page.enable, DOM.enable, Runtime.enable per
attached session.

Implement wait_for_lifecycle(): check → subscribe → re-check → await
event — never subscribe-and-blindly-wait. Replace the blind ~400ms
navigation sleep with this.

NOTE for Phase 9: this waiter gets generalized into the shared condition
engine there — write its check/subscribe/re-check/await primitive so
it's trivially liftable, not duplicated.
```

**DoD:** unchanged from v2 — instant fixture × 50 runs, no missed
lifecycle event; delayed-navigation timing against Phase 0B's HTTP
harness proves completion is event-driven, not timed.

---

## PHASE 5 — Target and tab manager

**Goal:** tabs as first-class runtime objects, using `TargetRef`
generations from §3 Identity Model.

**Agent prompt:**

```
Create src/browser_runtime/targets.rs.

Implement BrowserTargetManager as a module owned by BrowserRuntime
(rule 26) — it may read the runtime's current target_generation but
increments it only by calling back into BrowserRuntime's state-owner
API, never bumping its own separate counter.

TargetRecord {
    target_id, target_generation, session_id, connection_generation,
    url, title, target_type, active, lifecycle, created_at, crashed,
}

Consume: Target.targetCreated, Target.targetDestroyed,
Target.targetCrashed, Target.attachedToTarget, Target.detachedFromTarget,
Page.frameNavigated.

Implement: list_targets(), active_target(), switch_target(),
attach_target(), detach_target(). Switching target only changes active
session state — NEVER creates a WebSocket.

Popup discovery must be event-driven, not /json polling.
Target.setAutoAttach from Phase 1 must be used correctly.

Handle: existing targets, new popup, popup destruction, crashed target,
detached session, browser restart (i.e. connection_generation bump —
verify every TargetRecord surviving a restart gets the new generation,
none silently keep the old one).
```

**DoD:** unchanged from v2 (popup.html two-target test, one WebSocket
throughout, trace shows targetCreated/attachedToTarget). Add: confirm
`TargetRecord.target_generation` increments correctly across a simulated
reconnect and that a `TargetRef` captured before the reconnect is
rejected by any API that checks it afterward.

---

## PHASE 6 — Unified DOM + AX element index

**Goal:** the deterministic element-resolution engine — and where the
resolution bugs deferred from Phase 3 get fixed, with `frame_tree_version`
included in every ref (§3) and full snapshot-consistency support (§6).

**Agent prompt:**

```
Create:
src/browser_runtime/element_index.rs
src/browser_runtime/frames.rs

Do not build an AX-only element system — merge DOM info (tag, id,
classes, backendNodeId, nodeId, text, attributes, box, visibility,
shadow roots) with AX info (role, accessible name, semantic state).
Plain DOM elements with no meaningful AX role still get ElementRefs.

ElementRef must match this plan's §3 shape exactly:
ElementRef {
    id, backend_node_id, node_id, target_id, target_generation,
    frame_id, frame_tree_version, role, name, tag_name, dom_id, classes,
    selector, text_content, bounding_box, visible, enabled,
    dom_version_created,
}

Track frames via Page.frameAttached/frameNavigated/frameDetached:
frame_id, parent_frame, execution_context_id, and bump
FrameManager's frame_tree_version (owned via BrowserRuntime, rule 26) on
every one of these events for ANY frame, not just the main frame. Never
assume the main frame execution context.

Element resolution order (unchanged from v2): e_NNN ref → backendNodeId
→ DOM.resolveNode → correct frame execution context → cached selector →
DOM.querySelector → id/name → role+accessible name → semantic text →
fresh AX search → ResolutionFailed → AmbiguousElement. Never guess.

Shadow DOM: walk real shadow roots via DOM.describeNode and/or
DOM.getFlattenedDocument(pierce=true). Open shadow roots represented.

FIX THE DEFERRED BUGS FROM PHASE 3/0C HERE: eliminate :contains()
entirely (never generate invalid CSS); eliminate raw
format!("...user input...") JS construction — use DOM.querySelector,
DOM.resolveNode, Runtime.callFunctionOn with structured CDP parameters,
never string-concatenated JS. Cross-check against the exact list Phase
3's report produced.

Implement StaleElementRef, AmbiguousElement, ResolutionFailed,
ElementNotInteractable.
```

**DoD:** unchanged from v2 (iframe, shadow DOM, ambiguity, injection
tests) plus: capture an `ElementRef`, bump `frame_tree_version` via an
iframe navigation elsewhere on the page (not the element's own frame),
and confirm the ref used against its *own* unaffected frame is still
valid — i.e. `frame_tree_version` granularity doesn't over-invalidate
unrelated frames' refs. Then bump it via the element's *own* frame
navigating and confirm the ref does go stale.

---

## PHASE 7 — Structured execution plans

**Goal:** replace stringly-typed browser workflows. Unchanged in
substance from v2.

**Agent prompt:** (same as v2 — `PlanStep` tagged enum, `ExecutionPlan`,
validate syntax without eagerly resolving post-navigation targets, wire
`browser_execute_plan` as an additive MCP tool.)

**DoD:** unchanged from v2 — local search fixture: navigate → type
search → submit → wait_for URL → extract; verify real URL and DOM
state.

---

## PHASE 8 — Executor + verification + action journal

**Goal:** make automation trustworthy — this phase implements the full
§2 action state machine, rule 30 cancellation, rule 31 timeout
categories, §6 snapshot re-check, and rule 22/32 journal tagging.

**Agent prompt:**

```
Create:
src/browser_runtime/executor.rs
src/browser_runtime/action.rs
src/browser_runtime/action_journal.rs

Implement the FULL action state machine from this plan's §2:
Accepted → Resolving → Resolved → PreconditionCheck → Dispatching →
Dispatched → Verifying → Completed | Failed | Contradicted |
Inconclusive | Unknown. Do not collapse this to v2's simpler
Accepted/Executing/Completed — the intermediate states are what let you
correctly implement cancellation and timeout-category semantics below.

CANCELLATION (rule 30): a client/request cancellation may only abort
work in Resolving or earlier. Once the action transitions to
Dispatching, cancellation requests must be rejected/ignored for that
action — it proceeds to a terminal state or Unknown on its own.

TIMEOUTS (rule 31, this plan's §5): apply resolution/dispatch/
verification deadlines independently, not one shared timeout. A
dispatch-deadline hit on a state-changing action's Dispatching state
transitions to Unknown, never Failed. A resolution-deadline hit
transitions to Failed (nothing was dispatched, so Failed is safe here).

SNAPSHOT RE-CHECK (this plan's §6): the ElementRef used for dispatch was
resolved with specific dom_version/frame_tree_version/target_generation
values. Immediately before Dispatching begins, re-verify those values
against current runtime state. If any changed: return StaleElementRef
and require a brand-new Resolving pass — never substitute a
different-but-similar node silently.

Define:
VerificationResult { Verified(Value), Contradicted(String), Inconclusive(String) }
StepOutcome { Ok { verification: VerificationResult }, Failed(String), Skipped }

ActionRecord: action_id, target_id, target_generation, session_id,
step_index, action, capability_class (from Phase 2's model, rule 32),
started_at, completed_at, runtime_generation, dom_version_before/after,
frame_tree_version_before/after, resolution_method, verification,
outcome, production_profile: bool (rule 22 — true when target's
user_data_dir is not a known ephemeral test profile).

Verification: CLICK checks observable expected effects (DOM mutation,
attribute/class change, navigation, dialog, target change) → Contradicted
if an expected effect fails, Inconclusive if no meaningful signal
exists. TYPE/FILL always read back value/textContent, mismatch =
Contradicted. NAVIGATE requires CDP success + lifecycle event +
location.href. SELECT reads back selected values. EXTRACT validates
schema fields.

Only Failed or Contradicted cascades to Skipped for later steps.
Inconclusive continues. A disabled element fails in PreconditionCheck,
before Dispatching ever begins.
```

**DoD**

- **False-cascade:** analytics button → `Inconclusive`; next evaluate
  step still runs.
- **Genuine contradiction:** broken login → click → expected navigation
  absent → `Contradicted` → later steps `Skipped`.
- **Disabled:** click disabled button → `ElementNotInteractable` in
  `PreconditionCheck`, before `Dispatching`.
- **Unknown action:** kill browser after `Dispatching` begins but before
  response → `Unknown`, never auto-replayed.
- **Cancellation:** issue a cancel request while an action is in
  `Resolving` → confirm it stops cleanly; issue a cancel request after
  `Dispatching` has begun → confirm it is rejected/ignored and the
  action still reaches a terminal state or `Unknown` on its own.
- **Timeout category:** force a dispatch-phase timeout (e.g. artificial
  delay injection) on a state-changing action → confirm `Unknown`, not
  `Failed`. Force a resolution-phase timeout → confirm `Failed`.
- **Snapshot staleness:** resolve an element, mutate the DOM before
  dispatch, confirm `StaleElementRef` is returned and no click occurs
  against a substituted node.
- **Profile tagging:** one action against a fixture profile, one against
  a non-fixture profile → confirm `production_profile` is `false`/`true`
  respectively, and `capability_class` is populated correctly for a
  `Runtime.evaluate`-backed action vs. a plain DOM click.

---

## PHASE 9 — Wait/condition engine

**Goal:** reliable synchronization without sleeps, unified with Phase
4's lifecycle waiter (no duplicate waiting primitive in the codebase).

**Agent prompt:** (same conditions list as v2: UrlContains, UrlEquals,
TitleContains, ElementExists, ElementVisible, ElementEnabled,
ElementText, AttributeEquals, DomVersionAtLeast, NavigationComplete,
Lifecycle, TargetExists, TargetDestroyed, DialogAppeared,
ExpressionTrue.)

```
REFACTOR REQUIREMENT (unchanged from v2, still binding): Phase 4's
wait_for_lifecycle() must be reimplemented on top of this engine's
Lifecycle/NavigationComplete conditions — exactly one
check/subscribe/re-check/await primitive must exist codebase-wide by
the end of this phase.

Add a FrameTreeVersionAtLeast condition alongside DomVersionAtLeast,
since frame_tree_version (§3) is now a first-class generation counter
plans may need to wait on (e.g. "wait for this iframe to finish
navigating" is a frame_tree_version condition, not a dom_version one).
```

**DoD:** unchanged from v2, plus a `FrameTreeVersionAtLeast` test using
an iframe-navigation fixture, and confirmation the lifecycle waiter has
exactly one implementation.

---

## PHASE 10 — Deterministic recovery ladder

**Goal:** unchanged from v2 in substance — recover locally before
escalating, ladder order: existing ref → cached backend node → CSS
selector → DOM id/name → AX role/name → semantic text → fresh DOM/AX
search → frame/shadow traversal → explicit Ambiguous/ResolutionFailed →
vision only if applicable. Every tier's attempt recorded; never skip a
tier.

**DoD:** unchanged from v2 (mutation fixture: stale ref → same semantic
identity, new backendNodeId → recovery demonstrated; ambiguity-after-
recovery tested).

---

## PHASE 11 — Crash/restart recovery

**Goal:** recover without corrupting browser state, using the
default-off restart policy (rule 24) and the `TargetRef`/`SessionRef`
generation model from §3 to guarantee no stale identity reuse
(invariant I12).

**Agent prompt:**

```
Integrate crash recovery with the BrowserRuntime state machine (§1) and
existing src/task.rs persistence — read it completely first, do not
create a second task persistence system.

Config: browser_runtime.restart_on_crash: bool = false (default).

On CDP disconnect:
1. mark runtime dead, transition Connected → Reconnecting (§1)
2. increment connection_generation (bumps every SessionRef, invalidates
   all outstanding ones per §3)
3. increment runtime_generation
4. invalidate all ElementRefs (dom_version/frame_tree_version no longer
   trustworthy against a dead connection)
5. mark in-flight state-changing operations per their current action
   state (§2) — anything past Dispatching becomes Unknown, per rule 11
6. reconnect to the browser-level WebSocket if the browser process is
   still alive
7. rediscover targets, incrementing target_generation for the new set
   (§3) — a TargetRef from before this point must fail any check
   against current target_generation
8. recreate sessions, re-enable required CDP domains
9. rebuild frame state (bump frame_tree_version), rebuild DOM/AX index
   (bump dom_version/element_index_version)
10. transition Reconnecting → Connected (§1)

If the browser process itself died:
- restart_on_crash=false (default): transition to Disconnected, report
  clearly, do not restart.
- restart_on_crash=true AND hyprfast launched the browser originally
  (not merely attached): restart with identical launch parameters, then
  run the reconnect sequence above.
- hyprfast merely attached to an externally-launched browser: never
  restart it, regardless of the config flag.

Unknown state-changing actions are never replayed automatically,
regardless of restart_on_crash.
```

**DoD:** unchanged from v2's three scenarios (restart_on_crash=false;
restart_on_crash=true + hyprfast-launched; restart_on_crash=true +
externally-launched), plus: explicitly assert that a `TargetRef`/
`SessionRef` captured before the crash is rejected by generation check
after recovery even if the raw target_id/session_id string Chromium
issues happens to collide with the pre-crash value (construct this
deliberately if the real browser doesn't reuse IDs naturally, to prove
the generation check — not just the ID — is what's being relied on).

---

## PHASE 12 — Incremental DOM/AX updates

**Goal:** unchanged from v2 — incremental updates for
childNodeInserted/Removed/attributeModified, full rebuild for
documentUpdated/navigation/recovery, atomic `DomDiff` exposure, and
invalidate-rather-than-guess whenever correctness can't be proven.

**DoD:** unchanged from v2 (mutation fixture, thousands of real
mutations, before/after performance, correctness of add/remove/change/
invalidated_refs).

---

## PHASE 13 — Screenshot/vision fallback

**Goal:** unchanged from v2 — vision only after DOM+AX+semantic+frame/
shadow resolution are all exhausted; `VisualTarget` never silently
becomes a normal `ElementRef`; canvas/WebGL/PDF/visual-only-control use
cases only.

**DoD:** unchanged from v2 (real canvas fixture, resolver correctly
reports inability, vision fallback exercised, normal HTML never invokes
vision).

---

## PHASE 14 — Performance/tracing infrastructure

**Goal:** unchanged from v2's trace fields, plus **[V3]** the ordering-
pipeline diagnostics from Phase 1 (`reorder_buffer_depth`, offloaded-
decode count/latency) surfaced in `browser-runtime diagnostics`
alongside the existing latency percentiles, vision fallback count,
reconnect count, DOM rebuild count, incremental update count.

**DoD:** unchanged from v2 — real benchmark suite, before/after
measurements, confirmation the persistent runtime eliminates repeated
`/json` + handshake from normal actions.

---

## PHASE 15 — Full integration + MCP/CLI compatibility

**Goal:** unchanged from v2 — audit every `browser_*`/`stagehand_*` MCP
tool and CLI command (using Phase 0A's exact list, not the repo's
overall tool count), route everything through the same `BrowserRuntime`,
add `browser_execute_plan` without removing single-action tools.

**DoD:** unchanged from v2 — `cargo test --release`, clippy clean, every
CLI/MCP path exercised against real Chromium, `grep` for stray
`connect_async` returns nothing, exact tool-count reconciliation against
Phase 0A's list.

---

## PHASE 16 — Production hardening / final acceptance

**Goal:** verify every invariant in §7 (I1–I26), then run the full
failure-injection suite below, then the final acceptance scenario.

**Agent prompt:**

```
Perform a complete production audit of BrowserRuntime. Do not modify
behavior merely to make tests pass. Check off every invariant I1–I26
from this plan's §7 individually, with the specific test or code
inspection that verifies each one — not a blanket "looks fine."

Then execute the full Failure-Injection Test Suite below in its
entirety against the real runtime. This is not optional coverage — it
is more informative than additional happy-path tests, per this plan's
review history.
```

---

## Failure-injection test suite **[V3 — new, run in Phase 16]**

Each of these is induced against the real browser/runtime — never
mocked. Map each to the invariant(s) it's proving:

| # | Scenario | Invariant(s) proven |
|---|----------|----------------------|
| 1 | Browser closes during `Runtime.evaluate` | I1, I8, I11 |
| 2 | Browser closes immediately after click dispatch (before response) | I8, I10, I15 (`Unknown`) |
| 3 | Tab closes during element resolution | I7, I10 (cancellable phase) |
| 4 | Iframe navigates during resolution | I7, §6 snapshot re-check |
| 5 | DOM mutates between resolution and click dispatch | §6, I7 |
| 6 | Popup opens during a click | I4, I24 (no second WebSocket) |
| 7 | Target detaches mid-action | I3, I12 |
| 8 | Session detaches mid-action | I12 |
| 9 | Large AX tree returned while small requests are pending on another target | I9, I22, §4 |
| 10 | Oversized CDP message (over max accepted size) | I21 |
| 11 | Client disconnects during an already-dispatched action | I10 |
| 12 | Client disconnects during resolution (pre-dispatch) | I10 (should actually cancel) |
| 13 | Daemon receives SIGTERM during an in-flight action | I15, I8 |
| 14 | Browser crashes with `restart_on_crash=false` | I23 |
| 15 | Browser crashes with `restart_on_crash=true`, hyprfast-launched browser | I12, I23 |
| 16 | Browser crashes with `restart_on_crash=true`, externally-attached browser | I23 (must NOT restart) |
| 17 | **[V3]** IPC handshake sent with mismatched `protocol_version` | I26 |
| 18 | **[V3]** Command sent before handshake completes | I26 |
| 19 | **[V3]** `Target.setAutoAttach{flatten:true}` fails at startup | I25 |
| 20 | **[V3]** Two ambiguous candidates appear only after a recovery-tier fallback (not at initial resolution) | I18 |

Each row's test must show real command output, not a description of
expected behavior — per contract rule 20.

---

## Final acceptance test

```
Launch real Brave/Chromium
        │
        ▼
Start hyprfast BrowserRuntime daemon (socket 0600 verified, handshake
  protocol_version negotiated)
        │
        ▼
Open real fixture → discover target via Target events → attach session
  (TargetRef/SessionRef generation = current)
        │
        ▼
Build DOM + AX index → navigate → lifecycle event → resolve element
  (versions recorded) → snapshot re-check passes → click → verify
        │
        ▼
Type → read back value
        │
        ▼
Open popup → discover second target → switch target (no new WebSocket)
        │
        ▼
Resolve iframe element → resolve shadow DOM element
        │
        ▼
Perform ambiguous resolution → correctly reject ambiguity
        │
        ▼
Mutate DOM → incremental index update (ordering preserved per §4)
        │
        ▼
Crash browser → runtime detects RuntimeDead → Reconnecting (§1)
        │
        ▼
Recovery per restart_on_crash policy (test both settings) → new
  connection_generation → new TargetRef/SessionRef generation → old
  refs rejected even if raw IDs collide → new refs work
        │
        ▼
Execute multi-step plan → verification + action journal
  (production_profile + capability_class correctly tagged)
        │
        ▼
Run full Failure-Injection Test Suite (all 20 rows, real output)
        │
        ▼
Shutdown daemon → clean socket + connections + owned browser
        │
        ▼
PASS
```

---

## Final directory architecture

```
src/
├── browser/
│   └── mod.rs
│
├── browser_runtime/
│   ├── mod.rs
│   ├── connection.rs        (ordering pipeline, §4)
│   ├── error.rs
│   ├── server.rs             (handshake, capability tagging)
│   ├── client.rs
│   ├── state.rs               (§1 lifecycle state machine)
│   ├── events.rs
│   ├── dom_state.rs
│   ├── targets.rs             (TargetRef/target_generation, §3)
│   ├── frames.rs               (frame_tree_version, §3)
│   ├── element_index.rs         (snapshot-versioned ElementRef, §6)
│   ├── dom_diff.rs
│   ├── plan.rs
│   ├── executor.rs               (§2 action state machine, cancellation,
│   │                               timeout categories)
│   ├── action.rs
│   ├── action_journal.rs          (production_profile, capability_class)
│   ├── wait.rs
│   ├── vision.rs
│   ├── trace.rs
│   └── metrics.rs
│
├── cdp/
│   └── ...
├── stagehand/
│   └── ...
├── task.rs
├── daemon.rs
└── main.rs

docs/
└── browser_runtime_migration.md   (Phase 0C output)

tests/
└── browser_fixtures/
    ├── basic.html
    ├── iframe.html
    ├── shadow_dom.html
    ├── navigation.html
    ├── disabled_button.html
    ├── ambiguous_buttons.html
    ├── mutation.html
    ├── popup.html
    ├── search_form.html
    ├── analytics_button.html
    ├── broken_login.html
    ├── instant.html
    ├── contenteditable.html
    ├── select.html
    └── crash_test.html
```

---

## Phase boundary rule

The phase number is a hard architectural boundary. Do NOT implement code
belonging primarily to a future phase unless it's an explicit
prerequisite of the current one. After completing the current phase:
**STOP.** Do not proactively implement Phase N+1, "prepare" future
features, refactor unrelated code, or redesign later phases.

Report, exactly, at the end of every phase:

```
PHASE: N (or 0A/0B/0C)
STATUS: PASS / FAIL

FILES CHANGED:
...

ARCHITECTURAL CHANGES:
...

INVARIANTS TOUCHED (from §7):
...

COMMANDS RUN:
...

REAL OUTPUT:
...

TESTS:
...

KNOWN ISSUES:
...

UNIMPLEMENTED FUTURE WORK:
...
```

Then **STOP** and wait for the next instruction.
