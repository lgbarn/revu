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

use std::fs;
use std::path::Path;

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

#[test]
fn source_has_no_direct_network_primitives() {
    fn visit(path: &Path, files: &mut Vec<std::path::PathBuf>) {
        for entry in fs::read_dir(path).expect("read source directory") {
            let entry = entry.expect("read source entry");
            let path = entry.path();
            if path.is_dir() {
                visit(&path, files);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                files.push(path);
            }
        }
    }

    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    visit(&root, &mut files);
    for path in files {
        let source = fs::read_to_string(&path).expect("read Rust source");
        for forbidden in ["std::net", "TcpStream", "UdpSocket", "ToSocketAddrs"] {
            assert!(
                !source.contains(forbidden),
                "direct network primitive {forbidden:?} found in {}",
                path.display()
            );
        }
    }
}

#[test]
fn workflow_actions_are_pinned_to_full_commits() {
    let workflows = Path::new(env!("CARGO_MANIFEST_DIR")).join(".github/workflows");
    for entry in fs::read_dir(workflows).expect("read workflows") {
        let path = entry.expect("workflow entry").path();
        let text = fs::read_to_string(&path).expect("read workflow");
        for line in text.lines().map(str::trim) {
            let Some(action) = line
                .strip_prefix("- uses: ")
                .or_else(|| line.strip_prefix("uses: "))
            else {
                continue;
            };
            if action.starts_with("./") {
                continue;
            }
            let revision = action
                .rsplit_once('@')
                .map(|(_, revision)| revision)
                .unwrap_or_default();
            assert!(
                revision.len() == 40 && revision.bytes().all(|byte| byte.is_ascii_hexdigit()),
                "workflow action is not pinned to a full commit in {}: {action}",
                path.display()
            );
        }
    }
}
