use std::path::PathBuf;
use std::process::{Command, Output};

use anyhow::{anyhow, Context, Result};

use super::VcsAdapter;

/// Talks to git by shelling out via argument vectors. There is no shell
/// interpolation anywhere: every argument is passed as a discrete vector
/// element, so no input can be interpreted as a shell command.
pub struct GitAdapter {
    program: String,
}

impl GitAdapter {
    pub fn new() -> Self {
        Self {
            program: "git".to_string(),
        }
    }

    fn run(&self, args: &[&str]) -> Result<Output> {
        Command::new(&self.program)
            .args(args)
            .output()
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

    fn working_tree_diff(&self) -> Result<String> {
        // `-c color.ui=never` forces plain output regardless of the user's git
        // config, so our own parser/renderer sees clean diff text.
        let out = self.run(&["-c", "color.ui=never", "diff"])?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            return Err(anyhow!("git diff failed: {}", stderr.trim()));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }
}
