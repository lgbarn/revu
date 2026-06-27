//! Pure computation of "unchanged-line" folds.
//!
//! With full-file context (`git diff --unified=100000`, see [`crate::vcs::git`])
//! the diff model holds every line of each file, so long runs of unchanged
//! context appear between (and around) the actual changes. This module decides
//! which runs to collapse into a single `▼ N unchanged lines` bar, keeping a few
//! lines of context ([`FOLD_MARGIN`]) on each side of every change. It is pure:
//! lines in, fold ranges out — no I/O, no rendering — so it is exhaustively
//! unit-testable, and the renderer ([`crate::render`]) consumes the result.

use std::collections::HashSet;

use crate::diff::{DiffLine, LineKind};

/// Lines of unchanged context kept visible on each side of a change. A context
/// run is only foldable when collapsing it actually hides lines beyond these
/// margins (see [`compute_hunk_folds`]).
pub const FOLD_MARGIN: usize = 3;

/// Stable identity of a fold: the `file`-th file's `index`-th fold (0-based, in
/// top-to-bottom order across that file's hunks). Stable as long as the diff
/// model is, so the app can remember which folds the user expanded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FoldId {
    pub file: usize,
    pub index: usize,
}

/// Reserved `FoldId::index` for a whole-file fold (a generated file collapsed to
/// a single bar). Out of the range of per-hunk context folds, which number up
/// from 0, so the two never collide within a file.
pub const FILE_FOLD_INDEX: usize = usize::MAX;

/// The whole-file fold id for the `file`-th file. Lets generated-file collapse
/// reuse the existing fold toggle / expand-all controls with no new machinery.
pub fn file_fold_id(file: usize) -> FoldId {
    FoldId {
        file,
        index: FILE_FOLD_INDEX,
    }
}

/// Whether `path` names a generated/vendored file whose diff is noise to a
/// reviewer — lockfiles, minified bundles, source maps, and common codegen
/// outputs. Matched on the file name (and a few suffixes), so it is independent
/// of the directory. The renderer collapses these to a single bar by default.
///
/// ponytail: a fixed name/suffix list, not a content heuristic. Covers the
/// common ecosystems; extend the lists if a real diff slips through.
pub fn is_generated(path: &str) -> bool {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    const LOCKFILES: &[&str] = &[
        "Cargo.lock",
        "package-lock.json",
        "npm-shrinkwrap.json",
        "yarn.lock",
        "pnpm-lock.yaml",
        "bun.lockb",
        "deno.lock",
        "Gemfile.lock",
        "poetry.lock",
        "Pipfile.lock",
        "composer.lock",
        "go.sum",
        "flake.lock",
        "mix.lock",
        "pubspec.lock",
        "Podfile.lock",
        "packages.lock.json",
        "gradle.lockfile",
    ];
    if LOCKFILES.contains(&name) {
        return true;
    }
    // Minified bundles, source maps, and common generated sources.
    const SUFFIXES: &[&str] = &[
        ".min.js",
        ".min.mjs",
        ".min.css",
        ".js.map",
        ".css.map",
        ".pb.go",
        "_pb2.py",
        "_pb2_grpc.py",
        ".g.dart",
        ".freezed.dart",
    ];
    SUFFIXES.iter().any(|s| name.ends_with(s))
}

/// One fold: the half-open range `[start, end)` of HIDDEN line indices within a
/// hunk's `lines` (every line in the range is [`LineKind::Context`]). The kept
/// margin lines lie just outside the range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fold {
    pub id: FoldId,
    pub start: usize,
    pub end: usize,
}

impl Fold {
    /// Number of hidden lines this fold collapses (always >= 1).
    pub fn hidden(&self) -> usize {
        self.end - self.start
    }
}

