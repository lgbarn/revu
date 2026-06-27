//! Turns a [`DiffModel`] into renderable lines for the unified (stack) view.
//!
//! This is a pure function over the model, so the same output that the live UI
//! draws can be snapshot-tested against an in-memory buffer.

use std::collections::{HashMap, HashSet};

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::diff::{DiffLine, DiffModel, FileDiff, Hunk, LineKind};
use crate::fold::{compute_hunk_folds, file_fold_id, is_expanded, is_generated, FoldId};
use crate::highlight::Highlighter;
use crate::theme::Theme;

/// The layout the renderer emits: the classic unified (stack) view, or a
/// side-by-side two-column (split) view. `Copy` so it lives cheaply on
/// [`RenderOptions`]. The app resolves `auto` to one of these per frame from the
/// terminal width; the renderer itself only ever sees a concrete choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutMode {
    /// Unified diff: one column, old/new interleaved (the original view).
    Stack,
    /// Side-by-side: old on the left, new on the right, aligned per row.
    Split,
    /// Top/bottom: per hunk, the old block stacked above the new block, each
    /// full width. Useful on narrow-but-tall terminals where split is cramped.
    Vertical,
}

/// Display toggles that change what `render_lines` emits. Distinct from the
/// app's `wrap` toggle, which is a [`Paragraph`](ratatui::widgets::Paragraph)
/// property (line wrapping is not a line-content concern).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderOptions {
    /// Render a right-aligned line-number gutter. Stack shows BOTH old and new
    /// columns per line (context both, remove old-only, add new-only); split
    /// shows old on the left cell and new on the right. The left change-bar is
    /// drawn regardless of this toggle.
    pub line_numbers: bool,
    /// Emit the `@@ ... @@` hunk header lines.
    pub hunk_headers: bool,
    /// Collapse (omit) context lines, showing only added/removed lines.
    pub context_collapsed: bool,
    /// Stack (unified) vs split (side-by-side) layout. Default [`LayoutMode::Stack`]
    /// so every existing caller keeps byte-identical unified output.
    pub mode: LayoutMode,
}

impl Default for RenderOptions {
    fn default() -> Self {
        // Mirrors the config defaults: numbers + headers on, context expanded,
        // unified layout (split is opt-in via `--mode`/the `m` key).
        Self {
            line_numbers: true,
            hunk_headers: true,
            context_collapsed: false,
            mode: LayoutMode::Stack,
        }
    }
}

/// Blank gutter (5 cols) matching the width of a rendered `"{:>4} "` number, so
/// remove lines and unnumbered rows align with numbered ones.
const BLANK_GUTTER: &str = "     ";

/// The rendered diff plus the per-file line offsets that let the UI scroll the
/// main view to a chosen file. `file_starts[i]` is the index in `lines` of the
/// first rendered line of `model.files[i]` (its header row).
///
/// `fold_bars` maps the rendered ROW index of each fold bar (collapsed `▼` or
/// expanded `▲`) to its [`FoldId`], ascending by row, so the app can find the
/// fold nearest the viewport to toggle. Only the stack layout produces folds.
#[derive(Debug, Clone)]
pub struct RenderedDiff {
    pub lines: Vec<Line<'static>>,
    pub file_starts: Vec<usize>,
    /// Rendered ROW index where each hunk begins (its header row, or first body
    /// row when headers are off), ascending. Drives `{`/`}` prev/next-hunk jumps.
    /// A collapsed generated file contributes none (its hunks aren't emitted).
    pub hunk_starts: Vec<usize>,
    pub fold_bars: Vec<(usize, FoldId)>,
    /// Rendered ROW index -> `(file index, new-side line number)` for each
    /// content line that exists on the new side (context + adds), so the app can
    /// render an optional blame gutter aligned to those rows. Only the stack
    /// layout populates this; split/vertical leave it empty.
    pub blame_keys: HashMap<usize, (usize, usize)>,
}

/// One row of the file sidebar: a path with its add/remove line counts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileSummary {
    pub path: String,
    pub additions: usize,
    pub deletions: usize,
}

/// Per-file add/remove line counts for the sidebar. `additions` counts
/// [`LineKind::Add`] lines and `deletions` counts [`LineKind::Remove`] lines
/// across all hunks of each file. Pure over the model.
pub fn file_summaries(model: &DiffModel) -> Vec<FileSummary> {
    model
        .files
        .iter()
        .map(|file| {
            let mut additions = 0usize;
            let mut deletions = 0usize;
            for hunk in &file.hunks {
                for dl in &hunk.lines {
                    match dl.kind {
                        LineKind::Add => additions += 1,
                        LineKind::Remove => deletions += 1,
                        LineKind::Context => {}
                    }
                }
            }
            FileSummary {
                path: file.path.clone(),
                additions,
                deletions,
            }
        })
        .collect()
}

/// Render the model as a flat list of styled lines (unified/stack layout).
///
/// `highlighter` provides syntax coloring for the diff content. The leading
/// `+`/`-`/space prefix keeps its add/remove color (the change signal), while
/// the content after it is colored by language token. `opts` controls the
/// optional line-number gutter, hunk headers, and context collapsing.
///
/// Thin wrapper over [`render_diff`] that drops the per-file offsets, kept so
/// existing callers (and the snapshot test) need no change. The live UI now
/// calls [`render_diff`] directly for the offsets, leaving this used only by
/// tests in this binary crate — hence the `allow`.
#[allow(dead_code)]
pub fn render_lines(
    model: &DiffModel,
    highlighter: &Highlighter,
    theme: &Theme,
    opts: &RenderOptions,
    width: u16,
) -> Vec<Line<'static>> {
    // No expanded folds: all foldable runs collapse (the default view).
    render_diff(model, highlighter, theme, opts, &HashSet::new(), width).lines
}

/// Render the model, also recording where each file begins so the UI can jump
/// the scroll offset to a selected file. See [`RenderedDiff`].
///
/// `width` is the available content width in columns; only [`LayoutMode::Split`]
/// uses it (for column sizing). [`LayoutMode::Stack`] ignores it entirely and
/// produces byte-identical output regardless of `width`. Thin delegator over
/// [`layout_rows`], the pure row-generation core.
pub fn render_diff(
    model: &DiffModel,
    highlighter: &Highlighter,
    theme: &Theme,
    opts: &RenderOptions,
    folds: &HashSet<FoldId>,
    width: u16,
) -> RenderedDiff {
    layout_rows(model, highlighter, theme, opts, folds, width)
}

/// Pure row generation over `(model, opts, width)`: the single entry point the
/// unit tests drive for each [`LayoutMode`]. Dispatches to the unified or split
/// builder; the empty-model placeholder is shared by both.
fn layout_rows(
    model: &DiffModel,
    highlighter: &Highlighter,
    theme: &Theme,
    opts: &RenderOptions,
    folds: &HashSet<FoldId>,
    width: u16,
) -> RenderedDiff {
    if model.files.is_empty() {
        return RenderedDiff {
            lines: vec![Line::from("No changes in the working tree.")],
            file_starts: Vec::new(),
            hunk_starts: Vec::new(),
            fold_bars: Vec::new(),
            blame_keys: HashMap::new(),
        };
    }
    match opts.mode {
        LayoutMode::Stack => stack_rows(model, highlighter, theme, opts, folds, width),
        // ponytail: split renders every CONTEXT line uncollapsed (no per-hunk
        // folds); folding its paired change-groups is a later add. It does honor
        // whole-file generated-file collapse, so noise files fold in both layouts.
        LayoutMode::Split => split_rows(model, highlighter, theme, opts, folds, width),
        LayoutMode::Vertical => vertical_rows(model, highlighter, theme, opts, folds, width),
    }
}

/// Width (in digit columns) of each numeric gutter column for the whole model:
/// the digit count of the largest old- or new-side line number reached across
/// every hunk, floored at 3 so single-digit diffs still get a steady gutter.
/// Computed once per render so every line shares one column width.
fn gutter_width(model: &DiffModel) -> usize {
    let mut max_line = 0usize;
    for file in &model.files {
        for hunk in &file.hunks {
            let mut old = parse_hunk_old_start(&hunk.header);
            let mut new = parse_hunk_new_start(&hunk.header);
            for dl in &hunk.lines {
                // A line's number is its counter's value before the increment;
                // the counter advances on the side(s) the line exists on.
                match dl.kind {
                    LineKind::Context => {
                        if let Some(o) = old {
                            max_line = max_line.max(o);
                            old = Some(o + 1);
                        }
                        if let Some(n) = new {
                            max_line = max_line.max(n);
                            new = Some(n + 1);
                        }
                    }
                    LineKind::Remove => {
                        if let Some(o) = old {
                            max_line = max_line.max(o);
                            old = Some(o + 1);
                        }
                    }
                    LineKind::Add => {
                        if let Some(n) = new {
                            max_line = max_line.max(n);
                            new = Some(n + 1);
                        }
                    }
                }
            }
        }
    }
    let digits = if max_line == 0 {
        1
    } else {
        max_line.ilog10() as usize + 1
    };
    digits.max(3)
}

/// Format one numeric gutter cell, right-aligned to `w`; `None` (the absent
/// side of an add/remove line, or an unparseable header) renders `w` spaces.
fn gutter_cell(n: Option<usize>, w: usize) -> String {
    match n {
        Some(v) => format!("{v:>w$}"),
        None => " ".repeat(w),
    }
}

