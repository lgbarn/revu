//! Command-line interface: the clap command and argument definitions for the
//! `diff`, `show`, `stash`, `difftool`, `pager`, and `patch` subcommands, plus
//! the shared `DisplayFlags` that carry display overrides into config
//! resolution.

use clap::{Args, Parser, Subcommand};

use crate::config::ConfigOverrides;

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

/// Display flags shared by every subcommand that renders a diff interactively.
/// Flattened into each variant so `revu show --theme dracula`, `revu pager
/// --mode split`, etc. all work the same way `revu diff` does. Each is an
/// optional override: unset means "defer to config + saved view-state".
///
/// Each `--flag` / `--no-flag` pair resolves to an `Option<bool>` (unset =>
/// defer to config); `overrides_with` makes the last-specified flag win if both
/// are passed.
#[derive(Args, Default)]
pub struct DisplayFlags {
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

    /// Auto-refresh the diff while editing (overrides config)
    #[arg(long = "live", overrides_with = "no_live")]
    live: bool,
    /// Disable live auto-refresh (overrides config)
    #[arg(long = "no-live", overrides_with = "live")]
    no_live: bool,

    /// Color theme name (e.g. `auto`, `dracula`, `github-dark`)
    #[arg(long)]
    theme: Option<String>,
    /// Layout mode: `auto` (width-responsive), `split`, `stack`/`unified`, or
    /// `vertical` (old block above new block)
    #[arg(long)]
    mode: Option<String>,
}

impl DisplayFlags {
    /// Collapse the parsed flags into the config-override struct the app uses.
    pub fn into_overrides(self) -> ConfigOverrides {
        ConfigOverrides {
            theme: self.theme,
            mode: self.mode,
            line_numbers: flag_pair(self.line_numbers, self.no_line_numbers),
            wrap_lines: flag_pair(self.wrap, self.no_wrap),
            hunk_headers: flag_pair(self.hunk_headers, self.no_hunk_headers),
            live: flag_pair(self.live, self.no_live),
        }
    }
}

/// Collapse a `--flag` / `--no-flag` boolean pair into an `Option<bool>`:
/// `Some(true)`/`Some(false)` when set, `None` when neither was passed (defer to
/// config). clap's `overrides_with` guarantees at most one is true.
fn flag_pair(yes: bool, no: bool) -> Option<bool> {
    if yes {
        Some(true)
    } else if no {
        Some(false)
    } else {
        None
    }
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

        /// Review a GitHub pull request by number, via `gh pr diff <n>`. revu
        /// makes no network call itself — the fetch is delegated to your
        /// already-authenticated `gh`. Ignores the working tree and other flags.
        #[arg(long, value_name = "N")]
        pr: Option<u64>,

        #[command(flatten)]
        display: DisplayFlags,

        /// Limit the review to matching paths, or diff two files when given
        /// exactly two existing paths. Arguments after `--` land here too.
        targets: Vec<String>,
    },

    /// Review a commit (defaults to HEAD)
    Show {
        /// Commit to review (default HEAD)
        reff: Option<String>,
        #[command(flatten)]
        display: DisplayFlags,
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
        #[command(flatten)]
        display: DisplayFlags,
    },

    /// Render a diff piped on stdin; usable as git's core.pager
    Pager {
        #[command(flatten)]
        display: DisplayFlags,
    },

    /// Review a patch file, or a piped diff with `-` (or no argument)
    Patch {
        /// Patch file to review; `-` or omitted reads from stdin
        file: Option<String>,
        #[command(flatten)]
        display: DisplayFlags,
    },
}

#[derive(Subcommand)]
pub enum StashCmd {
    /// Review a stash entry (default latest)
    Show {
        /// Stash entry to review (default stash@{0})
        reff: Option<String>,
        #[command(flatten)]
        display: DisplayFlags,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_diff_pr_number() {
        let cli = Cli::parse_from(["revu", "diff", "--pr", "123"]);
        match cli.command {
            Command::Diff { pr, .. } => assert_eq!(pr, Some(123)),
            _ => panic!("expected Diff"),
        }
        // Absent by default.
        let cli = Cli::parse_from(["revu", "diff"]);
        match cli.command {
            Command::Diff { pr, .. } => assert!(pr.is_none()),
            _ => panic!("expected Diff"),
        }
    }

    #[test]
    fn parses_difftool_with_path() {
        let cli = Cli::parse_from(["revu", "difftool", "LOCAL", "REMOTE", "src/x.rs"]);
        match cli.command {
            Command::Difftool {
                left, right, path, ..
            } => {
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
            Command::Difftool {
                left, right, path, ..
            } => {
                assert_eq!((left.as_str(), right.as_str()), ("LOCAL", "REMOTE"));
                assert!(path.is_none());
            }
            _ => panic!("expected Difftool"),
        }
    }

    #[test]
    fn show_accepts_display_flags() {
        let cli = Cli::parse_from([
            "revu",
            "show",
            "HEAD",
            "--theme",
            "dracula",
            "--mode",
            "split",
            "--no-line-numbers",
        ]);
        match cli.command {
            Command::Show { reff, display } => {
                assert_eq!(reff.as_deref(), Some("HEAD"));
                let ov = display.into_overrides();
                assert_eq!(ov.theme.as_deref(), Some("dracula"));
                assert_eq!(ov.mode.as_deref(), Some("split"));
                assert_eq!(ov.line_numbers, Some(false));
            }
            _ => panic!("expected Show"),
        }
    }

    #[test]
    fn live_flags_resolve_to_override() {
        // --no-live -> Some(false).
        let cli = Cli::parse_from(["revu", "diff", "--no-live"]);
        match cli.command {
            Command::Diff { display, .. } => {
                assert_eq!(display.into_overrides().live, Some(false));
            }
            _ => panic!("expected Diff"),
        }
        // --live -> Some(true).
        let cli = Cli::parse_from(["revu", "diff", "--live"]);
        match cli.command {
            Command::Diff { display, .. } => {
                assert_eq!(display.into_overrides().live, Some(true));
            }
            _ => panic!("expected Diff"),
        }
        // Neither -> None (defer to config).
        let cli = Cli::parse_from(["revu", "diff"]);
        match cli.command {
            Command::Diff { display, .. } => assert_eq!(display.into_overrides().live, None),
            _ => panic!("expected Diff"),
        }
    }

    #[test]
    fn pager_parses_with_and_without_flags() {
        // Bare `revu pager` (how git invokes it) must still parse.
        assert!(matches!(
            Cli::parse_from(["revu", "pager"]).command,
            Command::Pager { .. }
        ));
        let cli = Cli::parse_from(["revu", "pager", "--theme", "nord"]);
        match cli.command {
            Command::Pager { display } => {
                assert_eq!(display.into_overrides().theme.as_deref(), Some("nord"));
            }
            _ => panic!("expected Pager"),
        }
    }
}
