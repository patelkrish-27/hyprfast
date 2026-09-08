# AGENTS.md — hyprfast BrowserRuntime work

> If this repo already has an AGENTS.md from `opencode init`, merge this
> section into it rather than overwriting the project-structure/build
> info opencode generated.

This project is mid-implementation of a persistent browser automation
runtime. The full spec — non-negotiable contract, architectural
invariants, and every phase's agent prompt + DoD — lives at:

`docs/hyprfast-browserruntime-plan-v3.md`

Before writing any code in this area, read the specific phase section
named in the user's current request from that file. Do not rely on
memory of earlier turns for its contents — re-read it.

## Hard rules for every session touching browser_runtime

1. **Real Chromium/Brave only.** Never mock CDP, fake WebSocket
   responses, or simulate DOM/AX state in production code paths.
   Fixtures are real HTML loaded into a real browser, never stand-ins
   for browser behavior.
2. **One transport.** Exactly one `connect_async` call site is allowed:
   `src/browser_runtime/connection.rs`. If you're about to add another,
   stop and say so instead of adding it.
3. **One phase at a time.** Implement only the phase named in the
   current user message. Do not start the next phase, "prepare" for it,
   fix unrelated bugs you notice along the way, or refactor files
   outside that phase's listed scope — note anything else you find in
   your phase report's `KNOWN ISSUES` / `UNIMPLEMENTED FUTURE WORK`
   instead of acting on it.
4. **No unverified claims.** Never write "implemented," "tested,"
   "passed," or "verified" unless you actually ran that exact command
   in this session and are pasting its real output below the claim. If
   you didn't run it, write `NOT RUN`.
5. **Stop at the phase boundary.** End every phase with the exact
   report block from the plan's "Phase boundary rule" section
   (PHASE / STATUS / FILES CHANGED / ARCHITECTURAL CHANGES / INVARIANTS
   TOUCHED / COMMANDS RUN / REAL OUTPUT / TESTS / KNOWN ISSUES /
   UNIMPLEMENTED FUTURE WORK), then stop and wait. Do not continue to
   the next phase on your own initiative.
6. **Don't improvise missing prerequisites.** If a phase's agent prompt
   assumes a module or file from an earlier phase that doesn't exist
   yet, say so and stop rather than building a stand-in for it.
7. **Deferred bugs stay deferred.** If a phase's instructions say a
   known issue is fixed in a later phase (see Phase 3 → Phase 6 in the
   plan), do not fix it early even if you notice it — record it as
   already-scheduled instead.