/// The unified (stack) layout: a flat list of styled lines, old/new interleaved.
///
/// Each diff line is laid out left-to-right as
/// `[change-bar(1)][old#(W)] [new#(W)] [prefix(1)][content...][row-pad]`:
/// a 1-column change-indicator bar (vivid add/remove background, blank for
/// context), then — when `opts.line_numbers` is on — the dual old/new line-number
/// gutter (context shows both, remove old-only, add new-only), then the
/// `+`/`-`/space prefix and the syntax-highlighted content.
///
/// `width` is used only to right-pad added/removed rows with spaces so their
/// background tint spans the full row. Padding with spaces leaves the rendered
/// *symbols* identical to the terminal's own blank-cell fill; the tint is a pure
/// background-style change drawn on top. The change-bar keeps its vivid
/// `theme.add`/`theme.remove` background (its bg is set explicitly, so the
/// full-row tint in `apply_row_bg` skips it).
fn stack_rows(
    model: &DiffModel,
    highlighter: &Highlighter,
    theme: &Theme,
    opts: &RenderOptions,
    folds: &HashSet<FoldId>,
    width: u16,
) -> RenderedDiff {
    // One column width for the whole render so every line's gutter aligns.
    let gw = gutter_width(model);

    let mut out: Vec<Line<'static>> = Vec::new();
    let mut file_starts: Vec<usize> = Vec::with_capacity(model.files.len());
    // Row where each hunk begins, ascending; drives `{`/`}` hunk navigation.
    let mut hunk_starts: Vec<usize> = Vec::new();
    // Row -> (file index, new-side line number) for the optional blame gutter.
    let mut blame_keys: HashMap<usize, (usize, usize)> = HashMap::new();
    // Row -> FoldId for every emitted fold bar, ascending by row (we append top
    // to bottom). The app uses this to find the fold nearest the viewport.
    let mut fold_bars: Vec<(usize, FoldId)> = Vec::new();
    for (file_idx, file) in model.files.iter().enumerate() {
        // Record the first rendered line index for this file before emitting it.
        file_starts.push(out.len());
        // Folds are numbered per file, sequentially across its hunks.
        let mut fold_index = 0usize;
        push_file_header(&mut out, file, theme);

        // A generated file (lockfile, minified bundle, codegen) collapses to a
        // single whole-file fold bar so its noise doesn't bury real changes. The
        // reserved fold id means the existing o/O/C controls expand it.
        let file_collapsed =
            file_collapse_bar(&mut out, &mut fold_bars, file, file_idx, folds, theme);

        // Resolve the file's syntax once; unknown extensions fall back to plain
        // text inside `syntax_for_path`.
        let syntax = highlighter.syntax_for_path(&file.path);

        for hunk in &file.hunks {
            if file_collapsed {
                break;
            }
            // Record where this hunk starts (its header row, or first body row
            // when headers are off) for `{`/`}` navigation.
            hunk_starts.push(out.len());
            push_hunk_header(&mut out, hunk, opts, theme);

            // Old- and new-side line counters, seeded from the hunk header.
            // `None` (an unparseable header) renders blank gutters for the whole
            // hunk. Old advances on context+remove, new on context+add.
            let mut old_line = parse_hunk_old_start(&hunk.header);
            let mut new_line = parse_hunk_new_start(&hunk.header);

            // Compute this hunk's folds and project them onto per-line arrays:
            // `bar_fold[i]` is the fold whose bar prints just before line `i`;
            // `hidden[i]` is true when line `i` is collapsed away by its fold.
            // `context_collapsed` (the all-context toggle) supersedes folds — with
            // every context line already hidden there is nothing to fold around.
            let n = hunk.lines.len();
            // `(FoldId, hidden-count)` of the fold whose bar prints before line i.
            let mut bar_fold: Vec<Option<(FoldId, usize)>> = vec![None; n];
            let mut hidden: Vec<bool> = vec![false; n];
            if !opts.context_collapsed {
                for f in compute_hunk_folds(&hunk.lines, file_idx, &mut fold_index) {
                    bar_fold[f.start] = Some((f.id, f.hidden()));
                    if !is_expanded(folds, f.id) {
                        hidden[f.start..f.end].fill(true);
                    }
                }
            }

            // ponytail: a fresh HighlightLines per hunk means highlight state
            // (open strings/comments spanning hunk boundaries) resets at each
            // `@@`. Acceptable for diff review — hunks are non-contiguous slices
            // of the file anyway. Whole-file state would need the full source.
            let mut hl = highlighter.line_highlighter(syntax);

            for (i, dl) in hunk.lines.iter().enumerate() {
                // A fold bar prints just above its first line: `▼` collapsed (the
                // hidden lines follow it suppressed) or `▲` expanded (the lines
                // follow it visible, so re-collapsing has a target row).
                if let Some((id, count)) = bar_fold[i] {
                    let expanded = is_expanded(folds, id);
                    fold_bars.push((out.len(), id));
                    out.push(fold_bar_line(count, expanded, theme));
                }
                let (prefix, mut prefix_style) = match dl.kind {
                    LineKind::Add => ('+', Style::default().fg(theme.add)),
                    LineKind::Remove => ('-', Style::default().fg(theme.remove)),
                    LineKind::Context => (' ', Style::default().fg(theme.context)),
                };
                // Moved lines (git `--color-moved`) get the zebra hues — the
                // moved-in (+) side and moved-out (-) side each get their own
                // theme hue so they read as relocations, not genuine add/removes.
                if dl.moved {
                    prefix_style = match dl.kind {
                        LineKind::Add => Style::default().fg(theme.moved_add),
                        LineKind::Remove => Style::default().fg(theme.moved_remove),
                        LineKind::Context => prefix_style,
                    };
                }

                // Always feed the highlighter in order so its token state stays
                // correct even when a context line is collapsed (not rendered).
                let highlighted = highlighter.highlight_line(&mut hl, &dl.content);

                // Suppress a line when the all-context toggle hides it OR when it
                // falls inside a collapsed fold's hidden range. Either way the
                // counters still advance below, so numbering jumps the hidden run.
                let collapsed =
                    (opts.context_collapsed && dl.kind == LineKind::Context) || hidden[i];
                if !collapsed {
                    let (row_bg, emph_bg) = row_tint(dl, theme);
                    let mut spans: Vec<Span<'static>> = Vec::new();
                    // Far-left change-indicator bar: a 1-col vivid block for
                    // add/remove, a plain space for context. Its bg is set
                    // explicitly so the full-row tint (apply_row_bg) skips it and
                    // it stays drawn even when line numbers are toggled off.
                    let bar_style = match dl.kind {
                        LineKind::Add => Style::default().bg(theme.add),
                        LineKind::Remove => Style::default().bg(theme.remove),
                        LineKind::Context => Style::default(),
                    };
                    spans.push(Span::styled(" ".to_string(), bar_style));
                    if opts.line_numbers {
                        // Dual gutter: context shows both numbers, remove the old
                        // number only, add the new number only.
                        let (old_disp, new_disp) = match dl.kind {
                            LineKind::Context => (old_line, new_line),
                            LineKind::Remove => (old_line, None),
                            LineKind::Add => (None, new_line),
                        };
                        let gutter = format!(
                            "{} {} ",
                            gutter_cell(old_disp, gw),
                            gutter_cell(new_disp, gw)
                        );
                        spans.push(Span::styled(gutter, Style::default().fg(theme.gutter)));
                    }
                    spans.push(Span::styled(prefix.to_string(), prefix_style));
                    // Emit the syntax-highlighted content, layering word-level
                    // emphasis over the changed byte ranges. The text is identical
                    // either way — only the style differs.
                    let mut byte_pos = 0usize;
                    for (color, text) in highlighted {
                        let len = text.len();
                        push_content_spans(
                            &mut spans,
                            &text,
                            byte_pos,
                            color,
                            &dl.emphasis,
                            emph_bg,
                        );
                        byte_pos += len;
                    }
                    // Added/removed rows get a full-row background tint: pad to the
                    // row width with spaces, then paint the tint behind every span
                    // that isn't already carrying the stronger emphasis background.
                    if let Some(bg) = row_bg {
                        pad_to_width(&mut spans, width as usize);
                        apply_row_bg(&mut spans, bg);
                    }
                    // Record the blame key for lines that exist on the new side
                    // (context + adds), keyed by this row, before the push.
                    if let Some(n) = new_line {
                        if dl.kind != LineKind::Remove {
                            blame_keys.insert(out.len(), (file_idx, n));
                        }
                    }
                    out.push(Line::from(spans));
                }

                // Advance the counters for the side(s) this line exists on, even
                // when a context line is collapsed (so numbering stays correct).
                match dl.kind {
                    LineKind::Context => {
                        old_line = old_line.map(|n| n + 1);
                        new_line = new_line.map(|n| n + 1);
                    }
                    LineKind::Remove => old_line = old_line.map(|n| n + 1),
                    LineKind::Add => new_line = new_line.map(|n| n + 1),
                }
            }
        }

        out.push(Line::from(""));
    }
    RenderedDiff {
        lines: out,
        file_starts,
        hunk_starts,
        fold_bars,
        blame_keys,
    }
}

/// One collapsible fold bar: `▼ N unchanged lines` when collapsed, `▲ N
/// unchanged lines` when expanded (the expanded header gives re-collapse a row
/// to target). Drawn dim in the gutter color so it reads as chrome, not content.
fn fold_bar_line(hidden: usize, expanded: bool, theme: &Theme) -> Line<'static> {
    let arrow = if expanded { '▲' } else { '▼' };
    let text = format!("  {arrow} {hidden} unchanged lines");
    Line::from(Span::styled(
        text,
        Style::default()
            .fg(theme.gutter)
            .add_modifier(Modifier::DIM),
    ))
}