/// Compute the folds for ONE hunk's `lines`. Folds are numbered with
/// `FoldId { file, index }` starting at `*next_index`, which is advanced past
/// every fold produced so indices stay unique and stable across a file's hunks.
///
/// For each maximal run of consecutive context lines:
/// - between two changes (a change directly before and after the run): keep
///   [`FOLD_MARGIN`] lines on each side; fold the middle when the run exceeds
///   `2 * FOLD_MARGIN`.
/// - at the hunk start (no change above): keep the last [`FOLD_MARGIN`]; fold
///   the lead when the run exceeds [`FOLD_MARGIN`].
/// - at the hunk end (no change below): keep the first [`FOLD_MARGIN`]; fold the
///   trailing remainder when the run exceeds [`FOLD_MARGIN`].
/// - a run that spans the whole hunk (no change at all) is never folded.
pub fn compute_hunk_folds(lines: &[DiffLine], file: usize, next_index: &mut usize) -> Vec<Fold> {
    let mut folds = Vec::new();
    let len = lines.len();
    let mut i = 0;
    while i < len {
        if lines[i].kind != LineKind::Context {
            i += 1;
            continue;
        }
        // Maximal context run `[s, e)`.
        let s = i;
        let mut e = i;
        while e < len && lines[e].kind == LineKind::Context {
            e += 1;
        }
        let has_before = s > 0; // a change line sits just above the run
        let has_after = e < len; // a change line sits just below the run
        let run = e - s;
        let hidden: Option<(usize, usize)> = match (has_before, has_after) {
            (true, true) if run > 2 * FOLD_MARGIN => Some((s + FOLD_MARGIN, e - FOLD_MARGIN)),
            (false, true) if run > FOLD_MARGIN => Some((s, e - FOLD_MARGIN)),
            (true, false) if run > FOLD_MARGIN => Some((s + FOLD_MARGIN, e)),
            // Run shorter than its threshold, or a change-free hunk: nothing to fold.
            _ => None,
        };
        if let Some((hs, he)) = hidden {
            folds.push(Fold {
                id: FoldId {
                    file,
                    index: *next_index,
                },
                start: hs,
                end: he,
            });
            *next_index += 1;
        }
        i = e;
    }
    folds
}

/// The fold whose bar the cursor (the viewport's top row `offset`) sits on: the
/// LAST bar at or above `offset`, else the FIRST bar below it. `None` when there
/// are no fold bars. `fold_bars` must be ascending by row (as the renderer emits
/// them). This is the selection the `o`/Enter toggle acts on.
pub fn fold_at_cursor(fold_bars: &[(usize, FoldId)], offset: usize) -> Option<FoldId> {
    let mut candidate: Option<FoldId> = None;
    for &(row, id) in fold_bars {
        if row <= offset {
            candidate = Some(id);
        } else if candidate.is_none() {
            // No bar at or above the cursor: take the first one below it.
            return Some(id);
        } else {
            break;
        }
    }
    candidate
}

