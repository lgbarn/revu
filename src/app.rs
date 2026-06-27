//! Terminal setup and the interactive review loop shared by `diff`, `pager`,
//! and `patch` (via [`review_text`]).

use std::collections::HashSet;
#[cfg(unix)]
use std::io::IsTerminal;
use std::time::Duration;

use anyhow::Result;
use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
    MouseEventKind,
};
use ratatui::crossterm::execute;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::{DefaultTerminal, Frame};

use crate::config::{Config, ConfigOverrides};
use crate::diff::{parse_unified_diff_colored, DiffModel};
use crate::fold::{fold_at_cursor, FoldId};
use crate::highlight::Highlighter;
use crate::render::{
    file_at_offset, file_summaries, render_diff, FileSummary, LayoutMode, RenderOptions,
};
use crate::search::{Match, Search};
use crate::state::ViewState;
use crate::theme::{self, resolve_theme, Theme};
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
    pr: Option<u64>,
    targets: Vec<String>,
    overrides: ConfigOverrides,
) -> Result<()> {
    // `--pr <n>` reviews a GitHub PR by delegating the fetch to the user's `gh`
    // (revu itself makes no network call). It supersedes the working-tree paths.
    if let Some(number) = pr {
        let diff_text = gh_pr_diff(number)?;
        return review_text(&diff_text, &overrides);
    }

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

/// The argv (after the `gh` program name) that fetches PR `number`'s diff.
/// Split out as a pure function so it is unit-testable without invoking `gh`,
/// mirroring the git-argv tests in `vcs::git`.
fn gh_pr_diff_args(number: u64) -> Vec<String> {
    vec!["pr".to_string(), "diff".to_string(), number.to_string()]
}

/// Fetch a GitHub pull request's diff by shelling out to `gh pr diff <n>`. revu
/// makes no network call itself — `gh` does, using the user's existing auth.
/// Surfaces a clear, actionable error when `gh` is missing or the command fails
/// (not installed, not a GitHub repo, unknown PR, not authenticated).
fn gh_pr_diff(number: u64) -> Result<String> {
    use anyhow::{bail, Context};

    let output = std::process::Command::new("gh")
        .args(gh_pr_diff_args(number))
        .output()
        .context("could not run `gh` — install the GitHub CLI and ensure it is on PATH")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = stderr.trim();
        if detail.is_empty() {
            bail!("`gh pr diff {number}` failed (exit {})", output.status);
        }
        bail!("`gh pr diff {number}` failed: {detail}");
    }
    String::from_utf8(output.stdout).context("`gh pr diff` produced non-UTF-8 output")
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

/// `revu difftool <left> <right> [path]`: render the diff between two file
/// paths, as invoked by `git difftool` (which passes the LOCAL and REMOTE temp
/// files, optionally the in-repo path). Uses `git diff --no-index`, so it does
/// NOT require a repository (no `repo_root` call).
///
/// ponytail: `path` is accepted for `git difftool` compatibility but unused in
/// v1. Syntax highlighting already derives the language from the diff's file
/// headers (which `--no-index` populates with the real paths), so the extra
/// hint is redundant. Wire it into highlight selection if that ever changes.
pub fn run_difftool(
    left: String,
    right: String,
    _path: Option<String>,
    overrides: ConfigOverrides,
) -> Result<()> {
    let text = GitAdapter::new().diff_files(&left, &right)?;
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

    // Resolve config first (this can fail cleanly on malformed TOML, before the
    // terminal is touched). It seeds the initial toggles; a saved state.json
    // then overrides them if one exists (last-session-wins precedence).
    let config = Config::load(overrides)?;

    // Resolve the active theme from config + the detected terminal background.
    // This too fails cleanly (unknown theme name, invalid custom-theme hex)
    // before the terminal is touched. The highlighter is then built on the
    // theme's bundled syntect theme; rebuilding the syntax/theme sets is
    // expensive, so it is created once here and swapped only on a live theme
    // change.
    let theme = resolve_theme(&config, theme::terminal_is_dark())?;
    // Honor `transparent_background`: drop the add/remove row tints so the
    // terminal's own background shows through (foreground +/- colors remain).
    let theme = if config.transparent_background {
        theme.into_transparent()
    } else {
        theme
    };
    let highlighter = Highlighter::with_theme(&theme.syntect_theme, &theme.syntax_overrides);
    let mut opts = RenderOptions {
        line_numbers: config.line_numbers,
        hunk_headers: config.hunk_headers,
        // No config key for context collapse — it is a view-state-only toggle.
        context_collapsed: false,
        // The effective layout is resolved per frame in `run_loop` from the
        // config `mode` string + terminal width; this seed is overwritten there.
        mode: LayoutMode::Stack,
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
    let mut terminal = init_review_terminal();
    let result = run_loop(
        &mut terminal,
        &model,
        highlighter,
        theme,
        opts,
        wrap,
        config.mode,
    );
    restore_review_terminal();
    result
}

/// Enter the review UI: ratatui's alternate screen + raw mode, plus mouse
/// capture so the wheel scrolls the diff. Capture routes mouse events to the
/// app, so the terminal's own click-drag text selection is suspended while
/// reviewing (most terminals still select on Shift+drag).
///
/// ponytail: the panic hook ratatui installs restores the screen but not mouse
/// capture, so a panic can leave capture on. Acceptable — a crash is rare and
/// the next program's init resets it; wire a custom hook only if it bites.
fn init_review_terminal() -> DefaultTerminal {
    let terminal = ratatui::init();
    // Best-effort: a terminal that rejects mouse capture just won't wheel-scroll;
    // the keyboard path is unaffected.
    let _ = execute!(std::io::stdout(), EnableMouseCapture);
    terminal
}

/// Leave the review UI: disable mouse capture, then restore the normal screen
/// and cooked mode, handing the terminal back clean to the shell, `$EDITOR`, or
/// the exiting process. Mirrors [`init_review_terminal`].
fn restore_review_terminal() {
    let _ = execute!(std::io::stdout(), DisableMouseCapture);
    ratatui::restore();
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

/// Minimum main-content width (columns) at which `--mode auto` switches from the
/// unified (stack) view to the side-by-side (split) view. Below this a split
/// would give each side ~55 cols or fewer, which is too cramped for code; above
/// it, two columns read comfortably.
const AUTO_SPLIT_MIN: u16 = 120;

/// Resolve the configured mode string + current main-content width into the
/// concrete layout the renderer needs. `auto` (and any unrecognized value)
/// picks split once the width reaches [`AUTO_SPLIT_MIN`], else stack — this is
/// re-evaluated every frame so a terminal resize flips the layout live.
fn effective_mode(mode: &str, width: u16) -> LayoutMode {
    match mode {
        "split" => LayoutMode::Split,
        "stack" | "unified" => LayoutMode::Stack,
        _ => {
            if width >= AUTO_SPLIT_MIN {
                LayoutMode::Split
            } else {
                LayoutMode::Stack
            }
        }
    }
}

/// The main-content width for a given terminal width: the full width, minus the
/// sidebar when it is visible and there is room for it (mirrors the split in
/// [`draw_review`]). Used to size split columns and drive `auto` selection.
fn main_content_width(term_width: u16, sidebar_visible: bool) -> u16 {
    if sidebar_visible && term_width >= SIDEBAR_W + 20 {
        term_width - SIDEBAR_W
    } else {
        term_width
    }
}

/// The interactive review loop. Owns the diff model + highlighter + render
/// options so display toggles can RE-RENDER the lines in place (rather than
/// re-running the whole pipeline). Persists the final toggle state to
/// `state.json` on quit.
fn run_loop(
    terminal: &mut DefaultTerminal,
    model: &DiffModel,
    mut highlighter: Highlighter,
    mut theme: Theme,
    mut opts: RenderOptions,
    mut wrap: bool,
    mut mode: String,
) -> Result<()> {
    let file_count = model.files.len();
    // The curated catalog the theme selector cycles through. The active theme's
    // index seeds the selector cursor (or 0 when a custom theme isn't in it).
    let catalog = theme::catalog();
    let mut show_theme_selector = false;
    let mut theme_cursor = catalog
        .iter()
        .position(|t| t.name == theme.name)
        .unwrap_or(0);
    // Rendered lines are (re)built lazily inside the loop: the first iteration
    // always renders, and thereafter only when the effective layout mode, the
    // split column width, or a display toggle changes.
    let mut lines: Vec<Line<'static>> = Vec::new();
    // Plain text of each rendered line, rebuilt alongside `lines`, for search.
    let mut line_texts: Vec<String> = Vec::new();
    let mut file_starts: Vec<usize> = Vec::new();
    // Row where each hunk begins, for `{`/`}` navigation; rebuilt with `lines`.
    let mut hunk_starts: Vec<usize> = Vec::new();
    // Row -> FoldId for the fold bars in the current render, ascending by row.
    // Rebuilt every render alongside `lines`; the `o`/Enter toggle reads it.
    let mut fold_bars: Vec<(usize, FoldId)> = Vec::new();
    // The folds the user has expanded. Empty = every fold collapsed (the default
    // view). A render toggle (like the display toggles) re-renders when it changes.
    let mut expanded_folds: HashSet<FoldId> = HashSet::new();
    let summaries = file_summaries(model);
    let mut offset: u16 = 0;
    let mut show_help = false;
    // Sidebar starts visible. The active file is DERIVED from the scroll offset
    // every frame (see `file_at_offset`), so plain scrolling moves through files
    // and the sidebar/status follow automatically — there is no separately-held
    // selection to drift out of sync.
    let mut sidebar_visible = true;
    // Render cache: the mode/width the current `lines` were built for, plus a
    // dirty flag set by display toggles. `None` mode forces the first render.
    let mut cur_mode: Option<LayoutMode> = None;
    let mut cur_width: u16 = 0;
    let mut needs_render = true;
    // Horizontal scroll offset (columns) for the left/right arrows when not
    // wrapping. ponytail: a fixed cap and step, not the measured longest line —
    // far cheaper and indistinguishable in use; revisit if a diff needs more.
    let mut h_offset: u16 = 0;
    const H_STEP: u16 = 8;
    const H_MAX: u16 = 512;
    // Active search over the rendered lines (matches + cursor), or `None`. While
    // `search_input` is `Some`, the user is live-typing a query in the prompt;
    // each keystroke recomputes `search` so highlights and the counter update.
    let mut search: Option<Search> = None;
    let mut search_input: Option<String> = None;

    loop {
        // Resolve the effective layout from the configured mode + live width so a
        // resize (which changes the width here) re-evaluates `auto` and split
        // column sizing without any extra event.
        let term_width = terminal.size()?.width;
        let main_width = main_content_width(term_width, sidebar_visible);
        let eff_mode = effective_mode(&mode, main_width);
        // Rebuild lines only when something that affects them changed: a toggle
        // (needs_render), the layout mode, or — for split — the column width.
        if needs_render
            || cur_mode != Some(eff_mode)
            || (eff_mode == LayoutMode::Split && main_width != cur_width)
        {
            opts.mode = eff_mode;
            let r = render_diff(
                model,
                &highlighter,
                &theme,
                &opts,
                &expanded_folds,
                main_width,
            );
            lines = r.lines;
            line_texts = lines.iter().map(line_plain_text).collect();
            file_starts = r.file_starts;
            hunk_starts = r.hunk_starts;
            fold_bars = r.fold_bars;
            cur_mode = Some(eff_mode);
            cur_width = main_width;
            needs_render = false;
            // A re-render can shift line indices (folds, toggled gutters), so
            // recompute an active search against the new text to keep matches
            // valid. Cursor resets to the first match — acceptable on a toggle.
            if let Some(s) = &search {
                search = Some(Search::new(s.query.clone(), &line_texts));
            }
        }

        // Clamp rather than `as u16` so a diff over 65535 lines saturates
        // instead of silently wrapping the scroll bound.
        let total = lines.len().min(u16::MAX as usize) as u16;
        // Derive the active file from the current scroll offset so the sidebar
        // highlight and the "file X/Y" status follow scrolling for free.
        let active_file = file_at_offset(&file_starts, offset as usize);
        // Status line for the search prompt / counter, shown in place of the
        // normal status bar while typing a query or with a search active.
        let search_status = search_status_text(search_input.as_deref(), search.as_ref());
        let (search_matches, search_current): (&[Match], Option<Match>) = match &search {
            Some(s) => (&s.matches, s.current_match()),
            None => (&[], None),
        };
        let view = ReviewFrame {
            lines: &lines,
            summaries: &summaries,
            opts: &opts,
            theme: &theme,
            file_count,
            selected_file: active_file,
            offset,
            h_offset,
            total,
            wrap,
            sidebar_visible,
            show_help,
            show_theme_selector,
            catalog: &catalog,
            theme_cursor,
            search_matches,
            search_current,
            search_status,
        };
        let mut view_h = 0u16;
        terminal.draw(|frame| view_h = draw_review(frame, &view))?;

        let max_offset = total.saturating_sub(view_h);
        offset = offset.min(max_offset);
        let page = view_h.saturating_sub(2).max(1);

        if !event::poll(Duration::from_millis(250))? {
            continue;
        }
        let ev = event::read()?;
        // Mouse wheel scrolls the diff vertically (capture is enabled in
        // `init_review_terminal`). Other mouse events are ignored for now. Like
        // the keyboard scroll keys, the wheel is inert while an overlay (theme
        // selector / search prompt) is open, so it doesn't scroll underneath it.
        if let Event::Mouse(me) = &ev {
            if !show_theme_selector && search_input.is_none() {
                if let Some(new_offset) = wheel_scroll(me.kind, offset, WHEEL_LINES, max_offset) {
                    offset = new_offset;
                }
            }
            continue;
        }
        if let Event::Key(key) = ev {
            if key.kind != KeyEventKind::Press {
                continue;
            }
            // When the theme selector is open it captures navigation/confirm keys
            // so they don't also scroll the diff. Enter applies the highlighted
            // theme live (swap the palette + rebuild the highlighter on its
            // syntect theme, then re-render); Esc/`t` cancel.
            if show_theme_selector {
                match key.code {
                    KeyCode::Esc | KeyCode::Char('t') | KeyCode::Char('q') => {
                        show_theme_selector = false;
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        theme_cursor = (theme_cursor + 1).min(catalog.len().saturating_sub(1));
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        theme_cursor = theme_cursor.saturating_sub(1);
                    }
                    KeyCode::Enter => {
                        theme = catalog[theme_cursor].clone();
                        highlighter =
                            Highlighter::with_theme(&theme.syntect_theme, &theme.syntax_overrides);
                        needs_render = true;
                        show_theme_selector = false;
                    }
                    _ => {}
                }
                continue;
            }
            // While typing a search query the prompt captures every key so they
            // don't scroll the diff. Each edit live-recomputes the matches and
            // jumps to the first; Enter confirms (keeping the search for n/N),
            // Esc cancels and clears it.
            if search_input.is_some() {
                let mut recompute = false;
                match key.code {
                    KeyCode::Esc => {
                        search_input = None;
                        search = None;
                    }
                    KeyCode::Enter => {
                        search_input = None;
                    }
                    KeyCode::Backspace => {
                        if let Some(b) = search_input.as_mut() {
                            b.pop();
                        }
                        recompute = true;
                    }
                    // Ctrl-C and Ctrl-Z keep working from the prompt rather than
                    // being typed into the query (the prompt must not trap them).
                    KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => break,
                    #[cfg(unix)]
                    KeyCode::Char('z') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        suspend_and_resume(terminal);
                    }
                    KeyCode::Char(c) => {
                        if let Some(b) = search_input.as_mut() {
                            b.push(c);
                        }
                        recompute = true;
                    }
                    _ => {}
                }
                if recompute {
                    let q = search_input.clone().unwrap_or_default();
                    let s = Search::new(q, &line_texts);
                    jump_to_match(&s, &mut offset, &mut h_offset, max_offset);
                    search = Some(s);
                }
                continue;
            }
            // Scroll + file/hunk navigation are pure offset math, extracted into
            // `scroll_offset` so they can be unit-tested. Handle them first;
            // every other key falls through to the match below.
            if let Some(new_offset) = scroll_offset(
                key.code,
                offset,
                page,
                max_offset,
                &file_starts,
                &hunk_starts,
            ) {
                offset = new_offset;
                continue;
            }
            // Horizontal scroll (left/right arrows) when not wrapping.
            if let Some(nh) = h_scroll_offset(key.code, h_offset, H_STEP, H_MAX, wrap) {
                h_offset = nh;
                continue;
            }
            match key.code {
                KeyCode::Char('q') => break,
                // Ctrl-C: in raw mode crossterm delivers it as a key event, not
                // a signal, so quit explicitly (the shared restore path runs).
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => break,
                // Ctrl-Z: suspend to the shell with the terminal restored, then
                // resume into a fresh alternate screen. Unix only; elsewhere the
                // event falls through to the no-op arm.
                #[cfg(unix)]
                KeyCode::Char('z') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    suspend_and_resume(terminal);
                }
                // Open the active file (derived from the scroll offset) in
                // $EDITOR. No-op when the diff has no files.
                KeyCode::Char('e') => {
                    let active = file_at_offset(&file_starts, offset as usize);
                    if let Some(summary) = summaries.get(active) {
                        open_in_editor(terminal, &summary.path);
                    }
                }
                // Esc clears an active search first, then closes help, then quits.
                KeyCode::Esc => {
                    if search.is_some() {
                        search = None;
                    } else if show_help {
                        show_help = false;
                    } else {
                        break;
                    }
                }
                KeyCode::Char('?') => show_help = !show_help,
                // Open the incremental-search prompt (cleared of any prior search).
                KeyCode::Char('/') => {
                    search_input = Some(String::new());
                    search = None;
                }
                // `n`/`N` step matches while a search is active; otherwise `n`
                // keeps its normal job of toggling the line-number gutter.
                KeyCode::Char('N') => {
                    if let Some(s) = search.as_mut().filter(|s| !s.is_empty()) {
                        s.prev();
                        jump_to_match(s, &mut offset, &mut h_offset, max_offset);
                    }
                }
                // Display toggles: flip the option and re-render the lines.
                // hunk-header / context toggles change line counts, so the
                // per-file offsets are refreshed alongside the lines.
                KeyCode::Char('n') => {
                    if let Some(s) = search.as_mut().filter(|s| !s.is_empty()) {
                        s.next();
                        jump_to_match(s, &mut offset, &mut h_offset, max_offset);
                    } else {
                        opts.line_numbers = !opts.line_numbers;
                        needs_render = true;
                    }
                }
                KeyCode::Char('w') => wrap = !wrap,
                KeyCode::Char('H') => {
                    opts.hunk_headers = !opts.hunk_headers;
                    needs_render = true;
                }
                KeyCode::Char('c') => {
                    opts.context_collapsed = !opts.context_collapsed;
                    needs_render = true;
                }
                // Folds: toggle the fold nearest the viewport top (`o`/Enter),
                // expand all (`O`), or collapse all (`C`). `fold_bars` lists every
                // fold's bar row, so expand-all just inserts them all.
                KeyCode::Char('o') | KeyCode::Enter => {
                    if let Some(id) = fold_at_cursor(&fold_bars, offset as usize) {
                        // insert() returns false when already expanded -> collapse it.
                        if !expanded_folds.insert(id) {
                            expanded_folds.remove(&id);
                        }
                        needs_render = true;
                    }
                }
                KeyCode::Char('O') => {
                    for &(_, id) in &fold_bars {
                        expanded_folds.insert(id);
                    }
                    needs_render = true;
                }
                KeyCode::Char('C') => {
                    expanded_folds.clear();
                    needs_render = true;
                }
                // Cycle the layout mode auto -> split -> stack -> auto. The next
                // loop iteration re-resolves the effective mode and re-renders.
                KeyCode::Char('m') => {
                    mode = match mode.as_str() {
                        "auto" => "split".to_string(),
                        "split" => "stack".to_string(),
                        _ => "auto".to_string(),
                    };
                    needs_render = true;
                }
                // Open the theme selector, seeding the cursor at the active theme.
                KeyCode::Char('t') => {
                    theme_cursor = catalog
                        .iter()
                        .position(|t| t.name == theme.name)
                        .unwrap_or(theme_cursor);
                    show_theme_selector = true;
                }
                // Sidebar visibility toggle. (Tab/BackTab and `[`/`]` file
                // navigation are handled by `scroll_offset` above.)
                KeyCode::Char('s') => sidebar_visible = !sidebar_visible,
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

/// Resolve which editor to launch: `$VISUAL`, else `$EDITOR`, else `vi`. The
/// spec is split on whitespace into a program plus its base args (the file path
/// is appended by the caller). Pure — the env values are passed in so it never
/// reads the environment and can be tested deterministically.
///
/// ponytail: whitespace split, not full shell-word parsing (mirrors
/// `pager.rs`). Covers `code -w` and bare program names; quoted args in the
/// editor spec are a later refinement.
fn editor_command(visual: Option<&str>, editor: Option<&str>) -> (String, Vec<String>) {
    let spec = visual
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or_else(|| editor.map(str::trim).filter(|s| !s.is_empty()))
        .unwrap_or("vi");
    let mut parts = spec.split_whitespace();
    let program = parts.next().unwrap_or("vi").to_string();
    let args = parts.map(str::to_string).collect();
    (program, args)
}

/// Suspend the TUI, run `$EDITOR <path>` as a foreground child (inheriting our
/// stdio so the editor takes over the terminal), then re-enter the alternate
/// screen. The path is a discrete arg-vector element appended after the
/// editor's base args, so there is no shell and nothing in it can be
/// interpreted as a command. Best-effort: a spawn failure must not break the
/// review, but we always re-enter the UI afterwards.
fn open_in_editor(terminal: &mut DefaultTerminal, path: &str) {
    let visual = std::env::var("VISUAL").ok();
    let editor = std::env::var("EDITOR").ok();
    let (program, args) = editor_command(visual.as_deref(), editor.as_deref());

    // Leave the alternate screen + raw mode (and mouse capture) so the editor
    // owns the terminal.
    restore_review_terminal();
    let _ = std::process::Command::new(&program)
        .args(&args)
        .arg(path)
        .status();
    // Re-enter our UI regardless of how (or whether) the editor ran. The next
    // loop iteration redraws.
    *terminal = init_review_terminal();
}

/// Suspend the process to the shell (Ctrl-Z semantics) with the terminal
/// restored, then resume into a fresh alternate screen. Restores cooked mode +
/// the normal screen, raises `SIGTSTP` on our own pid via rustix (no libc), and
/// the `kill` call returns once the shell continues us (`fg` -> `SIGCONT`).
#[cfg(unix)]
fn suspend_and_resume(terminal: &mut DefaultTerminal) {
    use rustix::process::{getpid, kill_process, Signal};

    // Hand the terminal back in a sane state before stopping.
    restore_review_terminal();
    // Stop ourselves; control returns to the shell. Best-effort — if the signal
    // cannot be raised we simply re-enter below rather than hanging.
    let _ = kill_process(getpid(), Signal::Tstp);
    // Foregrounded again: re-enter the alternate screen; the loop redraws.
    *terminal = init_review_terminal();
}

/// Everything `draw_review` needs to paint one frame. Bundled so the live loop
/// and the snapshot test render through the exact same code path.
struct ReviewFrame<'a> {
    lines: &'a [Line<'static>],
    summaries: &'a [FileSummary],
    opts: &'a RenderOptions,
    /// Active theme — drives the status-bar and sidebar chrome colors.
    theme: &'a Theme,
    file_count: usize,
    selected_file: usize,
    offset: u16,
    /// Horizontal scroll offset in columns (0 unless the user arrowed sideways).
    h_offset: u16,
    /// Total rendered line count (already clamped to `u16::MAX`).
    total: u16,
    wrap: bool,
    sidebar_visible: bool,
    show_help: bool,
    /// Whether the theme-selector overlay is open.
    show_theme_selector: bool,
    /// The curated catalog the selector lists.
    catalog: &'a [Theme],
    /// The selector's highlighted row.
    theme_cursor: usize,
    /// Search matches to highlight in the main view (empty = no active search).
    search_matches: &'a [Match],
    /// The current match (emphasized differently from the rest), if any.
    search_current: Option<Match>,
    /// Prompt/counter text shown in place of the status bar while searching.
    search_status: Option<String>,
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
        frame.render_widget(sidebar(v.summaries, v.selected_file, v.theme), panes[0]);
        panes[1]
    } else {
        body
    };

    // ponytail: clone per frame is wasteful for huge diffs; revisit if it ever
    // shows up in a profile. Fine for interactive review sizes. When a search is
    // active, overlay the match highlight on the (few) matched lines only.
    let display_lines: Vec<Line<'static>> = if v.search_matches.is_empty() {
        v.lines.to_vec()
    } else {
        let matched_rows: HashSet<usize> = v.search_matches.iter().map(|m| m.line).collect();
        v.lines
            .iter()
            .enumerate()
            .map(|(i, l)| {
                if matched_rows.contains(&i) {
                    highlight_line(l, i, v.search_matches, v.search_current)
                } else {
                    l.clone()
                }
            })
            .collect()
    };
    let mut paragraph = Paragraph::new(display_lines).scroll((v.offset, v.h_offset));
    if v.wrap {
        // `trim: false` keeps leading indentation when wrapping.
        paragraph = paragraph.wrap(Wrap { trim: false });
    }
    frame.render_widget(paragraph, main);

    let max_off = v.total.saturating_sub(main.height);
    if let Some(text) = &v.search_status {
        // The search prompt/counter takes over the status line while searching.
        frame.render_widget(
            Paragraph::new(Line::from(text.clone()))
                .style(Style::default().add_modifier(Modifier::BOLD)),
            chunks[1],
        );
    } else {
        frame.render_widget(
            status_bar(
                v.file_count,
                v.selected_file,
                v.offset,
                max_off,
                v.opts,
                v.wrap,
                v.theme,
            ),
            chunks[1],
        );
    }

    if v.show_help {
        render_help(frame, area, &v.theme.name);
    }
    if v.show_theme_selector {
        render_theme_selector(frame, area, v.catalog, v.theme_cursor, &v.theme.name);
    }
    main.height
}

/// Compute the new scroll offset for the scroll / file-navigation keys, or
/// `None` when `key` is not one of them (the caller then handles it). Pure: all
/// inputs are passed in, so it is unit-testable without a terminal. The
/// expressions mirror the inline arms they replaced, so behavior is unchanged.
fn scroll_offset(
    key: KeyCode,
    offset: u16,
    page: u16,
    max_offset: u16,
    file_starts: &[usize],
    hunk_starts: &[usize],
) -> Option<u16> {
    // Half a page (>= 1) for the `d`/`u` keys.
    let half = (page / 2).max(1);
    let new_offset = match key {
        KeyCode::Down | KeyCode::Char('j') => (offset + 1).min(max_offset),
        KeyCode::Up | KeyCode::Char('k') => offset.saturating_sub(1),
        KeyCode::PageDown | KeyCode::Char(' ') => (offset + page).min(max_offset),
        KeyCode::PageUp => offset.saturating_sub(page),
        // Half-page steps (vim `Ctrl-D`/`Ctrl-U`, here plain `d`/`u`).
        KeyCode::Char('d') => (offset + half).min(max_offset),
        KeyCode::Char('u') => offset.saturating_sub(half),
        // `g`/Home jump to the top; pressing `g` twice (vim `gg`) lands there too.
        KeyCode::Home | KeyCode::Char('g') => 0,
        KeyCode::End | KeyCode::Char('G') => max_offset,
        KeyCode::Tab | KeyCode::Char(']') => {
            if file_starts.is_empty() {
                return None;
            }
            let current = file_at_offset(file_starts, offset as usize);
            let target = (current + 1).min(file_starts.len() - 1);
            file_to_offset(file_starts, target, max_offset)
        }
        KeyCode::BackTab | KeyCode::Char('[') => {
            if file_starts.is_empty() {
                return None;
            }
            let current = file_at_offset(file_starts, offset as usize);
            let target = current.saturating_sub(1);
            file_to_offset(file_starts, target, max_offset)
        }
        // `{`/`}` hop between hunks. Unlike files (whose first start is row 0),
        // hunk_starts[0] sits below the file header, so the "current region +/- 1"
        // trick used for files would skip the first hunk when the cursor is above
        // it. Instead pick the first hunk strictly below the cursor (`}`) or the
        // last strictly above it (`{`) via partition_point.
        KeyCode::Char('}') => {
            if hunk_starts.is_empty() {
                return None;
            }
            let target = hunk_starts
                .partition_point(|&s| s <= offset as usize)
                .min(hunk_starts.len() - 1);
            file_to_offset(hunk_starts, target, max_offset)
        }
        KeyCode::Char('{') => {
            if hunk_starts.is_empty() {
                return None;
            }
            let target = hunk_starts
                .partition_point(|&s| s < offset as usize)
                .saturating_sub(1);
            file_to_offset(hunk_starts, target, max_offset)
        }
        _ => return None,
    };
    Some(new_offset)
}

/// New horizontal scroll offset for the left/right arrows, or `None` when `key`
/// isn't one (or wrapping is on, where horizontal scrolling is meaningless).
/// Pure so it is unit-testable. Right advances by `step` up to `max`; left backs
/// off toward 0.
fn h_scroll_offset(key: KeyCode, h: u16, step: u16, max: u16, wrap: bool) -> Option<u16> {
    if wrap {
        return None;
    }
    match key {
        KeyCode::Right => Some((h + step).min(max)),
        KeyCode::Left => Some(h.saturating_sub(step)),
        _ => None,
    }
}

/// Lines the mouse wheel scrolls per notch.
const WHEEL_LINES: u16 = 3;

/// New vertical offset for a mouse-wheel event, or `None` when the event isn't a
/// scroll (so the caller ignores it). Pure, mirroring `scroll_offset`, so the
/// wheel mapping is unit-testable without a terminal.
fn wheel_scroll(kind: MouseEventKind, offset: u16, step: u16, max_offset: u16) -> Option<u16> {
    match kind {
        MouseEventKind::ScrollDown => Some((offset + step).min(max_offset)),
        MouseEventKind::ScrollUp => Some(offset.saturating_sub(step)),
        _ => None,
    }
}

/// Map a selected file index to a clamped scroll offset for the main view.
fn file_to_offset(file_starts: &[usize], idx: usize, max_offset: u16) -> u16 {
    let start = file_starts.get(idx).copied().unwrap_or(0);
    (start.min(u16::MAX as usize) as u16).min(max_offset)
}

/// Scroll offset that brings match `m` into view (at the viewport top), clamped.
fn match_offset(m: Match, max_offset: u16) -> u16 {
    (m.line.min(u16::MAX as usize) as u16).min(max_offset)
}

/// After the search cursor moves, scroll so the current match sits at the
/// viewport top (clamped) and reset the horizontal offset so the match isn't
/// scrolled off-screen to the left. No-op when the search matched nothing.
fn jump_to_match(search: &Search, offset: &mut u16, h_offset: &mut u16, max_offset: u16) {
    if let Some(m) = search.current_match() {
        *offset = match_offset(m, max_offset);
        *h_offset = 0;
    }
}

/// Flatten a rendered line to its plain text (span contents concatenated), so
/// the search module can scan it without knowing about styling.
fn line_plain_text(line: &Line<'static>) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
}

/// Build the status-line text for the search prompt / counter. `input` is the
/// live-typed buffer (when the prompt is open); `search` carries the matches.
/// Returns `None` when there is nothing to show (no prompt, no active search).
fn search_status_text(input: Option<&str>, search: Option<&Search>) -> Option<String> {
    match (input, search) {
        // Live typing: show the buffer plus a match count.
        (Some(buf), s) => {
            let count = s.map(Search::len).unwrap_or(0);
            Some(if buf.is_empty() {
                "/".to_string()
            } else if count == 0 {
                format!("/{buf}  no matches")
            } else {
                format!("/{buf}  {count} matches")
            })
        }
        // Confirmed search (prompt closed): show current/total or no-match.
        (None, Some(s)) if !s.is_empty() => {
            Some(format!("/{}  {}/{}", s.query, s.current_ordinal(), s.len()))
        }
        (None, Some(s)) => Some(format!("/{}  no matches", s.query)),
        (None, None) => None,
    }
}

/// Rebuild one line with the search highlight overlaid on its matched ranges:
/// the current match is reversed, the rest underlined. Walks chars and coalesces
/// runs of equal style back into spans. Only called for lines that have a match.
fn highlight_line(
    line: &Line<'static>,
    line_idx: usize,
    matches: &[Match],
    current: Option<Match>,
) -> Line<'static> {
    // Expand to (char, style), inheriting each span's style.
    let mut chars: Vec<(char, Style)> = Vec::new();
    for span in &line.spans {
        for ch in span.content.chars() {
            chars.push((ch, span.style));
        }
    }
    // Overlay the match emphasis on the covered char ranges.
    for m in matches.iter().filter(|m| m.line == line_idx) {
        let emphasis = if current == Some(*m) {
            Modifier::REVERSED
        } else {
            Modifier::UNDERLINED
        };
        for (_, st) in chars.iter_mut().take(m.end).skip(m.start) {
            *st = st.add_modifier(emphasis);
        }
    }
    // Coalesce consecutive equal styles into spans.
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut buf = String::new();
    let mut cur: Option<Style> = None;
    for (ch, st) in chars {
        if Some(st) != cur {
            if let Some(s) = cur {
                spans.push(Span::styled(std::mem::take(&mut buf), s));
            }
            cur = Some(st);
        }
        buf.push(ch);
    }
    if let Some(s) = cur {
        spans.push(Span::styled(buf, s));
    }
    Line::from(spans)
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
fn sidebar(summaries: &[FileSummary], selected: usize, theme: &Theme) -> Paragraph<'static> {
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
                Span::styled(format!("+{}", s.additions), Style::default().fg(theme.add)),
                Span::raw(" "),
                Span::styled(
                    format!("-{}", s.deletions),
                    Style::default().fg(theme.remove),
                ),
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
    theme: &Theme,
) -> Paragraph<'static> {
    let pct = if max_offset == 0 {
        100
    } else {
        (offset as usize * 100 / max_offset as usize).min(100)
    };

    // Show only the toggles that are currently ON, mirroring the help labels.
    // SPLIT is shown only when the side-by-side layout is active; in the default
    // stack layout the mode is implicit (and the indicator stays absent).
    let mut active: Vec<&str> = Vec::new();
    if opts.mode == LayoutMode::Split {
        active.push("SPLIT");
    }
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
    let text = format!(
        " file {pos}/{file_count}  {pct}%  [{toggles}]  o fold  s sidebar  e edit  ? help "
    );
    Paragraph::new(text).style(
        Style::default()
            .fg(theme.status_fg)
            .bg(theme.status_bg)
            .add_modifier(Modifier::BOLD),
    )
}