/// Emit the generated-file collapse bar for `file` when it is generated,
/// recording its fold id in `fold_bars`. Returns true when the body should be
/// hidden (generated AND not expanded); false (a no-op) for ordinary files.
/// Shared by both layouts so generated files collapse the same way in each.
fn file_collapse_bar(
    out: &mut Vec<Line<'static>>,
    fold_bars: &mut Vec<(usize, FoldId)>,
    file: &FileDiff,
    file_idx: usize,
    folds: &HashSet<FoldId>,
    theme: &Theme,
) -> bool {
    if !is_generated(&file.path) {
        return false;
    }
    let id = file_fold_id(file_idx);
    let hidden: usize = file.hunks.iter().map(|h| h.lines.len()).sum();
    fold_bars.push((out.len(), id));
    let expanded = is_expanded(folds, id);
    out.push(file_fold_bar_line(hidden, expanded, theme));
    !expanded
}

/// The whole-file fold bar for a generated file: collapsed (`▼`) hides the whole
/// diff body behind a one-line summary; expanded (`▲`) gives re-collapse a row to
/// target. Drawn like a normal fold bar so it reads as the same chrome.
fn file_fold_bar_line(hidden: usize, expanded: bool, theme: &Theme) -> Line<'static> {
    let arrow = if expanded { '▲' } else { '▼' };
    let text = format!("  {arrow} {hidden} lines hidden (generated)");
    Line::from(Span::styled(
        text,
        Style::default()
            .fg(theme.gutter)
            .add_modifier(Modifier::DIM),
    ))
}

/// Column separator between the old (left) and new (right) panes in split view.
const SEP: u16 = 1;

/// The split (side-by-side) layout: per file a full-width header, then per hunk
/// the optional `@@` header and paired old/new rows. Context lines appear on
/// both sides; a change group (a run of removes then adds) emits
/// `max(removes, adds)` rows, blanking the shorter side. Each side is a fixed
/// `col_w`-wide cell so the separator and right column always align.
///
/// `width` is the available content width. `col_w = (width - SEP) / 2`, clamped
/// to at least 1 so a pathologically narrow pane truncates to a single column
/// instead of panicking (the app uses `auto` to avoid split below ~120 cols, so
/// this clamp only bites when split is forced on a tiny terminal).
fn split_rows(
    model: &DiffModel,
    highlighter: &Highlighter,
    theme: &Theme,
    opts: &RenderOptions,
    folds: &HashSet<FoldId>,
    width: u16,
) -> RenderedDiff {
    let col_w = (width.saturating_sub(SEP) / 2).max(1) as usize;

    let mut out: Vec<Line<'static>> = Vec::new();
    let mut file_starts: Vec<usize> = Vec::with_capacity(model.files.len());
    let mut hunk_starts: Vec<usize> = Vec::new();
    // Split emits only whole-file (generated) fold bars, not per-hunk context folds.
    let mut fold_bars: Vec<(usize, FoldId)> = Vec::new();
    for (file_idx, file) in model.files.iter().enumerate() {
        // Same file_starts contract as stack: the header row index per file.
        file_starts.push(out.len());
        push_file_header(&mut out, file, theme);

        let file_collapsed =
            file_collapse_bar(&mut out, &mut fold_bars, file, file_idx, folds, theme);

        let syntax = highlighter.syntax_for_path(&file.path);

        for hunk in &file.hunks {
            if file_collapsed {
                break;
            }
            hunk_starts.push(out.len());
            push_hunk_header(&mut out, hunk, opts, theme);

            // Old- and new-side line counters, seeded from the header. Old
            // advances on context+removes, new on context+adds.
            let mut old_line = parse_hunk_old_start(&hunk.header);
            let mut new_line = parse_hunk_new_start(&hunk.header);

            // Pre-highlight every line of the hunk in order so syntect's token
            // state matches what stack would feed it; cells then reference the
            // precomputed spans regardless of which side they land on.
            let mut hl = highlighter.line_highlighter(syntax);
            let highlighted: Vec<Vec<(Color, String)>> = hunk
                .lines
                .iter()
                .map(|dl| highlighter.highlight_line(&mut hl, &dl.content))
                .collect();

            let mut i = 0;
            while i < hunk.lines.len() {
                let dl = &hunk.lines[i];
                if dl.kind == LineKind::Context {
                    if !opts.context_collapsed {
                        let left = build_cell(dl, &highlighted[i], old_line, theme, opts, col_w);
                        let right = build_cell(dl, &highlighted[i], new_line, theme, opts, col_w);
                        out.push(compose_row(left, right, theme));
                    }
                    old_line = old_line.map(|n| n + 1);
                    new_line = new_line.map(|n| n + 1);
                    i += 1;
                    continue;
                }

                // A change group: a maximal run of removes followed by a
                // maximal run of adds (either run may be empty).
                let mut removes: Vec<usize> = Vec::new();
                while i < hunk.lines.len() && hunk.lines[i].kind == LineKind::Remove {
                    removes.push(i);
                    i += 1;
                }
                let mut adds: Vec<usize> = Vec::new();
                while i < hunk.lines.len() && hunk.lines[i].kind == LineKind::Add {
                    adds.push(i);
                    i += 1;
                }

                let rows = removes.len().max(adds.len());
                for r in 0..rows {
                    let left = match removes.get(r) {
                        Some(&idx) => {
                            let cell = build_cell(
                                &hunk.lines[idx],
                                &highlighted[idx],
                                old_line,
                                theme,
                                opts,
                                col_w,
                            );
                            old_line = old_line.map(|n| n + 1);
                            cell
                        }
                        None => blank_cell(col_w),
                    };
                    let right = match adds.get(r) {
                        Some(&idx) => {
                            let cell = build_cell(
                                &hunk.lines[idx],
                                &highlighted[idx],
                                new_line,
                                theme,
                                opts,
                                col_w,
                            );
                            new_line = new_line.map(|n| n + 1);
                            cell
                        }
                        None => blank_cell(col_w),
                    };
                    out.push(compose_row(left, right, theme));
                }
            }
        }

        out.push(Line::from(""));
    }
    RenderedDiff {
        lines: out,
        file_starts,
        hunk_starts,
        fold_bars,
        // Blame gutter is stack-only; split/vertical render no blame keys.
        blame_keys: HashMap::new(),
    }
}

/// The vertical (top/bottom) layout: per hunk, the old block (context + removes)
/// stacked above the new block (context + adds), each rendered full width with a
/// dim rule between them. Reuses [`build_cell`] for the per-line rendering, so a
/// row looks exactly like one side of the split layout. Line numbers stay
/// correct because the old counter advances on context+remove and the new on
/// context+add — the same as the interleaved walk, just split into two passes.
fn vertical_rows(
    model: &DiffModel,
    highlighter: &Highlighter,
    theme: &Theme,
    opts: &RenderOptions,
    folds: &HashSet<FoldId>,
    width: u16,
) -> RenderedDiff {
    let full_w = width.max(1) as usize;

    let mut out: Vec<Line<'static>> = Vec::new();
    let mut file_starts: Vec<usize> = Vec::with_capacity(model.files.len());
    let mut hunk_starts: Vec<usize> = Vec::new();
    let mut fold_bars: Vec<(usize, FoldId)> = Vec::new();
    for (file_idx, file) in model.files.iter().enumerate() {
        file_starts.push(out.len());
        push_file_header(&mut out, file, theme);

        let file_collapsed =
            file_collapse_bar(&mut out, &mut fold_bars, file, file_idx, folds, theme);

        let syntax = highlighter.syntax_for_path(&file.path);

        for hunk in &file.hunks {
            if file_collapsed {
                break;
            }
            hunk_starts.push(out.len());
            push_hunk_header(&mut out, hunk, opts, theme);

            let mut old_line = parse_hunk_old_start(&hunk.header);
            let mut new_line = parse_hunk_new_start(&hunk.header);

            // Pre-highlight in order so syntect's token state matches stack/split.
            let mut hl = highlighter.line_highlighter(syntax);
            let highlighted: Vec<Vec<(Color, String)>> = hunk
                .lines
                .iter()
                .map(|dl| highlighter.highlight_line(&mut hl, &dl.content))
                .collect();

            // Old block: context + removes, numbered on the old side.
            let mut old_emitted = 0usize;
            for (i, dl) in hunk.lines.iter().enumerate() {
                if dl.kind == LineKind::Add {
                    continue;
                }
                let cell = build_cell(dl, &highlighted[i], old_line, theme, opts, full_w);
                out.push(Line::from(cell));
                old_line = old_line.map(|n| n + 1);
                old_emitted += 1;
            }
            // Dim rule between the blocks — only when BOTH are non-empty, so a
            // pure-add or pure-delete hunk doesn't leave a rule floating against
            // an empty side.
            let new_nonempty = hunk.lines.iter().any(|dl| dl.kind != LineKind::Remove);
            if old_emitted > 0 && new_nonempty {
                out.push(Line::from(Span::styled(
                    "─".repeat(full_w),
                    Style::default()
                        .fg(theme.gutter)
                        .add_modifier(Modifier::DIM),
                )));
            }
            // New block: context + adds, numbered on the new side.
            for (i, dl) in hunk.lines.iter().enumerate() {
                if dl.kind == LineKind::Remove {
                    continue;
                }
                let cell = build_cell(dl, &highlighted[i], new_line, theme, opts, full_w);
                out.push(Line::from(cell));
                new_line = new_line.map(|n| n + 1);
            }
        }

        out.push(Line::from(""));
    }
    RenderedDiff {
        lines: out,
        file_starts,
        hunk_starts,
        fold_bars,
        // Blame gutter is stack-only; split/vertical render no blame keys.
        blame_keys: HashMap::new(),
    }
}

