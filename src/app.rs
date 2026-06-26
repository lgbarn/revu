//! Terminal setup and the interactive review loop shared by `diff`, `pager`,
//! and `patch` (via [`review_text`]).

#[cfg(unix)]
use std::io::IsTerminal;
use std::time::Duration;

use anyhow::Result;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::{DefaultTerminal, Frame};

use crate::config::{Config, ConfigOverrides};
use crate::diff::{parse_unified_diff, DiffModel};
use crate::highlight::Highlighter;
use crate::render::{render_lines, RenderOptions};
use crate::state::ViewState;
use crate::vcs::git::GitAdapter;
use crate::vcs::{DiffOptions, VcsAdapter};

/// Load the selected diff and review it interactively.
///
/// `revu diff <fileA> <fileB>` (two existing paths) diffs those files directly
/// and does not require a repository. Otherwise `targets` are treated as a path
/// filter on the working-tree (or staged) diff. `overrides` carries the CLI
/// display flags into config resolution.
pub fn run_diff(
    staged: bool,
    exclude_untracked: bool,
    targets: Vec<String>,
    overrides: ConfigOverrides,
) -> Result<()> {
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
    review_text(&diff_text, &overrides)
}

/// Parse unified diff text and review it interactively. Shared by `diff`,
/// `pager`, and `patch` so there is a single render-loop path. `overrides`
/// are the CLI display flags (empty for `pager`/`patch`).
pub fn review_text(diff_text: &str, overrides: &ConfigOverrides) -> Result<()> {
    let model = parse_unified_diff(diff_text);
    // One highlighter for the whole review: building the syntax/theme sets is
    // expensive, so it is created once here (the shared render path).
    let highlighter = Highlighter::new();

    // Resolve config first (this can fail cleanly on malformed TOML, before the
    // terminal is touched). It seeds the initial toggles; a saved state.json
    // then overrides them if one exists (last-session-wins precedence).
    let config = Config::load(overrides)?;
    let mut opts = RenderOptions {
        line_numbers: config.line_numbers,
        hunk_headers: config.hunk_headers,
        // No config key for context collapse — it is a view-state-only toggle.
        context_collapsed: false,
    };
    let mut wrap = config.wrap_lines;
    if let Some(state) = ViewState::load() {
        opts.line_numbers = state.line_numbers;
        opts.hunk_headers = state.hunk_headers;
        opts.context_collapsed = state.context_collapsed;
        wrap = state.wrap_lines;
    }

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
    let result = run_loop(&mut terminal, &model, &highlighter, opts, wrap);
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

/// The interactive review loop. Owns the diff model + highlighter + render
/// options so display toggles can RE-RENDER the lines in place (rather than
/// re-running the whole pipeline). Persists the final toggle state to
/// `state.json` on quit.
fn run_loop(
    terminal: &mut DefaultTerminal,
    model: &DiffModel,
    highlighter: &Highlighter,
    mut opts: RenderOptions,
    mut wrap: bool,
) -> Result<()> {
    let file_count = model.files.len();
    let mut lines = render_lines(model, highlighter, &opts);
    let mut offset: u16 = 0;
    let mut show_help = false;

    loop {
        // Clamp rather than `as u16` so a diff over 65535 lines saturates
        // instead of silently wrapping the scroll bound.
        let total = lines.len().min(u16::MAX as usize) as u16;
        let mut view_h = 0u16;
        terminal.draw(|frame| {
            let area = frame.area();
            // Split off a 1-line status bar at the bottom; the rest is the diff.
            let chunks = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(area);
            let body = chunks[0];
            view_h = body.height;

            // ponytail: clone per frame is wasteful for huge diffs; revisit if
            // it ever shows up in a profile. Fine for interactive review sizes.
            let mut paragraph = Paragraph::new(lines.to_vec()).scroll((offset, 0));
            if wrap {
                // `trim: false` keeps leading indentation when wrapping.
                paragraph = paragraph.wrap(Wrap { trim: false });
            }
            frame.render_widget(paragraph, body);

            let max_off = total.saturating_sub(body.height);
            frame.render_widget(
                status_bar(file_count, offset, max_off, &opts, wrap),
                chunks[1],
            );

            if show_help {
                render_help(frame, area);
            }
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
                KeyCode::Char('q') => break,
                // Esc closes help if open, otherwise quits.
                KeyCode::Esc => {
                    if show_help {
                        show_help = false;
                    } else {
                        break;
                    }
                }
                KeyCode::Char('?') => show_help = !show_help,
                KeyCode::Down | KeyCode::Char('j') => offset = (offset + 1).min(max_offset),
                KeyCode::Up | KeyCode::Char('k') => offset = offset.saturating_sub(1),
                KeyCode::PageDown | KeyCode::Char(' ') => offset = (offset + page).min(max_offset),
                KeyCode::PageUp => offset = offset.saturating_sub(page),
                KeyCode::Home | KeyCode::Char('g') => offset = 0,
                KeyCode::End | KeyCode::Char('G') => offset = max_offset,
                // Display toggles: flip the option and re-render the lines.
                KeyCode::Char('n') => {
                    opts.line_numbers = !opts.line_numbers;
                    lines = render_lines(model, highlighter, &opts);
                }
                KeyCode::Char('w') => wrap = !wrap,
                KeyCode::Char('H') => {
                    opts.hunk_headers = !opts.hunk_headers;
                    lines = render_lines(model, highlighter, &opts);
                }
                KeyCode::Char('c') => {
                    opts.context_collapsed = !opts.context_collapsed;
                    lines = render_lines(model, highlighter, &opts);
                }
                _ => {}
            }
        }
    }

    // Persist the toggle state so the next run reopens with the same view.
    // Best-effort: a write failure must not fail the (already-finished) review.
    let _ = ViewState {
        line_numbers: opts.line_numbers,
        wrap_lines: wrap,
        hunk_headers: opts.hunk_headers,
        context_collapsed: opts.context_collapsed,
    }
    .save();
    Ok(())
}

/// The 1-line status bar: file count, scroll position, active toggles, and a
/// pointer to the help dialog.
fn status_bar(
    file_count: usize,
    offset: u16,
    max_offset: u16,
    opts: &RenderOptions,
    wrap: bool,
) -> Paragraph<'static> {
    let pct = if max_offset == 0 {
        100
    } else {
        (offset as usize * 100 / max_offset as usize).min(100)
    };

    // Show only the toggles that are currently ON, mirroring the help labels.
    let mut active: Vec<&str> = Vec::new();
    if opts.line_numbers {
        active.push("LN");
    }
    if wrap {
        active.push("WRAP");
    }
    if opts.hunk_headers {
        active.push("HDR");
    }
    if opts.context_collapsed {
        active.push("COLLAPSE");
    }
    let toggles = if active.is_empty() {
        "-".to_string()
    } else {
        active.join(" ")
    };

    let files = if file_count == 1 { "file" } else { "files" };
    let text = format!(" {file_count} {files}  {pct}%  [{toggles}]  ? help ");
    Paragraph::new(text).style(
        Style::default()
            .fg(Color::Black)
            .bg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    )
}

