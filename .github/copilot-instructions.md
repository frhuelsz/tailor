# Copilot Agent Instructions

These instructions apply to Copilot agents working in this repo — both agents
writing code and agents reviewing pull requests. The PR-review rules below are
review-only. The "Nits" and "Architecture & structural soundness" sections
apply to **both** writers (follow them when generating new code) and
reviewers (see [Reviewer etiquette](#reviewer-etiquette-nits--architecture)
for when to surface them).

## PR Review Scope

- Only comment on issues that are **specific to the diff** (avoid generic best-practice reminders).
- Avoid repeating the same point across multiple files. If one example demonstrates a pattern, mention it once and reference the pattern.
- **ALWAYS check previous reviews** before commenting. Do NOT repeat points that have already been made in previous reviews if they have been acknowledged, dismissed, or closed.

### What to focus on (in priority order)

1. Correctness and logic bugs
2. Security issues (input validation, authz/authn, secrets, injection)
3. Performance regressions (hot paths only)
4. API/contract changes and backward compatibility
5. Test gaps only when risk is high or behavior changed

## What to avoid

- Do NOT suggest stylistic refactors unless they fix a bug or prevent a clear maintenance issue.
- Do NOT request documentation unless public APIs changed.
- Do NOT comment on naming unless it causes real ambiguity.
- Do NOT suggest "add null checks" if the code is already guarded or types guarantee non-null.

### Output style

- Prefer fewer, higher-signal comments.
- Use this structure when leaving feedback:
    - **Issue** (why it matters)
    - **Evidence** (where in diff / what behavior)
    - **Suggestion** (concrete fix)

## Nits (Rust, applies to writers and reviewers)

This workspace formats with `rustfmt` (workspace-wide — see `rustfmt.toml`).
Everything below is in addition to `cargo fmt` and is **aspirational**: real
files don't follow every rule 100% of the time. A counter-example in the tree
is not a license to ignore the rule in new code.

### Imports

1. **One `use` statement per top-level path, with a brace-tree for everything
   under it.** Never write two `use` lines that share a root:

    ```rust
    // BAD
    use std::fs;
    use std::path::PathBuf;

    // GOOD
    use std::{fs, path::PathBuf};
    ```

    So the `std` block is a single `use std::{…};`, each external crate is a
    single `use somecrate::{…};`, each workspace-local crate is a single
    `use workspacecrate::{…};`, and `crate::` and `super::` each get exactly
    one `use` per group.

2. **File ordering of imports.** Order the top-of-file import region in groups
   separated by a single blank line, alphabetical within each group:
    1. `use std::{…};`
    2. external crates (`anyhow`, `clap`, `serde`, …)
    3. workspace-local crates
    4. `use crate::{…};`
    5. `use super::{…};`

    Then a blank line, then **submodule declarations**:

    ```rust
    mod mysubmodule;
    mod othermodule;
    ```

    Then, **only when necessary**, submodule imports:

    ```rust
    use mysubmodule::Foo;
    use othermodule::bar;
    ```

    A full example:

    ```rust
    use std::{collections::HashMap, path::PathBuf};

    use anyhow::{Context, Error};
    use log::{debug, info};

    use crate::engine::EngineContext;

    use super::Subsystem;

    mod inner;
    mod helpers;

    use inner::InnerThing;
    ```

3. **Test modules:** the very first line inside `mod tests { … }` is
   `use super::*;` on its own, followed by a blank line and then the standard
   import groups above (rules 1 and 2).
4. **Uppercase identifiers — import directly.** Types, enums, traits,
   structs: `use foo::Bar;` → `Bar::new(…)`.
5. **Lowercase identifiers (free functions) — import the parent module, not the
   function.** `use somelib::files;` → `files::write(…)`, **not**
   `use somelib::files::write;` → `write(…)`. This keeps call sites
   self-documenting and avoids name collisions.
6. **Macros — import directly**, even though their names are lowercase:
   `use anyhow::{bail, ensure};` → `bail!(…)`, never `anyhow::bail!(…)` at the
   call site. Same for `log::{debug, info, warn, error, trace}`.
7. **Strictly prefer imports over full paths at the call site.** Never:

    ```rust
    // BAD
    let a = std::submodule::Type::new(...);
    ```

    Always:

    ```rust
    use std::submodule::Type;
    // …
    let a = Type::new(...);
    ```

    When two imports would collide on the same name, prefer a sensible
    `use … as …` alias over reaching for a full path — e.g. when multiple
    `Context` types are in scope, `use tera::Context as TeraCtx;` and then
    `TeraCtx::new(…)` at the call site. There is no fixed naming scheme for
    aliases; pick a short, locally-readable name case by case. Reach for a
    fully-qualified path only when even an alias would be misleading
    (e.g. a one-off use where the full path is the clearest documentation).

8. **When a crate provides its own `Result` alias and a file uses it more
   than ~5 times, prefer importing the alias.** Example: in a file doing a
   lot of IO,

    ```rust
    use std::io::Result as IoResult;

    fn my_func() -> IoResult<Type> { … }
    ```

    over

    ```rust
    fn my_func() -> Result<Type, IoError> { … }
    ```

    For imports that overload language primitives (`Result`, `Error`,
    `Option`), always alias — never shadow the prelude name in a `use`
    without an `as`. Alias naming is case-by-case (`IoResult`, `TeraCtx`, …).

    **Exception: `anyhow`.** Import `anyhow::Error` and spell
    `Result<T, Error>` out:

    ```rust
    use anyhow::Error;

    fn my_fn() -> Result<T, Error> { … }
    ```

    Don't alias `anyhow::Result`.

### Visibility & module layout

9. **Default to the strictest visibility that compiles.** New items start
   private (`fn`, `struct`), graduate to `pub(super)`, then `pub(crate)`, and
   only become `pub` when they intentionally cross the crate boundary. Be
   especially skeptical of `pub` that creates a dependency between distant
   modules — a `pub(crate)` re-export at `lib.rs` is usually a better answer
   than reaching deep into a submodule from elsewhere.

### Error handling

10. **Avoid `unwrap()`/`expect()`/`panic!` in non-test code.** Accepted
    patterns: (a) propagate with `?`; (b) `.expect("invariant: …")` documenting
    a static invariant that genuinely cannot fail.
11. **Use `anyhow::Context` to build informative error chains** when each layer
    adds genuinely new information (which subject failed, which path, which
    iteration). It is **not** required at every `?` — redundant context like
    `.context("failed to do thing")` on a function literally named `do_thing`
    is noise. The point is to make authors think about whether the next reader
    of the error can reconstruct what went wrong.
12. **When context is a `format!(...)`, use `.with_context(|| format!(…))`
    instead of `.context(format!(…))`** so the string is only built on the
    error path. Plain string literals stay on `.context("…")`.

### Logging

13. **Use the `log` crate** (`use log::{debug, info, warn, error, trace};`) for
    application logging.

### Tests

14. **Inline `#[cfg(test)] mod tests { … }`** at the bottom of the file under
    test (vs. separate `tests/` files), unless the test crosses crate
    boundaries.
15. **Prefer `.unwrap()`/`.unwrap_err()` over `assert!(x.is_ok())` /
    `assert!(x.is_err())`** — the panic surfaces the underlying error.
    For variant assertions: `assert!(matches!(err, ErrorKind::Foo { .. }), "got {err:?}")`.

### Cargo & workspace hygiene

16. **All third-party deps come from `[workspace.dependencies]`** — every
    crate's `Cargo.toml` says `foo = { workspace = true }`, never an inline
    version. New deps are added to the root `Cargo.toml` first.
17. **Workspace-local deps use a `path = "..."` reference** (see existing
    local crate dependency blocks).

### Misc Rust idioms

18. **Inline format args** (`format!("{name}")`, `info!("done: {count}")`) over
    positional (`format!("{}", name)`). Fall back to positional only when the
    expression isn't a bare identifier or a simple `expr.field` /
    `expr.method()`.
19. **Prefer `impl AsRef<Path>` over `&Path` / `&PathBuf`** for function
    arguments that just need to read a path, unless there is a concrete reason
    not to. Same principle for `impl AsRef<str>` / `impl AsRef<[u8]>` where
    appropriate. Inside the function body, immediately bind once:
    `let path = path.as_ref();`.
20. **No magic numbers or magic strings.** Use `const`s with explanatory names,
    scoped as tightly as the use justifies.
21. **Comments explain _why_, not _what_.** A doc that restates the function
    name is noise; a doc that names the invariant or links to the relevant
    design section is signal.
22. **Aim for shorter expressions that remain readable.** Prefer a sensible
    `match` over nested `if`/`else if` chains, especially when branching on
    multiple values at once (`match (a, b) { … }`). Lean on iterators
    (`.iter().filter().map().collect()`) when they're clearer than a manual
    loop, but prefer loops when an iterator chain would become too cumbersome
    or cryptic and a loop provides a more self-documenting solution. Reduce
    duplication with a local closure (`let normalize = |s: &str| …;`) instead
    of repeating a 3-line block four times. Avoid verbose blocks that can be
    expressed more succinctly — but stop short of cleverness that hurts the
    next reader (one-line iterator chains with side effects, deeply nested
    closures). The bar is: a reasonable reviewer should read the code at
    roughly the same speed as a more verbose version, with fewer tokens to skim.

## Architecture & structural soundness

This workspace is a collection of minimal binaries for Azure Linux (ACL)
system management. Each binary is a focused tool that does one thing well.

### Layering rules

- **Lower-level utility crates do not import higher-level binary crates.**
- **Shared logic goes in library crates**, not duplicated across binaries.
- **Each binary owns its CLI surface** (argument parsing, subcommands) and
  delegates actual work to library code where possible.

### Code reuse

Before introducing a new utility:

1. Check existing workspace crates for overlap.
2. Check `[workspace.dependencies]` before adding a new dep.
3. If a reusable helper emerges, factor it into a shared library crate rather
   than duplicating across binaries.

## Reviewer etiquette (Nits & Architecture)

The "What to avoid" rules at the top of this file still apply: **do not open
a separate review comment just to flag a nit on otherwise-correct code.**
Nits and architectural notes are for writers to follow proactively, and for
reviewers to use as a checklist when a diff already touches the area.
Specifically:

- **Don't drag pre-existing violations into a PR's diff.** If a file already
  violates a nit (or already has a layering issue) on untouched lines,
  ignore it. Use a separate, dedicated PR.
- **Cluster comments.** When a diff has several small nits in one region,
  leave **one** comment listing them — not one per occurrence.
- **Never block a PR on a nit alone.** Mark nit-only comments as
  non-blocking, or fold them into a broader comment whose primary point is
  substantive.

---

## Marvin-specific guidance for agents

This is the Marvin CI failure triage daemon. The conventions below
are **marvin-specific** and take precedence over the generic rules
above when they conflict (e.g. marvin uses `tracing` not `log`, and
custom error types not `anyhow`).

### What this codebase is

- **Rust workspace** with multiple crates under `crates/`:
  - `marvin-core` — domain types, ports (traits), in-memory test
    doubles. No I/O. No adapters.
  - `marvindb-sqlite` — `Database` port implementation backed by
    SQLite + `refinery` migrations.
  - `marvin-ado` — `AdoClient` port implementation (REST over
    `reqwest`).
  - `marvin-az` — Azure CLI auth helper.
  - `marvinbot-mcp` — per-run MCP server the bot talks to (subject
    reads, KB reads/writes, report submission).
  - `marvind` — the **daemon binary** + composition root. Owns
    pollers, orchestrator, action executor, MCP host process,
    systemd integration. The ONLY crate that knows concrete
    adapter types.
  - `marvinctl` — the **operator CLI binary**. Inspect DB, replay
    triages, clear bugs, etc. Reads-only against the running
    daemon's DB unless explicitly mutating with `--yes`.
- **The bot** (`marvinbot`) is NOT a Rust crate — it's a Copilot CLI
  session with `marvinbot/skill.md` + `marvinbot/prompts/triage.md`
  defining its behavior, plus the per-run MCP socket for tools.

### Conventions that differ from the generic rules above

- **Logging: use `tracing`, not `log`.** `info!`, `warn!`, `error!`,
  `debug!`, `trace!` from `tracing`. Structured fields, not
  `{format}` interpolation, for machine-readable logs:
  `tracing::info!(scope_id = %scope, run_uuid = %uuid, "claimed subject");`
- **Errors: use `CoreError` (from `marvin-core`) and the per-crate
  thiserror wrappers**, not `anyhow`. The `?` propagation pattern,
  `Context`-equivalent layering, and "no `unwrap` in non-test code"
  rules all still apply — just via `CoreError::wrap` /
  `adapter_error("…", err)` helpers instead of `.context(…)`.
- **Time: `jiff::Timestamp`, never `chrono` or `std::time`.** All
  DB column codecs go through marvindb-sqlite's canonical codec.
- **DB column inspection in tests/scripts: use `marvinctl db ...`**
  (e.g. `marvinctl db triage <uuid>`, `marvinctl db bugs --scope ...`)
  rather than raw `sqlite3` reads, unless you specifically need a
  query that the CLI doesn't expose.

### Required reading before editing

The repo's `AGENTS.md` is the source of truth for the `just`-based
operator workflow (install, lifecycle, observability). Read it
before:
- Modifying `justfile`, `crates/marvind/Cargo.toml`, or
  `packaging/systemd/*` (they're tied to the `just install`
  scaffolding).
- Writing operator-facing docs that reference commands.

### Test → rebuild → install → restart workflow

The Marvin workflow is gated by **`just validate`** and deployed via
**`just install && just restart`**. Use these commands; never invoke
`cargo` directly when a `just` recipe exists, and never hand-edit the
rendered systemd unit (it gets clobbered on every `just install`).

1. **Edit code.** Apply the [Nits](#nits-rust-applies-to-writers-and-reviewers)
   above (inline format args, `.unwrap()`/`unwrap_err()` in tests,
   strict visibility, no magic numbers, etc.).
2. **Pre-commit gate:** `just validate` (= `cargo fmt --check` +
   `cargo clippy --workspace --all-targets --all-features -- -D warnings`
   + `cargo test --workspace --all-features`). If clippy fails on a
   pedantic lint, prefer fixing it; reach for `#[allow(...)]` only
   with a justification in the line above.
3. **Build + install:** `just install` rebuilds release binaries
   AND re-renders the user systemd unit. It does NOT start the
   daemon; it's idempotent.
4. **Restart:** `just restart` (systemctl --user). Confirm with
   `systemctl --user is-active marvind.service` → `active`.
5. **Watch:** `just logs` (pretty), `just logs-raw` (JSON for grep),
   `just logs-file` (today's rolling on-disk file).
6. **Verify on a real bug:** `marvinctl db clear-bug <id> --yes`
   forces a re-triage; `marvinctl triage replay <run-uuid>` replays
   with captured inputs (no fresh ADO fetch).
7. **Inspect the result:** `marvinctl db triage <run-uuid>` (verdict
   + bot report + queued actions), `just run-report` (pretty-print
   newest `run-report.json`), `just monitor <uuid-prefix>` (live
   bot session as a chat transcript).

### Testing conventions

- **Unit tests live inline at the bottom of the module file** under
  `#[cfg(test)] mod tests { … }`. Integration-flavored tests live
  in `crates/<crate>/tests/<name>.rs`. The line-of-the-test imports
  start with `use super::*;` per the generic rule 3.
- **In-memory test DB:** `marvin_core::testing::InMemoryDatabase`.
  Use it for orchestrator tests; reach for `marvindb-sqlite` only
  when you specifically need SQL behavior under test.
- **Mock bot launcher:** `MockBotLauncher` / `MockBotHandle` (in
  `crates/marvind/src/orchestrator/bot_process.rs` under
  `#[cfg(test)]`). Submits scripted reports without retry — relevant
  for the coverage-gate feature flag below.
- **Coverage gates in tests:** the adversarial `report.submit` gates
  (`OrchestratorConfig.enable_report_coverage_gates`) default to
  `false` in `test_config()` / e2e configs because mock bots don't
  retry. New orchestrator tests should leave them disabled.
- **`#[cfg(test)]` for dead-code-only-in-prod helpers:** when a
  function or constant is now only referenced from tests (e.g.
  `render_observed_failures` after the comment-renderer trim), mark
  it `#[cfg(test)]` rather than `#[allow(dead_code)]` so it doesn't
  compile into the release binary.

### Per-run sandbox + MCP socket model

Each triage run:
1. Gets a UUIDv7 `run_uuid`.
2. Gets a sandbox directory at `run/sandboxes/<run-uuid>/` with
   `input/`, `output/`, `home/`, plus `mcp.json` pointing at the
   per-run socket and `AGENTS.md` copied from
   `marvinbot/skill.md`.
3. Spawns an MCP server bound to a unix socket at
   `run/runtime/mcp/<run-uuid>.sock`. The server is `McpServer<D>`
   from `marvinbot-mcp`, wired via `start_with_full_wiring(...)`
   (many arguments — match the existing call order in
   `crates/marvind/src/orchestrator/triage.rs`).
4. Spawns the bot subprocess (Copilot CLI) pointed at the sandbox
   and MCP socket.
5. Waits for `report.submit` MCP call (max 3 attempts; adversarial
   coverage gates may reject the first 2).
6. Drains observations, queues `Action` rows (comment, rerun,
   close-as-duplicate, etc.).
7. Cleans up sandbox (unless `keep_sandbox_on_error` or
   `keep_sandbox`).

### Adversarial coverage gates (`report.submit`)

The MCP `report.submit` handler enforces investigation-quality
gates after schema validation. Current gates:

| Code | Trigger |
|---|---|
| `missing_upstream_investigation` | `subject_failed_task_count ≥ 5` AND bot never called `subject.upstream_failures` |
| `missing_log_inspection` | `failed_task_count > 0` AND no `subject.log_*` call AND verdict is not `already_resolved` / `needs_human` |
| `missing_candidate_inspection` | `duplicate_candidate_ids` non-empty AND no `subject.candidate_bug_lookup` call |
| `close_as_duplicate_without_inspection` | `recommended_actions` contains `close_as_duplicate` AND no `subject.candidate_bug_lookup` call |
| `transient_without_evidence` | verdict is `transient` AND `key_errors` empty AND no log calls |

The bot retries in-session (same Copilot CLI process) until either
accepted or `MAX_REPORT_ATTEMPTS = 3` attempts are exhausted. The
final attempt always bypasses the gates so cost/latency stay
bounded.

**Adding a new gate:** edit
`crates/marvinbot-mcp/src/tools/report.rs` → `evaluate_coverage_gates`,
add tests, and update `marvinbot/prompts/triage.md` so the bot
knows what code to expect and how to fix it.

### Comment-body renderer

`build_comment_body` (in `crates/marvind/src/orchestrator/triage.rs`)
is the single entry point for ADO comment rendering. Section order
is stable and covered by golden tests in
`comment_renderer_tests`. When adding/removing sections, update both
the function AND the golden tests, AND the `e2e_flow_a` expected
body, in the same commit. Per operator feedback, the comment must
keep the headline + verdict + recommended action close to the top;
technical reference data (signature lists) belongs in a collapsed
`<details>` block.

### When in doubt

- Read the per-area design notes under `meta/` for the most recent
  date matching the area you're editing.
- Look for an existing test that exercises the path you're changing
  — `just validate` will reveal any regression.
- If you genuinely need to bypass a guideline (e.g. a `#[allow]`
  for a clippy::pedantic lint), leave a one-line comment on the
  line above explaining why.

