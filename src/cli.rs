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

        /// Color theme name (parsed/carried; applied in a later milestone)
        #[arg(long)]
        theme: Option<String>,
        /// Layout mode (parsed/carried; applied in a later milestone)
        #[arg(long)]
        mode: Option<String>,

        /// Limit the review to matching paths, or diff two files when given
        /// exactly two existing paths. Arguments after `--` land here too.
        targets: Vec<String>,
    },

    /// Render a diff piped on stdin; usable as git's core.pager
    Pager,

    /// Review a patch file, or a piped diff with `-` (or no argument)
    Patch {
        /// Patch file to review; `-` or omitted reads from stdin
        file: Option<String>,
    },
}
