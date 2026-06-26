//! Effective configuration resolved from layered TOML + CLI overrides.
//!
//! Precedence (lowest to highest): built-in defaults < global
//! `~/.config/revu/config.toml` < repo-local `.revu/config.toml` < CLI flags.
//! The TOML key names mirror hunk's exactly (`theme`, `mode`, `vcs`,
//! `line_numbers`, `wrap_lines`, `hunk_headers`, `transparent_background`,
//! `[custom_theme]`) so an existing hunk config copies over verbatim.
//!
//! The merge core ([`resolve`]) is pure — it takes the layer texts as strings —
//! so precedence is unit-tested without touching the filesystem.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::vcs::git::GitAdapter;
use crate::vcs::VcsAdapter;

/// Fully-resolved configuration the rest of the app consumes.
///
/// `theme`, `mode`, `transparent_background`, and `custom_theme` are parsed and
/// carried here but NOT applied in this issue: theme application lands in #9 and
/// the `mode` (split/auto layout) in #6. They live on `Config` now so those
/// issues can consume them without re-plumbing config.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Color theme name. Parsed only; applied in #9. Default "auto".
    pub theme: String,
    /// Layout mode (auto/split/unified). Parsed only; applied in #6. Default "auto".
    pub mode: String,
    /// Show a line-number gutter. Default true.
    pub line_numbers: bool,
    /// Wrap long lines instead of truncating. Default false.
    pub wrap_lines: bool,
    /// Show `@@ ... @@` hunk headers. Default true.
    pub hunk_headers: bool,
    /// Use the terminal's background instead of the theme's. Parsed only;
    /// applied in #9. Default false.
    pub transparent_background: bool,
    /// Preferred VCS backend (e.g. "git"). Parsed only; carried for later use.
    pub vcs: Option<String>,
    /// Custom theme definition. Parsed only; applied in #9.
    pub custom_theme: Option<CustomTheme>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            theme: "auto".to_string(),
            mode: "auto".to_string(),
            line_numbers: true,
            wrap_lines: false,
            hunk_headers: true,
            transparent_background: false,
            vcs: None,
            custom_theme: None,
        }
    }
}

/// A user-defined theme (`[custom_theme]`). Parsed only in this issue (#9
/// applies it). `base`/`label` are reserved keys; every other top-level string
/// key under `[custom_theme]` is a UI color and flattens into `colors`, while
/// `[custom_theme.syntax]` holds the per-token syntax colors.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CustomTheme {
    pub base: Option<String>,
    pub label: Option<String>,
    pub syntax: Option<BTreeMap<String, String>>,
    /// UI color keys (`added`, `removed`, ...) captured verbatim.
    #[serde(flatten)]
    pub colors: BTreeMap<String, String>,
}

/// One TOML layer, every field optional so "unset" cleanly defers to the layer
/// below it. Keys are the exact hunk names.
//
// Unknown keys are intentionally tolerated (serde's default): a hunk config has
// keys revu doesn't model (`watch`, `exclude_untracked`, `agent_notes`, ...), and
// the AC requires such a config to copy over verbatim, so extra keys are ignored
// rather than rejected.
#[derive(Debug, Default, Deserialize)]
struct PartialConfig {
    theme: Option<String>,
    mode: Option<String>,
    vcs: Option<String>,
    line_numbers: Option<bool>,
    wrap_lines: Option<bool>,
    hunk_headers: Option<bool>,
    // hunk also accepts the camelCase spelling.
    #[serde(alias = "transparentBackground")]
    transparent_background: Option<bool>,
    custom_theme: Option<CustomTheme>,
}

/// CLI-supplied overrides, applied last (highest precedence). A `None` field
/// means the flag was not passed and the lower layers decide.
#[derive(Debug, Clone, Default)]
pub struct ConfigOverrides {
    pub theme: Option<String>,
    pub mode: Option<String>,
    pub line_numbers: Option<bool>,
    pub wrap_lines: Option<bool>,
    pub hunk_headers: Option<bool>,
}

