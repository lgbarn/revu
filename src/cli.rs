use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "revu",
    version,
    about = "Terminal diff/review tool (a Rust port of hunk)"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Review the working-tree diff
    Diff {
        /// Show only staged changes (alias: --cached)
        #[arg(long, visible_alias = "cached")]
        staged: bool,

        /// Omit untracked files from the working-tree review
        #[arg(long)]
        exclude_untracked: bool,

        // Display overrides. Each is a `--flag` / `--no-flag` pair resolving to
        // an `Option<bool>` (unset => defer to config). `overrides_with` makes
        // the last-specified flag win if both are passed.
        /// Show the line-number gutter (overrides config)
        #[arg(long = "line-numbers", overrides_with = "no_line_numbers")]
        line_numbers: bool,
        /// Hide the line-number gutter (overrides config)
        #[arg(long = "no-line-numbers", overrides_with = "line_numbers")]
        no_line_numbers: bool,

        /// Wrap long lines (overrides config)
        #[arg(long = "wrap", overrides_with = "no_wrap")]
        wrap: bool,
        /// Truncate long lines (overrides config)
        #[arg(long = "no-wrap", overrides_with = "wrap")]
        no_wrap: bool,

        /// Show `@@` hunk headers (overrides config)
        #[arg(long = "hunk-headers", overrides_with = "no_hunk_headers")]
        hunk_headers: bool,
        /// Hide `@@` hunk headers (overrides config)
        #[arg(long = "no-hunk-headers", overrides_with = "hunk_headers")]
        no_hunk_headers: bool,

        /// Color theme name (e.g. `auto`, `dracula`, `github-dark`)
        #[arg(long)]
        theme: Option<String>,
        /// Layout mode (parsed/carried; applied in a later milestone)
        #[arg(long)]
        mode: Option<String>,

        /// Limit the review to matching paths, or diff two files when given
        /// exactly two existing paths. Arguments after `--` land here too.
        targets: Vec<String>,
    },

    /// Review a commit (defaults to HEAD)
    Show {
        /// Commit to review (default HEAD)
        reff: Option<String>,
    },

    /// Review stash entries
    Stash {
        #[command(subcommand)]
        cmd: StashCmd,
    },

    /// Diff two files, as invoked by `git difftool`
    ///
    /// `git difftool` runs the configured tool with the LOCAL and REMOTE temp
    /// file paths (and optionally the in-repo path). This renders that file
    /// pair via `git diff --no-index`, so no repository is required.
    Difftool {
        /// Left (LOCAL) file path
        left: String,
        /// Right (REMOTE) file path
        right: String,
        /// In-repo path git difftool passes (informational in v1)
        path: Option<String>,
    },

    /// Render a diff piped on stdin; usable as git's core.pager
    Pager,

    /// Review a patch file, or a piped diff with `-` (or no argument)
    Patch {
        /// Patch file to review; `-` or omitted reads from stdin
        file: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum StashCmd {
    /// Review a stash entry (default latest)
    Show {
        /// Stash entry to review (default stash@{0})
        reff: Option<String>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_difftool_with_path() {
        let cli = Cli::parse_from(["revu", "difftool", "LOCAL", "REMOTE", "src/x.rs"]);
        match cli.command {
            Command::Difftool { left, right, path } => {
                assert_eq!(left, "LOCAL");
                assert_eq!(right, "REMOTE");
                assert_eq!(path.as_deref(), Some("src/x.rs"));
            }
            _ => panic!("expected Difftool"),
        }
    }

    #[test]
    fn parses_difftool_path_optional() {
        let cli = Cli::parse_from(["revu", "difftool", "LOCAL", "REMOTE"]);
        match cli.command {
            Command::Difftool { left, right, path } => {
                assert_eq!((left.as_str(), right.as_str()), ("LOCAL", "REMOTE"));
                assert!(path.is_none());
            }
            _ => panic!("expected Difftool"),
        }
    }
}
