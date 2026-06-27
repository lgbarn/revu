# Changelog

All notable changes to revu are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.5] - 2026-06-26

### Added

- Display flags now work on every diff-rendering subcommand, not just `diff`:
  `--theme`, `--mode`, `--line-numbers`/`--no-line-numbers`, `--wrap`/`--no-wrap`,
  and `--hunk-headers`/`--no-hunk-headers` are honored by `show`, `stash show`,
  `difftool`, `pager`, and `patch` (#42).
- Contributor tooling: a `justfile` mirroring the CI gates (`just check`), a
  pinned `rust-toolchain.toml`, and `CONTRIBUTING.md` (#39); this `CHANGELOG.md`
  and a `SECURITY.md` policy; module-level API docs and a CI `cargo doc` gate (#38).
- A complete CLI options reference in the README, covering every flag including
  `-V`/`--version` (#47).

### Fixed

- Corrected the stale `--mode` `--help` text, which claimed the flag was
  "applied in a later milestone" though it has been fully implemented (#40).

## [0.1.4] - 2026-06-26

### Added

- Collapsible "N unchanged lines" folds, expandable in place (#30).

## [0.1.3] - 2026-06-26

### Added

- Unified view: dual old/new line-number gutter and a left change bar.

### Changed

- Default to the unified (stack) layout; `auto`/`split` remain available via
  `--mode` and the `m` key (#29).

## [0.1.2] - 2026-06-26

### Changed

- Subtler intra-line word-diff emphasis (medium tint, no underline) (#28).

## [0.1.1] - 2026-06-26

### Added

- Full-row red/green background tint on changed lines, and a scroll-derived
  active file so the sidebar and status follow scrolling automatically (#27).

## [0.1.0] - 2026-06-26

### Added

- Initial release: a fast, memory-safe terminal diff and code-review tool, a
  behavioral-equivalence port of [hunk](https://github.com/modem-dev/hunk) in
  pure Rust.
- Review the working tree, staged changes, a commit, a stash, a patch file, or
  two arbitrary files; drop-in `git` pager and `difftool` integration.
- Side-by-side or unified layouts, syntax highlighting, intra-line word-diff
  emphasis, moved-line coloring, and a file sidebar.
- 12 built-in themes plus custom TOML themes with automatic light/dark
  detection; configuration via `~/.config/revu/config.toml` and a repo-local
  `.revu/config.toml`. No telemetry and zero network calls.

[Unreleased]: https://github.com/lgbarn/revu/compare/v0.1.5...HEAD
[0.1.5]: https://github.com/lgbarn/revu/compare/v0.1.4...v0.1.5
[0.1.4]: https://github.com/lgbarn/revu/compare/v0.1.3...v0.1.4
[0.1.3]: https://github.com/lgbarn/revu/compare/v0.1.2...v0.1.3
[0.1.2]: https://github.com/lgbarn/revu/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/lgbarn/revu/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/lgbarn/revu/releases/tag/v0.1.0