impl Config {
    /// Read the real config files and resolve. Missing files are fine (their
    /// layer is simply absent); malformed TOML returns a clear error so the
    /// caller can report it cleanly before entering the terminal UI.
    ///
    /// Deviation from the issue's `-> Config` sketch: this returns `Result` so a
    /// malformed `config.toml` surfaces a clean error instead of being silently
    /// swallowed (the smoke test requires that).
    pub fn load(overrides: &ConfigOverrides) -> Result<Config> {
        let global = global_config_path().and_then(|p| fs::read_to_string(p).ok());
        let repo = fs::read_to_string(repo_config_path()).ok();
        resolve(global.as_deref(), repo.as_deref(), overrides)
    }
}

/// Pure resolver: merge the (optional) global and repo TOML layers over the
/// defaults, then apply the CLI overrides. Tested exhaustively with in-memory
/// strings.
pub fn resolve(
    global_toml: Option<&str>,
    repo_toml: Option<&str>,
    overrides: &ConfigOverrides,
) -> Result<Config> {
    let mut cfg = Config::default();

    for (text, source) in [(global_toml, "global"), (repo_toml, "repo-local")] {
        if let Some(text) = text {
            let partial: PartialConfig =
                toml::from_str(text).with_context(|| format!("malformed {source} config TOML"))?;
            apply_partial(&mut cfg, partial);
        }
    }

    // CLI overrides win over every file layer.
    if let Some(v) = &overrides.theme {
        cfg.theme = v.clone();
    }
    if let Some(v) = &overrides.mode {
        cfg.mode = v.clone();
    }
    if let Some(v) = overrides.line_numbers {
        cfg.line_numbers = v;
    }
    if let Some(v) = overrides.wrap_lines {
        cfg.wrap_lines = v;
    }
    if let Some(v) = overrides.hunk_headers {
        cfg.hunk_headers = v;
    }

    Ok(cfg)
}

/// Overlay a parsed layer onto `cfg`: only keys present in the layer override.
fn apply_partial(cfg: &mut Config, p: PartialConfig) {
    if let Some(v) = p.theme {
        cfg.theme = v;
    }
    if let Some(v) = p.mode {
        cfg.mode = v;
    }
    if let Some(v) = p.vcs {
        cfg.vcs = Some(v);
    }
    if let Some(v) = p.line_numbers {
        cfg.line_numbers = v;
    }
    if let Some(v) = p.wrap_lines {
        cfg.wrap_lines = v;
    }
    if let Some(v) = p.hunk_headers {
        cfg.hunk_headers = v;
    }
    if let Some(v) = p.transparent_background {
        cfg.transparent_background = v;
    }
    if let Some(v) = p.custom_theme {
        cfg.custom_theme = Some(v);
    }
}

/// `$XDG_CONFIG_HOME/revu` else `$HOME/.config/revu`. `None` if neither env var
/// is set (e.g. a stripped environment) — callers then skip that layer.
pub(crate) fn config_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .map(|base| base.join("revu"))
}

fn global_config_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("config.toml"))
}

