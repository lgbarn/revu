# revu

[![CI](https://github.com/lgbarn/revu/actions/workflows/ci.yml/badge.svg)](https://github.com/lgbarn/revu/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/lgbarn/revu)](https://github.com/lgbarn/revu/releases/latest)
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)](#license)

A fast, memory-safe **terminal diff and code-review tool** in Rust — a
behavioral-equivalence port of [hunk](https://github.com/modem-dev/hunk), built
to escape the npm supply chain and ship as a single static binary with no
runtime.

```sh
revu diff            # review your working-tree changes
git show | less?     # no — point git's pager at revu instead (see below)
```

## Why revu

hunk is an excellent review-first terminal diff viewer, but it runs on Node/Bun
with a large transitive npm dependency tree and pings the npm registry on
startup. revu gives you the same day-to-day reviewing experience with:

- **No npm, no runtime** — one static binary per platform, nothing to install
  alongside it.
- **Memory safety, no C in the core** — pure-Rust throughout, including syntax
  highlighting (`syntect` on the `fancy-regex` engine, not oniguruma).
- **A small, auditable dependency tree** — enforced in CI with `cargo-deny`,
  `cargo-audit`, and a hard transitive-crate budget. New dependencies need a
  written justification.
- **No telemetry, ever** — revu makes zero network calls. There is no update
  check and no analytics.

## Features

- **Review anything**: the working tree, staged changes, a commit, a stash, a
  patch file, or two arbitrary files.
- **Drop-in git pager**: set `revu pager` as git's `core.pager` and every
  `git diff` / `git show` opens in revu; non-diff output falls back to your
  plain-text pager.
- **Side-by-side, unified, or top/bottom**: `--mode split`, `stack`, `vertical`
  (old block above new), or `auto` (responsive to terminal width); cycle live
  with `m`.
- **Syntax highlighting** by language, plus **intra-line word-diff** emphasis and
  **moved-line** coloring (via git's `--color-moved`).
- **File sidebar** with add/remove counts and keyboard navigation.
- **Search, blame, and reload**: in-diff search (`/`), an optional `git blame`
  gutter (`B`, working-tree diffs), and reload-from-source (`r`).
- **12 built-in themes** + custom TOML themes + automatic light/dark detection,
  switchable live from a theme picker.
- **Configurable** via `~/.config/revu/config.toml` and a repo-local
  `.revu/config.toml`, using the same key names as hunk (copy your config over
  verbatim). View toggles persist between runs.
- **Good terminal citizen**: clean `Ctrl-Z` suspend/resume, `Ctrl-C` exit, and
  open-in-`$EDITOR`.

## Install

revu is a single static binary with no runtime and no npm package.

### Homebrew

```sh
brew install lgbarn/tap/revu
```

### Prebuilt binaries

Download the archive for your platform from the
[latest release](https://github.com/lgbarn/revu/releases/latest) — static musl
Linux (x86_64, aarch64) and macOS (Intel, Apple Silicon) — unpack it, and put
`revu` on your `PATH`.

### From source

```sh
cargo install --git https://github.com/lgbarn/revu
```

(Once revu is published to crates.io, `cargo install revu` and
`cargo binstall revu` will also work.)

## Usage

```sh
revu diff                     # working-tree changes
revu diff --staged            # only staged changes
revu diff -- src/             # scope to a path
revu diff old.txt new.txt     # compare two arbitrary files
revu diff --pr 123            # review GitHub PR #123 (via your gh CLI)
revu show                     # review HEAD
revu show <commit>            # review a specific commit
revu stash show               # review the latest stash
revu patch changes.patch      # review a patch file
git diff | revu patch -       # review a piped diff
```

### Use as git's pager

```sh
git config --global core.pager "revu pager"
# now `git diff`, `git show`, `git log -p` all open in revu
```

`revu difftool <local> <remote>` is also wired for `git difftool`.

### Options

Run `revu --help` or `revu <command> --help` for the authoritative list; this is
the full surface.

Global (work anywhere):

| Flag | Description |
| --- | --- |
| `-h`, `--help` | Print help for revu or a subcommand |
| `-V`, `--version` | Print the version and exit |

`diff` additionally accepts:

| Flag | Description |
| --- | --- |
| `--staged` (alias `--cached`) | Review only staged changes |
| `--exclude-untracked` | Omit untracked files from the working-tree review |
| `--pr <N>` | Review GitHub PR `<N>` via `gh pr diff` (no in-process network; needs the `gh` CLI) |

Display flags — override the config file, and are accepted by `diff`, `show`,
`stash show`, `difftool`, `pager`, and `patch`:

| Flag | Description |
| --- | --- |
| `--theme <THEME>` | Color theme (e.g. `auto`, `dracula`, `github-dark`) |
| `--mode <MODE>` | Layout: `auto` (width-responsive), `split`, `stack`/`unified`, or `vertical` (old above new) |
| `--line-numbers` / `--no-line-numbers` | Show / hide the line-number gutter |
| `--wrap` / `--no-wrap` | Wrap / truncate long lines |
| `--hunk-headers` / `--no-hunk-headers` | Show / hide `@@` hunk headers |

Each display flag mirrors a key in `config.toml` (see [Configuration](#configuration));
the CLI flag wins when both are set.

## Keybindings

| Key | Action | Key | Action |
| --- | --- | --- | --- |
| `j` / `k`, arrows | scroll | `s` | toggle sidebar |
| `Space` / `PgUp` | page down / up | `Tab` / `[` `]` | next / prev file |
| `d` / `u` | half page down / up | `{` / `}` | prev / next hunk |
| `g` / `G` | top / bottom | `Left` / `Right` | scroll horizontally |
| `/`, then `n` / `N` | search; next / prev match | `m` | cycle layout (auto/split/stack/vertical) |
| `n` | line numbers | `t` | theme picker |
| `w` | line wrap | `e` | open file in `$EDITOR` |
| `H` | hunk headers | `r` | reload diff from source |
| `c` | collapse all context | `B` | blame gutter (working-tree diff) |
| `o` / `Enter` | toggle fold at cursor | `O` / `C` | expand / collapse all folds |
| mouse wheel | scroll | drag | select lines; release copies (OSC52) |
| `?` | help | `Ctrl-Z` | suspend (resume with `fg`) |
| `q` / `Esc` / `Ctrl-C` | quit | | |

## Configuration

`~/.config/revu/config.toml` (global) and `.revu/config.toml` (per repo), merged
with CLI flags taking precedence:

```toml
theme = "auto"          # "auto", or any built-in / custom theme name
mode = "auto"           # "auto", "split", "stack", "vertical"
line_numbers = true
wrap_lines = false
hunk_headers = true
transparent_background = false   # true: use the terminal's background, not the theme's

[custom_theme]          # optional: override a base theme
base = "dracula"
add = "#9ece6a"
remove = "#f7768e"

[custom_theme.syntax]
keyword = "#bb9af7"
string = "#9ece6a"
```

Themes: `github-light`, `github-dark`, `catppuccin-mocha`, `dracula`, `nord`,
`tokyo-night`, `gruvbox-dark`, `gruvbox-light`, `solarized-dark`,
`solarized-light`, `monokai`, `one-dark`.

`transparent_background = true` drops the add/remove row tints so the terminal's
own background shows through (the `+`/`-` foreground colors and the change-bar
still mark changes). The `vcs` (string) key is accepted for hunk-config
compatibility but **not yet applied** — only `git` is supported today, so setting
it has no effect yet.

## Status

The full review feature set is implemented and tested. revu is young; issues
and PRs welcome. See [CHANGELOG.md](CHANGELOG.md) for the release history and
[SECURITY.md](SECURITY.md) to report a vulnerability.

## Releasing

Pushing a `v*` tag triggers the release workflow, which:

1. creates the GitHub Release,
2. builds and uploads static binaries for all four targets, and
3. regenerates the Homebrew formula (from `packaging/homebrew/revu.rb`) with the
   new version + checksums and pushes it to the tap — so `brew upgrade` just
   works, no manual formula edits.

```sh
# bump version in Cargo.toml first, then:
git tag -a vX.Y.Z -m "revu vX.Y.Z" && git push origin vX.Y.Z
```

**One-time setup for the auto-tap-update** (step 3): the release runs in this
repo and the default token can't write to the separate tap repo, so add a
cross-repo token once:

1. Create a fine-grained PAT with **Contents: read and write** scoped to
   `lgbarn/homebrew-tap`.
2. Store it as an Actions secret in this repo:
   ```sh
   gh secret set HOMEBREW_TAP_TOKEN --repo lgbarn/revu
   ```

Without the secret the release still publishes binaries; only the tap update is
skipped (with a warning).

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your
option.
