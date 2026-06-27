//! Git implementation of the `VcsAdapter` trait: builds and runs `git`
//! subprocesses (always via argument vectors, never a shell string) to produce
//! the diff text that the rest of revu parses and renders.

use std::path::PathBuf;
use std::process::{Command, Output};

use anyhow::{anyhow, Context, Result};

use super::{DiffOptions, VcsAdapter};

/// Talks to git by shelling out via argument vectors. There is no shell
/// interpolation anywhere: every argument is passed as a discrete vector
/// element, so no input can be interpreted as a shell command.
pub struct GitAdapter {
    program: String,
    /// Working directory to run git in. `None` uses the process CWD; tests set
    /// it to a fixture repo via `GitAdapter::in_dir` (test-only).
    cwd: Option<PathBuf>,
}

impl GitAdapter {
    pub fn new() -> Self {
        Self {
            program: "git".to_string(),
            cwd: None,
        }
    }

    /// Like [`GitAdapter::new`], but runs git inside `dir` rather than the
    /// process working directory. Primarily for testing against fixture repos.
    #[cfg(test)]
    pub fn in_dir(dir: impl Into<PathBuf>) -> Self {
        Self {
            program: "git".to_string(),
            cwd: Some(dir.into()),
        }
    }

    fn run(&self, args: &[&str]) -> Result<Output> {
        let mut cmd = Command::new(&self.program);
        cmd.args(args);
        if let Some(cwd) = &self.cwd {
            cmd.current_dir(cwd);
        }
        cmd.output()
            .with_context(|| format!("failed to execute `{}` (is git installed?)", self.program))
    }
}

impl Default for GitAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl VcsAdapter for GitAdapter {
    fn repo_root(&self) -> Result<PathBuf> {
        let out = self.run(&["rev-parse", "--show-toplevel"])?;
        if !out.status.success() {
            return Err(anyhow!(
                "not a git repository (run revu inside a git working tree)"
            ));
        }
        let root = String::from_utf8(out.stdout)
            .context("git printed non-UTF-8 path")?
            .trim()
            .to_string();
        Ok(PathBuf::from(root))
    }

    fn diff(&self, opts: &DiffOptions) -> Result<String> {
        // `-c color.ui=always` forces colored output regardless of the user's
        // git config, and `--color-moved=zebra` annotates moved blocks with the
        // move palette. The ANSI-aware parser (`parse_unified_diff_colored`)
        // recovers clean text and reads the move colors. Untracked files are
        // synthesized plain below (no move detection there — acceptable).
        let mut args: Vec<String> = vec![
            "-c".into(),
            "color.ui=always".into(),
            "diff".into(),
            "--color-moved=zebra".into(),
            // Load the WHOLE file as context (not git's default 3 lines) so the
            // model holds every line; the renderer folds long unchanged runs into
            // collapsible bars, keeping the visible render small (see src/fold.rs).
            "--unified=100000".into(),
        ];
        if opts.staged {
            args.push("--staged".into());
        }
        if !opts.paths.is_empty() {
            args.push("--".into());
            args.extend(opts.paths.iter().cloned());
        }
        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let out = self.run(&arg_refs)?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            return Err(anyhow!("git diff failed: {}", stderr.trim()));
        }
        let mut text = String::from_utf8_lossy(&out.stdout).into_owned();

