//! View-state persistence (`state.json`).
//!
//! The interactive toggles survive across runs by being saved to
//! `~/.config/revu/state.json` (XDG-honored) on quit and reloaded on startup.
//!
//! Precedence: the resolved [`Config`](crate::config::Config) provides the
//! initial toggle defaults; if a `state.json` is present it OVERRIDES those
//! defaults (the user's last session wins). A missing or malformed file is not
//! an error — startup falls back to the config-derived defaults rather than
//! crashing.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::config::config_dir;

/// The persisted display toggles. Defaults mirror the config defaults so a
/// fresh install (no `state.json`) behaves identically whether or not the file
/// exists yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewState {
    pub line_numbers: bool,
    pub wrap_lines: bool,
    pub hunk_headers: bool,
    pub context_collapsed: bool,
}

impl Default for ViewState {
    fn default() -> Self {
        Self {
            line_numbers: true,
            wrap_lines: false,
            hunk_headers: true,
            context_collapsed: false,
        }
    }
}

impl ViewState {
    /// Parse view-state from JSON text, falling back to defaults on malformed
    /// input (no panic). Pure, so the missing/malformed behavior is testable
    /// without the filesystem.
    pub fn from_json(s: &str) -> ViewState {
        serde_json::from_str(s).unwrap_or_default()
    }

    /// Load the persisted state if a `state.json` exists and is readable. Returns
    /// `None` when the file is absent so the caller can keep the config-derived
    /// defaults; a malformed file yields `Some(default)` (present but unusable).
    pub fn load() -> Option<ViewState> {
        let text = fs::read_to_string(state_path()?).ok()?;
        Some(ViewState::from_json(&text))
    }

    /// Write the state to `state.json`, creating the config directory if needed.
    /// Best-effort: a write failure is reported to the caller but never aborts
    /// the review session.
    pub fn save(&self) -> std::io::Result<()> {
        let Some(path) = state_path() else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        fs::write(path, json)
    }
}

fn state_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("state.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_json() {
        let state = ViewState {
            line_numbers: false,
            wrap_lines: true,
            hunk_headers: false,
            context_collapsed: true,
        };
        let json = serde_json::to_string(&state).unwrap();
        let back = ViewState::from_json(&json);
        assert_eq!(state, back);
    }

    #[test]
    fn malformed_json_falls_back_to_default() {
        assert_eq!(
            ViewState::from_json("not json at all"),
            ViewState::default()
        );
        assert_eq!(ViewState::from_json(""), ViewState::default());
        // Partial/garbage object also degrades to default rather than panicking.
        assert_eq!(
            ViewState::from_json("{\"line_numbers\":"),
            ViewState::default()
        );
    }
}
