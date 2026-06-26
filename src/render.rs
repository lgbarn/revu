//! Turns a [`DiffModel`] into renderable lines for the unified (stack) view.
//!
//! This is a pure function over the model, so the same output that the live UI
//! draws can be snapshot-tested against an in-memory buffer.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::diff::{DiffModel, LineKind};
use crate::highlight::Highlighter;

/// Display toggles that change what `render_lines` emits. Distinct from the
/// app's `wrap` toggle, which is a [`Paragraph`](ratatui::widgets::Paragraph)
/// property (line wrapping is not a line-content concern).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderOptions {
    /// Render a right-aligned new-side line-number gutter.
    pub line_numbers: bool,
    /// Emit the `@@ ... @@` hunk header lines.
    pub hunk_headers: bool,
    /// Collapse (omit) context lines, showing only added/removed lines.
    pub context_collapsed: bool,
}

impl Default for RenderOptions {
    fn default() -> Self {
        // Mirrors the config defaults: numbers + headers on, context expanded.
        Self {
            line_numbers: true,
            hunk_headers: true,
            context_collapsed: false,
        }
    }
}

/// Blank gutter (5 cols) matching the width of a rendered `"{:>4} "` number, so
/// remove lines and unnumbered rows align with numbered ones.
const BLANK_GUTTER: &str = "     ";

/// Render the model as a flat list of styled lines (unified/stack layout).
///
/// `highlighter` provides syntax coloring for the diff content. The leading
/// `+`/`-`/space prefix keeps its add/remove color (the change signal), while
/// the content after it is colored by language token. `opts` controls the
/// optional line-number gutter, hunk headers, and context collapsing.
pub fn render_lines(
    model: &DiffModel,
    highlighter: &Highlighter,
    opts: &RenderOptions,
) -> Vec<Line<'static>> {
    if model.files.is_empty() {
        return vec![Line::from("No changes in the working tree.")];
    }

    let mut out: Vec<Line<'static>> = Vec::new();
    for file in &model.files {
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
                let (prefix, prefix_style, on_new_side) = match dl.kind {
                    LineKind::Add => ('+', Style::default().fg(Color::Green), true),
                    LineKind::Remove => ('-', Style::default().fg(Color::Red), false),
                    LineKind::Context => (' ', Style::default().fg(Color::Gray), true),
                };

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
                    for (color, text) in highlighted {
                        spans.push(Span::styled(text, Style::default().fg(color)));
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
    out
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
        let lines = render_lines(&model, &highlighter, &RenderOptions::default());

        let backend = TestBackend::new(50, 12);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| f.render_widget(Paragraph::new(lines.clone()), f.area()))
            .unwrap();

        insta::assert_snapshot!(terminal.backend());
    }

    #[test]
    fn empty_model_shows_placeholder() {
        let highlighter = Highlighter::new();
        let lines = render_lines(
            &DiffModel::default(),
            &highlighter,
            &RenderOptions::default(),
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
        let lines = render_lines(&model, &highlighter, &opts);
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
        let lines = render_lines(&model, &highlighter, &opts);
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
        let lines = render_lines(&model, &highlighter, &RenderOptions::default());
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
}
