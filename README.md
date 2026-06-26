# revu

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

## Status

Pre-v1. Design is locked; see the PRD in the issue tracker.

## License

Dual-licensed under MIT or Apache-2.0.
