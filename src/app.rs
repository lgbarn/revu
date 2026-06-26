//! Terminal setup and the interactive review loop shared by `diff`, `pager`,
//! and `patch` (via [`review_text`]).

#[cfg(unix)]
use std::io::IsTerminal;
use std::time::Duration;

use anyhow::Result;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::{DefaultTerminal, Frame};

use crate::config::{Config, ConfigOverrides};
use crate::diff::{parse_unified_diff_colored, DiffModel};
use crate::highlight::Highlighter;
use crate::render::{file_summaries, render_diff, FileSummary, RenderOptions};
use crate::state::ViewState;
use crate::vcs::git::GitAdapter;
use crate::vcs::{DiffOptions, VcsAdapter};
use crate::worddiff::compute_word_emphasis;

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

/// Review a commit. `reff` defaults to `HEAD`. Reuses the shared [`review_text`]
/// render path. `overrides` carries CLI display flags (currently always
/// default for `show`).
pub fn run_show(reff: Option<String>, overrides: ConfigOverrides) -> Result<()> {
    let adapter = GitAdapter::new();
    // Fail fast (and cleanly) before touching the terminal if not in a repo.
    adapter.repo_root()?;
    let reff = reff.unwrap_or_else(|| "HEAD".to_string());
    let text = adapter.revision_show(&reff)?;
    review_text(&text, &overrides)
}

/// Review a stash entry. `reff` defaults to `stash@{0}` (the latest stash).
/// Reuses the shared [`review_text`] render path. `overrides` carries CLI
/// display flags (currently always default for `stash show`).
pub fn run_stash_show(reff: Option<String>, overrides: ConfigOverrides) -> Result<()> {
    let adapter = GitAdapter::new();
    // Fail fast (and cleanly) before touching the terminal if not in a repo.
    adapter.repo_root()?;
    let reff = reff.unwrap_or_else(|| "stash@{0}".to_string());
    let text = adapter.stash_show(&reff)?;
    review_text(&text, &overrides)
}

