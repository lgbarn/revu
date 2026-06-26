//! Turns a [`DiffModel`] into renderable lines for the unified (stack) view.
//!
//! This is a pure function over the model, so the same output that the live UI
//! draws can be snapshot-tested against an in-memory buffer.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::diff::{DiffLine, DiffModel, LineKind};
use crate::highlight::Highlighter;

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
}

/// Display toggles that change what `render_lines` emits. Distinct from the
/// app's `wrap` toggle, which is a [`Paragraph`](ratatui::widgets::Paragraph)
/// property (line wrapping is not a line-content concern).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderOptions {
    /// Render a right-aligned line-number gutter (old-side on the left column,
    /// new-side on the right column in split; new-side only in stack).
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
#[derive(Debug, Clone)]
pub struct RenderedDiff {
    pub lines: Vec<Line<'static>>,
    pub file_starts: Vec<usize>,
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
    opts: &RenderOptions,
    width: u16,
) -> Vec<Line<'static>> {
    render_diff(model, highlighter, opts, width).lines
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
    opts: &RenderOptions,
    width: u16,
) -> RenderedDiff {
    layout_rows(model, highlighter, opts, width)
}

/// Pure row generation over `(model, opts, width)`: the single entry point the
/// unit tests drive for each [`LayoutMode`]. Dispatches to the unified or split
/// builder; the empty-model placeholder is shared by both.
fn layout_rows(
    model: &DiffModel,
    highlighter: &Highlighter,
    opts: &RenderOptions,
    width: u16,
) -> RenderedDiff {
    if model.files.is_empty() {
        return RenderedDiff {
            lines: vec![Line::from("No changes in the working tree.")],
            file_starts: Vec::new(),
        };
    }
    match opts.mode {
        LayoutMode::Stack => stack_rows(model, highlighter, opts),
        LayoutMode::Split => split_rows(model, highlighter, opts, width),
    }
}

