# revu

[![CI](https://github.com/lgbarn/revu/actions/workflows/ci.yml/badge.svg)](https://github.com/lgbarn/revu/actions/workflows/ci.yml)

A fast, memory-safe terminal diff/review tool in Rust — a behavioral-equivalence
port of [hunk](https://github.com/modem-dev/hunk), built to escape the npm
supply chain and ship as a single static binary with no runtime.

## Why

hunk is an excellent review-first terminal diff viewer, but it runs on
Node/Bun with a large transitive npm dependency tree. revu targets the same
day-to-day workflow with:

- **No npm / no runtime** — one static binary per platform.
- **Memory safety** — Rust, with no C dependencies in the core (pure-Rust
  syntax highlighting via `syntect` + `fancy-regex`).
- **A small, auditable dependency tree** — enforced in CI (`cargo-deny`,
  `cargo-audit`, and a transitive-crate budget).
- **No telemetry, ever** — no phone-home update check.

## Install

revu ships as a single static binary with no runtime and no npm package.

### Prebuilt binaries (fastest)

Download the archive for your platform from the
[latest release](https://github.com/lgbarn/revu/releases/latest), unpack it, and
put `revu` on your `PATH`. Static musl Linux (x86_64, aarch64) and macOS (Intel,
Apple Silicon) builds are published per tag.

### cargo binstall

Fetches the prebuilt release binary instead of compiling:

```sh
cargo binstall revu
```

### Homebrew

```sh
brew install lgbarn/tap/revu
```

(Requires the tap repo to exist; see `packaging/homebrew/revu.rb`.)

### cargo install (build from source)

```sh
cargo install revu
```

## Status

Pre-v1. Design is locked; see the PRD in the issue tracker.

## License

Dual-licensed under MIT or Apache-2.0.
