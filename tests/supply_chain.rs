//! Supply-chain guard (no network / no telemetry).
//!
//! revu is sync and offline by design: it performs no outbound network calls
//! and emits no telemetry (the hunk-style phone-home update check is
//! intentionally absent). This test parses the committed `Cargo.lock` and fails
//! if any crate from a focused denylist of HTTP-client / async-runtime /
//! telemetry crates enters the resolved dependency graph.
//!
//! It runs under `cargo test` (no external tooling required) and is the
//! always-on complement to the CI `cargo-deny` + `cargo-audit` gates.

/// Crates that imply outbound network I/O or telemetry. Kept focused on real
/// network/telemetry crates so it never trips on something revu legitimately
/// uses. (Verified against the actual tree: none of these are present.)
const FORBIDDEN: &[&str] = &[
    // HTTP / network clients
    "reqwest",
    "hyper",
    "ureq",
    "isahc",
    "curl",
    "surf",
    "attohttpc",
    "http-client",
    // async runtimes (revu is intentionally synchronous)
    "tokio",
    "async-std",
    "smol",
    // telemetry / metrics exporters
    "sentry",
    "opentelemetry",
    "datadog",
    "statsd",
];

#[test]
fn no_network_or_telemetry_crates() {
    let lock = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.lock"));

    // Cargo.lock lists each package name on its own line as: name = "<crate>"
    let mut found: Vec<&str> = FORBIDDEN
        .iter()
        .copied()
        .filter(|bad| {
            let needle = format!("name = \"{bad}\"");
            lock.lines().any(|line| line.trim() == needle)
        })
        .collect();
    found.sort_unstable();

    assert!(
        found.is_empty(),
        "forbidden network/telemetry crate(s) entered the dependency tree: {found:?}. \
         revu is offline-by-design with no telemetry; if this dependency is truly \
         required, the supply-chain policy must be updated deliberately."
    );
}
