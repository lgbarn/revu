//! Intra-line word-level emphasis for modified lines.
//!
//! Pure over the model: given a [`DiffModel`], pair each removed line in a hunk
//! with the correspondingly-positioned added line and compute, via word-level
//! diffing, which byte ranges of each changed. Those ranges land in
//! [`crate::diff::DiffLine::emphasis`] so the renderer can highlight just the words that
//! actually changed (not the whole line). No I/O, so it is exhaustively
//! testable without git or a terminal.

use similar::{ChangeTag, TextDiff};

use crate::diff::{DiffModel, Hunk, LineKind};

/// A list of `[start, end)` byte ranges into a line's `content`. Matches the
/// type of [`crate::diff::DiffLine::emphasis`].
pub type ByteRanges = Vec<(usize, usize)>;

/// Maximum line length sent through the quadratic worst-case word matcher.
/// Longer changed lines fall back to whole-line emphasis so rendering remains
/// responsive on generated/minified input.
const MAX_WORD_DIFF_BYTES: usize = 64 * 1024;
const MAX_WORD_DIFF_TOKENS: usize = 4 * 1024;

/// Fill [`crate::diff::DiffLine::emphasis`] across every hunk of `model` with word-level
/// changed byte ranges. Idempotent-ish: it overwrites any existing emphasis.
pub fn compute_word_emphasis(model: &mut DiffModel) {
    for file in &mut model.files {
        for hunk in &mut file.hunks {
            emphasize_hunk(hunk);
        }
    }
}

/// Pair consecutive Remove-runs with the Add-run that immediately follows and
/// emphasize each `(remove, add)` pair. The i-th removed line pairs with the
/// i-th added line; when the runs differ in length the overlap is paired and
/// the extras are left un-emphasized (a pure add or pure delete is not a
/// modification, so it carries no word-level emphasis).
fn emphasize_hunk(hunk: &mut Hunk) {
    // Pass 1: collect (remove_index, add_index) pairs under an immutable borrow.
    let mut pairs: Vec<(usize, usize)> = Vec::new();
    {
        let lines = &hunk.lines;
        let n = lines.len();
        let mut i = 0;
        while i < n {
            if lines[i].kind != LineKind::Remove {
                i += 1;
                continue;
            }
            let r_start = i;
            while i < n && lines[i].kind == LineKind::Remove {
                i += 1;
            }
            let r_end = i;
            // An add-run immediately following the remove-run forms the pairs.
            if i < n && lines[i].kind == LineKind::Add {
                let a_start = i;
                while i < n && lines[i].kind == LineKind::Add {
                    i += 1;
                }
                let a_end = i;
                let overlap = (r_end - r_start).min(a_end - a_start);
                for k in 0..overlap {
                    pairs.push((r_start + k, a_start + k));
                }
            }
        }
    }

    // Pass 2: compute ranges and write them back.
    for (ri, ai) in pairs {
        let (removed, added) = word_emphasis(&hunk.lines[ri].content, &hunk.lines[ai].content);
        hunk.lines[ri].emphasis = removed;
        hunk.lines[ai].emphasis = added;
    }
}

/// Word-level diff of a `(old, new)` line pair. Returns `(old_ranges,
/// new_ranges)`: byte ranges of deleted words within `old` and inserted words
/// within `new`. Unchanged words (including the shared prefix and whitespace)
/// produce no ranges. Adjacent changed tokens are merged into one range.
///
/// Tokenization is `similar::TextDiff::from_words`, which splits on whitespace
/// (a token is a run of non-space chars, so `foo(1);` is one word). This keeps
/// granularity at the whitespace-word level without pulling similar's optional
/// `unicode` feature (which would add a transitive dep) — a deliberate
/// dep-minimizing choice; finer sub-word splitting is a later refinement.
///
/// Public so the renderer's tests — and a future split layout — can reuse the
/// exact span computation.
pub fn word_emphasis(old: &str, new: &str) -> (ByteRanges, ByteRanges) {
    if old == new {
        return (Vec::new(), Vec::new());
    }
    let too_many_tokens = |line: &str| {
        line.split_whitespace()
            .take(MAX_WORD_DIFF_TOKENS + 1)
            .count()
            > MAX_WORD_DIFF_TOKENS
    };
    if old.len() > MAX_WORD_DIFF_BYTES
        || new.len() > MAX_WORD_DIFF_BYTES
        || too_many_tokens(old)
        || too_many_tokens(new)
    {
        let old_ranges = (!old.is_empty())
            .then_some((0, old.len()))
            .into_iter()
            .collect();
        let new_ranges = (!new.is_empty())
            .then_some((0, new.len()))
            .into_iter()
            .collect();
        return (old_ranges, new_ranges);
    }
    let diff = TextDiff::from_words(old, new);
    let mut old_ranges: ByteRanges = Vec::new();
    let mut new_ranges: ByteRanges = Vec::new();
    let mut old_pos = 0usize;
    let mut new_pos = 0usize;

    for change in diff.iter_all_changes() {
        let len = change.value().len();
        match change.tag() {
            ChangeTag::Equal => {
                old_pos += len;
                new_pos += len;
            }
            ChangeTag::Delete => {
                push_range(&mut old_ranges, old_pos, len);
                old_pos += len;
            }
            ChangeTag::Insert => {
                push_range(&mut new_ranges, new_pos, len);
                new_pos += len;
            }
        }
    }
    (old_ranges, new_ranges)
}