/// Parse unified diff text and review it interactively. Shared by `diff`,
/// `pager`, and `patch` so there is a single render-loop path. `overrides`
/// are the CLI display flags (empty for `pager`/`patch`).
pub fn review_text(diff_text: &str, overrides: &ConfigOverrides) -> Result<()> {
    // The colored parser is ANSI-aware (for git's `--color-moved` output) but
    // behaves identically to the plain parser on zero-ANSI input, so it safely
    // handles arbitrary pager/patch stdin too.
    let mut model = parse_unified_diff_colored(diff_text);
    // Fill intra-line word-level emphasis on modified lines before rendering.
    compute_word_emphasis(&mut model);
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

/// Preferred sidebar width in columns (border + content). The sidebar is
/// hidden when the terminal is too narrow to leave room for the main view.
const SIDEBAR_W: u16 = 28;

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
    let initial = render_diff(model, highlighter, &opts);
    let mut lines = initial.lines;
    let mut file_starts = initial.file_starts;
    let summaries = file_summaries(model);
    let mut offset: u16 = 0;
    let mut show_help = false;
    // Sidebar starts visible; selection tracks which file the main view is on.
    let mut sidebar_visible = true;
    let mut selected_file: usize = 0;

    loop {
        // Clamp rather than `as u16` so a diff over 65535 lines saturates
        // instead of silently wrapping the scroll bound.
        let total = lines.len().min(u16::MAX as usize) as u16;
        let view = ReviewFrame {
            lines: &lines,
            summaries: &summaries,
            opts: &opts,
            file_count,
            selected_file,
            offset,
            total,
            wrap,
            sidebar_visible,
            show_help,
        };
        let mut view_h = 0u16;
        terminal.draw(|frame| view_h = draw_review(frame, &view))?;

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
                // hunk-header / context toggles change line counts, so the
                // per-file offsets are refreshed alongside the lines.
                KeyCode::Char('n') => {
                    opts.line_numbers = !opts.line_numbers;
                    let r = render_diff(model, highlighter, &opts);
                    lines = r.lines;
                    file_starts = r.file_starts;
                }
                KeyCode::Char('w') => wrap = !wrap,
                KeyCode::Char('H') => {
                    opts.hunk_headers = !opts.hunk_headers;
                    let r = render_diff(model, highlighter, &opts);
                    lines = r.lines;
                    file_starts = r.file_starts;
                }
                KeyCode::Char('c') => {
                    opts.context_collapsed = !opts.context_collapsed;
                    let r = render_diff(model, highlighter, &opts);
                    lines = r.lines;
                    file_starts = r.file_starts;
                }
                // Sidebar: toggle visibility and navigate between files. Moving
                // selection jumps the main view to that file's start offset.
                KeyCode::Char('s') => sidebar_visible = !sidebar_visible,
                KeyCode::Tab | KeyCode::Char(']') => {
                    if !file_starts.is_empty() {
                        selected_file = (selected_file + 1).min(file_starts.len() - 1);
                        offset = file_to_offset(&file_starts, selected_file, max_offset);
                    }
                }
                KeyCode::BackTab | KeyCode::Char('[') => {
                    if !file_starts.is_empty() {
                        selected_file = selected_file.saturating_sub(1);
                        offset = file_to_offset(&file_starts, selected_file, max_offset);
                    }
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

/// Everything `draw_review` needs to paint one frame. Bundled so the live loop
/// and the snapshot test render through the exact same code path.
struct ReviewFrame<'a> {
    lines: &'a [Line<'static>],
    summaries: &'a [FileSummary],
    opts: &'a RenderOptions,
    file_count: usize,
    selected_file: usize,
    offset: u16,
    /// Total rendered line count (already clamped to `u16::MAX`).
    total: u16,
    wrap: bool,
    sidebar_visible: bool,
    show_help: bool,
}

/// Paint one frame: optional sidebar + scrolled main view + status bar (+ help
/// overlay). Returns the main view height so the caller can clamp the scroll
/// offset for the next iteration.
fn draw_review(frame: &mut Frame, v: &ReviewFrame) -> u16 {
    let area = frame.area();
    // Split off a 1-line status bar at the bottom; the rest is the body.
    let chunks = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(area);
    let body = chunks[0];

    // Split the body into sidebar + main when the sidebar is on AND the terminal
    // is wide enough to leave a usable main pane; otherwise the main view takes
    // the whole body (graceful hide on narrow widths).
    let main = if v.sidebar_visible && body.width >= SIDEBAR_W + 20 {
        let panes =
            Layout::horizontal([Constraint::Length(SIDEBAR_W), Constraint::Min(0)]).split(body);
        frame.render_widget(sidebar(v.summaries, v.selected_file), panes[0]);
        panes[1]
    } else {
        body
    };

    // ponytail: clone per frame is wasteful for huge diffs; revisit if it ever
    // shows up in a profile. Fine for interactive review sizes.
    let mut paragraph = Paragraph::new(v.lines.to_vec()).scroll((v.offset, 0));
    if v.wrap {
        // `trim: false` keeps leading indentation when wrapping.
        paragraph = paragraph.wrap(Wrap { trim: false });
    }
    frame.render_widget(paragraph, main);

    let max_off = v.total.saturating_sub(main.height);
    frame.render_widget(
        status_bar(
            v.file_count,
            v.selected_file,
            v.offset,
            max_off,
            v.opts,
            v.wrap,
        ),
        chunks[1],
    );

    if v.show_help {
        render_help(frame, area);
    }
    main.height
}

/// Map a selected file index to a clamped scroll offset for the main view.
fn file_to_offset(file_starts: &[usize], idx: usize, max_offset: u16) -> u16 {
    let start = file_starts.get(idx).copied().unwrap_or(0);
    (start.min(u16::MAX as usize) as u16).min(max_offset)
}

/// Truncate a path to `max` columns, keeping the tail (the filename is the most
/// useful part) and marking the cut with a leading `..`.
fn truncate_path(path: &str, max: usize) -> String {
    let count = path.chars().count();
    if count <= max {
        return path.to_string();
    }
    if max <= 2 {
        return path.chars().take(max).collect();
    }
    let keep = max - 2;
    let tail: String = path.chars().skip(count - keep).collect();
    format!("..{tail}")
}

/// The file sidebar: a bordered list of changed files, each row showing the
/// (truncated) path with right-aligned `+adds -dels` counts. The selected row
/// is drawn as a reversed/bold bar. Empty diffs render an empty list.
///
/// ponytail: no internal scrolling — if there are more files than rows, the
/// overflow is clipped. Fine for typical review sizes; revisit for huge diffs.
fn sidebar(summaries: &[FileSummary], selected: usize) -> Paragraph<'static> {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" files ")
        .style(Style::default().fg(Color::White));
    // Inner content width = sidebar width minus the two border columns.
    let inner_w = (SIDEBAR_W as usize).saturating_sub(2);

    let mut rows: Vec<Line<'static>> = Vec::new();
    for (i, s) in summaries.iter().enumerate() {
        let counts = format!("+{} -{}", s.additions, s.deletions);
        // Reserve room for the counts (plus a one-column gap); the rest is the
        // path budget.
        let path_budget = inner_w.saturating_sub(counts.len() + 1).max(1);
        let path = truncate_path(&s.path, path_budget);
        let pad = inner_w.saturating_sub(path.chars().count() + counts.len());

        if i == selected {
            // Selected: one reversed+bold span spanning the inner width.
            let text = format!("{path}{}{counts}", " ".repeat(pad));
            rows.push(Line::from(Span::styled(
                text,
                Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD),
            )));
        } else {
            rows.push(Line::from(vec![
                Span::raw(path),
                Span::raw(" ".repeat(pad)),
                Span::styled(
                    format!("+{}", s.additions),
                    Style::default().fg(Color::Green),
                ),
                Span::raw(" "),
                Span::styled(format!("-{}", s.deletions), Style::default().fg(Color::Red)),
            ]));
        }
    }

    Paragraph::new(rows).block(block)
}

