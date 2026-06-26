//! Terminal setup and the interactive review loop shared by `diff`, `pager`,
//! and `patch` (via [`review_text`]).

#[cfg(unix)]
use std::io::IsTerminal;
use std::time::Duration;

use anyhow::Result;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use ratatui::DefaultTerminal;

use crate::diff::parse_unified_diff;
use crate::highlight::Highlighter;
use crate::render::render_lines;
use crate::vcs::git::GitAdapter;
use crate::vcs::{DiffOptions, VcsAdapter};

/// Load the selected diff and review it interactively.
///
/// `revu diff <fileA> <fileB>` (two existing paths) diffs those files directly
/// and does not require a repository. Otherwise `targets` are treated as a path
/// filter on the working-tree (or staged) diff.
pub fn run_diff(staged: bool, exclude_untracked: bool, targets: Vec<String>) -> Result<()> {
    let adapter = GitAdapter::new();

    let diff_text = if targets.len() == 2
        && std::path::Path::new(&targets[0]).exists()
        && std::path::Path::new(&targets[1]).exists()
    {
        // Two-file mode: arbitrary file comparison, no repo required.
        adapter.diff_files(&targets[0], &targets[1])?
    } else {
        // Fail fast (and cleanly) before touching the terminal if not in a repo.
        adapter.repo_root()?;
        adapter.diff(&DiffOptions {
            staged,
            paths: targets,
            include_untracked: !exclude_untracked,
        })?
    };
    review_text(&diff_text)
}

/// Parse unified diff text and review it interactively. Shared by `diff`,
/// `pager`, and `patch` so there is a single render-loop path.
pub fn review_text(diff_text: &str) -> Result<()> {
    let model = parse_unified_diff(diff_text);
    // One highlighter for the whole review: building the syntax/theme sets is
    // expensive, so it is created once here (the shared render path).
    let highlighter = Highlighter::new();
    let lines = render_lines(&model, &highlighter);

    // When the diff arrives on a pipe (e.g. git's pager), stdin is not the
    // terminal, so crossterm has nothing to read key events from. Reopen the
    // controlling tty onto fd 0 before initializing the UI.
    #[cfg(unix)]
    if !std::io::stdin().is_terminal() {
        reopen_controlling_tty()?;
    }

    // `ratatui::init` installs a panic hook that restores the terminal, so a
    // crash mid-render won't leave the user in a broken alternate screen.
    let mut terminal = ratatui::init();
    let result = run_loop(&mut terminal, &lines);
    ratatui::restore();
    result
}

/// Redirect `/dev/tty` onto stdin (fd 0) so the interactive loop can read key
/// events even when the diff was piped in. Uses rustix (no libc).
#[cfg(unix)]
fn reopen_controlling_tty() -> Result<()> {
    use anyhow::Context;
    use std::fs::OpenOptions;

    let tty = OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .context("failed to open /dev/tty for interactive input")?;
    // dup2 makes fd 0 an independent handle on the tty; dropping `tty`
    // afterwards closes only the extra fd, leaving fd 0 valid.
    rustix::stdio::dup2_stdin(&tty).context("failed to redirect /dev/tty onto stdin")?;
    Ok(())
}

fn run_loop(terminal: &mut DefaultTerminal, lines: &[Line<'static>]) -> Result<()> {
    // Clamp rather than `as u16` so a diff over 65535 lines saturates instead
    // of silently wrapping the scroll bound. (Paragraph scroll is u16-bound
    // anyway, so this is the most the simple stack view can address.)
    let total = lines.len().min(u16::MAX as usize) as u16;
    let mut offset: u16 = 0;

    loop {
        let mut view_h = 0u16;
        terminal.draw(|frame| {
            let area = frame.area();
            view_h = area.height;
            // ponytail: clone per frame is wasteful for huge diffs; revisit if
            // it ever shows up in a profile. Fine for interactive review sizes.
            let paragraph = Paragraph::new(lines.to_vec()).scroll((offset, 0));
            frame.render_widget(paragraph, area);
        })?;

        let max_offset = total.saturating_sub(view_h);
        offset = offset.min(max_offset);
        let page = view_h.saturating_sub(2).max(1);

        if !event::poll(Duration::from_millis(250))? {
            continue;
        }
        if let Event::Key(key) = event::read()? {
            if key.kind != KeyEventKind::Press {
                continue;
            }
            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => break,
                KeyCode::Down | KeyCode::Char('j') => offset = (offset + 1).min(max_offset),
                KeyCode::Up | KeyCode::Char('k') => offset = offset.saturating_sub(1),
                KeyCode::PageDown | KeyCode::Char(' ') => offset = (offset + page).min(max_offset),
                KeyCode::PageUp => offset = offset.saturating_sub(page),
                KeyCode::Home | KeyCode::Char('g') => offset = 0,
                KeyCode::End | KeyCode::Char('G') => offset = max_offset,
                _ => {}
            }
        }
    }
    Ok(())
}
