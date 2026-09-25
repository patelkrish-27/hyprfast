# AGENTS.md — hyprfast BrowserRuntime work

> If this repo already has an AGENTS.md from `opencode init`, merge this
> section into it rather than overwriting the project-structure/build
> info opencode generated.

This project is mid-implementation of a persistent browser automation
runtime. The full spec — non-negotiable contract, architectural
invariants, and every phase's agent prompt + DoD — lives at:

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
8. **Build release after features.** After implementing a feature, run
   `cargo build --release` and fix any errors before reporting done.
   A debug-only `cargo build` is not sufficient.
9. **Install to PATH after release build.** After `cargo build --release` succeeds, run `cargo install --path .` (or `cp target/release/hyprfast ~/.cargo/bin/hyprfast`) so the `hyprfast` on `PATH` reflects the new code. Without this, `hyprfast categories` / `hyprfast category` will show stale output from `~/.cargo/bin/hyprfast`.
10. **User-facing smoke test required.** After implementing a feature,
   prove a real user can invoke it directly end-to-end — run the
   actual public entry point (CLI command, MCP tool call, etc.) exactly
   as a user would and paste its real output. Do NOT substitute
   internal/debugging paths (unit tests alone, `cargo test`, direct
   calls to internal fns, debug flags, test fixtures wired around the
   public interface) for this check. If the feature cannot be
   exercised through its public interface in this session, write
   `NOT RUN` and say why. Always test via the `PATH` binary (`hyprfast ...`), not just `./target/debug/hyprfast` or `./target/release/hyprfast`.