/// Append `[start, start+len)` to `ranges`, coalescing with the previous range
/// when they touch so a run of consecutive changed tokens is one span.
fn push_range(ranges: &mut ByteRanges, start: usize, len: usize) {
    if len == 0 {
        return;
    }
    let end = start + len;
    if let Some(last) = ranges.last_mut() {
        if last.1 == start {
            last.1 = end;
            return;
        }
    }
    ranges.push((start, end));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::parse_unified_diff;

    #[test]
    fn word_emphasis_covers_only_changed_words() {
        // "the quick brown fox" -> "the quick red fox": only the middle word
        // changes; the "the quick " prefix and trailing " fox" are unchanged.
        let (old, new) = word_emphasis("the quick brown fox", "the quick red fox");
        // "brown" is bytes 10..15 in the old line.
        assert_eq!(old, vec![(10, 15)]);
        // "red" is bytes 10..13 in the new line.
        assert_eq!(new, vec![(10, 13)]);
        // The unchanged prefix "the quick " (bytes 0..10) is never emphasized.
        assert!(old.iter().all(|&(s, _)| s >= 10));
        assert!(new.iter().all(|&(s, _)| s >= 10));
    }

    #[test]
    fn identical_lines_have_no_emphasis() {
        let (old, new) = word_emphasis("same text", "same text");
        assert!(old.is_empty());
        assert!(new.is_empty());
    }

    #[test]
    fn adjacent_changed_tokens_merge_into_one_range() {
        // Whole content differs: a single coalesced range covering everything.
        let (old, new) = word_emphasis("aaa", "bbb");
        assert_eq!(old, vec![(0, 3)]);
        assert_eq!(new, vec![(0, 3)]);
    }

    #[test]
    fn compute_word_emphasis_pairs_remove_then_add() {
        let text = "\
diff --git a/x.txt b/x.txt
--- a/x.txt
+++ b/x.txt
@@ -1,2 +1,2 @@
 unchanged
-the quick brown fox
+the quick red fox
";
        let mut model = parse_unified_diff(text);
        compute_word_emphasis(&mut model);
        let lines = &model.files[0].hunks[0].lines;
        // Context line gets no emphasis.
        assert!(lines[0].emphasis.is_empty());
        // Remove line emphasizes "brown", add line emphasizes "red".
        assert_eq!(lines[1].emphasis, vec![(10, 15)]);
        assert_eq!(lines[2].emphasis, vec![(10, 13)]);
    }

    #[test]
    fn unbalanced_runs_leave_extra_lines_unemphasized() {
        // Two removes, one add: only the first remove pairs with the add.
        let text = "\
diff --git a/x.rs b/x.rs
--- a/x.rs
+++ b/x.rs
@@ -1,2 +1,1 @@
-alpha one
-beta two
+alpha ONE
";
        let mut model = parse_unified_diff(text);
        compute_word_emphasis(&mut model);
        let lines = &model.files[0].hunks[0].lines;
        // First remove pairs with the add: "one" -> "ONE" emphasized.
        assert!(!lines[0].emphasis.is_empty());
        // Second remove has no partner, so no emphasis.
        assert!(lines[1].emphasis.is_empty());
        // The add carries emphasis for its changed word.
        assert!(!lines[2].emphasis.is_empty());
    }

    #[test]
    fn oversized_word_diff_uses_whole_line_fallback() {
        let old = "alpha ".repeat(MAX_WORD_DIFF_BYTES / 3);
        let new = "beta ".repeat(MAX_WORD_DIFF_BYTES / 3);
        let (old_ranges, new_ranges) = word_emphasis(&old, &new);
        assert_eq!(old_ranges, vec![(0, old.len())]);
        assert_eq!(new_ranges, vec![(0, new.len())]);
        assert_eq!(word_emphasis(&old, &old), (Vec::new(), Vec::new()));

        let old = "a ".repeat(MAX_WORD_DIFF_TOKENS + 1);
        let new = "b ".repeat(MAX_WORD_DIFF_TOKENS + 1);
        assert!(old.len() < MAX_WORD_DIFF_BYTES);
        assert_eq!(word_emphasis(&old, &new).0, vec![(0, old.len())]);
    }
}
