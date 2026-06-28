# Contributing to revu

Thanks for your interest! revu is a single static Rust binary with no runtime.

## Prerequisites

- A stable Rust toolchain (pinned via `rust-toolchain.toml`; rustup will pick it
  up automatically).
- [`just`](https://github.com/casey/just) as the task runner (optional but
  recommended).
- `jq` for the crate-budget recipe.

## Run the checks locally

`just check` runs everything CI runs that does not need extra tooling:

```sh
just check        # fmt-check + clippy + test + crate-budget
just fmt          # auto-format
just test         # tests only
just audit        # cargo-deny + cargo-audit (skipped if those aren't installed)
just coverage     # line-coverage summary (needs cargo-llvm-cov; informational)
```

These mirror `.github/workflows/ci.yml`. If `just check` is green, CI's
fmt/clippy/test/budget jobs will be too. The `audit` job additionally runs
`cargo deny check` and `cargo audit`; CI installs those tools on the runner.
`just coverage` is informational only (no threshold is enforced).

## Manual verification (terminal paths)

A few unix-only paths drive the real controlling terminal and cannot be
exercised by the automated suite (they would suspend or take over the test
process). Verify these by hand on a Unix terminal when you change `app.rs`'s
terminal setup, the suspend/resume path, or `pager.rs`:

- **Ctrl-Z suspend / resume** (`suspend_and_resume`): inside `revu diff`, press
  `Ctrl-Z` — you should drop to the shell with a working, cooked-mode prompt
  (no stray escape sequences). Run `fg` — revu should redraw cleanly in the
  alternate screen.
- **Quit restores the terminal**: `q` and `Ctrl-C` should both return you to a
  normal prompt with the scrollback intact and the cursor visible.
- **Pager from a real tty**: `git -c core.pager='revu pager' show` should open
  in revu and, on quit, leave the terminal in a sane state.

## Dependency policy

This project keeps a small, auditable dependency tree (enforced by a
transitive-crate budget in CI and by `cargo-deny`). New dependencies need a
written justification in the PR — see the existing rationale comments in
`Cargo.toml`.

## Submitting changes

1. Branch off `main`.
2. Make your change; add or update tests.
3. Run `just check` until green.
4. Open a PR. Commit messages follow Conventional Commits (`feat:`, `fix:`,
   `chore:`, `docs:`, `ci:`), matching the existing history.
