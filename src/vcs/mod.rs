//! Version-control access behind a neutral adapter trait.
//!
//! Only git is implemented today, but every review operation the UI needs is
//! expressed through [`VcsAdapter`] so that jj/sl adapters can be added later
//! without touching callers.

pub mod git;

use std::path::PathBuf;

use anyhow::Result;

/// How a working-tree (or index) diff should be scoped.
#[derive(Debug, Clone, Default)]
pub struct DiffOptions {
    /// Diff the index against HEAD (staged changes) instead of the working tree.
    pub staged: bool,
    /// Restrict the diff to these pathspecs. Empty means no path filter.
    pub paths: Vec<String>,
    /// Include untracked files in the working-tree diff. Ignored when `staged`.
    pub include_untracked: bool,
}

/// Neutral review operations the UI depends on, independent of the underlying
/// version-control system.
pub trait VcsAdapter {
    /// Absolute path to the repository root. Errors if the working directory is
    /// not inside a repository.
    fn repo_root(&self) -> Result<PathBuf>;

    /// The unified diff selected by `opts`, as the VCS would print it (plain
    /// text, no color).
    fn diff(&self, opts: &DiffOptions) -> Result<String>;

    /// The unified diff between two arbitrary files, which need not be inside a
    /// repository.
    fn diff_files(&self, left: &str, right: &str) -> Result<String>;
}