/// `.revu/config.toml` at the repository root, falling back to the current
/// directory when not inside a repo (e.g. `revu pager` on an arbitrary diff).
fn repo_config_path() -> PathBuf {
    let root = GitAdapter::new()
        .repo_root()
        .unwrap_or_else(|_| PathBuf::from("."));
    root.join(".revu").join("config.toml")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_when_no_layers_present() {
        let cfg = resolve(None, None, &ConfigOverrides::default()).unwrap();
        assert_eq!(cfg, Config::default());
        assert_eq!(cfg.theme, "auto");
        assert_eq!(cfg.mode, "auto");
        assert!(cfg.line_numbers);
        assert!(!cfg.wrap_lines);
        assert!(cfg.hunk_headers);
        assert!(!cfg.transparent_background);
    }

    #[test]
    fn global_only_overrides_defaults() {
        let global = "line_numbers = false\ntheme = \"dark\"\n";
        let cfg = resolve(Some(global), None, &ConfigOverrides::default()).unwrap();
        assert!(!cfg.line_numbers);
        assert_eq!(cfg.theme, "dark");
        // Untouched keys keep their defaults.
        assert!(cfg.hunk_headers);
    }

    #[test]
    fn repo_overrides_global() {
        let global = "line_numbers = false\nwrap_lines = false\n";
        let repo = "line_numbers = true\n";
        let cfg = resolve(Some(global), Some(repo), &ConfigOverrides::default()).unwrap();
        // repo flips line_numbers back on; wrap_lines (global-only) stays false.
        assert!(cfg.line_numbers);
        assert!(!cfg.wrap_lines);
    }

    #[test]
    fn cli_overrides_repo() {
        let global = "line_numbers = true\n";
        let repo = "line_numbers = true\nhunk_headers = true\n";
        let overrides = ConfigOverrides {
            line_numbers: Some(false),
            hunk_headers: Some(false),
            theme: Some("solarized".to_string()),
            ..Default::default()
        };
        let cfg = resolve(Some(global), Some(repo), &overrides).unwrap();
        assert!(!cfg.line_numbers);
        assert!(!cfg.hunk_headers);
        assert_eq!(cfg.theme, "solarized");
    }

    #[test]
    fn hunk_key_names_accepted_unchanged() {
        // A representative hunk-style config using every key verbatim.
        let toml = "\
theme = \"auto\"
mode = \"auto\"
vcs = \"git\"
line_numbers = false
wrap_lines = true
hunk_headers = false
transparent_background = true

[custom_theme]
base = \"dark\"
label = \"mine\"
added = \"#00ff00\"
removed = \"#ff0000\"

[custom_theme.syntax]
keyword = \"#ff00ff\"
";
        let cfg = resolve(None, Some(toml), &ConfigOverrides::default()).unwrap();
        assert_eq!(cfg.vcs.as_deref(), Some("git"));
        assert!(!cfg.line_numbers);
        assert!(cfg.wrap_lines);
        assert!(!cfg.hunk_headers);
        assert!(cfg.transparent_background);

        let ct = cfg.custom_theme.expect("custom_theme parsed");
        assert_eq!(ct.base.as_deref(), Some("dark"));
        assert_eq!(ct.label.as_deref(), Some("mine"));
        // Non-reserved UI keys flatten into `colors`.
        assert_eq!(ct.colors.get("added").map(String::as_str), Some("#00ff00"));
        assert_eq!(
            ct.colors.get("removed").map(String::as_str),
            Some("#ff0000")
        );
        // Reserved keys do NOT leak into `colors`.
        assert!(!ct.colors.contains_key("base"));
        assert!(!ct.colors.contains_key("label"));
        // Syntax sub-table is parsed separately.
        let syntax = ct.syntax.expect("syntax table parsed");
        assert_eq!(syntax.get("keyword").map(String::as_str), Some("#ff00ff"));
    }

    #[test]
    fn malformed_toml_is_an_error() {
        let err = resolve(
            Some("this is = = not toml"),
            None,
            &ConfigOverrides::default(),
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("malformed"),
            "expected a clear malformed-config error, got: {err}"
        );
    }

    #[test]
    fn unknown_hunk_keys_are_tolerated() {
        // A hunk config has keys revu doesn't model; it must copy over verbatim.
        // Unknown keys are ignored, and the keys we DO model still apply.
        let cfg = resolve(
            Some(
                "line_numbers = false\nwatch = true\nexclude_untracked = false\nagent_notes = true\n",
            ),
            None,
            &ConfigOverrides::default(),
        )
        .expect("a verbatim hunk config should load, ignoring unmodeled keys");
        assert!(!cfg.line_numbers);
    }

    #[test]
    fn transparent_background_camelcase_alias() {
        let cfg = resolve(
            Some("transparentBackground = true\n"),
            None,
            &ConfigOverrides::default(),
        )
        .unwrap();
        assert!(cfg.transparent_background);
    }
}
