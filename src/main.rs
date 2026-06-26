mod app;
mod cli;
mod config;
mod diff;
mod highlight;
mod pager;
mod render;
mod state;
mod vcs;
mod worddiff;

use clap::Parser;

use cli::{Cli, Command, StashCmd};
use config::ConfigOverrides;

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
            line_numbers,
            no_line_numbers,
            wrap,
            no_wrap,
            hunk_headers,
            no_hunk_headers,
            theme,
            mode,
            targets,
        } => {
            let overrides = ConfigOverrides {
                theme,
                mode,
                line_numbers: flag_pair(line_numbers, no_line_numbers),
                wrap_lines: flag_pair(wrap, no_wrap),
                hunk_headers: flag_pair(hunk_headers, no_hunk_headers),
            };
            app::run_diff(staged, exclude_untracked, targets, overrides)
        }
        Command::Show { reff } => app::run_show(reff, ConfigOverrides::default()),
        Command::Stash {
            cmd: StashCmd::Show { reff },
        } => app::run_stash_show(reff, ConfigOverrides::default()),
        Command::Difftool { left, right, path } => {
            app::run_difftool(left, right, path, ConfigOverrides::default())
        }
        Command::Pager => pager::run_pager(),
        Command::Patch { file } => pager::run_patch(file),
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
