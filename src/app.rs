//! Terminal setup and the interactive event loop for `revu diff`.

use std::time::Duration;

use anyhow::Result;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use ratatui::DefaultTerminal;

use crate::diff::parse_unified_diff;
use crate::render::render_lines;
use crate::vcs::git::GitAdapter;
use crate::vcs::VcsAdapter;

/// Load the working-tree diff and review it interactively.
pub fn run_diff() -> Result<()> {
    let adapter = GitAdapter::new();
    // Fail fast (and cleanly) before touching the terminal if we're not in a repo.
    adapter.repo_root()?;
    let diff_text = adapter.working_tree_diff()?;
    let model = parse_unified_diff(&diff_text);
    let lines = render_lines(&model);

    // `ratatui::init` installs a panic hook that restores the terminal, so a
    // crash mid-render won't leave the user in a broken alternate screen.
    let mut terminal = ratatui::init();
    let result = run_loop(&mut terminal, &lines);
    ratatui::restore();
    result
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