/// Render the centered, bordered help overlay listing every keybinding.
fn render_help(frame: &mut Frame, area: Rect) {
    let help = [
        "  Keybindings",
        "",
        "  j / Down      scroll down",
        "  k / Up        scroll up",
        "  Space / PgDn  page down",
        "  PgUp          page up",
        "  g / Home      jump to top",
        "  G / End       jump to bottom",
        "",
        "  n   toggle line numbers",
        "  w   toggle line wrap",
        "  H   toggle hunk headers",
        "  c   toggle context collapse",
        "",
        "  ?   toggle this help",
        "  q   quit   (Esc closes help)",
    ];
    let lines: Vec<Line<'static>> = help.iter().map(|s| Line::from(s.to_string())).collect();

    // Size the box to its content, clamped to the available area.
    let width = 38.min(area.width);
    let height = (lines.len() as u16 + 2).min(area.height);
    let popup = centered_rect(width, height, area);

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" help ")
        .style(Style::default().fg(Color::White).bg(Color::Black));
    // Clear the cells behind the popup so the diff does not bleed through.
    frame.render_widget(Clear, popup);
    frame.render_widget(Paragraph::new(lines).block(block), popup);
}

/// A `width` x `height` rectangle centered within `area`.
fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height) / 2;
    Rect {
        x,
        y,
        width: width.min(area.width),
        height: height.min(area.height),
    }
}
