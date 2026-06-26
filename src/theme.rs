//! The theme system: a curated catalog of named themes, custom-TOML overrides,
//! and `auto` light/dark resolution.
//!
//! A [`Theme`] is the resolved palette the renderer consumes: a set of UI-chrome
//! colors (file/hunk headers, diff add/remove/context, gutter, moved hues, the
//! status bar) plus the name of the bundled [`syntect`](crate::highlight) theme
//! that drives syntax coloring. [`resolve_theme`] is a pure function from
//! `(Config, terminal_is_dark)` to a `Theme`, so the named/custom/auto logic is
//! unit-tested in -> out without any terminal.
//!
//! ponytail: the syntax palette is approximate. Each curated theme maps to the
//! NEAREST of syntect's seven bundled themes (there is no exact match for, say,
//! Dracula), so the syntax colors are in the right family but not pixel-perfect.
//! The upgrade path is embedding real `.tmTheme` files per catalog entry later;
//! we deliberately do NOT vendor 12 tmThemes now.

use std::collections::BTreeMap;

use anyhow::{anyhow, bail, Result};
use ratatui::style::Color;

use crate::config::Config;

/// The curated theme picked for `theme = "auto"` on a dark terminal.
const AUTO_DARK: &str = "github-dark";
/// The curated theme picked for `theme = "auto"` on a light terminal.
const AUTO_LIGHT: &str = "github-light";

/// A fully-resolved theme: the UI-chrome palette the renderer draws plus the
/// bundled syntect theme name that colors syntax. `syntax_overrides` is empty
/// for catalog themes and carries a custom theme's `[custom_theme.syntax]`
/// token colors (token-name -> color) when one is applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Theme {
    /// Display name (catalog id, or a custom theme's `label`).
    pub name: String,
    /// Whether this is a dark theme (drives `auto` selection and is informational).
    pub is_dark: bool,
    /// `── path ──` file-header rows.
    pub file_header: Color,
    /// `@@ ... @@` hunk-header rows.
    pub hunk_header: Color,
    /// Added (`+`) line prefix.
    pub add: Color,
    /// Removed (`-`) line prefix.
    pub remove: Color,
    /// Full-row background tint behind added (`+`) lines. A low-luminance hue so
    /// the syntax-highlighted foreground stays readable on top.
    pub add_bg: Color,
    /// Full-row background tint behind removed (`-`) lines. Companion to `add_bg`.
    pub remove_bg: Color,
    /// Context (` `) line prefix.
    pub context: Color,
    /// Line-number gutter, the binary-file note, and the split separator.
    pub gutter: Color,
    /// Moved-in (`+`) line prefix under git `--color-moved`.
    pub moved_add: Color,
    /// Moved-out (`-`) line prefix under git `--color-moved`.
    pub moved_remove: Color,
    /// Selection accent (the sidebar's selected-file bar).
    pub selection: Color,
    /// Status-bar foreground.
    pub status_fg: Color,
    /// Status-bar background.
    pub status_bg: Color,
    /// Bundled syntect theme name driving syntax colors.
    pub syntect_theme: String,
    /// Custom `[custom_theme.syntax]` token overrides (token-name -> color).
    /// Empty for every catalog theme.
    pub syntax_overrides: Vec<(String, Color)>,
}

impl Default for Theme {
    /// The default theme is the `auto` dark pick, so test code and any caller
    /// without an explicit theme gets a sensible, valid palette.
    fn default() -> Self {
        find(&catalog(), AUTO_DARK).expect("auto-dark default theme present in catalog")
    }
}

