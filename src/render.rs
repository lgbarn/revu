//! Turns a [`DiffModel`] into renderable lines for the unified (stack) view.
//!
//! This is a pure function over the model, so the same output that the live UI
//! draws can be snapshot-tested against an in-memory buffer.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::diff::{DiffModel, LineKind};
use crate::highlight::Highlighter;

/// Render the model as a flat list of styled lines (unified/stack layout).
///
/// `highlighter` provides syntax coloring for the diff content. The leading
/// `+`/`-`/space prefix keeps its add/remove color (the change signal), while
/// the content after it is colored by language token.
pub fn render_lines(model: &DiffModel, highlighter: &Highlighter) -> Vec<Line<'static>> {
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
            out.push(Line::from(Span::styled(
                hunk.header.clone(),
                Style::default().fg(Color::Cyan),
            )));

            // ponytail: a fresh HighlightLines per hunk means highlight state
            // (open strings/comments spanning hunk boundaries) resets at each
            // `@@`. Acceptable for diff review — hunks are non-contiguous slices
            // of the file anyway. Whole-file state would need the full source.
            let mut hl = highlighter.line_highlighter(syntax);

            for dl in &hunk.lines {
                let (prefix, prefix_style) = match dl.kind {
                    LineKind::Add => ('+', Style::default().fg(Color::Green)),
                    LineKind::Remove => ('-', Style::default().fg(Color::Red)),
                    LineKind::Context => (' ', Style::default().fg(Color::Gray)),
                };

                let mut spans: Vec<Span<'static>> =
                    vec![Span::styled(prefix.to_string(), prefix_style)];
                for (color, text) in highlighter.highlight_line(&mut hl, &dl.content) {
                    spans.push(Span::styled(text, Style::default().fg(color)));
                }
                out.push(Line::from(spans));
            }
        }

        out.push(Line::from(""));
    }
    out
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
        let lines = render_lines(&model, &highlighter);

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
        let lines = render_lines(&DiffModel::default(), &highlighter);
        assert_eq!(lines.len(), 1);
    }
}
