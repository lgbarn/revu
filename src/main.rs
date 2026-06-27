//! revu — a fast, memory-safe terminal diff and code-review tool (a Rust port
//! of hunk).
//!
//! `main` parses the CLI (`cli`) and dispatches each subcommand to an
//! `app::run_*` entry point, which loads the diff, parses it (`diff`), and
//! renders it interactively (`render`).

mod app;
mod cli;
mod config;
mod diff;
mod fold;
mod highlight;
mod pager;
mod render;
mod state;
mod theme;
mod vcs;
mod worddiff;

use clap::Parser;

use cli::{Cli, Command, StashCmd};

fn main() {
    if let Err(e) = run() {
        // Clean single-line error to stderr, non-zero exit. `{e:#}` includes the
        // anyhow context chain (e.g. "not a git repository").
        eprintln!("revu: {e:#}");
        std::process::exit(1);
    }
}

fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Diff {
            staged,
            exclude_untracked,
            display,
            targets,
        } => app::run_diff(staged, exclude_untracked, targets, display.into_overrides()),
        Command::Show { reff, display } => app::run_show(reff, display.into_overrides()),
        Command::Stash {
            cmd: StashCmd::Show { reff, display },
        } => app::run_stash_show(reff, display.into_overrides()),
        Command::Difftool {
            left,
            right,
            path,
            display,
        } => app::run_difftool(left, right, path, display.into_overrides()),
        Command::Pager { display } => pager::run_pager(display.into_overrides()),
        Command::Patch { file, display } => pager::run_patch(file, display.into_overrides()),
    }
}