/// Build one `col_w`-wide split cell: optional line-number gutter, the
/// `+`/`-`/space prefix (add/remove/move-colored exactly as in stack), then the
/// syntax-highlighted, word-emphasized content — all truncated and padded to
/// `col_w` so the separator and right column align.
fn build_cell(
    dl: &DiffLine,
    highlighted: &[(Color, String)],
    gutter: Option<usize>,
    theme: &Theme,
    opts: &RenderOptions,
    col_w: usize,
) -> Vec<Span<'static>> {
    let (prefix, prefix_style) = prefix_for(dl, theme);
    let (row_bg, emph_bg) = row_tint(dl, theme);
    let mut spans: Vec<Span<'static>> = Vec::new();
    if opts.line_numbers {
        let g = match gutter {
            Some(n) => format!("{n:>4} "),
            None => BLANK_GUTTER.to_string(),
        };
        spans.push(Span::styled(g, Style::default().fg(theme.gutter)));
    }
    spans.push(Span::styled(prefix.to_string(), prefix_style));
    append_content_spans(&mut spans, highlighted, &dl.emphasis, emph_bg);
    // The cell is already padded to `col_w` by `fit_to_width`, so the tint reaches
    // the column edge once painted behind every not-yet-backgrounded span.
    let mut cell = fit_to_width(spans, col_w);
    if let Some(bg) = row_bg {
        apply_row_bg(&mut cell, bg);
    }
    cell
}

/// The full-row background tint and the word-emphasis background for a diff
/// line. Added/removed lines get the theme's subtle add/remove row tint plus a
/// medium emphasis tint (`add_emph_bg`/`remove_emph_bg`) for the changed words —
/// clearly stronger than the row but well short of the vivid foreground, so
/// intra-line emphasis reads without a jarring bright box. Context lines get
/// neither. Moved lines (git `--color-moved`) are left untinted so their distinct
/// prefix hue keeps them from reading as a plain add/remove.
fn row_tint(dl: &DiffLine, theme: &Theme) -> (Option<Color>, Option<Color>) {
    if dl.moved {
        return (None, None);
    }
    match dl.kind {
        LineKind::Add => (Some(theme.add_bg), Some(theme.add_emph_bg)),
        LineKind::Remove => (Some(theme.remove_bg), Some(theme.remove_emph_bg)),
        LineKind::Context => (None, None),
    }
}

/// Append a trailing space run so `spans` reach `width` display columns (chars).
/// No-op when already at/over `width`. The pad is plain spaces, matching the
/// terminal's own blank-cell fill, so it never changes a snapshot's symbols.
fn pad_to_width(spans: &mut Vec<Span<'static>>, width: usize) {
    let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
    if used < width {
        spans.push(Span::raw(" ".repeat(width - used)));
    }
}

/// Paint `bg` behind every span that has no explicit background, leaving the
/// emphasized word ranges on their stronger hue. Pure style mutation — the
/// rendered text is untouched.
fn apply_row_bg(spans: &mut [Span<'static>], bg: Color) {
    for span in spans.iter_mut() {
        if span.style.bg.is_none() {
            span.style = span.style.bg(bg);
        }
    }
}

/// An empty `col_w`-wide cell (the blank side of a lopsided change group).
fn blank_cell(col_w: usize) -> Vec<Span<'static>> {
    fit_to_width(Vec::new(), col_w)
}

/// The leading prefix char and its style for a diff line, matching stack: green
/// `+` / red `-` / gray space, with git `--color-moved` lines recolored to the
/// cyan/magenta zebra hues.
fn prefix_for(dl: &DiffLine, theme: &Theme) -> (char, Style) {
    let (prefix, mut style) = match dl.kind {
        LineKind::Add => ('+', Style::default().fg(theme.add)),
        LineKind::Remove => ('-', Style::default().fg(theme.remove)),
        LineKind::Context => (' ', Style::default().fg(theme.context)),
    };
    if dl.moved {
        style = match dl.kind {
            LineKind::Add => Style::default().fg(theme.moved_add),
            LineKind::Remove => Style::default().fg(theme.moved_remove),
            LineKind::Context => style,
        };
    }
    (prefix, style)
}

/// Append the syntax-highlighted content of a line (with word emphasis) to
/// `spans`, reusing the shared [`push_content_spans`] helper per token.
fn append_content_spans(
    spans: &mut Vec<Span<'static>>,
    highlighted: &[(Color, String)],
    emphasis: &[(usize, usize)],
    emph_bg: Option<Color>,
) {
    let mut byte_pos = 0usize;
    for (color, text) in highlighted {
        let len = text.len();
        push_content_spans(spans, text, byte_pos, *color, emphasis, emph_bg);
        byte_pos += len;
    }
}

/// Truncate `spans` to at most `width` columns and right-pad with spaces to
/// exactly `width`. Width is measured in chars (matching the sidebar's
/// `truncate_path`); truncation cuts on char boundaries so multibyte content
/// never panics. The padding span carries no style, so it inherits the
/// terminal default.
fn fit_to_width(spans: Vec<Span<'static>>, width: usize) -> Vec<Span<'static>> {
    let mut out: Vec<Span<'static>> = Vec::new();
    let mut used = 0usize;
    for span in spans {
        if used >= width {
            break;
        }
        let remaining = width - used;
        let count = span.content.chars().count();
        if count <= remaining {
            used += count;
            out.push(span);
        } else {
            // Cut this span on a char boundary; we have filled the column.
            let truncated: String = span.content.chars().take(remaining).collect();
            used = width;
            out.push(Span::styled(truncated, span.style));
            break;
        }
    }
    if used < width {
        out.push(Span::raw(" ".repeat(width - used)));
    }
    out
}

/// Compose a left cell, the column separator, and a right cell into one line.
fn compose_row(
    mut left: Vec<Span<'static>>,
    right: Vec<Span<'static>>,
    theme: &Theme,
) -> Line<'static> {
    left.push(Span::styled("│", Style::default().fg(theme.gutter)));
    left.extend(right);
    Line::from(left)
}

/// The index of the file whose rendered region contains scroll `offset`: the
/// last file whose start line is `<= offset`. This derives the active file from
/// the scroll position so plain scrolling moves through files. Returns 0 when
/// `file_starts` is empty or `offset` precedes the first file. `file_starts` is
/// ascending (as produced by [`render_diff`]), so a partition point locates the
/// boundary in O(log n).
pub fn file_at_offset(file_starts: &[usize], offset: usize) -> usize {
    match file_starts.partition_point(|&start| start <= offset) {
        0 => 0,
        n => n - 1,
    }
}

/// Parse the old-side start line from a unified hunk header `@@ -old,n +new,m @@`,
/// returning `old`. Mirrors [`parse_hunk_new_start`] for the left column's
/// gutter. Returns `None` if no `-<digits>` group is present.
pub fn parse_hunk_old_start(header: &str) -> Option<usize> {
    // The first `-` in a hunk header precedes the old-side range; take the
    // leading digit run after it.
    let after_minus = header.split('-').nth(1)?;
    let digits: String = after_minus
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse().ok()
}

/// Parse the new-side start line from a unified hunk header
/// `@@ -old,n +new,m @@`, returning `new`. Returns `None` if no `+<digits>`
/// group is present. The optional trailing `,m` count and any text after the
/// closing `@@` are ignored.
pub fn parse_hunk_new_start(header: &str) -> Option<usize> {
    // The first `+` in a hunk header always precedes the new-side range; take
    // the leading digit run after it.
    let after_plus = header.split('+').nth(1)?;
    let digits: String = after_plus
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse().ok()
}

/// Append a syntax-highlighted content token (`text`, a slice of the line
/// content starting at byte `start`) to `spans`, underlining the sub-segments
/// that fall inside an `emphasis` byte range while keeping the syntax `color`.
///
/// The token is split only at emphasis boundaries, which sit on char boundaries
/// (word-diff ranges and syntect tokens both respect UTF-8), so slicing is safe
/// and the visible text is byte-for-byte unchanged — emphasis is style-only.
///
/// `emph_bg` is the medium emphasis tint painted behind emphasized ranges so the
/// changed words read distinctly against the subtle row tint (`None` leaves the
/// segment un-emphasized, e.g. for moved lines).
fn push_content_spans(
    spans: &mut Vec<Span<'static>>,
    text: &str,
    start: usize,
    color: Color,
    emphasis: &[(usize, usize)],
    emph_bg: Option<Color>,
) {
    if text.is_empty() {
        return;
    }
    let base = Style::default().fg(color);
    if emphasis.is_empty() {
        spans.push(Span::styled(text.to_string(), base));
        return;
    }
    // Walk the token's chars, flushing a span whenever the emphasized state
    // flips so each emitted span is uniformly emphasized or not.
    let mut seg_start = 0usize;
    let mut cur = byte_emphasized(start, emphasis);
    for (rel, _) in text.char_indices() {
        if rel == 0 {
            continue;
        }
        let here = byte_emphasized(start + rel, emphasis);
        if here != cur {
            spans.push(styled_segment(&text[seg_start..rel], base, cur, emph_bg));
            seg_start = rel;
            cur = here;
        }
    }
    spans.push(styled_segment(&text[seg_start..], base, cur, emph_bg));
}

