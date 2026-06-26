//! `revu pager` and `revu patch` — entrypoints that consume stdin or a file.

use std::fs;
use std::io::{Read, Write};
use std::process::{Command, Stdio};

use anyhow::{Context, Result};

use crate::app;

/// Where a `patch` review should read its input from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatchSource {
    Stdin,
    File(String),
}

/// `revu pager`: render a diff piped on stdin (git's `core.pager`). If the
/// input is not a diff, hand it to a plain-text pager so non-diff `git`
/// output still paginates.
pub fn run_pager() -> Result<()> {
    let mut bytes = Vec::new();
    std::io::stdin()
        .read_to_end(&mut bytes)
        .context("failed to read stdin")?;
    let text = String::from_utf8_lossy(&bytes);

    if looks_like_diff(&text) {
        app::review_text(&text)
    } else {
        spawn_text_pager(&bytes)
    }
}

/// `revu patch [file]`: review a patch file, or a piped diff with `-` / no arg.
pub fn run_patch(file: Option<String>) -> Result<()> {
    let text = match patch_source(file.as_deref()) {
        PatchSource::Stdin => {
            let mut s = String::new();
            std::io::stdin()
                .read_to_string(&mut s)
                .context("failed to read patch from stdin")?;
            s
        }
        PatchSource::File(path) => fs::read_to_string(&path)
            .with_context(|| format!("failed to read patch file `{path}`"))?,
    };
    app::review_text(&text)
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

/// Spawn a plain-text pager and feed it the captured bytes. Uses an argument
/// vector (no shell string), trying `$HUNK_TEXT_PAGER`, then `$PAGER`, then
/// `less -R`.
fn spawn_text_pager(bytes: &[u8]) -> Result<()> {
    let spec = std::env::var("HUNK_TEXT_PAGER")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| std::env::var("PAGER").ok().filter(|s| !s.trim().is_empty()))
        .unwrap_or_else(|| "less -R".to_string());

    // ponytail: whitespace split, not full shell-word parsing. Covers `less -R`
    // and bare program names; quoted args in $PAGER are a later refinement.
    let mut parts = spec.split_whitespace();
    let program = parts.next().unwrap_or("less");
    let args: Vec<&str> = parts.collect();

    let mut child = Command::new(program)
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
}