/// Whether `expanded` contains `id` — a tiny readability wrapper used by the
/// renderer to decide collapsed-vs-expanded for each fold.
pub fn is_expanded(expanded: &HashSet<FoldId>, id: FoldId) -> bool {
    expanded.contains(&id)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a hunk-line slice from a compact spec: `'c'` context, `'+'` add,
    /// `'-'` remove. Content is irrelevant to fold computation.
    fn lines(spec: &str) -> Vec<DiffLine> {
        spec.chars()
            .map(|c| {
                let kind = match c {
                    '+' => LineKind::Add,
                    '-' => LineKind::Remove,
                    _ => LineKind::Context,
                };
                DiffLine {
                    kind,
                    content: String::new(),
                    emphasis: Vec::new(),
                    moved: false,
                }
            })
            .collect()
    }

    fn folds(spec: &str) -> Vec<Fold> {
        let mut idx = 0;
        compute_hunk_folds(&lines(spec), 0, &mut idx)
    }

    #[test]
    fn middle_run_folds_only_beyond_both_margins() {
        // 10 context lines between two changes: keep 3 each side, fold 4 in the
        // middle (indices 4..8 of the hunk: +, then 10 ctx at 1..=10, then +).
        let f = folds(&format!("+{}+", "c".repeat(10)));
        assert_eq!(f.len(), 1);
        assert_eq!((f[0].start, f[0].end), (4, 8));
        assert_eq!(f[0].hidden(), 4);
    }

    #[test]
    fn middle_run_at_exactly_twice_margin_is_not_folded() {
        // 6 == 2*FOLD_MARGIN context lines: nothing beyond the margins, no fold.
        let f = folds(&format!("+{}+", "c".repeat(6)));
        assert!(f.is_empty(), "run of 2*margin must not fold: {f:?}");
    }

    #[test]
    fn start_run_folds_the_lead_keeping_last_margin() {
        // 8 leading context lines (no change above), then a change: keep the last
        // 3, fold indices 0..5.
        let f = folds(&format!("{}+", "c".repeat(8)));
        assert_eq!(f.len(), 1);
        assert_eq!((f[0].start, f[0].end), (0, 5));
        assert_eq!(f[0].hidden(), 5);
    }

    #[test]
    fn start_run_at_exactly_margin_is_not_folded() {
        let f = folds(&format!("{}+", "c".repeat(3)));
        assert!(f.is_empty(), "leading run of margin must not fold: {f:?}");
    }

    #[test]
    fn end_run_folds_the_tail_keeping_first_margin() {
        // A change, then 8 trailing context lines (no change below): keep the
        // first 3 (indices 1..=3), fold indices 4..9.
        let f = folds(&format!("+{}", "c".repeat(8)));
        assert_eq!(f.len(), 1);
        assert_eq!((f[0].start, f[0].end), (4, 9));
        assert_eq!(f[0].hidden(), 5);
    }

    #[test]
    fn end_run_at_exactly_margin_is_not_folded() {
        let f = folds(&format!("+{}", "c".repeat(3)));
        assert!(f.is_empty(), "trailing run of margin must not fold: {f:?}");
    }

    #[test]
    fn all_context_hunk_is_never_folded() {
        let f = folds(&"c".repeat(50));
        assert!(f.is_empty(), "change-free hunk must not fold: {f:?}");
    }

    #[test]
    fn multiple_runs_get_sequential_ids_from_next_index() {
        // start run (8 ctx) + change + middle run (10 ctx) + change + end run (8 ctx).
        let spec = format!("{}+{}+{}", "c".repeat(8), "c".repeat(10), "c".repeat(8));
        let mut idx = 5; // start numbering partway through the file
        let f = compute_hunk_folds(&lines(&spec), 2, &mut idx);
        assert_eq!(f.len(), 3);
        assert_eq!(f[0].id, FoldId { file: 2, index: 5 });
        assert_eq!(f[1].id, FoldId { file: 2, index: 6 });
        assert_eq!(f[2].id, FoldId { file: 2, index: 7 });
        assert_eq!(idx, 8, "next_index advanced past the three folds");
    }

    #[test]
    fn is_generated_flags_lockfiles_minified_and_codegen() {
        // Lockfiles, regardless of directory.
        assert!(is_generated("Cargo.lock"));
        assert!(is_generated("frontend/package-lock.json"));
        assert!(is_generated("a/b/c/yarn.lock"));
        assert!(is_generated("go.sum"));
        // Minified bundles and source maps.
        assert!(is_generated("dist/app.min.js"));
        assert!(is_generated("public/styles.min.css"));
        assert!(is_generated("build/bundle.js.map"));
        // Common generated sources.
        assert!(is_generated("api/schema.pb.go"));
        assert!(is_generated("gen/service_pb2.py"));
    }

    #[test]
    fn is_generated_leaves_ordinary_source_alone() {
        assert!(!is_generated("src/main.rs"));
        assert!(!is_generated("README.md"));
        assert!(!is_generated("src/lock.rs"));
        // Case-sensitive: only the real capitalized lockfile name matches.
        assert!(!is_generated("src/cargo.lock"));
        // A plain map module is not a source map.
        assert!(!is_generated("src/world_map.rs"));
    }

    #[test]
    fn file_fold_id_uses_reserved_index_distinct_from_context_folds() {
        let id = file_fold_id(3);
        assert_eq!(
            id,
            FoldId {
                file: 3,
                index: FILE_FOLD_INDEX
            }
        );
        // Context folds number up from 0, so they never reach the reserved index.
        let f = folds(&format!("+{}+", "c".repeat(10)));
        assert_ne!(f[0].id.index, FILE_FOLD_INDEX);
    }

    #[test]
    fn fold_at_cursor_picks_last_at_or_above_else_first_below() {
        let id = |i| FoldId { file: 0, index: i };
        let bars = [(2usize, id(0)), (10, id(1)), (20, id(2))];

        // No bars: None.
        assert_eq!(fold_at_cursor(&[], 5), None);
        // Cursor before the first bar: first below.
        assert_eq!(fold_at_cursor(&bars, 0), Some(id(0)));
        // Cursor exactly on a bar: that bar.
        assert_eq!(fold_at_cursor(&bars, 10), Some(id(1)));
        // Cursor between bars: the last one at or above.
        assert_eq!(fold_at_cursor(&bars, 15), Some(id(1)));
        // Cursor past the last bar: the last bar.
        assert_eq!(fold_at_cursor(&bars, 999), Some(id(2)));
    }
}
