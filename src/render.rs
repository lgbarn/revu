//! Turns a [`DiffModel`] into renderable lines for the unified (stack) view.
//!
//! This is a pure function over the model, so the same output that the live UI
//! draws can be snapshot-tested against an in-memory buffer.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::diff::{DiffModel, LineKind};

/// Render the model as a flat list of styled lines (unified/stack layout).
pub fn render_lines(model: &DiffModel) -> Vec<Line<'static>> {
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

        for hunk in &file.hunks {
            out.push(Line::from(Span::styled(
                hunk.header.clone(),
                Style::default().fg(Color::Cyan),
            )));

            for dl in &hunk.lines {
                let (prefix, style) = match dl.kind {
                    LineKind::Add => ('+', Style::default().fg(Color::Green)),
                    LineKind::Remove => ('-', Style::default().fg(Color::Red)),
                    LineKind::Context => (' ', Style::default().fg(Color::Gray)),
                };
                out.push(Line::from(Span::styled(
                    format!("{prefix}{}", dl.content),
                    style,
                )));
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
        let lines = render_lines(&model);

        let backend = TestBackend::new(50, 12);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| f.render_widget(Paragraph::new(lines.clone()), f.area()))
            .unwrap();

        insta::assert_snapshot!(terminal.backend());
    }

    #[test]
    fn empty_model_shows_placeholder() {
        let lines = render_lines(&DiffModel::default());
        assert_eq!(lines.len(), 1);
    }
}
