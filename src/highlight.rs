//! Syntax highlighting for diff content, backed by [`syntect`] on its pure-Rust
//! `fancy-regex` engine (no oniguruma / no C). The [`Highlighter`] owns the
//! bundled syntax and theme sets — both expensive to build — so it is created
//! once and shared by the renderer.

use ratatui::style::Color;
use syntect::easy::HighlightLines;
use syntect::highlighting::{Style as SynStyle, Theme, ThemeSet};
use syntect::parsing::{SyntaxReference, SyntaxSet};

/// Owns the loaded syntax + theme sets and the active theme. Construct once
/// (the default sets parse embedded `.sublime-syntax`/`.tmTheme` data on build,
/// which is not cheap) and reuse for every file.
pub struct Highlighter {
    syntax_set: SyntaxSet,
    theme: Theme,
}

impl Highlighter {
    /// Build the highlighter from syntect's bundled defaults.
    ///
    /// `load_defaults_newlines` is used (rather than `load_defaults_nonewlines`)
    /// because we feed the highlighter one line at a time with its trailing
    /// newline re-appended, which the newline-aware syntaxes expect.
    pub fn new() -> Self {
        let syntax_set = SyntaxSet::load_defaults_newlines();
        let mut theme_set = ThemeSet::load_defaults();
        // `base16-ocean.dark` is always present in the bundled set; fall back to
        // the first available theme rather than panicking if that ever changes.
        let theme = theme_set
            .themes
            .remove("base16-ocean.dark")
            .or_else(|| theme_set.themes.values().next().cloned())
            .expect("syntect bundles at least one default theme");
        Self { syntax_set, theme }
    }

    /// Pick a syntax by file path/extension, falling back to plain text for
    /// unknown or extension-less paths so every file highlights without error.
    pub fn syntax_for_path(&self, path: &str) -> &SyntaxReference {
        // Try the extension first (cheap, no I/O); `find_syntax_for_file` would
        // read the file off disk, which we deliberately avoid here.
        let ext = std::path::Path::new(path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");
        self.syntax_set
            .find_syntax_by_extension(ext)
            .unwrap_or_else(|| self.syntax_set.find_syntax_plain_text())
    }

    /// Construct a fresh per-file [`HighlightLines`] for `syntax`. Highlight
    /// state (open strings, comments) carries across calls on the same
    /// instance, so the renderer makes one per file/hunk and feeds lines in
    /// order.
    pub fn line_highlighter<'a>(&'a self, syntax: &'a SyntaxReference) -> HighlightLines<'a> {
        HighlightLines::new(syntax, &self.theme)
    }

    /// Highlight one line of source (no trailing newline) into `(Color, text)`
    /// spans. A newline is re-appended internally because the newline-aware
    /// syntaxes match end-of-line; it is stripped from the returned text so the
    /// rendered layout is unchanged. Only the foreground color is mapped —
    /// syntect backgrounds are ignored in favor of the terminal background.
    pub fn highlight_line(
        &self,
        hl: &mut HighlightLines,
        line_no_newline: &str,
    ) -> Vec<(Color, String)> {
        let with_nl = format!("{line_no_newline}\n");
        let ranges: Vec<(SynStyle, &str)> = hl
            .highlight_line(&with_nl, &self.syntax_set)
            // On a regex error, degrade gracefully to a single unstyled span
            // rather than dropping the line entirely.
            .unwrap_or_else(|_| vec![(SynStyle::default(), with_nl.as_str())]);

        ranges
            .into_iter()
            .map(|(style, text)| {
                let text = text.strip_suffix('\n').unwrap_or(text).to_string();
                (syn_color_to_ratatui(style.foreground), text)
            })
            .filter(|(_, text)| !text.is_empty())
            .collect()
    }
}

impl Default for Highlighter {
    fn default() -> Self {
        Self::new()
    }
}

/// Map a syntect RGBA color to a ratatui truecolor foreground. The alpha
/// channel is dropped (terminals do not blend), so only `r,g,b` are used.
fn syn_color_to_ratatui(c: syntect::highlighting::Color) -> Color {
    Color::Rgb(c.r, c.g, c.b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn syntax_for_path_resolves_known_extension() {
        let h = Highlighter::new();
        let rs = h.syntax_for_path("src/main.rs");
        assert_eq!(rs.name, "Rust");
    }

    #[test]
    fn syntax_for_path_falls_back_to_plain_text() {
        let h = Highlighter::new();
        let plain = h.syntax_for_path("notes.unknownext");
        // The bundled plain-text syntax is named "Plain Text".
        assert_eq!(plain.name, "Plain Text");
        // An extension-less path also falls back.
        assert_eq!(h.syntax_for_path("Makefileish").name, "Plain Text");
    }

    /// Distinct colors observed in a highlighted line (text content ignored).
    fn distinct_colors(spans: &[(Color, String)]) -> std::collections::HashSet<String> {
        spans.iter().map(|(c, _)| format!("{c:?}")).collect()
    }

    #[test]
    fn rust_keyword_colored_differently_from_identifier() {
        let h = Highlighter::new();
        let syntax = h.syntax_for_path("x.rs");
        let mut hl = h.line_highlighter(syntax);
        let spans = h.highlight_line(&mut hl, "fn main() {}");

        // More than one span: the line is tokenized, not returned whole.
        assert!(spans.len() > 1, "expected multiple spans, got {spans:?}");
        // Multiple distinct foreground colors: keyword/identifier/punctuation
        // do not all share one color.
        assert!(
            distinct_colors(&spans).len() > 1,
            "expected multiple distinct colors, got {spans:?}"
        );

        // The `fn` keyword's color differs from the `main` identifier's color.
        let color_of = |needle: &str| {
            spans
                .iter()
                .find(|(_, t)| t.contains(needle))
                .map(|(c, _)| *c)
        };
        let kw = color_of("fn").expect("`fn` span present");
        let ident = color_of("main").expect("`main` span present");
        assert_ne!(kw, ident, "keyword and identifier share a color: {spans:?}");
    }

    #[test]
    fn python_keyword_colored_differently_from_identifier() {
        let h = Highlighter::new();
        let syntax = h.syntax_for_path("script.py");
        assert_eq!(syntax.name, "Python");
        let mut hl = h.line_highlighter(syntax);
        let spans = h.highlight_line(&mut hl, "def greet(name):");

        assert!(spans.len() > 1, "expected multiple spans, got {spans:?}");
        assert!(
            distinct_colors(&spans).len() > 1,
            "expected multiple distinct colors, got {spans:?}"
        );

        let color_of = |needle: &str| {
            spans
                .iter()
                .find(|(_, t)| t.contains(needle))
                .map(|(c, _)| *c)
        };
        let kw = color_of("def").expect("`def` span present");
        let ident = color_of("greet").expect("`greet` span present");
        assert_ne!(kw, ident, "keyword and identifier share a color: {spans:?}");
    }

    #[test]
    fn highlight_line_preserves_text_byte_for_byte() {
        let h = Highlighter::new();
        let syntax = h.syntax_for_path("x.rs");
        let mut hl = h.line_highlighter(syntax);
        let line = "    println!(\"hi\");";
        let spans = h.highlight_line(&mut hl, line);
        let joined: String = spans.iter().map(|(_, t)| t.as_str()).collect();
        assert_eq!(joined, line, "reassembled spans must equal input text");
    }
}
