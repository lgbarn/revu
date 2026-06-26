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
    Diff,

    /// Render a diff piped on stdin; usable as git's core.pager
    Pager,

    /// Review a patch file, or a piped diff with `-` (or no argument)
    Patch {
        /// Patch file to review; `-` or omitted reads from stdin
        file: Option<String>,
    },
}
