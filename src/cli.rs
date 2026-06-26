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