/// Whether byte position `pos` lies inside any emphasis range.
fn byte_emphasized(pos: usize, emphasis: &[(usize, usize)]) -> bool {
    emphasis.iter().any(|&(s, e)| pos >= s && pos < e)
}

/// A content sub-span with the base syntax style; when emphasized it is painted
/// on the medium emphasis tint (`emph_bg`) so the changed words read distinctly
/// against the subtle row tint. No underline — the tint alone carries emphasis,
/// avoiding the jarring box-plus-underline double-treatment.
fn styled_segment(
    text: &str,
    base: Style,
    emphasized: bool,
    emph_bg: Option<Color>,
) -> Span<'static> {
    let style = match (emphasized, emph_bg) {
        (true, Some(bg)) => base.bg(bg),
        _ => base,
    };
    Span::styled(text.to_string(), style)
}

/// Emit a file's header line (and a marker line for binary files). Shared by the
/// stack and split layouts so the header format has one source of truth.
fn push_file_header(out: &mut Vec<Line<'static>>, file: &FileDiff, theme: &Theme) {
    out.push(Line::from(Span::styled(
        format!("── {} ", file.path),
        Style::default()
            .fg(theme.file_header)
            .add_modifier(Modifier::BOLD),
    )));
    if file.binary {
        out.push(Line::from(Span::styled(
            "  (binary file differs)",
            Style::default().fg(theme.gutter),
        )));
    }
}

