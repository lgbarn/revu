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
    /// it to a fixture repo via [`GitAdapter::in_dir`].
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
        // `-c color.ui=never` forces plain output regardless of the user's git
        // config, so our own parser/renderer sees clean diff text.
        let mut args: Vec<String> = vec!["-c".into(), "color.ui=never".into(), "diff".into()];
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
