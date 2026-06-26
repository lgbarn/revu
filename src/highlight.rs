//! Syntax highlighting for diff content, backed by [`syntect`] on its pure-Rust
//! `fancy-regex` engine (no oniguruma / no C). The [`Highlighter`] owns the
//! bundled syntax and theme sets — both expensive to build — so it is created
//! once and shared by the renderer.

use std::str::FromStr;

use ratatui::style::Color;
use syntect::easy::HighlightLines;
use syntect::highlighting::{
    Color as SynColor, ScopeSelectors, Style as SynStyle, StyleModifier, Theme, ThemeItem, ThemeSet,
};
use syntect::parsing::{SyntaxReference, SyntaxSet};

/// Owns the loaded syntax + theme sets and the active theme. Construct once
/// (the default sets parse embedded `.sublime-syntax`/`.tmTheme` data on build,
/// which is not cheap) and reuse for every file.
pub struct Highlighter {
    syntax_set: SyntaxSet,
    theme: Theme,
}

impl Highlighter {
    /// Build the highlighter on the default `base16-ocean.dark` syntect theme
    /// with no custom token overrides. Retained for tests and the `Default` impl;
    /// the live app uses [`Highlighter::with_theme`] to honor the active theme.
    pub fn new() -> Self {
        Self::with_theme("base16-ocean.dark", &[])
    }

    /// Build the highlighter on a named bundled syntect theme, injecting any
    /// custom `[custom_theme.syntax]` token overrides.
    ///
    /// `load_defaults_newlines` is used (rather than `load_defaults_nonewlines`)
    /// because we feed the highlighter one line at a time with its trailing
    /// newline re-appended, which the newline-aware syntaxes expect. An unknown
    /// `syntect_theme_name` falls back to `base16-ocean.dark`, then to any
    /// bundled theme, so it can never panic. `syntax_overrides` are token-name ->
    /// color pairs (already hex-validated by [`crate::theme`]) layered on top of
    /// the base theme.
    pub fn with_theme(syntect_theme_name: &str, syntax_overrides: &[(String, Color)]) -> Self {
        let syntax_set = SyntaxSet::load_defaults_newlines();
        let mut theme_set = ThemeSet::load_defaults();
        let mut theme = theme_set
            .themes
            .remove(syntect_theme_name)
            .or_else(|| theme_set.themes.remove("base16-ocean.dark"))
            .or_else(|| theme_set.themes.values().next().cloned())
            .expect("syntect bundles at least one default theme");
        apply_syntax_overrides(&mut theme, syntax_overrides);
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

/// Layer a custom theme's token-color overrides onto a syntect [`Theme`]. Each
/// token name is mapped to its TextMate scope via [`token_scope`], then:
///
/// 1. Any base rule covering that EXACT scope is stripped. This matters because
///    syntect resolves ties with a strictly-greater score (`update_scored`), so a
///    naively-appended rule with the same scope as a base rule loses to it. By
///    removing the base coverage first, the appended override becomes the sole
///    provider for that scope and reliably wins.
/// 2. The override `ThemeItem` is appended (foreground only — backgrounds are
///    ignored by the renderer in favor of the terminal background).
///
/// Non-truecolor or unparseable entries are skipped (the base color stands).
fn apply_syntax_overrides(theme: &mut Theme, overrides: &[(String, Color)]) {
    for (token, color) in overrides {
        let Color::Rgb(r, g, b) = *color else {
            continue;
        };
        let selectors = match ScopeSelectors::from_str(&token_scope(token)) {
            Ok(s) => s,
            Err(_) => continue,
        };
        // The single Scope this override targets (when it is one). Strip every
        // base selector covering exactly that scope so the tie-break can't keep
        // the base color in place.
        if let Some(target) = selectors
            .selectors
            .first()
            .and_then(|s| s.extract_single_scope())
        {
            for item in &mut theme.scopes {
                item.scope
                    .selectors
                    .retain(|sel| sel.extract_single_scope() != Some(target));
            }
            theme.scopes.retain(|item| !item.scope.selectors.is_empty());
        }
        theme.scopes.push(ThemeItem {
            scope: selectors,
            style: StyleModifier {
                foreground: Some(SynColor { r, g, b, a: 0xff }),
                background: None,
                font_style: None,
            },
        });
    }
}

/// Map a friendly token name (as used in `[custom_theme.syntax]`) to its
/// TextMate scope selector. Unknown tokens pass through verbatim so a caller can
/// target any scope directly.
fn token_scope(token: &str) -> String {
    match token {
        "keyword" => "keyword",
        "string" => "string",
        "comment" => "comment",
        "function" => "entity.name.function",
        "number" => "constant.numeric",
        "type" => "entity.name.type",
        "constant" => "constant",
        "variable" => "variable",
        "operator" => "keyword.operator",
        "punctuation" => "punctuation",
        "tag" => "entity.name.tag",
        "attribute" => "entity.other.attribute-name",
        other => return other.to_string(),
    }
    .to_string()
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
    fn syntax_override_recolors_the_targeted_token() {
        // The `function` token maps to the `entity.name.function` scope, which
        // colors the `main` identifier. Without the override it has the base
        // theme's color; with it, it must become pure red.
        let syntax_name = "x.rs";
        let base = Highlighter::with_theme("base16-ocean.dark", &[]);
        let base_color = {
            let syntax = base.syntax_for_path(syntax_name);
            let mut hl = base.line_highlighter(syntax);
            base.highlight_line(&mut hl, "fn main() {}")
                .into_iter()
                .find(|(_, t)| t.contains("main"))
                .map(|(c, _)| c)
                .expect("`main` span present")
        };
        assert_ne!(base_color, Color::Rgb(255, 0, 0), "fixture color clashes");

        let overrides = vec![("function".to_string(), Color::Rgb(255, 0, 0))];
        let h = Highlighter::with_theme("base16-ocean.dark", &overrides);
        let syntax = h.syntax_for_path(syntax_name);
        let mut hl = h.line_highlighter(syntax);
        let spans = h.highlight_line(&mut hl, "fn main() {}");

        let func = spans
            .iter()
            .find(|(_, t)| t.contains("main"))
            .map(|(c, _)| *c)
            .expect("`main` span present");
        assert_eq!(
            func,
            Color::Rgb(255, 0, 0),
            "function override not applied: {spans:?}"
        );
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