/// Emit a hunk's `@@` header line when hunk headers are enabled. Shared by both
/// layouts.
fn push_hunk_header(
    out: &mut Vec<Line<'static>>,
    hunk: &Hunk,
    opts: &RenderOptions,
    theme: &Theme,
) {
    if opts.hunk_headers {
        out.push(Line::from(Span::styled(
            hunk.header.clone(),
            Style::default().fg(theme.hunk_header),
        )));
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;
    use crate::diff::parse_unified_diff;
    use ratatui::backend::TestBackend;
    use ratatui::widgets::Paragraph;
    use ratatui::Terminal;

    const SAMPLE: &str = "\
diff --git a/src/main.rs b/src/main.rs
index e69de29..4b825dc 100644
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,2 +1,3 @@
 fn main() {
-    println!(\"old\");
+    println!(\"new\");
+    // added line
 }
";

    #[test]
    fn changed_rows_get_full_width_background_tint() {
        let model = parse_unified_diff(SAMPLE);
        let highlighter = Highlighter::new();
        let theme = Theme::default();
        let width: u16 = 50;
        let lines = render_lines(
            &model,
            &highlighter,
            &theme,
            &RenderOptions::default(),
            width,
        );

        // Char width of a line's visible text (what the user sees / a snapshot
        // captures). Used to confirm the tint pad reaches the row width.
        let char_width = |l: &Line<'static>| -> usize {
            l.spans.iter().map(|s| s.content.chars().count()).sum()
        };
        // Whether EVERY span of a line carries a background (the tint covers the
        // whole row, including gutter, prefix, content, and the trailing pad).
        let fully_backed = |l: &Line<'static>| l.spans.iter().all(|s| s.style.bg.is_some());

        let added = lines
            .iter()
            .find(|l| l.to_string().contains("// added line"))
            .expect("added line present");
        assert!(
            added.spans.iter().any(|s| s.style.bg == Some(theme.add_bg)),
            "added row should carry the add_bg tint"
        );
        assert!(fully_backed(added), "add tint must span the full row");
        assert_eq!(char_width(added), width as usize, "add row padded to width");

        let removed = lines
            .iter()
            .find(|l| l.to_string().contains(r#"println!("old")"#))
            .expect("removed line present");
        assert!(
            removed
                .spans
                .iter()
                .any(|s| s.style.bg == Some(theme.remove_bg)),
            "removed row should carry the remove_bg tint"
        );
        assert!(fully_backed(removed), "remove tint must span the full row");

        // Context rows keep the default background (no tint, no forced pad span).
        let context = lines
            .iter()
            .find(|l| l.to_string().contains("fn main()"))
            .expect("context line present");
        assert!(
            context.spans.iter().all(|s| s.style.bg.is_none()),
            "context row must not be tinted"
        );
    }

    #[test]
    fn renders_diff_to_buffer() {
        let model = parse_unified_diff(SAMPLE);
        let highlighter = Highlighter::new();
        let lines = render_lines(
            &model,
            &highlighter,
            &Theme::default(),
            &RenderOptions::default(),
            50,
        );

        let backend = TestBackend::new(50, 12);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| f.render_widget(Paragraph::new(lines.clone()), f.area()))
            .unwrap();

        insta::assert_snapshot!(terminal.backend());
    }

    #[test]
    fn word_emphasis_is_style_only_text_unchanged() {
        use crate::worddiff::compute_word_emphasis;

        // A diff with a genuinely modified line so word emphasis is non-empty.
        let text = "\
diff --git a/x.txt b/x.txt
--- a/x.txt
+++ b/x.txt
@@ -1,1 +1,1 @@
-the quick brown fox
+the quick red fox
";
        let highlighter = Highlighter::new();
        let opts = RenderOptions::default();

        let plain = parse_unified_diff(text);
        let mut emphasized = parse_unified_diff(text);
        compute_word_emphasis(&mut emphasized);

        // Emphasis must actually be present (otherwise this test is vacuous).
        let has_emphasis = emphasized.files[0].hunks[0]
            .lines
            .iter()
            .any(|l| !l.emphasis.is_empty());
        assert!(has_emphasis, "fixture produced no emphasis");

        let render_text = |m: &DiffModel| -> Vec<String> {
            render_lines(m, &highlighter, &Theme::default(), &opts, 80)
                .iter()
                .map(|l| l.to_string())
                .collect()
        };
        // The visible text is byte-for-byte identical with or without emphasis.
        assert_eq!(render_text(&plain), render_text(&emphasized));
    }

    #[test]
    fn empty_model_shows_placeholder() {
        let highlighter = Highlighter::new();
        let lines = render_lines(
            &DiffModel::default(),
            &highlighter,
            &Theme::default(),
            &RenderOptions::default(),
            80,
        );
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn hunk_headers_off_omits_at_at_lines() {
        let model = parse_unified_diff(SAMPLE);
        let highlighter = Highlighter::new();
        let opts = RenderOptions {
            hunk_headers: false,
            ..RenderOptions::default()
        };
        let lines = render_lines(&model, &highlighter, &Theme::default(), &opts, 80);
        let text: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
        assert!(
            !text.iter().any(|l| l.starts_with("@@")),
            "hunk header leaked: {text:?}"
        );
    }

    #[test]
    fn context_collapsed_drops_context_lines() {
        let model = parse_unified_diff(SAMPLE);
        let highlighter = Highlighter::new();
        let opts = RenderOptions {
            line_numbers: false,
            context_collapsed: true,
            ..RenderOptions::default()
        };
        let lines = render_lines(&model, &highlighter, &Theme::default(), &opts, 80);
        let text: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
        // The context rows " fn main() {" and " }" must be gone; add/remove stay.
        assert!(
            !text.iter().any(|l| l.contains("fn main")),
            "context line not collapsed: {text:?}"
        );
        assert!(
            text.iter().any(|l| l.contains("old")),
            "removed line missing"
        );
        assert!(text.iter().any(|l| l.contains("new")), "added line missing");
    }

    #[test]
    fn line_numbers_gutter_uses_new_side_numbering() {
        let model = parse_unified_diff(SAMPLE);
        let highlighter = Highlighter::new();
        let lines = render_lines(
            &model,
            &highlighter,
            &Theme::default(),
            &RenderOptions::default(),
            80,
        );
        let text: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
        // Context "fn main" is new line 1; the added "new" line is new line 2.
        assert!(
            text.iter()
                .any(|l| l.contains("   1 ") && l.contains("fn main")),
            "expected line 1 gutter on context: {text:?}"
        );
        assert!(
            text.iter()
                .any(|l| l.contains("   2 ") && l.contains("new")),
            "expected line 2 gutter on added line: {text:?}"
        );
    }

    #[test]
    fn stack_dual_gutter_and_change_bar() {
        let model = parse_unified_diff(SAMPLE);
        let highlighter = Highlighter::new();
        let theme = Theme::default();
        let rendered = render_diff(
            &model,
            &highlighter,
            &theme,
            &RenderOptions::default(),
            &HashSet::new(),
            80,
        );

        let find = |needle: &str| -> Line<'static> {
            rendered
                .lines
                .iter()
                .find(|l| l.to_string().contains(needle))
                .unwrap_or_else(|| panic!("no row with {needle:?}"))
                .clone()
        };

        // Every diff row leads with a 1-column change-bar span; on add/remove it
        // carries a vivid background, on context it is a plain unbacked space.
        let bar = |l: &Line<'static>| l.spans.first().expect("a leading bar span").clone();

        // Context line: bar is a plain space, gutter shows BOTH old and new (1, 1).
        let context = find("fn main()");
        let cbar = bar(&context);
        assert_eq!(cbar.content.as_ref(), " ", "context bar is one column");
        assert!(cbar.style.bg.is_none(), "context bar carries no background");
        let ctx_text = context.to_string();
        // gutter width is 3 here, so "  1   1 " precedes the prefix+content.
        assert!(
            ctx_text.contains("  1   1 "),
            "context should show both line numbers: {ctx_text:?}"
        );

        // Removed line: bar uses the vivid remove color; gutter shows the old
        // number and a BLANK new column.
        let removed = find(r#"println!("old")"#);
        assert_eq!(
            bar(&removed).style.bg,
            Some(theme.remove),
            "remove bar should use the vivid remove background"
        );
        // old = 2, new blank: "  2     -" (3-wide old, space, 3 blanks, space).
        assert!(
            removed.to_string().contains("  2     -"),
            "removed row should show old# only, blank new#: {:?}",
            removed.to_string()
        );

        // Added line: bar uses the vivid add color; gutter shows a BLANK old
        // column and the new number.
        let added = find("// added line");
        assert_eq!(
            bar(&added).style.bg,
            Some(theme.add),
            "add bar should use the vivid add background"
        );
        // old blank, new = 3: "      3 +" (3 blanks, space, "  3", space).
        assert!(
            added.to_string().contains("      3 +"),
            "added row should show new# only, blank old#: {:?}",
            added.to_string()
        );
    }

    #[test]
    fn stack_change_bar_kept_when_line_numbers_off() {
        let model = parse_unified_diff(SAMPLE);
        let highlighter = Highlighter::new();
        let theme = Theme::default();
        let opts = RenderOptions {
            line_numbers: false,
            ..RenderOptions::default()
        };
        let rendered = render_diff(&model, &highlighter, &theme, &opts, &HashSet::new(), 80);

        let added = rendered
            .lines
            .iter()
            .find(|l| l.to_string().contains("// added line"))
            .expect("added line present");
        // First span is still the 1-col change-bar (vivid add bg)...
        assert_eq!(added.spans[0].content.as_ref(), " ");
        assert_eq!(added.spans[0].style.bg, Some(theme.add));
        // ...and with numbers off, the prefix follows immediately (no digits).
        assert!(
            added.to_string().starts_with(" +"),
            "numbers-off row is bar + prefix: {:?}",
            added.to_string()
        );
    }

    const TWO_FILE: &str = "\
diff --git a/a.txt b/a.txt
index 1111111..2222222 100644
--- a/a.txt
+++ b/a.txt
@@ -1,2 +1,2 @@
 keep
-old
+new
@@ -10 +10,2 @@
 ctx
+added
diff --git a/b.txt b/b.txt
index 3333333..4444444 100644
--- a/b.txt
+++ b/b.txt
@@ -1 +1 @@
-removed
+inserted
";

    #[test]
    fn file_summaries_counts_adds_and_removes_per_file() {
        let model = parse_unified_diff(TWO_FILE);
        let summaries = file_summaries(&model);
        assert_eq!(summaries.len(), 2);
        // a.txt: one Remove (old), two Add (new, added).
        assert_eq!(summaries[0].path, "a.txt");
        assert_eq!(summaries[0].additions, 2);
        assert_eq!(summaries[0].deletions, 1);
        // b.txt: one Remove (removed), one Add (inserted).
        assert_eq!(summaries[1].path, "b.txt");
        assert_eq!(summaries[1].additions, 1);
        assert_eq!(summaries[1].deletions, 1);
    }

    #[test]
    fn file_summaries_empty_model_is_empty() {
        assert!(file_summaries(&DiffModel::default()).is_empty());
    }

    #[test]
    fn render_diff_file_starts_point_at_each_file_header() {
        let model = parse_unified_diff(TWO_FILE);
        let highlighter = Highlighter::new();
        let rendered = render_diff(
            &model,
            &highlighter,
            &Theme::default(),
            &RenderOptions::default(),
            &HashSet::new(),
            80,
        );

        // One entry per file, strictly ascending.
        assert_eq!(rendered.file_starts.len(), 2);
        assert!(rendered.file_starts[0] < rendered.file_starts[1]);
        assert_eq!(rendered.file_starts[0], 0);

        // Each recorded start is that file's header line.
        let first = rendered.lines[rendered.file_starts[0]].to_string();
        assert!(first.contains("a.txt"), "first header was {first:?}");
        let second = rendered.lines[rendered.file_starts[1]].to_string();
        assert!(second.contains("b.txt"), "second header was {second:?}");
    }

    #[test]
    fn render_lines_matches_render_diff_lines() {
        let model = parse_unified_diff(TWO_FILE);
        let highlighter = Highlighter::new();
        let opts = RenderOptions::default();
        let plain = render_lines(&model, &highlighter, &Theme::default(), &opts, 80);
        let rendered = render_diff(
            &model,
            &highlighter,
            &Theme::default(),
            &opts,
            &HashSet::new(),
            80,
        );
        assert_eq!(plain.len(), rendered.lines.len());
    }

    #[test]
    fn parse_hunk_new_start_handles_normal_and_edge_headers() {
        assert_eq!(parse_hunk_new_start("@@ -1,2 +1,3 @@"), Some(1));
        assert_eq!(parse_hunk_new_start("@@ -10 +10,2 @@"), Some(10));
        assert_eq!(parse_hunk_new_start("@@ -1 +1 @@"), Some(1));
        // Trailing section heading after the closing `@@` is ignored.
        assert_eq!(
            parse_hunk_new_start("@@ -5,3 +42,6 @@ fn foo(a + b)"),
            Some(42)
        );
        // No `+` group at all.
        assert_eq!(parse_hunk_new_start("not a hunk header"), None);
        assert_eq!(parse_hunk_new_start("@@ -1,2 @@"), None);
    }

    #[test]
    fn file_at_offset_derives_active_file_from_scroll() {
        // Empty: always file 0.
        assert_eq!(file_at_offset(&[], 0), 0);
        assert_eq!(file_at_offset(&[], 42), 0);

        let starts = [0usize, 10, 25];
        // Before / at the first start -> file 0.
        assert_eq!(file_at_offset(&starts, 0), 0);
        assert_eq!(file_at_offset(&starts, 5), 0);
        // Exactly at a start -> that file.
        assert_eq!(file_at_offset(&starts, 10), 1);
        assert_eq!(file_at_offset(&starts, 25), 2);
        // Between starts -> the earlier file.
        assert_eq!(file_at_offset(&starts, 24), 1);
        assert_eq!(file_at_offset(&starts, 11), 1);
        // Past the last start -> the last file.
        assert_eq!(file_at_offset(&starts, 999), 2);

        // A leading non-zero start (offset before the first file) -> file 0.
        let offset_starts = [5usize, 20];
        assert_eq!(file_at_offset(&offset_starts, 0), 0);
        assert_eq!(file_at_offset(&offset_starts, 4), 0);
        assert_eq!(file_at_offset(&offset_starts, 5), 0);
        assert_eq!(file_at_offset(&offset_starts, 20), 1);
    }

    #[test]
    fn parse_hunk_old_start_handles_normal_and_edge_headers() {
        assert_eq!(parse_hunk_old_start("@@ -1,2 +1,3 @@"), Some(1));
        assert_eq!(parse_hunk_old_start("@@ -10 +10,2 @@"), Some(10));
        assert_eq!(parse_hunk_old_start("@@ -5,3 +42,6 @@ fn foo()"), Some(5));
        // No `-` group at all.
        assert_eq!(parse_hunk_old_start("not a hunk header"), None);
        assert_eq!(parse_hunk_old_start("@@ +1,2 @@"), None);
    }

    /// Split builds a side-by-side view: at a wide width, each composed row is a
    /// left cell + `│` separator + right cell, with context on both sides and a
    /// change group placing remove-content on the left and add-content on the
    /// right. Cells are padded to a fixed column width so the separator aligns.
    #[test]
    fn split_mode_pairs_old_and_new_into_aligned_columns() {
        let model = parse_unified_diff(SAMPLE);
        let highlighter = Highlighter::new();
        let width: u16 = 80;
        let opts = RenderOptions {
            mode: LayoutMode::Split,
            ..RenderOptions::default()
        };
        let rendered = render_diff(
            &model,
            &highlighter,
            &Theme::default(),
            &opts,
            &HashSet::new(),
            width,
        );
        let col_w = ((width - SEP) / 2) as usize;

        // Every composed paired row contains exactly one separator, and the left
        // cell occupies exactly `col_w` columns before it (alignment guarantee).
        let sep_rows: Vec<&Line<'static>> = rendered
            .lines
            .iter()
            .filter(|l| l.to_string().contains('│'))
            .collect();
        assert!(!sep_rows.is_empty(), "split produced no paired rows");
        for line in &sep_rows {
            let s = line.to_string();
            let sep_at = s.find('│').unwrap();
            // Left-of-separator width is exactly col_w (chars, not bytes).
            let left_chars = s[..sep_at].chars().count();
            assert_eq!(left_chars, col_w, "left cell not padded to col_w: {s:?}");
        }

        // The context line `fn main() {` appears on BOTH sides of one row.
        let ctx = sep_rows
            .iter()
            .map(|l| l.to_string())
            .find(|s| s.matches("fn main() {").count() == 2)
            .expect("context line should appear on both columns");
        let sep_at = ctx.find('│').unwrap();
        assert!(ctx[..sep_at].contains("fn main() {"), "ctx missing left");
        assert!(ctx[sep_at..].contains("fn main() {"), "ctx missing right");

        // The change group: `println!("old")` (remove) on the LEFT, the matching
        // `println!("new")` (add) on the RIGHT of the same composed row.
        let change = sep_rows
            .iter()
            .map(|l| l.to_string())
            .find(|s| s.contains("old"))
            .expect("removed line should be present");
        let sep_at = change.find('│').unwrap();
        assert!(
            change[..sep_at].contains(r#"println!("old")"#),
            "remove not on left: {change:?}"
        );
        assert!(
            change[sep_at..].contains(r#"println!("new")"#),
            "add not on right: {change:?}"
        );
    }

    /// Stack and split disagree on shape: split emits the `│` separator that
    /// stack never does, and stack's row count differs from split's paired rows.
    #[test]
    fn stack_and_split_produce_distinct_shapes() {
        let model = parse_unified_diff(TWO_FILE);
        let highlighter = Highlighter::new();
        let stack = render_diff(
            &model,
            &highlighter,
            &Theme::default(),
            &RenderOptions::default(),
            &HashSet::new(),
            80,
        );
        let split = render_diff(
            &model,
            &highlighter,
            &Theme::default(),
            &RenderOptions {
                mode: LayoutMode::Split,
                ..RenderOptions::default()
            },
            &HashSet::new(),
            80,
        );
        // Stack never emits the column separator; split does.
        assert!(stack.lines.iter().all(|l| !l.to_string().contains('│')));
        assert!(split.lines.iter().any(|l| l.to_string().contains('│')));
        // Both record one file_start per file (sidebar navigation contract).
        assert_eq!(stack.file_starts.len(), split.file_starts.len());
        assert_eq!(split.file_starts.len(), 2);
        assert_eq!(split.file_starts[0], 0);
    }

    /// A pure deletion (removes with no matching adds) blanks the right column;
    /// a pure insertion blanks the left. Both still pad to `col_w`.
    #[test]
    fn split_blanks_the_shorter_side_of_a_change_group() {
        // One pure-remove and one pure-add hunk.
        let text = "\
diff --git a/x.txt b/x.txt
--- a/x.txt
+++ b/x.txt
@@ -1,2 +1,1 @@
-gone one
-gone two
 kept
@@ -10,1 +10,3 @@
 anchor
+brand new
";
        let model = parse_unified_diff(text);
        let highlighter = Highlighter::new();
        let width: u16 = 60;
        let opts = RenderOptions {
            mode: LayoutMode::Split,
            ..RenderOptions::default()
        };
        let rendered = render_diff(
            &model,
            &highlighter,
            &Theme::default(),
            &opts,
            &HashSet::new(),
            width,
        );
        let col_w = ((width - SEP) / 2) as usize;

        let row_with = |needle: &str| -> String {
            rendered
                .lines
                .iter()
                .map(|l| l.to_string())
                .find(|s| s.contains(needle) && s.contains('│'))
                .unwrap_or_else(|| panic!("no row with {needle:?}"))
        };

        // Pure remove: "gone one" on the left, right side blank (spaces only).
        let del = row_with("gone one");
        let sep_at = del.find('│').unwrap();
        assert!(del[..sep_at].contains("gone one"));
        assert!(
            del[sep_at + '│'.len_utf8()..].trim().is_empty(),
            "right side should be blank for a pure delete: {del:?}"
        );

        // Pure add: left side blank, "brand new" on the right.
        let ins = row_with("brand new");
        let sep_at = ins.find('│').unwrap();
        assert!(
            ins[..sep_at].trim().is_empty(),
            "left side should be blank for a pure insert: {ins:?}"
        );
        assert!(ins[sep_at..].contains("brand new"));
        // Left cell still padded to col_w even when blank.
        assert_eq!(ins[..sep_at].chars().count(), col_w);
    }

    /// Tiny widths must clamp the column to >= 1 and never panic, even on
    /// multibyte content (truncation stays on char boundaries).
    #[test]
    fn split_narrow_width_and_multibyte_do_not_panic() {
        let text = "\
diff --git a/u.txt b/u.txt
--- a/u.txt
+++ b/u.txt
@@ -1,1 +1,1 @@
-café crème brûlée ☕ 日本語
+café latte ☕ 日本語テキスト
";
        let model = parse_unified_diff(text);
        let highlighter = Highlighter::new();
        let opts = RenderOptions {
            mode: LayoutMode::Split,
            ..RenderOptions::default()
        };
        // Widths from pathological (0/1/2) up to comfortable must all render.
        for w in [0u16, 1, 2, 3, 10, 40] {
            let rendered = render_diff(
                &model,
                &highlighter,
                &Theme::default(),
                &opts,
                &HashSet::new(),
                w,
            );
            assert!(!rendered.lines.is_empty(), "width {w} produced no rows");
        }
    }

    #[test]
    fn renders_split_layout_to_buffer() {
        let model = parse_unified_diff(TWO_FILE);
        let highlighter = Highlighter::new();
        let opts = RenderOptions {
            mode: LayoutMode::Split,
            ..RenderOptions::default()
        };
        let lines = render_lines(&model, &highlighter, &Theme::default(), &opts, 120);

        let backend = TestBackend::new(120, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| f.render_widget(Paragraph::new(lines.clone()), f.area()))
            .unwrap();

        insta::assert_snapshot!(terminal.backend());
    }

    // A 20-line file with one change in the middle, rendered with FULL context
    // (as `git diff --unified=100000` produces). Lines 1-9 precede the change,
    // lines 11-20 follow it. With FOLD_MARGIN=3 this yields two folds: a leading
    // fold hiding "line 1".."line 6" (6 lines, keeping 7/8/9) and a trailing fold
    // hiding "line 14".."line 20" (7 lines, keeping 11/12/13).
    const FOLD_SAMPLE: &str = "\
diff --git a/file.txt b/file.txt
index 1111111..2222222 100644
--- a/file.txt
+++ b/file.txt
@@ -1,20 +1,20 @@
 line 1
 line 2
 line 3
 line 4
 line 5
 line 6
 line 7
 line 8
 line 9
-line 10 old
+line 10 new
 line 11
 line 12
 line 13
 line 14
 line 15
 line 16
 line 17
 line 18
 line 19
 line 20
";

    #[test]
    fn collapsed_folds_show_bars_with_counts_and_margins() {
        let model = parse_unified_diff(FOLD_SAMPLE);
        let highlighter = Highlighter::new();
        // Default = no expanded folds = everything collapsed.
        let rendered = render_diff(
            &model,
            &highlighter,
            &Theme::default(),
            &RenderOptions::default(),
            &HashSet::new(),
            80,
        );
        let text: Vec<String> = rendered.lines.iter().map(|l| l.to_string()).collect();

        // Both fold bars appear with their exact hidden counts.
        assert!(
            text.iter().any(|l| l.contains("▼ 6 unchanged lines")),
            "leading fold bar (6 lines) missing: {text:?}"
        );
        assert!(
            text.iter().any(|l| l.contains("▼ 7 unchanged lines")),
            "trailing fold bar (7 lines) missing: {text:?}"
        );
        // Kept margins (3 lines either side of the change) stay visible...
        for kept in [
            "line 7", "line 8", "line 9", "line 11", "line 12", "line 13",
        ] {
            assert!(
                text.iter().any(|l| l.contains(kept)),
                "margin line {kept:?} should stay visible: {text:?}"
            );
        }
        // ...and the hidden lines do NOT render (keeping the output small).
        // `ends_with` so "line 1" does not match the visible "line 10".."line 13".
        for gone in ["line 1", "line 5", "line 6", "line 14", "line 20"] {
            assert!(
                !text.iter().any(|l| l.ends_with(gone)),
                "hidden line {gone:?} leaked into the render: {text:?}"
            );
        }
    }

    #[test]
    fn gutter_numbers_jump_across_a_collapsed_fold() {
        // The leading fold hides lines 1-6; the first visible content row is
        // "line 7", so its dual gutter must read old=7 new=7 (the counters
        // advanced through the six hidden lines).
        let model = parse_unified_diff(FOLD_SAMPLE);
        let highlighter = Highlighter::new();
        let rendered = render_diff(
            &model,
            &highlighter,
            &Theme::default(),
            &RenderOptions::default(),
            &HashSet::new(),
            80,
        );
        let row7 = rendered
            .lines
            .iter()
            .map(|l| l.to_string())
            .find(|s| s.contains("line 7"))
            .expect("line 7 row present");
        // gutter width is 3 here: "  7   7 " (old, space, new, space) before content.
        assert!(
            row7.contains("  7   7 "),
            "line 7 should carry old=7 new=7 after the fold: {row7:?}"
        );
    }

    #[test]
    fn fold_bars_map_rows_to_fold_ids() {
        let model = parse_unified_diff(FOLD_SAMPLE);
        let highlighter = Highlighter::new();
        let rendered = render_diff(
            &model,
            &highlighter,
            &Theme::default(),
            &RenderOptions::default(),
            &HashSet::new(),
            80,
        );
        // Two folds -> two bars, ascending by row, each row IS a fold bar line.
        assert_eq!(rendered.fold_bars.len(), 2);
        assert!(rendered.fold_bars[0].0 < rendered.fold_bars[1].0);
        for &(row, _id) in &rendered.fold_bars {
            assert!(
                rendered.lines[row].to_string().contains("unchanged lines"),
                "fold_bars row {row} is not a fold bar"
            );
        }
        // The two folds are file 0's index 0 and 1.
        assert_eq!(rendered.fold_bars[0].1, FoldId { file: 0, index: 0 });
        assert_eq!(rendered.fold_bars[1].1, FoldId { file: 0, index: 1 });
    }

    #[test]
    fn expanding_a_fold_reveals_its_hidden_lines() {
        let model = parse_unified_diff(FOLD_SAMPLE);
        let highlighter = Highlighter::new();
        // Expand the leading fold (file 0, index 0).
        let mut expanded = HashSet::new();
        expanded.insert(FoldId { file: 0, index: 0 });
        let rendered = render_diff(
            &model,
            &highlighter,
            &Theme::default(),
            &RenderOptions::default(),
            &expanded,
            80,
        );
        let text: Vec<String> = rendered.lines.iter().map(|l| l.to_string()).collect();
        // The expanded fold now shows a `▲` header and reveals its hidden lines.
        assert!(
            text.iter().any(|l| l.contains("▲ 6 unchanged lines")),
            "expanded fold should show the ▲ header: {text:?}"
        );
        assert!(
            // `ends_with` distinguishes "line 1" from "line 10".."line 19".
            text.iter().any(|l| l.ends_with("line 1")),
            "expanded fold should reveal line 1: {text:?}"
        );
        // The OTHER (trailing) fold is still collapsed.
        assert!(
            text.iter().any(|l| l.contains("▼ 7 unchanged lines")),
            "trailing fold should remain collapsed: {text:?}"
        );
        assert!(
            !text.iter().any(|l| l.contains("line 20")),
            "trailing fold's hidden lines must stay hidden: {text:?}"
        );
    }

    #[test]
    fn renders_collapsed_fold_to_buffer() {
        let model = parse_unified_diff(FOLD_SAMPLE);
        let highlighter = Highlighter::new();
        let rendered = render_diff(
            &model,
            &highlighter,
            &Theme::default(),
            &RenderOptions::default(),
            &HashSet::new(),
            50,
        );

        let backend = TestBackend::new(50, 14);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| f.render_widget(Paragraph::new(rendered.lines.clone()), f.area()))
            .unwrap();

        insta::assert_snapshot!(terminal.backend());
    }

    #[test]
    fn renders_expanded_fold_to_buffer() {
        let model = parse_unified_diff(FOLD_SAMPLE);
        let highlighter = Highlighter::new();
        let mut expanded = HashSet::new();
        expanded.insert(FoldId { file: 0, index: 0 });
        let rendered = render_diff(
            &model,
            &highlighter,
            &Theme::default(),
            &RenderOptions::default(),
            &expanded,
            50,
        );

        let backend = TestBackend::new(50, 18);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| f.render_widget(Paragraph::new(rendered.lines.clone()), f.area()))
            .unwrap();

        insta::assert_snapshot!(terminal.backend());
    }

    const GENERATED_SAMPLE: &str = "\
diff --git a/Cargo.lock b/Cargo.lock
index 1111111..2222222 100644
--- a/Cargo.lock
+++ b/Cargo.lock
@@ -1,3 +1,3 @@
 [[package]]
-version = \"1.0.0\"
+version = \"1.0.1\"
 name = \"revu\"
";

    /// Flatten rendered lines to their plain text for content assertions.
    fn flatten(lines: &[Line<'static>]) -> Vec<String> {
        lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect()
    }

    #[test]
    fn generated_file_collapses_by_default_and_expands_on_request() {
        let model = parse_unified_diff(GENERATED_SAMPLE);
        let highlighter = Highlighter::new();
        let theme = Theme::default();
        let opts = RenderOptions::default();

        // Collapsed by default (no expanded folds): the header and a "(generated)"
        // bar show, but the diff body is hidden.
        let collapsed = render_diff(&model, &highlighter, &theme, &opts, &HashSet::new(), 60);
        let text = flatten(&collapsed.lines);
        assert!(
            text.iter().any(|l| l.contains("Cargo.lock")),
            "file header still shown: {text:?}"
        );
        assert!(
            text.iter().any(|l| l.contains("(generated)")),
            "collapse bar shown: {text:?}"
        );
        assert!(
            !text.iter().any(|l| l.contains("version =")),
            "body hidden while collapsed: {text:?}"
        );
        // The whole-file fold bar is registered so the o/O/C controls can find it.
        assert!(collapsed
            .fold_bars
            .iter()
            .any(|&(_, id)| id == file_fold_id(0)));

        // Expanding the whole-file fold reveals the body.
        let mut expanded = HashSet::new();
        expanded.insert(file_fold_id(0));
        let shown = render_diff(&model, &highlighter, &theme, &opts, &expanded, 60);
        assert!(
            flatten(&shown.lines)
                .iter()
                .any(|l| l.contains("version =")),
            "body shown once the file fold is expanded"
        );
    }

    #[test]
    fn vertical_layout_stacks_old_block_above_new_block() {
        let model = parse_unified_diff(SAMPLE);
        let highlighter = Highlighter::new();
        let opts = RenderOptions {
            mode: LayoutMode::Vertical,
            ..RenderOptions::default()
        };
        let r = render_diff(
            &model,
            &highlighter,
            &Theme::default(),
            &opts,
            &HashSet::new(),
            60,
        );
        let text = flatten(&r.lines);
        let find = |needle: &str| text.iter().position(|l| l.contains(needle));
        let old_pos = find("old\")").expect("removed line in the old block");
        let new_pos = find("new\")").expect("added line in the new block");
        let sep_pos = find("───").expect("dim rule between the blocks");
        assert!(
            old_pos < sep_pos && sep_pos < new_pos,
            "old block, then rule, then new block: old={old_pos} sep={sep_pos} new={new_pos}"
        );
        // The added line must not appear above the separator (it's new-side only).
        assert!(
            !text[..sep_pos].iter().any(|l| l.contains("added line")),
            "added line must not be in the old block"
        );
    }

    #[test]
    fn vertical_pure_add_hunk_omits_the_dim_rule() {
        // A hunk with only added lines (no context, no removes) must not leave a
        // dim rule floating above an empty old block.
        let add_only = "\
diff --git a/new.txt b/new.txt
new file mode 100644
index 0000000..1111111
--- /dev/null
+++ b/new.txt
@@ -0,0 +1,2 @@
+line one
+line two
";
        let model = parse_unified_diff(add_only);
        let highlighter = Highlighter::new();
        let opts = RenderOptions {
            mode: LayoutMode::Vertical,
            ..RenderOptions::default()
        };
        let r = render_diff(
            &model,
            &highlighter,
            &Theme::default(),
            &opts,
            &HashSet::new(),
            60,
        );
        let text = flatten(&r.lines);
        assert!(
            text.iter().any(|l| l.contains("line one")),
            "added lines render"
        );
        assert!(
            !text.iter().any(|l| l.contains("───")),
            "no dim rule for a pure-add hunk: {text:?}"
        );
    }

    #[test]
    fn stack_blame_keys_map_new_side_content_rows() {
        let model = parse_unified_diff(SAMPLE);
        let highlighter = Highlighter::new();
        let opts = RenderOptions::default(); // stack
        let r = render_diff(
            &model,
            &highlighter,
            &Theme::default(),
            &opts,
            &HashSet::new(),
            80,
        );
        // SAMPLE's hunk has 4 new-side lines (2 context + 2 adds); the single
        // removed line has no new-side number, so it gets no blame key.
        assert_eq!(r.blame_keys.len(), 4);
        for (file, newline) in r.blame_keys.values() {
            assert_eq!(*file, 0);
            assert!(*newline >= 1);
        }
        // The new-side line numbers are exactly 1..=4.
        let mut nums: Vec<usize> = r.blame_keys.values().map(|&(_, n)| n).collect();
        nums.sort_unstable();
        assert_eq!(nums, vec![1, 2, 3, 4]);
    }

    #[test]
    fn transparent_theme_drops_the_add_remove_row_tint() {
        let model = parse_unified_diff(SAMPLE); // contains add and remove lines
        let highlighter = Highlighter::new();
        let opts = RenderOptions::default();
        let base = Theme::default();

        // Control: the normal theme paints the add row tint somewhere.
        let normal = render_diff(&model, &highlighter, &base, &opts, &HashSet::new(), 50);
        let has_tint = |r: &RenderedDiff, bg: Color| {
            r.lines
                .iter()
                .any(|l| l.spans.iter().any(|s| s.style.bg == Some(bg)))
        };
        assert!(
            has_tint(&normal, base.add_bg),
            "control: normal theme should paint the add_bg tint"
        );

        // Transparent: neither the add nor remove row tint is painted.
        let transparent = base.clone().into_transparent();
        let tr = render_diff(
            &model,
            &highlighter,
            &transparent,
            &opts,
            &HashSet::new(),
            50,
        );
        assert!(
            !has_tint(&tr, base.add_bg) && !has_tint(&tr, base.remove_bg),
            "transparent mode must not paint the add/remove row tints"
        );
    }

    #[test]
    fn generated_file_collapses_in_split_layout_too() {
        let model = parse_unified_diff(GENERATED_SAMPLE);
        let highlighter = Highlighter::new();
        let opts = RenderOptions {
            mode: LayoutMode::Split,
            ..RenderOptions::default()
        };
        let collapsed = render_diff(
            &model,
            &highlighter,
            &Theme::default(),
            &opts,
            &HashSet::new(),
            80,
        );
        let text = flatten(&collapsed.lines);
        assert!(text.iter().any(|l| l.contains("(generated)")), "{text:?}");
        assert!(
            !text.iter().any(|l| l.contains("version =")),
            "split also hides a collapsed generated file: {text:?}"
        );
    }
}
