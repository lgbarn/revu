mod app;
mod cli;
mod diff;
mod pager;
mod render;
mod vcs;

use clap::Parser;

use cli::{Cli, Command};

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
        Command::Diff => app::run_diff(),
        Command::Pager => pager::run_pager(),
        Command::Patch { file } => pager::run_patch(file),
    }
}