        // Untracked files never appear in `git diff`; synthesize a diff for each
        // by comparing against /dev/null. Staged-only reviews skip this.
        if opts.include_untracked && !opts.staged {
            for path in self.untracked_files(&opts.paths)? {
                text.push_str(&self.no_index_diff("/dev/null", &path)?);
            }
        }
        Ok(text)
    }

    fn diff_files(&self, left: &str, right: &str) -> Result<String> {
        self.no_index_diff(left, right)
    }

    fn revision_show(&self, reff: &str) -> Result<String> {
        // `-c color.ui=always` + `--color-moved=zebra` mirror `diff` so the
        // ANSI-aware parser and move detection apply identically. git prints
        // commit metadata before the diff; the parser ignores non-hunk header
        // lines, so only the diff renders.
        //
        // ponytail: commit metadata (author/message) is not displayed in v1.
        // Surfacing it (e.g. as a synthetic header pane) is a later enhancement.
        // `--end-of-options` stops a ref that begins with `-` from being parsed
        // as a flag, while still treating it as a revision (unlike `--`, which
        // would force pathspec interpretation).
        let out = self.run(&[
            "-c",
            "color.ui=always",
            "show",
            "--color-moved=zebra",
            // Full-file context (see `diff`): fold long unchanged runs at render.
            "--unified=100000",
            "--end-of-options",
            reff,
        ])?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            return Err(anyhow!("git show failed: {}", stderr.trim()));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    fn stash_show(&self, reff: &str) -> Result<String> {
        let out = self.run(&[
            "-c",
            "color.ui=always",
            "stash",
            "show",
            "-p",
            "--color-moved=zebra",
            // Full-file context (see `diff`): fold long unchanged runs at render.
            "--unified=100000",
            "--end-of-options",
            reff,
        ])?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            return Err(anyhow!("git stash show failed: {}", stderr.trim()));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }
}

impl GitAdapter {
    /// List untracked, non-ignored files, optionally scoped by `paths`. Uses
    /// `-z` (NUL-separated) so paths with spaces/newlines survive intact.
    fn untracked_files(&self, paths: &[String]) -> Result<Vec<String>> {
        let mut args: Vec<String> = vec![
            "ls-files".into(),
            "--others".into(),
            "--exclude-standard".into(),
            "-z".into(),
        ];
        if !paths.is_empty() {
            args.push("--".into());
            args.extend(paths.iter().cloned());
        }
        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let out = self.run(&arg_refs)?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            return Err(anyhow!("git ls-files failed: {}", stderr.trim()));
        }
        Ok(String::from_utf8_lossy(&out.stdout)
            .split('\0')
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect())
    }

    /// Diff two paths with `--no-index`. git exits 1 (not 0) when the files
    /// differ, which is the normal, expected case here, so only a status > 1 or
    /// a spawn failure is treated as an error.
    fn no_index_diff(&self, left: &str, right: &str) -> Result<String> {
        let out = self.run(&[
            "-c",
            "color.ui=never",
            "diff",
            "--no-index",
            // Full-file context (see `diff`): fold long unchanged runs at render.
            "--unified=100000",
            "--",
            left,
            right,
        ])?;
        let code = out.status.code();
        if !out.status.success() && code != Some(1) {
            let stderr = String::from_utf8_lossy(&out.stderr);
            return Err(anyhow!("git diff --no-index failed: {}", stderr.trim()));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }
}

