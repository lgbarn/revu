//! Live auto-refresh policy: whether a diff source can be re-fetched at all,
//! and whether live auto-refresh starts enabled for it. Pure decisions, kept
//! out of the render loop so they can be tested in isolation.

/// Where a reviewed diff came from. Determines its live-refresh policy.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DiffSource {
    /// Unstaged working-tree diff — changes as you edit. Live by default.
    WorkingTree,
    /// Staged diff — changes as you stage/unstage. Live by default.
    Staged,
    /// Two arbitrary files (`revu diff a b`, `difftool`). Re-fetchable, but
    /// live is opt-in (the runtime toggle, added later, can enable it).
    TwoFile,
    /// A commit (`show`), a stash, a GitHub PR (`--pr`), or a patch file:
    /// re-fetchable for the `r` key, but never auto-polled.
    Fixed,
    /// Piped stdin (`pager`/`patch -`) — no re-fetch source at all.
    Stdin,
}

/// The live-refresh policy for a [`DiffSource`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct LivePolicy {
    /// Whether the source can be re-fetched at all (the `r` reload keys off this).
    pub has_reload_source: bool,
    /// Whether live auto-refresh starts enabled for this source.
    pub default_on: bool,
    /// Whether the `L` key can turn live auto-refresh on/off here. False for
    /// sources that are re-fetchable for `r` but must never auto-poll (a commit,
    /// stash, PR, or patch file), and for non-reloadable stdin. When false, the
    /// status-bar indicator is greyed out and `L` is a no-op.
    pub toggleable: bool,
}

impl DiffSource {
    /// Resolve this source's live-refresh policy.
    pub fn live_policy(self) -> LivePolicy {
        match self {
            DiffSource::WorkingTree | DiffSource::Staged => LivePolicy {
                has_reload_source: true,
                default_on: true,
                toggleable: true,
            },
            DiffSource::TwoFile => LivePolicy {
                has_reload_source: true,
                default_on: false,
                toggleable: true,
            },
            DiffSource::Fixed => LivePolicy {
                has_reload_source: true,
                default_on: false,
                toggleable: false,
            },
            DiffSource::Stdin => LivePolicy {
                has_reload_source: false,
                default_on: false,
                toggleable: false,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn working_tree_and_staged_poll_by_default_and_toggle() {
        for source in [DiffSource::WorkingTree, DiffSource::Staged] {
            let p = source.live_policy();
            assert_eq!(
                (p.default_on, p.has_reload_source, p.toggleable),
                (true, true, true),
                "{source:?} should auto-poll, be re-fetchable, and toggle"
            );
        }
    }

    #[test]
    fn two_file_is_off_by_default_but_toggleable() {
        // An arbitrary file pair can change, so `L` may enable it, but it does
        // not auto-poll unprompted.
        let p = DiffSource::TwoFile.live_policy();
        assert_eq!(
            (p.default_on, p.has_reload_source, p.toggleable),
            (false, true, true)
        );
    }

    #[test]
    fn fixed_source_is_reloadable_but_never_live() {
        // A commit / stash / PR / patch file is re-fetchable for `r`, but `L`
        // must not enable auto-poll (network or immutable source).
        let p = DiffSource::Fixed.live_policy();
        assert_eq!(
            (p.default_on, p.has_reload_source, p.toggleable),
            (false, true, false)
        );
    }

    #[test]
    fn non_reloadable_source_neither_polls_reloads_nor_toggles() {
        let p = DiffSource::Stdin.live_policy();
        assert_eq!(
            (p.default_on, p.has_reload_source, p.toggleable),
            (false, false, false)
        );
    }
}