/// The 1-line status bar: file position, scroll position, active toggles, and
/// pointers to the sidebar and help dialog.
fn status_bar(
    file_count: usize,
    selected_file: usize,
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

    // 1-based file position; an empty diff shows 0/0.
    let pos = if file_count == 0 {
        0
    } else {
        selected_file + 1
    };
    let text = format!(" file {pos}/{file_count}  {pct}%  [{toggles}]  s sidebar  ? help ");
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
        "  Tab / ]       next file",
        "  Shift-Tab / [ previous file",
        "  s             toggle sidebar",
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::render_diff;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    // A 3-file diff so the sidebar lists multiple entries with varied counts.
    const MULTI_FILE: &str = "\
diff --git a/src/lib.rs b/src/lib.rs
index 1111111..2222222 100644
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,3 +1,4 @@
 fn one() {}
-fn two() {}
+fn two() -> u8 { 2 }
+fn three() {}
 fn end() {}
diff --git a/src/main.rs b/src/main.rs
index 3333333..4444444 100644
--- a/src/main.rs
+++ b/src/main.rs
@@ -1 +1,2 @@
 fn main() {}
+// note
diff --git a/README.md b/README.md
index 5555555..6666666 100644
--- a/README.md
+++ b/README.md
@@ -1,2 +1,1 @@
-old title
-old body
+new title
";

    #[test]
    fn renders_sidebar_and_main_view() {
        let model = parse_unified_diff_colored(MULTI_FILE);
        let highlighter = Highlighter::new();
        let opts = RenderOptions::default();
        let rendered = render_diff(&model, &highlighter, &opts);
        let summaries = file_summaries(&model);

        // Select the second file and scroll the main view to its start.
        let selected_file = 1;
        let offset = rendered.file_starts[selected_file] as u16;
        let view = ReviewFrame {
            lines: &rendered.lines,
            summaries: &summaries,
            opts: &opts,
            file_count: model.files.len(),
            selected_file,
            offset,
            total: rendered.lines.len() as u16,
            wrap: false,
            sidebar_visible: true,
            show_help: false,
        };

        let backend = TestBackend::new(80, 16);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| {
                draw_review(f, &view);
            })
            .unwrap();

        insta::assert_snapshot!(terminal.backend());
    }
}
