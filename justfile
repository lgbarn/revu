# Run `just` with no args to list recipes. `just check` mirrors CI's gates.

# Default: show available recipes.
default:
    @just --list

# Everything CI runs that can run locally without extra tools.
check: fmt-check clippy test crate-budget

# Format check (CI: fmt job).
fmt-check:
    cargo fmt --all --check

# Apply formatting.
fmt:
    cargo fmt --all

# Lint with warnings denied (CI: clippy job).
clippy:
    cargo clippy --all-targets -- -D warnings

# Run the full test suite, including the supply-chain guard (CI: test job).
test:
    cargo test --all-targets

# Enforce the transitive-crate budget (CI: crate-budget job).
# Budget lives in .github/workflows/ci.yml as CRATE_BUDGET; keep this in sync.
crate-budget budget="155":
    #!/usr/bin/env bash
    set -euo pipefail
    count=$(cargo metadata --format-version 1 | jq '[.resolve.nodes[].id] | length - 1')
    echo "Transitive crate count: ${count} (budget: {{budget}})"
    if [ "${count}" -gt "{{budget}}" ]; then
        echo "ERROR: crate count ${count} exceeds budget {{budget}}" >&2
        exit 1
    fi

# Supply-chain audit (CI: supply-chain job). Needs cargo-deny + cargo-audit,
# which CI installs on the runner; skipped locally when they are not present.
audit:
    #!/usr/bin/env bash
    set -euo pipefail
    if command -v cargo-deny >/dev/null 2>&1; then cargo deny check; else echo "skip: cargo-deny not installed"; fi
    if command -v cargo-audit >/dev/null 2>&1; then cargo audit; else echo "skip: cargo-audit not installed"; fi
