//! `revu pager` and `revu patch` — entrypoints that consume stdin or a file.

use std::fs;
use std::io::{Read, Write};
use std::process::{Command, Stdio};

use anyhow::{Context, Result};

use crate::app;
use crate::config::ConfigOverrides;

/// Where a `patch` review should read its input from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatchSource {
    Stdin,
    File(String),
}

/// `revu pager`: render a diff piped on stdin (git's `core.pager`). If the
/// input is not a diff, hand it to a plain-text pager so non-diff `git`
/// output still paginates.
pub fn run_pager(overrides: ConfigOverrides) -> Result<()> {
    let mut bytes = Vec::new();
    std::io::stdin()
        .read_to_end(&mut bytes)
        .context("failed to read stdin")?;
    let text = String::from_utf8_lossy(&bytes);

    if looks_like_diff(&text) {
        // Display flags from the CLI apply to the diff view; config + state.json
        // still layer underneath. Non-diff input goes to a plain pager below.
        // No reload: the diff arrived on stdin and cannot be re-fetched.
        app::review_text(&text, &overrides, None, false)
    } else {
        spawn_text_pager(&bytes)
    }
}

/// `revu patch [file]`: review a patch file, or a piped diff with `-` / no arg.
pub fn run_patch(file: Option<String>, overrides: ConfigOverrides) -> Result<()> {
    match patch_source(file.as_deref()) {
        PatchSource::Stdin => {
            // Piped patch: no reload (stdin can't be re-read).
            let mut s = String::new();
            std::io::stdin()
                .read_to_string(&mut s)
                .context("failed to read patch from stdin")?;
            app::review_text(&s, &overrides, None, false)
        }
        PatchSource::File(path) => {
            // A patch file CAN be re-read, so `r` reloads it from disk.
            let fetch: app::ReloadFn = Box::new(move || {
                fs::read_to_string(&path)
                    .with_context(|| format!("failed to read patch file `{path}`"))
            });
            let text = fetch()?;
            app::review_text(&text, &overrides, Some(fetch), false)
        }
    }
}

/// Decide where `patch` reads from. Pure so it can be tested without touching
/// stdin or the filesystem.
pub fn patch_source(arg: Option<&str>) -> PatchSource {
    match arg {
        None | Some("-") => PatchSource::Stdin,
        Some(path) => PatchSource::File(path.to_string()),
    }
}

/// Heuristic: does this text look like a unified diff? `git diff`/`git show`
/// always emit a `diff --git` header; a hunk header is a strong secondary
/// signal. Plain `git log`/status output has neither.
pub fn looks_like_diff(text: &str) -> bool {
    text.lines()
        .any(|l| l.starts_with("diff --git ") || (l.starts_with("@@ ") && l.contains(" @@")))
}

/// Resolve the plain-text pager command: `$HUNK_TEXT_PAGER`, else `$PAGER`,
/// else `less -R`. The spec is split on whitespace into a program plus its base
/// args. Pure — the env values are passed in, so it never reads the environment
/// and can be tested deterministically (mirrors `app::editor_command`).
///
/// ponytail: whitespace split, not full shell-word parsing. Covers `less -R`
/// and bare program names; quoted args in $PAGER are a later refinement.
fn pager_command(hunk_text_pager: Option<&str>, pager: Option<&str>) -> (String, Vec<String>) {
    let spec = hunk_text_pager
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or_else(|| pager.map(str::trim).filter(|s| !s.is_empty()))
        .unwrap_or("less -R");
    let mut parts = spec.split_whitespace();
    let program = parts.next().unwrap_or("less").to_string();
    let args = parts.map(str::to_string).collect();
    (program, args)
}

/// Spawn a plain-text pager and feed it the captured bytes. Uses an argument
/// vector (no shell string), trying `$HUNK_TEXT_PAGER`, then `$PAGER`, then
/// `less -R`.
fn spawn_text_pager(bytes: &[u8]) -> Result<()> {
    let hunk = std::env::var("HUNK_TEXT_PAGER").ok();
    let pager = std::env::var("PAGER").ok();
    let (program, args) = pager_command(hunk.as_deref(), pager.as_deref());

    let mut child = Command::new(&program)
        .args(&args)
        .stdin(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to spawn pager `{program}`"))?;

    if let Some(mut child_stdin) = child.stdin.take() {
        // Ignore a broken pipe if the pager exits before consuming everything.
        let _ = child_stdin.write_all(bytes);
    }
    child.wait().context("pager process failed")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patch_source_selects_stdin_for_dash_or_none() {
        assert_eq!(patch_source(None), PatchSource::Stdin);
        assert_eq!(patch_source(Some("-")), PatchSource::Stdin);
        assert_eq!(
            patch_source(Some("changes.patch")),
            PatchSource::File("changes.patch".to_string())
        );
    }

    #[test]
    fn detects_git_diff_output() {
        let diff = "diff --git a/x b/x\nindex 1..2 100644\n--- a/x\n+++ b/x\n@@ -1 +1 @@\n-a\n+b\n";
        assert!(looks_like_diff(diff));
    }

    #[test]
    fn detects_git_show_with_commit_header() {
        // git show prints commit metadata before the diff.
        let show = "commit deadbeef\nAuthor: A\nDate: today\n\n    msg\n\ndiff --git a/x b/x\n@@ -1 +1 @@\n-a\n+b\n";
        assert!(looks_like_diff(show));
    }

    #[test]
    fn rejects_non_diff_text() {
        let log = "commit deadbeef\nAuthor: A\nDate: today\n\n    just a log message\n";
        assert!(!looks_like_diff(log));
        assert!(!looks_like_diff("hello world\n"));
        assert!(!looks_like_diff(""));
    }

    #[test]
    fn pager_command_prefers_hunk_text_pager() {
        let (prog, args) = pager_command(Some("bat -p"), Some("less -R"));
        assert_eq!(prog, "bat");
        assert_eq!(args, vec!["-p"]);
    }

    #[test]
    fn pager_command_falls_back_to_pager_then_less() {
        // HUNK_TEXT_PAGER unset/blank -> use PAGER.
        let (prog, args) = pager_command(None, Some("more"));
        assert_eq!((prog.as_str(), args.as_slice()), ("more", &[][..]));
        // Blank values are ignored (treated as unset).
        let (prog, _) = pager_command(Some("   "), Some("more"));
        assert_eq!(prog, "more");
        // Both unset -> default `less -R`.
        let (prog, args) = pager_command(None, None);
        assert_eq!(prog, "less");
        assert_eq!(args, vec!["-R"]);
    }
}