/// `0xRRGGBB` -> a ratatui truecolor.
fn rgb(hex: u32) -> Color {
    Color::Rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

/// Build one catalog [`Theme`] from its hand-picked UI hex palette.
#[allow(clippy::too_many_arguments)]
fn mk(
    name: &str,
    is_dark: bool,
    syntect: &str,
    file_header: u32,
    hunk_header: u32,
    add: u32,
    remove: u32,
    add_bg: u32,
    remove_bg: u32,
    context: u32,
    gutter: u32,
    moved_add: u32,
    moved_remove: u32,
    selection: u32,
    status_fg: u32,
    status_bg: u32,
) -> Theme {
    Theme {
        name: name.to_string(),
        is_dark,
        file_header: rgb(file_header),
        hunk_header: rgb(hunk_header),
        add: rgb(add),
        remove: rgb(remove),
        add_bg: rgb(add_bg),
        remove_bg: rgb(remove_bg),
        context: rgb(context),
        gutter: rgb(gutter),
        moved_add: rgb(moved_add),
        moved_remove: rgb(moved_remove),
        selection: rgb(selection),
        status_fg: rgb(status_fg),
        status_bg: rgb(status_bg),
        syntect_theme: syntect.to_string(),
        syntax_overrides: Vec::new(),
    }
}

/// The curated catalog of named themes. Each entry pairs a hand-defined UI hex
/// palette with the nearest bundled syntect theme (see the module ponytail note).
/// Names are unique; every `syntect_theme` is a real bundled theme (asserted in
/// the tests so a typo can't panic at runtime).
pub fn catalog() -> Vec<Theme> {
    vec![
        // name, dark, syntect, header, hunk, add, remove, add_bg, remove_bg, context, gutter, moved+, moved-, selection, status_fg, status_bg
        mk(
            "github-light",
            false,
            "InspiredGitHub",
            0x0969da,
            0x6639ba,
            0x1a7f37,
            0xcf222e,
            0xe6ffed,
            0xffeef0,
            0x57606a,
            0x8c959f,
            0x0969da,
            0x8250df,
            0xddf4ff,
            0xffffff,
            0x0969da,
        ),
        mk(
            "github-dark",
            true,
            "base16-ocean.dark",
            0x58a6ff,
            0xbc8cff,
            0x3fb950,
            0xf85149,
            0x12261c,
            0x2d1517,
            0x8b949e,
            0x484f58,
            0x58a6ff,
            0xbc8cff,
            0x1f6feb,
            0x0d1117,
            0x58a6ff,
        ),
        mk(
            "catppuccin-mocha",
            true,
            "base16-mocha.dark",
            0x89b4fa,
            0xcba6f7,
            0xa6e3a1,
            0xf38ba8,
            0x1f3328,
            0x35202b,
            0xa6adc8,
            0x585b70,
            0x89dceb,
            0xf5c2e7,
            0x45475a,
            0x1e1e2e,
            0x89b4fa,
        ),
        mk(
            "dracula",
            true,
            "base16-eighties.dark",
            0xbd93f9,
            0xff79c6,
            0x50fa7b,
            0xff5555,
            0x1d3327,
            0x3a2128,
            0x6272a4,
            0x44475a,
            0x8be9fd,
            0xff79c6,
            0x44475a,
            0x282a36,
            0xbd93f9,
        ),
        mk(
            "nord",
            true,
            "base16-ocean.dark",
            0x88c0d0,
            0xb48ead,
            0xa3be8c,
            0xbf616a,
            0x20342a,
            0x382229,
            0xd8dee9,
            0x4c566a,
            0x8fbcbb,
            0xb48ead,
            0x434c5e,
            0x2e3440,
            0x88c0d0,
        ),
        mk(
            "tokyo-night",
            true,
            "base16-ocean.dark",
            0x7aa2f7,
            0xbb9af7,
            0x9ece6a,
            0xf7768e,
            0x1b3328,
            0x33202a,
            0xa9b1d6,
            0x414868,
            0x7dcfff,
            0xbb9af7,
            0x283457,
            0x1a1b26,
            0x7aa2f7,
        ),
        mk(
            "gruvbox-dark",
            true,
            "base16-eighties.dark",
            0xfabd2f,
            0xd3869b,
            0xb8bb26,
            0xfb4934,
            0x2a3320,
            0x3a221c,
            0xa89984,
            0x504945,
            0x83a598,
            0xd3869b,
            0x3c3836,
            0x282828,
            0xfabd2f,
        ),
        mk(
            "gruvbox-light",
            false,
            "base16-ocean.light",
            0xb57614,
            0x8f3f71,
            0x79740e,
            0x9d0006,
            0xe4ecca,
            0xf6ddc9,
            0x7c6f64,
            0xbdae93,
            0x076678,
            0x8f3f71,
            0xebdbb2,
            0xfbf1c7,
            0xb57614,
        ),
        mk(
            "solarized-dark",
            true,
            "Solarized (dark)",
            0x268bd2,
            0x6c71c4,
            0x859900,
            0xdc322f,
            0x123a2c,
            0x3a2024,
            0x93a1a1,
            0x586e75,
            0x2aa198,
            0xd33682,
            0x073642,
            0x002b36,
            0x268bd2,
        ),
        mk(
            "solarized-light",
            false,
            "Solarized (light)",
            0x268bd2,
            0x6c71c4,
            0x859900,
            0xdc322f,
            0xe6ecc8,
            0xf6ddcc,
            0x657b83,
            0x93a1a1,
            0x2aa198,
            0xd33682,
            0xeee8d5,
            0xfdf6e3,
            0x268bd2,
        ),
        mk(
            "monokai",
            true,
            "base16-mocha.dark",
            0x66d9ef,
            0xae81ff,
            0xa6e22e,
            0xf92672,
            0x26331d,
            0x3a1f2a,
            0x75715e,
            0x49483e,
            0x66d9ef,
            0xae81ff,
            0x3e3d32,
            0x272822,
            0x66d9ef,
        ),
        mk(
            "one-dark",
            true,
            "base16-ocean.dark",
            0x61afef,
            0xc678dd,
            0x98c379,
            0xe06c75,
            0x1d3326,
            0x33202a,
            0xabb2bf,
            0x5c6370,
            0x56b6c2,
            0xc678dd,
            0x3e4451,
            0x282c34,
            0x61afef,
        ),
    ]
}

/// Find a theme by name in `cat`, returning a clone.
fn find(cat: &[Theme], name: &str) -> Option<Theme> {
    cat.iter().find(|t| t.name == name).cloned()
}

/// Resolve the active theme from config and the detected terminal background.
///
/// `theme = "auto"` picks the curated dark or light default by `terminal_is_dark`.
/// Otherwise the name is looked up in the catalog (an unknown name is a clear
/// error). A `[custom_theme]` then layers on top: it may switch the base theme
/// (`base = "..."`), rename it (`label`), override individual UI colors
/// (`#rrggbb`), and supply `[custom_theme.syntax]` token colors. Any malformed
/// hex anywhere in the custom theme is rejected with a clear error. Pure.
pub fn resolve_theme(cfg: &Config, terminal_is_dark: bool) -> Result<Theme> {
    let cat = catalog();

    let mut theme = if cfg.theme == "auto" {
        let want = if terminal_is_dark {
            AUTO_DARK
        } else {
            AUTO_LIGHT
        };
        find(&cat, want).expect("auto default theme present in catalog")
    } else {
        find(&cat, &cfg.theme).ok_or_else(|| {
            anyhow!(
                "unknown theme {:?}; pick one of: {}",
                cfg.theme,
                cat.iter()
                    .map(|t| t.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })?
    };

    if let Some(ct) = &cfg.custom_theme {
        // An explicit `base` switches which catalog theme we start from.
        if let Some(base) = &ct.base {
            theme =
                find(&cat, base).ok_or_else(|| anyhow!("unknown custom_theme base {base:?}"))?;
        }
        if let Some(label) = &ct.label {
            theme.name = label.clone();
        }
        apply_ui_overrides(&mut theme, &ct.colors)?;
        if let Some(syntax) = &ct.syntax {
            theme.syntax_overrides = parse_syntax_overrides(syntax)?;
        }
    }

    Ok(theme)
}

/// Apply a custom theme's UI color overrides onto `theme`. EVERY value is hex-
/// parsed (so invalid hex anywhere is rejected), then known keys are mapped onto
/// the palette. Unknown UI keys are tolerated (a hunk config may carry keys revu
/// doesn't model) but still validated.
fn apply_ui_overrides(theme: &mut Theme, colors: &BTreeMap<String, String>) -> Result<()> {
    for (key, value) in colors {
        let color = parse_hex(value).map_err(|e| anyhow!("custom_theme color {key:?}: {e}"))?;
        match key.as_str() {
            "added" | "add" => theme.add = color,
            "removed" | "remove" => theme.remove = color,
            "added_bg" | "add_bg" => theme.add_bg = color,
            "removed_bg" | "remove_bg" => theme.remove_bg = color,
            "context" => theme.context = color,
            "file_header" | "header" => theme.file_header = color,
            "hunk_header" | "hunk" => theme.hunk_header = color,
            "gutter" | "line_number" => theme.gutter = color,
            "moved_added" | "moved_add" => theme.moved_add = color,
            "moved_removed" | "moved_remove" => theme.moved_remove = color,
            "selection" => theme.selection = color,
            "status_fg" => theme.status_fg = color,
            "status_bg" | "status_bar" => theme.status_bg = color,
            // Unknown UI key: validated above, ignored here.
            _ => {}
        }
    }
    Ok(())
}

/// Hex-parse and validate every `[custom_theme.syntax]` entry, preserving the
/// token names for [`crate::highlight`] to map onto TextMate scopes.
fn parse_syntax_overrides(syntax: &BTreeMap<String, String>) -> Result<Vec<(String, Color)>> {
    syntax
        .iter()
        .map(|(token, value)| {
            let color =
                parse_hex(value).map_err(|e| anyhow!("custom_theme.syntax {token:?}: {e}"))?;
            Ok((token.clone(), color))
        })
        .collect()
}

/// Parse a `#rrggbb` hex color into a ratatui truecolor. Rejects a missing `#`,
/// the wrong length, and non-hex digits — each with a clear message.
pub fn parse_hex(s: &str) -> Result<Color> {
    let body = s
        .strip_prefix('#')
        .ok_or_else(|| anyhow!("invalid hex color {s:?}: must start with '#'"))?;
    if body.len() != 6 {
        bail!(
            "invalid hex color {s:?}: expected 6 hex digits after '#', found {}",
            body.len()
        );
    }
    let parse = |range: std::ops::Range<usize>| -> Result<u8> {
        u8::from_str_radix(&body[range], 16)
            .map_err(|_| anyhow!("invalid hex color {s:?}: non-hex digit"))
    };
    Ok(Color::Rgb(parse(0..2)?, parse(2..4)?, parse(4..6)?))
}

/// Detect whether the terminal has a dark background. Honest heuristic: parse
/// `$COLORFGBG` if present, else assume dark (the common terminal default).
///
/// ponytail: a real OSC 11 background query (writing the escape and reading the
/// reply off the tty) is the full solution; `$COLORFGBG` is a widely-set, zero-IO
/// proxy that covers the common case. Upgrade to OSC 11 later if needed.
pub fn terminal_is_dark() -> bool {
    std::env::var("COLORFGBG")
        .ok()
        .and_then(|v| bg_is_dark_from_colorfgbg(&v))
        .unwrap_or(true)
}

/// Pure parser for `$COLORFGBG` ("fg;bg" or "fg;default;bg"): the background is
/// the last `;`-separated field, a color index. Indices 0..=6 and 8 are dark
/// backgrounds; 7 and the bright range read as light. Returns `None` when the
/// value is missing/garbage so the caller can fall back.
pub fn bg_is_dark_from_colorfgbg(value: &str) -> Option<bool> {
    let bg = value.rsplit(';').next()?.trim();
    let index: u8 = bg.parse().ok()?;
    Some(matches!(index, 0..=6 | 8))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, CustomTheme};

    fn cfg_with_theme(theme: &str) -> Config {
        Config {
            theme: theme.to_string(),
            ..Config::default()
        }
    }

    #[test]
    fn catalog_has_twelve_unique_well_formed_themes() {
        let cat = catalog();
        assert_eq!(cat.len(), 12, "expected ~12 curated themes");

        // Names are unique.
        let mut names: Vec<&str> = cat.iter().map(|t| t.name.as_str()).collect();
        names.sort_unstable();
        let unique = names.len();
        names.dedup();
        assert_eq!(names.len(), unique, "duplicate theme name in catalog");

        // Every syntect_theme must be a real bundled theme so it can never panic
        // when the highlighter loads it at runtime.
        let bundled = syntect::highlighting::ThemeSet::load_defaults();
        for t in &cat {
            assert!(
                bundled.themes.contains_key(&t.syntect_theme),
                "theme {:?} maps to non-bundled syntect theme {:?}",
                t.name,
                t.syntect_theme
            );
            // Every theme defines distinct add/remove row tints so the two
            // change kinds are never confusable by background alone.
            assert_ne!(
                t.add_bg, t.remove_bg,
                "theme {:?} has identical add_bg/remove_bg tints",
                t.name
            );
        }
    }

    #[test]
    fn resolve_named_theme_returns_that_theme() {
        let theme = resolve_theme(&cfg_with_theme("dracula"), true).unwrap();
        assert_eq!(theme.name, "dracula");
        assert_eq!(theme.syntect_theme, "base16-eighties.dark");
    }

    #[test]
    fn resolve_unknown_theme_errors_clearly() {
        let err = resolve_theme(&cfg_with_theme("nonsuch"), true).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("unknown theme"), "got: {msg}");
        assert!(
            msg.contains("nonsuch"),
            "error should name the theme: {msg}"
        );
    }

    #[test]
    fn resolve_auto_picks_dark_or_light_by_background() {
        let dark = resolve_theme(&cfg_with_theme("auto"), true).unwrap();
        assert!(
            dark.is_dark,
            "auto on a dark terminal should be a dark theme"
        );
        assert_eq!(dark.name, AUTO_DARK);

        let light = resolve_theme(&cfg_with_theme("auto"), false).unwrap();
        assert!(!light.is_dark, "auto on a light terminal should be light");
        assert_eq!(light.name, AUTO_LIGHT);
    }

    #[test]
    fn custom_theme_overrides_a_ui_color_on_top_of_a_base() {
        let base = resolve_theme(&cfg_with_theme("nord"), true).unwrap();

        let mut colors = BTreeMap::new();
        colors.insert("added".to_string(), "#00ff00".to_string());
        let cfg = Config {
            theme: "nord".to_string(),
            custom_theme: Some(CustomTheme {
                base: None,
                label: None,
                syntax: None,
                colors,
            }),
            ..Config::default()
        };

        let resolved = resolve_theme(&cfg, true).unwrap();
        // The overridden field changed...
        assert_eq!(resolved.add, Color::Rgb(0, 255, 0));
        assert_ne!(resolved.add, base.add);
        // ...while an untouched field still matches the base theme.
        assert_eq!(resolved.remove, base.remove);
    }

    #[test]
    fn custom_theme_overrides_row_tints() {
        let base = resolve_theme(&cfg_with_theme("nord"), true).unwrap();

        let mut colors = BTreeMap::new();
        colors.insert("add_bg".to_string(), "#102010".to_string());
        colors.insert("remove_bg".to_string(), "#201010".to_string());
        let cfg = Config {
            theme: "nord".to_string(),
            custom_theme: Some(CustomTheme {
                base: None,
                label: None,
                syntax: None,
                colors,
            }),
            ..Config::default()
        };

        let resolved = resolve_theme(&cfg, true).unwrap();
        assert_eq!(resolved.add_bg, Color::Rgb(0x10, 0x20, 0x10));
        assert_eq!(resolved.remove_bg, Color::Rgb(0x20, 0x10, 0x10));
        assert_ne!(resolved.add_bg, base.add_bg);
        assert_ne!(resolved.remove_bg, base.remove_bg);
    }

    #[test]
    fn custom_theme_invalid_row_tint_hex_is_rejected() {
        let mut colors = BTreeMap::new();
        colors.insert("add_bg".to_string(), "#12345".to_string()); // 5 digits
        let cfg = Config {
            theme: "nord".to_string(),
            custom_theme: Some(CustomTheme {
                base: None,
                label: None,
                syntax: None,
                colors,
            }),
            ..Config::default()
        };
        let err = resolve_theme(&cfg, true).unwrap_err();
        assert!(
            err.to_string().contains("invalid hex color"),
            "expected a clear invalid-hex error, got: {err}"
        );
    }

    #[test]
    fn custom_theme_base_switches_starting_palette() {
        // theme = nord, but custom_theme.base = dracula => start from dracula.
        let cfg = Config {
            theme: "nord".to_string(),
            custom_theme: Some(CustomTheme {
                base: Some("dracula".to_string()),
                label: Some("mine".to_string()),
                syntax: None,
                colors: BTreeMap::new(),
            }),
            ..Config::default()
        };
        let resolved = resolve_theme(&cfg, true).unwrap();
        assert_eq!(resolved.name, "mine", "label should rename the theme");
        // Palette + syntect mapping come from the dracula base, not nord.
        assert_eq!(resolved.syntect_theme, "base16-eighties.dark");
    }

    #[test]
    fn custom_theme_invalid_hex_is_rejected() {
        let mut colors = BTreeMap::new();
        colors.insert("added".to_string(), "#xyz123".to_string());
        let cfg = Config {
            theme: "nord".to_string(),
            custom_theme: Some(CustomTheme {
                base: None,
                label: None,
                syntax: None,
                colors,
            }),
            ..Config::default()
        };
        let err = resolve_theme(&cfg, true).unwrap_err();
        assert!(
            err.to_string().contains("invalid hex color"),
            "expected a clear invalid-hex error, got: {err}"
        );
    }

    #[test]
    fn custom_theme_invalid_syntax_hex_is_rejected() {
        let mut syntax = BTreeMap::new();
        syntax.insert("keyword".to_string(), "ff0000".to_string()); // missing '#'
        let cfg = Config {
            theme: "nord".to_string(),
            custom_theme: Some(CustomTheme {
                base: None,
                label: None,
                syntax: Some(syntax),
                colors: BTreeMap::new(),
            }),
            ..Config::default()
        };
        let err = resolve_theme(&cfg, true).unwrap_err();
        assert!(
            err.to_string().contains("must start with '#'"),
            "expected a clear hex error, got: {err}"
        );
    }

    #[test]
    fn custom_theme_syntax_overrides_are_preserved() {
        let mut syntax = BTreeMap::new();
        syntax.insert("keyword".to_string(), "#ff8800".to_string());
        let cfg = Config {
            theme: "nord".to_string(),
            custom_theme: Some(CustomTheme {
                base: None,
                label: None,
                syntax: Some(syntax),
                colors: BTreeMap::new(),
            }),
            ..Config::default()
        };
        let resolved = resolve_theme(&cfg, true).unwrap();
        assert_eq!(
            resolved.syntax_overrides,
            vec![("keyword".to_string(), Color::Rgb(0xff, 0x88, 0x00))]
        );
    }

    #[test]
    fn parse_hex_accepts_valid_and_rejects_invalid() {
        assert_eq!(parse_hex("#000000").unwrap(), Color::Rgb(0, 0, 0));
        assert_eq!(parse_hex("#ffffff").unwrap(), Color::Rgb(255, 255, 255));
        assert_eq!(parse_hex("#1a2b3c").unwrap(), Color::Rgb(0x1a, 0x2b, 0x3c));

        // Missing '#'.
        assert!(parse_hex("ffffff").is_err());
        // Too short / too long.
        assert!(parse_hex("#fff").is_err());
        assert!(parse_hex("#fffffff").is_err());
        // Non-hex digit.
        assert!(parse_hex("#gggggg").is_err());
        // Empty.
        assert!(parse_hex("").is_err());
    }

    #[test]
    fn bg_is_dark_from_colorfgbg_classifies() {
        assert_eq!(bg_is_dark_from_colorfgbg("15;0"), Some(true));
        assert_eq!(bg_is_dark_from_colorfgbg("0;15"), Some(false));
        // Three-field form ("fg;default;bg") takes the last field.
        assert_eq!(bg_is_dark_from_colorfgbg("15;default;0"), Some(true));
        assert_eq!(bg_is_dark_from_colorfgbg("0;default;7"), Some(false));
        // Garbage / empty => None (caller falls back).
        assert_eq!(bg_is_dark_from_colorfgbg("garbage"), None);
        assert_eq!(bg_is_dark_from_colorfgbg(""), None);
    }
}