/// The unified (stack) layout: a flat list of styled lines, old/new interleaved.
/// This is the original `render_diff` body, untouched, so its output stays
/// byte-identical to before split was added.
fn stack_rows(model: &DiffModel, highlighter: &Highlighter, opts: &RenderOptions) -> RenderedDiff {
    let mut out: Vec<Line<'static>> = Vec::new();
    let mut file_starts: Vec<usize> = Vec::with_capacity(model.files.len());
    for file in &model.files {
        // Record the first rendered line index for this file before emitting it.
        file_starts.push(out.len());
        out.push(Line::from(Span::styled(
            format!("── {} ", file.path),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )));

        if file.binary {
            out.push(Line::from(Span::styled(
                "  (binary file differs)",
                Style::default().fg(Color::DarkGray),
            )));
        }

        // Resolve the file's syntax once; unknown extensions fall back to plain
        // text inside `syntax_for_path`.
        let syntax = highlighter.syntax_for_path(&file.path);

        for hunk in &file.hunks {
            if opts.hunk_headers {
                out.push(Line::from(Span::styled(
                    hunk.header.clone(),
                    Style::default().fg(Color::Cyan),
                )));
            }

            // New-side line counter, seeded from the hunk header. `None` (an
            // unparseable header) renders blank gutters for the whole hunk.
            let mut new_line = parse_hunk_new_start(&hunk.header);

            // ponytail: a fresh HighlightLines per hunk means highlight state
            // (open strings/comments spanning hunk boundaries) resets at each
            // `@@`. Acceptable for diff review — hunks are non-contiguous slices
            // of the file anyway. Whole-file state would need the full source.
            let mut hl = highlighter.line_highlighter(syntax);

            for dl in &hunk.lines {
                // Add/Context advance the new-side counter; Remove does not (it
                // only exists on the old side), so it gets a blank gutter.
                let (prefix, mut prefix_style, on_new_side) = match dl.kind {
                    LineKind::Add => ('+', Style::default().fg(Color::Green), true),
                    LineKind::Remove => ('-', Style::default().fg(Color::Red), false),
                    LineKind::Context => (' ', Style::default().fg(Color::Gray), true),
                };
                // Moved lines (git `--color-moved`) get the zebra hues — cyan
                // for the moved-in (+) side, magenta for the moved-out (-) side
                // — so they read as relocations, not genuine add/removes.
                if dl.moved {
                    prefix_style = match dl.kind {
                        LineKind::Add => Style::default().fg(Color::Cyan),
                        LineKind::Remove => Style::default().fg(Color::Magenta),
                        LineKind::Context => prefix_style,
                    };
                }

                // Always feed the highlighter in order so its token state stays
                // correct even when a context line is collapsed (not rendered).
                let highlighted = highlighter.highlight_line(&mut hl, &dl.content);

                let collapsed = opts.context_collapsed && dl.kind == LineKind::Context;
                if !collapsed {
                    let mut spans: Vec<Span<'static>> = Vec::new();
                    if opts.line_numbers {
                        let gutter = match (on_new_side, new_line) {
                            (true, Some(n)) => format!("{n:>4} "),
                            _ => BLANK_GUTTER.to_string(),
                        };
                        spans.push(Span::styled(gutter, Style::default().fg(Color::DarkGray)));
                    }
                    spans.push(Span::styled(prefix.to_string(), prefix_style));
                    // Emit the syntax-highlighted content, layering word-level
                    // emphasis (underline) over the changed byte ranges. The
                    // text is identical either way — only the style differs.
                    let mut byte_pos = 0usize;
                    for (color, text) in highlighted {
                        let len = text.len();
                        push_content_spans(&mut spans, &text, byte_pos, color, &dl.emphasis);
                        byte_pos += len;
                    }
                    out.push(Line::from(spans));
                }

                if on_new_side {
                    new_line = new_line.map(|n| n + 1);
                }
            }
        }

        out.push(Line::from(""));
    }
    RenderedDiff {
        lines: out,
        file_starts,
    }
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
    opts: &RenderOptions,
    width: u16,
) -> RenderedDiff {
    let col_w = (width.saturating_sub(SEP) / 2).max(1) as usize;

    let mut out: Vec<Line<'static>> = Vec::new();
    let mut file_starts: Vec<usize> = Vec::with_capacity(model.files.len());
    for file in &model.files {
        // Same file_starts contract as stack: the header row index per file.
        file_starts.push(out.len());
        out.push(Line::from(Span::styled(
            format!("── {} ", file.path),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )));

        if file.binary {
            out.push(Line::from(Span::styled(
                "  (binary file differs)",
                Style::default().fg(Color::DarkGray),
            )));
        }

        let syntax = highlighter.syntax_for_path(&file.path);

        for hunk in &file.hunks {
            if opts.hunk_headers {
                out.push(Line::from(Span::styled(
                    hunk.header.clone(),
                    Style::default().fg(Color::Cyan),
                )));
            }

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
                        let left = build_cell(dl, &highlighted[i], old_line, opts, col_w);
                        let right = build_cell(dl, &highlighted[i], new_line, opts, col_w);
                        out.push(compose_row(left, right));
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
                                opts,
                                col_w,
                            );
                            new_line = new_line.map(|n| n + 1);
                            cell
                        }
                        None => blank_cell(col_w),
                    };
                    out.push(compose_row(left, right));
                }
            }
        }

        out.push(Line::from(""));
    }
    RenderedDiff {
        lines: out,
        file_starts,
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
    opts: &RenderOptions,
    col_w: usize,
) -> Vec<Span<'static>> {
    let (prefix, prefix_style) = prefix_for(dl);
    let mut spans: Vec<Span<'static>> = Vec::new();
    if opts.line_numbers {
        let g = match gutter {
            Some(n) => format!("{n:>4} "),
            None => BLANK_GUTTER.to_string(),
        };
        spans.push(Span::styled(g, Style::default().fg(Color::DarkGray)));
    }
    spans.push(Span::styled(prefix.to_string(), prefix_style));
    append_content_spans(&mut spans, highlighted, &dl.emphasis);
    fit_to_width(spans, col_w)
}

