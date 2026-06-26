//! Version-control access behind a neutral adapter trait.
//!
//! Only git is implemented today, but every review operation the UI needs is
//! expressed through [`VcsAdapter`] so that jj/sl adapters can be added later
//! without touching callers.

pub mod git;

use std::path::PathBuf;

use anyhow::Result;

/// Neutral review operations the UI depends on, independent of the underlying
/// version-control system.
pub trait VcsAdapter {
    /// Absolute path to the repository root. Errors if the working directory is
    /// not inside a repository.
    fn repo_root(&self) -> Result<PathBuf>;

    /// The unified diff of the working tree against the index/HEAD, as the VCS
    /// would print it (plain text, no color).
    fn working_tree_diff(&self) -> Result<String>;
}
