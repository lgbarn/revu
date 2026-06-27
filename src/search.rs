//! In-diff text search over the rendered lines.
//!
//! Pure and ratatui-free: the caller flattens each rendered line to its plain
//! text and hands in `&[String]`, so this module is unit-testable without a
//! terminal. It finds every match and exposes a wrapping cursor; the app layer
//! owns the prompt UI, viewport jumping, and on-screen highlighting.

/// One match: which rendered line it is on, and the half-open char range
/// `[start, end)` within that line's plain text. Char offsets (not bytes) so the
/// highlight overlay can split spans on character boundaries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Match {
    pub line: usize,
    pub start: usize,
    pub end: usize,
}

/// Find every (case-insensitive) occurrence of `query` across `lines`, in
/// reading order (top line first, left-to-right within a line). An empty query
/// matches nothing — there is no "match every position" mode.
pub fn find_matches(lines: &[String], query: &str) -> Vec<Match> {
    if query.is_empty() {
        return Vec::new();
    }
    let needle: Vec<char> = query.chars().flat_map(char::to_lowercase).collect();
    let mut out = Vec::new();
    for (line_idx, text) in lines.iter().enumerate() {
        // Work in chars so `start`/`end` are char offsets, and lowercase both
        // sides for a case-insensitive compare.
        let hay: Vec<char> = text.chars().collect();
        let hay_lower: Vec<char> = text.chars().flat_map(char::to_lowercase).collect();
        // Lowercasing can change length (rare: e.g. some ligatures), which would
        // desync char offsets. Fall back to the unfolded chars in that case so
        // offsets stay valid; the common ASCII/most-Unicode path keeps lengths.
        if hay_lower.len() != hay.len() {
            find_in_line(&hay, &lower_simple(query), line_idx, &mut out, true);
            continue;
        }
        find_in_line(&hay_lower, &needle, line_idx, &mut out, false);
    }
    out
}

/// Push every occurrence of `needle` in `hay` (both already lowercased unless
/// `simple`) onto `out`, advancing past each match so they do not overlap.
fn find_in_line(hay: &[char], needle: &[char], line: usize, out: &mut Vec<Match>, simple: bool) {
    let hay_cmp: Vec<char> = if simple {
        hay.iter().map(|c| c.to_ascii_lowercase()).collect()
    } else {
        hay.to_vec()
    };
    if needle.is_empty() || needle.len() > hay_cmp.len() {
        return;
    }
    let mut i = 0;
    while i + needle.len() <= hay_cmp.len() {
        if hay_cmp[i..i + needle.len()] == needle[..] {
            out.push(Match {
                line,
                start: i,
                end: i + needle.len(),
            });
            i += needle.len();
        } else {
            i += 1;
        }
    }
}

/// ponytail: ASCII-only lowercasing fallback for the rare length-changing
/// Unicode fold. Keeps char offsets aligned; upgrade to a grapheme-aware search
/// only if a real diff ever needs it.
fn lower_simple(query: &str) -> Vec<char> {
    query.chars().map(|c| c.to_ascii_lowercase()).collect()
}

/// A search and its wrapping cursor over the matches. `current` indexes into
/// `matches`; it is meaningless (and `current_match` returns `None`) when there
/// are no matches.
#[derive(Debug, Clone)]
pub struct Search {
    pub query: String,
    pub matches: Vec<Match>,
    current: usize,
}

impl Search {
    /// Build a search for `query` over `lines`, positioned at the first match.
    pub fn new(query: String, lines: &[String]) -> Self {
        let matches = find_matches(lines, &query);
        Search {
            query,
            matches,
            current: 0,
        }
    }

    /// Number of matches found.
    pub fn len(&self) -> usize {
        self.matches.len()
    }

    pub fn is_empty(&self) -> bool {
        self.matches.is_empty()
    }