/// An empty `col_w`-wide cell (the blank side of a lopsided change group).
fn blank_cell(col_w: usize) -> Vec<Span<'static>> {
    fit_to_width(Vec::new(), col_w)
}

/// The leading prefix char and its style for a diff line, matching stack: green
/// `+` / red `-` / gray space, with git `--color-moved` lines recolored to the
/// cyan/magenta zebra hues.
fn prefix_for(dl: &DiffLine) -> (char, Style) {
    let (prefix, mut style) = match dl.kind {
        LineKind::Add => ('+', Style::default().fg(Color::Green)),
        LineKind::Remove => ('-', Style::default().fg(Color::Red)),
        LineKind::Context => (' ', Style::default().fg(Color::Gray)),
    };
    if dl.moved {
        style = match dl.kind {
            LineKind::Add => Style::default().fg(Color::Cyan),
            LineKind::Remove => Style::default().fg(Color::Magenta),
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
) {
    let mut byte_pos = 0usize;
    for (color, text) in highlighted {
        let len = text.len();
        push_content_spans(spans, text, byte_pos, *color, emphasis);
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
fn compose_row(mut left: Vec<Span<'static>>, right: Vec<Span<'static>>) -> Line<'static> {
    left.push(Span::styled("│", Style::default().fg(Color::DarkGray)));
    left.extend(right);
    Line::from(left)
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
fn push_content_spans(
    spans: &mut Vec<Span<'static>>,
    text: &str,
    start: usize,
    color: Color,
    emphasis: &[(usize, usize)],
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
            spans.push(styled_segment(&text[seg_start..rel], base, cur));
            seg_start = rel;
            cur = here;
        }
    }
    spans.push(styled_segment(&text[seg_start..], base, cur));
}

/// Whether byte position `pos` lies inside any emphasis range.
fn byte_emphasized(pos: usize, emphasis: &[(usize, usize)]) -> bool {
    emphasis.iter().any(|&(s, e)| pos >= s && pos < e)
}

/// A content sub-span with the base syntax style, underlined when emphasized.
fn styled_segment(text: &str, base: Style, emphasized: bool) -> Span<'static> {
    let style = if emphasized {
        base.add_modifier(Modifier::UNDERLINED)
    } else {
        base
    };
    Span::styled(text.to_string(), style)
}

#[cfg(test)]
mod tests {
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
    fn renders_diff_to_buffer() {
        let model = parse_unified_diff(SAMPLE);
        let highlighter = Highlighter::new();
        let lines = render_lines(&model, &highlighter, &RenderOptions::default(), 50);

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
            render_lines(m, &highlighter, &opts, 80)
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
        let lines = render_lines(&model, &highlighter, &opts, 80);
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
        let lines = render_lines(&model, &highlighter, &opts, 80);
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
        let lines = render_lines(&model, &highlighter, &RenderOptions::default(), 80);
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
        let rendered = render_diff(&model, &highlighter, &RenderOptions::default(), 80);

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
        let plain = render_lines(&model, &highlighter, &opts, 80);
        let rendered = render_diff(&model, &highlighter, &opts, 80);
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
        let rendered = render_diff(&model, &highlighter, &opts, width);
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
        let stack = render_diff(&model, &highlighter, &RenderOptions::default(), 80);
        let split = render_diff(
            &model,
            &highlighter,
            &RenderOptions {
                mode: LayoutMode::Split,
                ..RenderOptions::default()
            },
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
        let rendered = render_diff(&model, &highlighter, &opts, width);
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
            let rendered = render_diff(&model, &highlighter, &opts, w);
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
        let lines = render_lines(&model, &highlighter, &opts, 120);

        let backend = TestBackend::new(120, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| f.render_widget(Paragraph::new(lines.clone()), f.area()))
            .unwrap();

        insta::assert_snapshot!(terminal.backend());
    }
}
