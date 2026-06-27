//! Pure parsing of `git blame --porcelain` output, plus relative-age
//! formatting. No I/O: the VCS layer shells out and hands the porcelain text
//! here, so the line-attribution logic is exhaustively unit-testable. The app
//! renders the result as an optional per-line gutter (author + age).

use std::collections::HashMap;

/// Blame for one source line: the last author to touch it and the author time
/// (unix epoch seconds) used to render a relative age.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlameLine {
    pub author: String,
    /// Author time, unix epoch seconds. `0` when unknown.
    pub time: i64,
}

/// Parse `git blame --porcelain` text into per-line blame, in final-line order
/// (the new-side line numbers, 1-based -> `Vec` index 0-based).
///
/// Porcelain emits the author/time headers only the FIRST time a commit
/// appears; later lines from the same commit carry just the `<sha> <orig>
/// <final>` header and the tab-prefixed content. So we cache commit info by sha
/// as we go and attribute each content line (the `\t`-prefixed one) to the sha
/// of the header that preceded it.
pub fn parse_blame(porcelain: &str) -> Vec<BlameLine> {
    let mut commits: HashMap<String, BlameLine> = HashMap::new();
    let mut out: Vec<BlameLine> = Vec::new();

    let mut cur_sha: Option<String> = None;
    let mut pending_author: Option<String> = None;
    let mut pending_time: Option<i64> = None;

    for line in porcelain.lines() {
        if let Some(_content) = line.strip_prefix('\t') {
            // The actual source line: attribute it to the current commit.
            let Some(sha) = &cur_sha else { continue };
            // First content line of a freshly-seen commit: bank its headers.
            commits.entry(sha.clone()).or_insert_with(|| BlameLine {
                author: pending_author.take().unwrap_or_default(),
                time: pending_time.take().unwrap_or(0),
            });
            let info = commits.get(sha).cloned().unwrap_or(BlameLine {
                author: String::new(),
                time: 0,
            });
            out.push(info);
        } else if let Some(rest) = line.strip_prefix("author ") {
            pending_author = Some(rest.to_string());
        } else if let Some(rest) = line.strip_prefix("author-time ") {
            pending_time = rest.trim().parse().ok();
        } else if is_header_line(line) {
            // `<40-hex-sha> <orig-line> <final-line> [<num-lines>]`
            cur_sha = line.split(' ').next().map(str::to_string);
            // Reset pending header state; a repeated sha won't re-emit headers
            // (we read them from the cache instead), a new sha will fill these.
            pending_author = None;
            pending_time = None;
        }
        // Other porcelain headers (author-mail, committer*, summary, previous,
        // filename, boundary) are not needed for the gutter and are ignored.
    }
    out
}

/// Whether `line` is a porcelain group header: a 40-char hex sha followed by a
/// space (and the orig/final line numbers). Distinguishes it from the info
/// lines (`author ...`) and content lines (`\t...`).
fn is_header_line(line: &str) -> bool {
    line.len() > 40
        && line.as_bytes()[40] == b' '
        && line[..40].bytes().all(|b| b.is_ascii_hexdigit())
}

/// A compact relative age like `3d`, `5mo`, `2y`, or `now`, given the line's
/// author `time` and the current epoch `now` (passed in, so this stays pure and
/// testable). A non-positive or future time renders `now`.
pub fn relative_age(time: i64, now: i64) -> String {
    let secs = now - time;
    if time == 0 || secs <= 0 {
        return "now".to_string();
    }
    const MIN: i64 = 60;
    const HOUR: i64 = 60 * MIN;
    const DAY: i64 = 24 * HOUR;
    const WEEK: i64 = 7 * DAY;
    const MONTH: i64 = 30 * DAY;
    const YEAR: i64 = 365 * DAY;
    match secs {
        s if s < HOUR => format!("{}m", s / MIN),
        s if s < DAY => format!("{}h", s / HOUR),
        s if s < WEEK => format!("{}d", s / DAY),
        s if s < MONTH => format!("{}w", s / WEEK),
        s if s < YEAR => format!("{}mo", s / MONTH),
        s => format!("{}y", s / YEAR),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(author: &str, time: i64) -> BlameLine {
        BlameLine {
            author: author.to_string(),
            time,
        }
    }

    // One commit covering three consecutive lines: headers appear once, then the
    // two later lines carry only the `<sha> <orig> <final>` header + content.
    const SINGLE: &str = "\
abcdef0123456789abcdef0123456789abcdef01 1 1 3
author Ada Lovelace
author-mail <ada@example.com>
author-time 1700000000
author-tz +0000
committer Ada Lovelace
committer-time 1700000000
summary first
filename src/x.rs
\tline one
abcdef0123456789abcdef0123456789abcdef01 2 2
\tline two
abcdef0123456789abcdef0123456789abcdef01 3 3
\tline three
";

    #[test]
    fn single_commit_attributes_every_line_via_cache() {
        let r = parse_blame(SINGLE);
        assert_eq!(
            r,
            vec![
                b("Ada Lovelace", 1700000000),
                b("Ada Lovelace", 1700000000),
                b("Ada Lovelace", 1700000000),
            ]
        );
    }

    #[test]
    fn multiple_commits_each_get_their_own_author() {
        let text = "\
1111111111111111111111111111111111111111 1 1 1
author Alice
author-time 1000
summary a
filename f
\tfirst
2222222222222222222222222222222222222222 2 2 1
author Bob
author-time 2000
summary b
filename f
\tsecond
1111111111111111111111111111111111111111 3 3 1
\tthird via cached Alice
";
        let r = parse_blame(text);
        // The third line repeats Alice's sha with no headers -> read from cache.
        assert_eq!(r, vec![b("Alice", 1000), b("Bob", 2000), b("Alice", 1000)]);
    }

    #[test]
    fn uncommitted_line_is_attributed_to_not_committed_yet() {
        let text = "\
0000000000000000000000000000000000000000 1 1 1
author Not Committed Yet
author-time 1699999999
summary Version of x.rs from x.rs
filename src/x.rs
\tlocal edit
";
        let r = parse_blame(text);
        assert_eq!(r, vec![b("Not Committed Yet", 1699999999)]);
    }

    #[test]
    fn empty_input_yields_no_blame() {
        assert!(parse_blame("").is_empty());
    }

    #[test]
    fn content_lines_with_leading_tab_content_are_not_mistaken_for_headers() {
        // The source line itself begins with what looks like hex after the tab;
        // the tab prefix must win so it's treated as content, not a header.
        let text = "\
deadbeefdeadbeefdeadbeefdeadbeefdeadbeef 1 1 1
author Carol
author-time 500
summary c
filename f
\tabcdef0123456789 is just code
";
        let r = parse_blame(text);
        assert_eq!(r, vec![b("Carol", 500)]);
    }

    #[test]
    fn relative_age_buckets() {
        let now = 1_000_000_000;
        assert_eq!(relative_age(now, now), "now");
        assert_eq!(relative_age(0, now), "now"); // unknown time
        assert_eq!(relative_age(now + 100, now), "now"); // future
        assert_eq!(relative_age(now - 120, now), "2m");
        assert_eq!(relative_age(now - 3 * 3600, now), "3h");
        assert_eq!(relative_age(now - 3 * 86400, now), "3d");
        assert_eq!(relative_age(now - 14 * 86400, now), "2w");
        assert_eq!(relative_age(now - 60 * 86400, now), "2mo");
        assert_eq!(relative_age(now - 800 * 86400, now), "2y");
    }
}