/// Render the centered, bordered help overlay listing every keybinding. The
/// active theme name is shown so the user can see what `t` will be switching from.
fn render_help(frame: &mut Frame, area: Rect, active_theme: &str) {
    let theme_line = format!("  theme: {active_theme}");
    let help = [
        "  Keybindings",
        "",
        "  j / Down      scroll down",
        "  k / Up        scroll up",
        "  Space / PgDn  page down",
        "  PgUp          page up",
        "  d / u         half page down / up",
        "  Left / Right  scroll horizontally",
        "  g / Home      jump to top",
        "  G / End       jump to bottom",
        "",
        "  Tab / ]       next file",
        "  Shift-Tab / [ previous file",
        "  } / {         next / prev hunk",
        "  s             toggle sidebar",
        "",
        "  /   search   (n / N next / prev)",
        "  n   toggle line numbers",
        "  w   toggle line wrap",
        "  H   toggle hunk headers",
        "  c   toggle context collapse",
        "  o / Enter   toggle fold",
        "  O   expand all folds",
        "  C   collapse all folds",
        "  m   cycle layout auto/split/stack",
        "  t   theme selector",
        "",
        "  e   open file in $EDITOR",
        "  Ctrl-Z suspend  Ctrl-C quit",
        "",
        &theme_line,
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

/// Render the centered theme-selector overlay: the curated catalog with the
/// cursor row highlighted and the currently-active theme marked. Mirrors the
/// help overlay's centered, cleared, bordered popup.
fn render_theme_selector(
    frame: &mut Frame,
    area: Rect,
    catalog: &[Theme],
    cursor: usize,
    active: &str,
) {
    let mut lines: Vec<Line<'static>> = Vec::with_capacity(catalog.len() + 2);
    lines.push(Line::from("  Theme  (Enter apply, Esc cancel)".to_string()));
    lines.push(Line::from(""));
    for (i, t) in catalog.iter().enumerate() {
        // Marker column: `>` cursor row, `*` the active theme, else blank.
        let marker = if i == cursor { '>' } else { ' ' };
        let active_mark = if t.name == active { " *" } else { "" };
        let text = format!("  {marker} {}{active_mark}", t.name);
        if i == cursor {
            lines.push(Line::from(Span::styled(
                text,
                Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD),
            )));
        } else {
            lines.push(Line::from(text));
        }
    }

    let width = 36.min(area.width);
    let height = (lines.len() as u16 + 2).min(area.height);
    let popup = centered_rect(width, height, area);

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" themes ")
        .style(Style::default().fg(Color::White).bg(Color::Black));
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
    fn editor_command_prefers_visual_over_editor() {
        let (prog, args) = editor_command(Some("nvim"), Some("vi"));
        assert_eq!(prog, "nvim");
        assert!(args.is_empty());
    }

    #[test]
    fn editor_command_falls_back_to_editor_then_vi() {
        let (prog, _) = editor_command(None, Some("emacs"));
        assert_eq!(prog, "emacs");

        let (prog, args) = editor_command(None, None);
        assert_eq!(prog, "vi");
        assert!(args.is_empty());

        // Blank/whitespace specs are ignored, falling through to the next source.
        let (prog, _) = editor_command(Some("   "), Some("nano"));
        assert_eq!(prog, "nano");
        let (prog, _) = editor_command(Some(""), None);
        assert_eq!(prog, "vi");
    }

    #[test]
    fn editor_command_splits_multiword_spec() {
        // A multi-word spec splits into program + base args; the caller appends
        // the file path, so the resolver must NOT include it.
        let (prog, args) = editor_command(Some("code -w --reuse-window"), None);
        assert_eq!(prog, "code");
        assert_eq!(args, vec!["-w".to_string(), "--reuse-window".to_string()]);
    }

    #[test]
    fn renders_sidebar_and_main_view() {
        let model = parse_unified_diff_colored(MULTI_FILE);
        let highlighter = Highlighter::new();
        let theme = Theme::default();
        let catalog = crate::theme::catalog();
        let opts = RenderOptions::default();
        let rendered = render_diff(&model, &highlighter, &theme, &opts, &HashSet::new(), 80);
        let summaries = file_summaries(&model);

        // Select the second file and scroll the main view to its start.
        let selected_file = 1;
        let offset = rendered.file_starts[selected_file] as u16;
        let view = ReviewFrame {
            lines: &rendered.lines,
            summaries: &summaries,
            opts: &opts,
            theme: &theme,
            file_count: model.files.len(),
            selected_file,
            offset,
            h_offset: 0,
            total: rendered.lines.len() as u16,
            wrap: false,
            sidebar_visible: true,
            show_help: false,
            show_theme_selector: false,
            catalog: &catalog,
            theme_cursor: 0,
            search_matches: &[],
            search_current: None,
            search_status: None,
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

    #[test]
    fn effective_mode_resolves_explicit_and_auto() {
        // Explicit modes ignore width.
        assert_eq!(effective_mode("split", 1), LayoutMode::Split);
        assert_eq!(effective_mode("stack", 9999), LayoutMode::Stack);
        assert_eq!(effective_mode("unified", 9999), LayoutMode::Stack);
        // Auto (and unknown values) switch at AUTO_SPLIT_MIN.
        assert_eq!(
            effective_mode("auto", AUTO_SPLIT_MIN - 1),
            LayoutMode::Stack
        );
        assert_eq!(effective_mode("auto", AUTO_SPLIT_MIN), LayoutMode::Split);
        assert_eq!(effective_mode("bogus", AUTO_SPLIT_MIN), LayoutMode::Split);
    }

    #[test]
    fn main_content_width_accounts_for_sidebar() {
        // Wide enough: full width minus the sidebar.
        assert_eq!(main_content_width(160, true), 160 - SIDEBAR_W);
        // Sidebar hidden: full width.
        assert_eq!(main_content_width(160, false), 160);
        // Too narrow for the sidebar: it is dropped, so full width.
        assert_eq!(main_content_width(30, true), 30);
    }

    #[test]
    fn scroll_offset_line_and_page_movement() {
        let fs: &[usize] = &[];
        let hs: &[usize] = &[];
        // Down/j increments and clamps at max_offset (at max, stays).
        assert_eq!(scroll_offset(KeyCode::Char('j'), 0, 10, 5, fs, hs), Some(1));
        assert_eq!(scroll_offset(KeyCode::Down, 5, 10, 5, fs, hs), Some(5));
        // Up/k saturates at 0.
        assert_eq!(scroll_offset(KeyCode::Char('k'), 0, 10, 5, fs, hs), Some(0));
        // Page down/up by `page`, clamped.
        assert_eq!(
            scroll_offset(KeyCode::Char(' '), 0, 4, 100, fs, hs),
            Some(4)
        );
        assert_eq!(scroll_offset(KeyCode::PageUp, 3, 10, 100, fs, hs), Some(0));
        // Half-page d/u move by page/2 (>= 1), clamped.
        assert_eq!(
            scroll_offset(KeyCode::Char('d'), 0, 10, 100, fs, hs),
            Some(5)
        );
        assert_eq!(
            scroll_offset(KeyCode::Char('u'), 4, 10, 100, fs, hs),
            Some(0)
        );
        // page/2 floors to 1 so `d` always advances.
        assert_eq!(
            scroll_offset(KeyCode::Char('d'), 0, 1, 100, fs, hs),
            Some(1)
        );
        // Home/g -> 0 (vim `gg` lands there too), End/G -> max_offset.
        assert_eq!(
            scroll_offset(KeyCode::Char('g'), 50, 10, 80, fs, hs),
            Some(0)
        );
        assert_eq!(
            scroll_offset(KeyCode::Char('G'), 0, 10, 80, fs, hs),
            Some(80)
        );
        // A non-scroll key returns None so the caller handles it.
        assert_eq!(scroll_offset(KeyCode::Char('x'), 0, 10, 80, fs, hs), None);
    }

    #[test]
    fn scroll_offset_file_navigation() {
        // Three files starting at rows 0, 10, 25.
        let fs: &[usize] = &[0, 10, 25];
        let hs: &[usize] = &[];
        // Tab from the first file jumps to the second file's start.
        assert_eq!(scroll_offset(KeyCode::Tab, 0, 10, 100, fs, hs), Some(10));
        // Tab past the last file clamps to the last file's start.
        assert_eq!(
            scroll_offset(KeyCode::Char(']'), 25, 10, 100, fs, hs),
            Some(25)
        );
        // BackTab from the second file goes to the first.
        assert_eq!(
            scroll_offset(KeyCode::BackTab, 10, 10, 100, fs, hs),
            Some(0)
        );
        // file_to_offset clamps the target to max_offset.
        assert_eq!(scroll_offset(KeyCode::Tab, 0, 10, 5, fs, hs), Some(5));
        // With no files, Tab/BackTab are not handled here (return None).
        assert_eq!(scroll_offset(KeyCode::Tab, 0, 10, 100, &[], hs), None);
    }

    #[test]
    fn scroll_offset_hunk_navigation() {
        let fs: &[usize] = &[0];
        // Two hunks starting at rows 3 and 12 (the file header occupies rows 0..3).
        let hs: &[usize] = &[3, 12];
        // From the top (above the first hunk) `}` reaches the FIRST hunk, not the
        // second — the file-header region must not be treated as "on hunk 0".
        assert_eq!(
            scroll_offset(KeyCode::Char('}'), 0, 10, 100, fs, hs),
            Some(3)
        );
        // `}` on the first hunk advances to the second.
        assert_eq!(
            scroll_offset(KeyCode::Char('}'), 3, 10, 100, fs, hs),
            Some(12)
        );
        // `}` past the last hunk clamps to it.
        assert_eq!(
            scroll_offset(KeyCode::Char('}'), 12, 10, 100, fs, hs),
            Some(12)
        );
        // `{` steps back to the previous hunk.
        assert_eq!(
            scroll_offset(KeyCode::Char('{'), 12, 10, 100, fs, hs),
            Some(3)
        );
        // `{` from inside a hunk lands on that hunk's own start (vim `{` feel),
        // not the previous hunk: cursor at row 15 inside the hunk at row 12.
        let hs3: &[usize] = &[3, 12, 20];
        assert_eq!(
            scroll_offset(KeyCode::Char('{'), 15, 10, 100, fs, hs3),
            Some(12)
        );
        // With no hunks, `{`/`}` are not handled (return None).
        assert_eq!(scroll_offset(KeyCode::Char('}'), 0, 10, 100, fs, &[]), None);
    }

    #[test]
    fn gh_pr_diff_args_builds_pr_diff_command() {
        assert_eq!(
            gh_pr_diff_args(123),
            vec!["pr".to_string(), "diff".to_string(), "123".to_string()]
        );
    }

    #[test]
    fn wheel_scroll_maps_scroll_events_and_ignores_others() {
        use ratatui::crossterm::event::MouseEventKind;
        // Down advances by step, clamped at max; Up backs off toward 0.
        assert_eq!(wheel_scroll(MouseEventKind::ScrollDown, 0, 3, 10), Some(3));
        assert_eq!(wheel_scroll(MouseEventKind::ScrollDown, 9, 3, 10), Some(10));
        assert_eq!(wheel_scroll(MouseEventKind::ScrollUp, 2, 3, 10), Some(0));
        // A non-scroll mouse event (e.g. a move) is not handled here.
        assert_eq!(wheel_scroll(MouseEventKind::Moved, 5, 3, 10), None);
    }

    #[test]
    fn h_scroll_offset_moves_within_bounds_and_respects_wrap() {
        // Right advances by step, clamped at max; Left backs off toward 0.
        assert_eq!(h_scroll_offset(KeyCode::Right, 0, 8, 512, false), Some(8));
        assert_eq!(
            h_scroll_offset(KeyCode::Right, 510, 8, 512, false),
            Some(512)
        );
        assert_eq!(h_scroll_offset(KeyCode::Left, 4, 8, 512, false), Some(0));
        // Wrapping disables horizontal scroll.
        assert_eq!(h_scroll_offset(KeyCode::Right, 0, 8, 512, true), None);
        // Other keys are not horizontal-scroll keys.
        assert_eq!(h_scroll_offset(KeyCode::Char('j'), 0, 8, 512, false), None);
    }

    #[test]
    fn file_to_offset_clamps_index_and_max() {
        let fs = [0usize, 10, 25];
        assert_eq!(file_to_offset(&fs, 1, 100), 10);
        // Out-of-range index falls back to 0.
        assert_eq!(file_to_offset(&fs, 9, 100), 0);
        // Start beyond max_offset is clamped down.
        assert_eq!(file_to_offset(&fs, 2, 20), 20);
    }

    #[test]
    fn renders_auto_split_when_wide() {
        // `auto` at a wide main width selects split; render the full frame the
        // way run_loop would (mode resolved, then drawn) so the snapshot covers
        // the split main view + the SPLIT status-bar indicator.
        let model = parse_unified_diff_colored(MULTI_FILE);
        let highlighter = Highlighter::new();
        let theme = Theme::default();
        let catalog = crate::theme::catalog();

        // Sidebar hidden so the whole 130-col width is the main content; auto
        // then resolves to split (130 >= AUTO_SPLIT_MIN).
        let sidebar_visible = false;
        let main_width = main_content_width(130, sidebar_visible);
        let mode = effective_mode("auto", main_width);
        assert_eq!(
            mode,
            LayoutMode::Split,
            "auto should pick split at 130 cols"
        );

        let opts = RenderOptions {
            mode,
            ..RenderOptions::default()
        };
        let rendered = render_diff(
            &model,
            &highlighter,
            &theme,
            &opts,
            &HashSet::new(),
            main_width,
        );
        let summaries = file_summaries(&model);
        let view = ReviewFrame {
            lines: &rendered.lines,
            summaries: &summaries,
            opts: &opts,
            theme: &theme,
            file_count: model.files.len(),
            selected_file: 0,
            offset: 0,
            h_offset: 0,
            total: rendered.lines.len() as u16,
            wrap: false,
            sidebar_visible,
            show_help: false,
            show_theme_selector: false,
            catalog: &catalog,
            theme_cursor: 0,
            search_matches: &[],
            search_current: None,
            search_status: None,
        };

        let backend = TestBackend::new(130, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| {
                draw_review(f, &view);
            })
            .unwrap();

        insta::assert_snapshot!(terminal.backend());
    }
}
