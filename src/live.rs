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
    /// Whether the source can be re-fetched (the `r` reload and the future `L`
    /// toggle / indicator key off this).
    pub has_reload_source: bool,
    /// Whether live auto-refresh starts enabled for this source.
    pub default_on: bool,
}

impl DiffSource {
    /// Resolve this source's live-refresh policy.
    pub fn live_policy(self) -> LivePolicy {
        match self {
            DiffSource::WorkingTree | DiffSource::Staged => LivePolicy {
                has_reload_source: true,
                default_on: true,
            },
            DiffSource::TwoFile | DiffSource::Fixed => LivePolicy {
                has_reload_source: true,
                default_on: false,
            },
            DiffSource::Stdin => LivePolicy {
                has_reload_source: false,
                default_on: false,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn working_tree_and_staged_poll_by_default() {
        for source in [DiffSource::WorkingTree, DiffSource::Staged] {
            let p = source.live_policy();
            assert_eq!(
                (p.default_on, p.has_reload_source),
                (true, true),
                "{source:?} should auto-poll and be re-fetchable"
            );
        }
    }

    #[test]
    fn reloadable_but_not_auto_polled_sources_are_opt_in() {
        for source in [DiffSource::TwoFile, DiffSource::Fixed] {
            let p = source.live_policy();
            assert_eq!(
                (p.default_on, p.has_reload_source),
                (false, true),
                "{source:?} should be re-fetchable but not auto-poll"
            );
        }
    }

    #[test]
    fn non_reloadable_source_neither_polls_nor_reloads() {
        let p = DiffSource::Stdin.live_policy();
        assert_eq!((p.default_on, p.has_reload_source), (false, false));
    }
}
