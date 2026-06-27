# CLAUDE.md

## Agent skills

### Issue tracker

Issues and PRDs live in the repo's GitHub Issues, managed via the `gh` CLI. See `docs/agents/issue-tracker.md`.

### Triage labels

Default vocabulary: `needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`, `wontfix`. See `docs/agents/triage-labels.md`.

### Domain docs

Single-context: one `CONTEXT.md` + `docs/adr/` at the repo root. See `docs/agents/domain.md`.

## Architecture

revu is a single Rust binary: a terminal diff/review tool. Data flows
`git/stdin -> parse -> model -> render -> ratatui`. Source modules (`src/`):

| Module | Role |
|--------|------|
| `main.rs` | Entry point; parses CLI, dispatches to an `app::run_*`. |
| `cli.rs` | clap command/arg defs (`diff`, `show`, `stash`, `difftool`, `pager`, `patch`). |
| `app.rs` | Terminal setup + the interactive review loop (`run_loop`); owns scroll/sidebar/fold/theme-selector state. Hottest file. |
| `render.rs` | Pure `DiffModel` -> ratatui buffer rendering; `stack` (unified) and `split` (side-by-side) layouts. |
| `diff.rs` | Pure unified-diff parser (ANSI-aware); text in, `DiffModel` out. |
| `fold.rs` | Collapsible "N unchanged lines" fold computation. |
| `worddiff.rs` | Intra-line word-level emphasis on modified lines. |
| `highlight.rs` | syntect syntax highlighting (pure-Rust fancy-regex engine). |
| `theme.rs` | Theme catalog, custom-theme parsing, terminal light/dark detection. |
| `config.rs` | Layered config (global + repo-local TOML) + CLI overrides. |
| `state.rs` | View-state (`state.json`) persistence across runs. |
| `pager.rs` | `pager`/`patch` entrypoints; stdin/tty + plain-text pager fallback. |
| `vcs/git.rs` | `VcsAdapter` trait + git implementation (shells out via argv). |

Each module has an inline `#[cfg(test)]` block; `render.rs`/`app.rs` also have
insta snapshot tests in `src/snapshots/`. `tests/supply_chain.rs` fails CI if a
network/telemetry crate enters the dependency tree.

## Development

Run the same checks CI runs:

```sh
just check    # fmt + clippy (-D warnings) + tests + crate budget  (if a justfile exists)
# or directly:
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
```

New dependencies need a written justification (see the rationale comments in
`Cargo.toml`); a CI job enforces a hard transitive-crate budget.