    /// The 1-based index of the current match for display (e.g. "3/12"), or 0
    /// when there are no matches.
    pub fn current_ordinal(&self) -> usize {
        if self.matches.is_empty() {
            0
        } else {
            self.current + 1
        }
    }

    /// The current match, or `None` when the query matched nothing.
    pub fn current_match(&self) -> Option<Match> {
        self.matches.get(self.current).copied()
    }

    /// Advance to the next match, wrapping from the last back to the first.
    pub fn next(&mut self) {
        if self.matches.is_empty() {
            return;
        }
        self.current = (self.current + 1) % self.matches.len();
    }

    /// Step to the previous match, wrapping from the first to the last.
    pub fn prev(&mut self) {
        if self.matches.is_empty() {
            return;
        }
        self.current = (self.current + self.matches.len() - 1) % self.matches.len();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(xs: &[&str]) -> Vec<String> {
        xs.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn finds_single_match_with_position() {
        let ls = lines(&["the quick brown fox"]);
        let m = find_matches(&ls, "brown");
        assert_eq!(
            m,
            vec![Match {
                line: 0,
                start: 10,
                end: 15
            }]
        );
    }

    #[test]
    fn finds_multiple_matches_across_and_within_lines() {
        let ls = lines(&["foo bar foo", "no match here", "foofoo"]);
        let m = find_matches(&ls, "foo");
        assert_eq!(
            m,
            vec![
                Match {
                    line: 0,
                    start: 0,
                    end: 3
                },
                Match {
                    line: 0,
                    start: 8,
                    end: 11
                },
                Match {
                    line: 2,
                    start: 0,
                    end: 3
                },
                Match {
                    line: 2,
                    start: 3,
                    end: 6
                },
            ]
        );
    }

    #[test]
    fn no_match_returns_empty() {
        let ls = lines(&["alpha", "beta"]);
        assert!(find_matches(&ls, "gamma").is_empty());
    }

    #[test]
    fn empty_query_matches_nothing() {
        let ls = lines(&["alpha"]);
        assert!(find_matches(&ls, "").is_empty());
    }

    #[test]
    fn search_is_case_insensitive() {
        let ls = lines(&["Hello WORLD"]);
        let m = find_matches(&ls, "world");
        assert_eq!(
            m,
            vec![Match {
                line: 0,
                start: 6,
                end: 11
            }]
        );
        // And the reverse: lowercase haystack, uppercase query.
        let m2 = find_matches(&lines(&["hello world"]), "WORLD");
        assert_eq!(m2.len(), 1);
    }

    #[test]
    fn char_offsets_are_not_byte_offsets() {
        // "café " is 5 chars but 6 bytes; "x" should be at char index 5.
        let ls = lines(&["café x"]);
        let m = find_matches(&ls, "x");
        assert_eq!(
            m,
            vec![Match {
                line: 0,
                start: 5,
                end: 6
            }]
        );
    }

    #[test]
    fn cursor_wraps_forward_and_back() {
        let ls = lines(&["a", "a", "a"]);
        let mut s = Search::new("a".to_string(), &ls);
        assert_eq!(s.len(), 3);
        assert_eq!(s.current_ordinal(), 1);
        s.next();
        assert_eq!(s.current_ordinal(), 2);
        s.next();
        assert_eq!(s.current_ordinal(), 3);
        s.next(); // wrap to first
        assert_eq!(s.current_ordinal(), 1);
        s.prev(); // wrap to last
        assert_eq!(s.current_ordinal(), 3);
    }

    #[test]
    fn cursor_on_empty_is_inert() {
        let s_empty = Search::new("zzz".to_string(), &lines(&["abc"]));
        assert!(s_empty.is_empty());
        assert_eq!(s_empty.current_ordinal(), 0);
        assert_eq!(s_empty.current_match(), None);
        let mut s2 = s_empty.clone();
        s2.next();
        s2.prev();
        assert_eq!(s2.current_match(), None);
    }
}