// These tests shell out to real git and synthesize untracked diffs against
// `/dev/null`, so they only run on unix.
#[cfg(all(test, unix))]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::process::Command;

    use tempfile::tempdir;

    use super::*;

    /// Run a git command in `dir`, asserting it succeeds (fixture setup).
    fn git(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(dir)
            .status()
            .expect("failed to spawn git");
        assert!(status.success(), "git {args:?} failed in {dir:?}");
    }

    /// A repo with: a committed file modified but unstaged, a new staged file,
    /// and an untracked file.
    fn fixture_repo() -> tempfile::TempDir {
        let dir = tempdir().expect("tempdir");
        let p = dir.path();
        git(p, &["init", "-q"]);
        git(p, &["config", "user.email", "test@example.com"]);
        git(p, &["config", "user.name", "Test"]);

        fs::write(p.join("committed.txt"), "original\n").unwrap();
        git(p, &["add", "committed.txt"]);
        git(p, &["commit", "-q", "-m", "initial"]);

        // Unstaged modification to the committed file.
        fs::write(p.join("committed.txt"), "original\nUNSTAGED_LINE\n").unwrap();

        // A brand-new file added to the index (staged-only).
        fs::write(p.join("staged.txt"), "STAGED_LINE\n").unwrap();
        git(p, &["add", "staged.txt"]);

        // An untracked file (never added).
        fs::write(p.join("untracked.txt"), "UNTRACKED_LINE\n").unwrap();

        dir
    }

    #[test]
    fn diff_default_includes_unstaged_and_untracked() {
        let dir = fixture_repo();
        let adapter = GitAdapter::in_dir(dir.path());
        let out = adapter
            .diff(&DiffOptions {
                staged: false,
                paths: vec![],
                include_untracked: true,
            })
            .unwrap();
        assert!(
            out.contains("committed.txt"),
            "missing committed.txt: {out}"
        );
        assert!(
            out.contains("UNSTAGED_LINE"),
            "missing unstaged change: {out}"
        );
        assert!(
            out.contains("untracked.txt"),
            "missing untracked file: {out}"
        );
        assert!(
            out.contains("UNTRACKED_LINE"),
            "missing untracked content: {out}"
        );
    }

    #[test]
    fn diff_staged_shows_staged_not_unstaged() {
        let dir = fixture_repo();
        let adapter = GitAdapter::in_dir(dir.path());
        let out = adapter
            .diff(&DiffOptions {
                staged: true,
                paths: vec![],
                include_untracked: true,
            })
            .unwrap();
        assert!(out.contains("staged.txt"), "missing staged file: {out}");
        assert!(out.contains("STAGED_LINE"), "missing staged content: {out}");
        // Unstaged-only change and untracked files must not leak into --staged.
        assert!(
            !out.contains("UNSTAGED_LINE"),
            "staged diff leaked unstaged: {out}"
        );
        assert!(
            !out.contains("UNTRACKED_LINE"),
            "staged diff leaked untracked: {out}"
        );
    }

    #[test]
    fn diff_exclude_untracked_omits_untracked() {
        let dir = fixture_repo();
        let adapter = GitAdapter::in_dir(dir.path());
        let out = adapter
            .diff(&DiffOptions {
                staged: false,
                paths: vec![],
                include_untracked: false,
            })
            .unwrap();
        assert!(
            out.contains("UNSTAGED_LINE"),
            "missing unstaged change: {out}"
        );
        assert!(
            !out.contains("untracked.txt"),
            "untracked not excluded: {out}"
        );
    }

    #[test]
    fn diff_paths_scopes_to_pathspec() {
        let dir = fixture_repo();
        let adapter = GitAdapter::in_dir(dir.path());
        let out = adapter
            .diff(&DiffOptions {
                staged: false,
                paths: vec!["committed.txt".to_string()],
                include_untracked: true,
            })
            .unwrap();
        assert!(out.contains("committed.txt"), "missing scoped file: {out}");
        assert!(
            out.contains("UNSTAGED_LINE"),
            "missing scoped change: {out}"
        );
        // The untracked file is outside the pathspec, so it must be excluded.
        assert!(
            !out.contains("untracked.txt"),
            "pathspec did not scope: {out}"
        );
    }

    #[test]
    fn diff_colored_marks_moved_block_via_real_git() {
        use crate::diff::parse_unified_diff_colored;

        let dir = tempdir().expect("tempdir");
        let p = dir.path();
        git(p, &["init", "-q"]);
        git(p, &["config", "user.email", "test@example.com"]);
        git(p, &["config", "user.name", "Test"]);

        // A header, an 8-line distinctive block, then a footer. Relocating the
        // footer above the block is a move git's `--color-moved=zebra` flags.
        let mut original = String::from("header line one\nheader line two\n");
        for i in 1..=8 {
            original.push_str(&format!("the moved content number {i} here\n"));
        }
        original.push_str("footer line one\nfooter line two\n");
        fs::write(p.join("f.txt"), &original).unwrap();
        git(p, &["add", "f.txt"]);
        git(p, &["commit", "-q", "-m", "init"]);

        let mut moved = String::from("header line one\nheader line two\n");
        moved.push_str("footer line one\nfooter line two\n");
        for i in 1..=8 {
            moved.push_str(&format!("the moved content number {i} here\n"));
        }
        fs::write(p.join("f.txt"), &moved).unwrap();

        let adapter = GitAdapter::in_dir(p);
        let text = adapter
            .diff(&DiffOptions {
                staged: false,
                paths: vec![],
                include_untracked: false,
            })
            .unwrap();
        let model = parse_unified_diff_colored(&text);

        // At least one added/removed line is flagged as moved, and the genuine
        // (non-moved) context lines are not.
        let moved_count = model
            .files
            .iter()
            .flat_map(|f| f.hunks.iter())
            .flat_map(|h| h.lines.iter())
            .filter(|dl| dl.moved)
            .count();
        assert!(
            moved_count > 0,
            "expected git --color-moved to flag a moved line; diff was:\n{text}"
        );
    }

    #[test]
    fn diff_files_compares_arbitrary_files() {
        let dir = tempdir().expect("tempdir");
        let left = dir.path().join("left.txt");
        let right = dir.path().join("right.txt");
        fs::write(&left, "hello\n").unwrap();
        fs::write(&right, "world\n").unwrap();

        // No repo here; diff_files must not require one.
        let adapter = GitAdapter::new();
        let out = adapter
            .diff_files(left.to_str().unwrap(), right.to_str().unwrap())
            .unwrap();
        assert!(out.contains("-hello"), "missing removed line: {out}");
        assert!(out.contains("+world"), "missing added line: {out}");
    }

    #[test]
    fn revision_show_renders_head_commit() {
        let dir = tempdir().expect("tempdir");
        let p = dir.path();
        git(p, &["init", "-q"]);
        git(p, &["config", "user.email", "test@example.com"]);
        git(p, &["config", "user.name", "Test"]);

        fs::write(p.join("shown.txt"), "SHOWN_LINE\n").unwrap();
        git(p, &["add", "shown.txt"]);
        git(p, &["commit", "-q", "-m", "add shown"]);

        let adapter = GitAdapter::in_dir(p);
        let out = adapter.revision_show("HEAD").unwrap();
        assert!(out.contains("shown.txt"), "missing committed file: {out}");
        assert!(out.contains("SHOWN_LINE"), "missing added line: {out}");
    }

    #[test]
    fn revision_show_bad_ref_errors() {
        let dir = tempdir().expect("tempdir");
        let p = dir.path();
        git(p, &["init", "-q"]);
        git(p, &["config", "user.email", "test@example.com"]);
        git(p, &["config", "user.name", "Test"]);
        fs::write(p.join("f.txt"), "x\n").unwrap();
        git(p, &["add", "f.txt"]);
        git(p, &["commit", "-q", "-m", "init"]);

        let adapter = GitAdapter::in_dir(p);
        assert!(adapter.revision_show("does-not-exist-ref").is_err());
    }

    #[test]
    fn stash_show_renders_latest_entry() {
        let dir = tempdir().expect("tempdir");
        let p = dir.path();
        git(p, &["init", "-q"]);
        git(p, &["config", "user.email", "test@example.com"]);
        git(p, &["config", "user.name", "Test"]);

        fs::write(p.join("stashed.txt"), "original\n").unwrap();
        git(p, &["add", "stashed.txt"]);
        git(p, &["commit", "-q", "-m", "init"]);

        // Modify the tracked file, then stash the change.
        fs::write(p.join("stashed.txt"), "original\nSTASHED_LINE\n").unwrap();
        git(p, &["stash", "-q"]);

        let adapter = GitAdapter::in_dir(p);
        let out = adapter.stash_show("stash@{0}").unwrap();
        assert!(out.contains("stashed.txt"), "missing stashed file: {out}");
        assert!(
            out.contains("STASHED_LINE"),
            "missing stashed change: {out}"
        );
    }
}
