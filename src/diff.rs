//! A pure unified-diff parser. No I/O: text in, [`DiffModel`] out, so it can be
//! tested exhaustively without git or a terminal.

/// The role a single rendered line plays in a hunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LineKind {
    Context,
    Add,
    Remove,
}

/// One line within a hunk, with its leading `+`/`-`/space stripped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: LineKind,
    pub content: String,
    /// Byte ranges into `content` that changed relative to the paired line, for
    /// intra-line word-level emphasis. Empty unless filled by
    /// [`crate::worddiff::compute_word_emphasis`]. Carried on the model so a
    /// future split layout can emphasize the same segments.
    pub emphasis: Vec<(usize, usize)>,
    /// Whether git's `--color-moved` classified this added/removed line as part
    /// of a moved block (vs a genuine add/remove). Set only by
    /// [`parse_unified_diff_colored`]; always `false` from the plain parser.
    pub moved: bool,
}

/// A single `@@ ... @@` hunk and its lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    pub header: String,
    pub lines: Vec<DiffLine>,
}

/// All changes to one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDiff {
    pub path: String,
    pub binary: bool,
    pub hunks: Vec<Hunk>,
}

/// The parsed diff: an ordered list of changed files.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DiffModel {
    pub files: Vec<FileDiff>,
    /// Leading lines before the first `diff --git` — the commit hash, author,
    /// date, and message that `git show` prints ahead of its diff. Empty for a
    /// plain working-tree diff (which starts at `diff --git`). SGR already
    /// stripped; rendered as a header above the diff.
    pub preamble: Vec<String>,
}

/// Parse unified diff text (as produced by `git diff`) into a [`DiffModel`].
///
/// Header lines that precede the first `@@` (`index`, `--- a/...`, `+++ b/...`,
/// `new file mode`, etc.) are consumed for the file path and otherwise ignored.
// The live paths all go through `parse_unified_diff_colored` (ANSI-safe on
// plain input too), leaving this plain parser used only by tests in this binary
// crate as the reference for plain-input behavior — hence the `allow`.
#[allow(dead_code)]
pub fn parse_unified_diff(text: &str) -> DiffModel {
    // Plain input carries no move information, so every line is `moved = false`.
    build_model(text.lines().map(|line| (line.to_string(), false)))
}

/// Like [`parse_unified_diff`], but ANSI-aware: it strips SGR escape sequences
/// from each line to recover clean content/kind, AND inspects the SGR colors to
/// detect git's `--color-moved` classification, setting [`DiffLine::moved`].
///
/// On plain (zero-ANSI) input this behaves exactly like [`parse_unified_diff`]
/// (all `moved = false`), so it is safe to route arbitrary pager/patch stdin
/// through it. See [`codes_indicate_move`] for the move-color heuristic.
pub fn parse_unified_diff_colored(text: &str) -> DiffModel {
    build_model(text.lines().map(|raw| {
        let (clean, codes) = strip_sgr(raw);
        (clean, codes_indicate_move(&codes))
    }))
}

/// Shared parser core. Each input item is a `(clean_line, moved)` pair: the
/// line with any ANSI already stripped, and whether its color marked it moved.
/// `moved` is only honored on added/removed lines (context/headers are never
/// moved). Keeping one core means the plain and colored parsers cannot drift.
fn build_model<I: Iterator<Item = (String, bool)>>(lines: I) -> DiffModel {
    let mut files: Vec<FileDiff> = Vec::new();
    let mut cur_file: Option<FileDiff> = None;
    let mut cur_hunk: Option<Hunk> = None;
    // Metadata lines preceding the first file (the `git show` commit header).
    let mut preamble: Vec<String> = Vec::new();

    for (line, moved) in lines {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            flush_hunk(&mut cur_file, &mut cur_hunk);
            if let Some(f) = cur_file.take() {
                files.push(f);
            }
            cur_file = Some(FileDiff {
                path: parse_diff_git_path(rest),
                binary: false,
                hunks: Vec::new(),
            });
        } else if line.starts_with("Binary files ") {
            if let Some(f) = cur_file.as_mut() {
                f.binary = true;
            }
        } else if line.starts_with("@@") {
            flush_hunk(&mut cur_file, &mut cur_hunk);
            cur_hunk = Some(Hunk {
                header: line.clone(),
                lines: Vec::new(),
            });
        } else if cur_hunk.is_none() && (line.starts_with("--- ") || line.starts_with("+++ ")) {
            // File-header path lines (before the first `@@`). Unlike the
            // `diff --git a/x b/x` line, each carries a SINGLE path, so it is
            // unambiguous even when the path contains spaces. Prefer it as the
            // authoritative path; `+++` (new side) wins over `---` (old side),
            // and `/dev/null` (add/delete) is skipped. The `cur_hunk.is_none()`
            // guard keeps a real removed line like `--- text` inside a hunk from
            // being mistaken for a header.
            if let (Some(f), Some(path)) = (cur_file.as_mut(), parse_header_path(&line[4..])) {
                f.path = path;
            }
        } else if let Some(hunk) = cur_hunk.as_mut() {
            // Inside a hunk, classify by the first character. The `---`/`+++`
            // file headers never reach here because they appear before the
            // first `@@`, when `cur_hunk` is still `None`.
            let (kind, content) = match line.chars().next() {
                Some('+') => (LineKind::Add, line[1..].to_string()),
                Some('-') => (LineKind::Remove, line[1..].to_string()),
                Some(' ') => (LineKind::Context, line[1..].to_string()),
                // e.g. "\ No newline at end of file" — keep as context.
                _ => (LineKind::Context, line.clone()),
            };
            let moved = moved && matches!(kind, LineKind::Add | LineKind::Remove);
            hunk.lines.push(DiffLine {
                kind,
                content,
                emphasis: Vec::new(),
                moved,
            });
        } else if files.is_empty() && cur_file.is_none() {
            // Before the first file: leading metadata (the `git show` commit
            // header). Anything here is preamble, not diff content.
            preamble.push(line);
        }
    }

    flush_hunk(&mut cur_file, &mut cur_hunk);
    if let Some(f) = cur_file.take() {
        files.push(f);
    }
    // Drop the blank line(s) git puts between the message and the first diff.
    while preamble.last().is_some_and(|l| l.trim().is_empty()) {
        preamble.pop();
    }
    DiffModel { files, preamble }
}

/// Strip ANSI SGR (`ESC [ ... m`) escape sequences from `line`, returning the
/// clean text and the numeric SGR parameters encountered (e.g. `[1, 36]` for
/// bold cyan). Non-SGR CSI sequences and lone escapes are dropped without
/// contributing codes. A small hand-rolled scanner — no regex crate needed.
///
/// ponytail: only CSI (`ESC [`) sequences are handled; other escape forms
/// (OSC, etc.) do not appear in `git diff` color output, so they are out of
/// scope. Final bytes other than `m` are treated as non-SGR and ignored.
fn strip_sgr(line: &str) -> (String, Vec<u16>) {
    let mut clean = String::with_capacity(line.len());
    let mut codes: Vec<u16> = Vec::new();
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            clean.push(c);
            continue;
        }
        // ESC: only a following `[` (CSI) is a recognized sequence.
        if chars.peek() != Some(&'[') {
            continue;
        }
        chars.next(); // consume '['
        let mut params = String::new();
        let mut final_byte = None;
        // CSI parameter/intermediate bytes run until a final byte (an ASCII
        // letter here covers `m` and the cursor-movement finals we discard).
        while let Some(&nc) = chars.peek() {
            chars.next();
            if nc.is_ascii_alphabetic() {
                final_byte = Some(nc);
                break;
            }
            params.push(nc);
        }
        if final_byte == Some('m') {
            for p in params.split(';') {
                if let Ok(n) = p.parse::<u16>() {
                    codes.push(n);
                }
            }
        }
    }
    (clean, codes)
}

/// Decide whether an added/removed line's SGR colors mark it as part of a git
/// `--color-moved` block. Normal changed lines are colored green (32) / red
/// (31); git's move palette uses other hues — magenta/cyan for the main moved
/// blocks and blue/yellow for the zebra alternates (plus their bright 9x
/// variants). So a foreground among {33,34,35,36,93,94,95,96} flags a move.
///
/// Heuristic and git-version dependent (git owns the move classification; revu
/// only reads the resulting color). Documented + isolated here on purpose.
fn codes_indicate_move(codes: &[u16]) -> bool {
    codes
        .iter()
        .any(|&c| matches!(c, 33 | 34 | 35 | 36 | 93 | 94 | 95 | 96))
}

fn flush_hunk(file: &mut Option<FileDiff>, hunk: &mut Option<Hunk>) {
    if let (Some(f), Some(h)) = (file.as_mut(), hunk.take()) {
        f.hunks.push(h);
    }
}

/// Extract the new-side path from the remainder of a `diff --git a/x b/x` line.
/// This line is ambiguous for paths with spaces (no delimiter between the two
/// sides), so it is only an initial guess: [`build_model`] overrides it with the
/// unambiguous single-path `--- a/x` / `+++ b/x` header lines whenever they are
/// present (every file with content changes has them). Handles git's C-quoting
/// for the quoted form `diff --git "a/p" "b/p"`.
fn parse_diff_git_path(rest: &str) -> String {
    if let Some(inner) = rest.strip_prefix('"') {
        // Quoted form: the new side is the second quoted token. Find the close
        // of the first quote (respecting `\"`), then parse the second quote.
        if let Some(second) = rest[1..].find("\"b/").map(|i| &rest[1 + i..]) {
            if let Some(path) = parse_header_path(second) {
                return path;
            }
        }
        // Single quoted token fallback (e.g. malformed/rename): decode it.
        let inner = inner.strip_suffix('"').unwrap_or(inner);
        return strip_ab_prefix(&unquote_c_path(inner));
    }
    match rest.split(' ').next_back() {
        Some(b) => strip_ab_prefix(b),
        None => rest.to_string(),
    }
}

/// Extract a clean file path from a `--- ` / `+++ ` header remainder (the text
/// after the 4-char prefix). Handles git's C-quoting (paths with spaces, quotes,
/// or non-ASCII bytes) and strips the `a/`/`b/` prefix. Returns `None` for
/// `/dev/null` (the add/delete side, which carries no real path).
fn parse_header_path(rest: &str) -> Option<String> {
    // git diff emits no trailing timestamp, but `diff -u` appends `\t<time>`;
    // drop it defensively. A literal tab cannot appear unquoted in a real path.
    let rest = rest.split('\t').next().unwrap_or(rest);
    let path = if let Some(inner) = rest.strip_prefix('"') {
        unquote_c_path(inner.strip_suffix('"').unwrap_or(inner))
    } else {
        rest.to_string()
    };
    if path == "/dev/null" {
        return None;
    }
    Some(strip_ab_prefix(&path))
}

/// Strip the leading `a/` or `b/` that git prepends to diff paths.
fn strip_ab_prefix(p: &str) -> String {
    p.strip_prefix("a/")
        .or_else(|| p.strip_prefix("b/"))
        .unwrap_or(p)
        .to_string()
}

/// Decode a git C-quoted path body (the text inside the surrounding quotes).
/// git escapes `"`, `\`, the control chars (`\a \b \t \n \v \f \r`), and — when
/// `core.quotepath` is on — high-bit bytes as `\ooo` octal. Bytes are rebuilt
/// and lossily re-UTF-8'd, so multi-byte chars emitted as octal round-trip.
fn unquote_c_path(body: &str) -> String {
    let bytes = body.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'\\' {
            out.push(bytes[i]);
            i += 1;
            continue;
        }
        i += 1; // consume the backslash
        let Some(&c) = bytes.get(i) else { break };
        match c {
            b'a' => out.push(0x07),
            b'b' => out.push(0x08),
            b't' => out.push(b'\t'),
            b'n' => out.push(b'\n'),
            b'v' => out.push(0x0b),
            b'f' => out.push(0x0c),
            b'r' => out.push(b'\r'),
            b'0'..=b'7' => {
                // Up to three octal digits encode one byte.
                let mut val = (c - b'0') as u32;
                let mut k = 1;
                while k < 3 && matches!(bytes.get(i + 1), Some(b'0'..=b'7')) {
                    i += 1;
                    val = val * 8 + (bytes[i] - b'0') as u32;
                    k += 1;
                }
                out.push(val as u8);
            }
            other => out.push(other), // `\"`, `\\`, and any literal escape
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_multiple_files_and_hunks() {
        let text = "\
diff --git a/a.txt b/a.txt
index 1111111..2222222 100644
--- a/a.txt
+++ b/a.txt
@@ -1,2 +1,2 @@
 keep
-old
+new
@@ -10 +10,2 @@
 ctx
+added
diff --git a/b.txt b/b.txt
index 3333333..4444444 100644
--- a/b.txt
+++ b/b.txt
@@ -1 +1 @@
-removed
+inserted
";
        let model = parse_unified_diff(text);
        assert_eq!(model.files.len(), 2);

        let a = &model.files[0];
        assert_eq!(a.path, "a.txt");
        assert_eq!(a.hunks.len(), 2);
        let h0 = &a.hunks[0];
        assert_eq!(h0.lines.len(), 3);
        assert_eq!(h0.lines[0].kind, LineKind::Context);
        assert_eq!(h0.lines[1].kind, LineKind::Remove);
        assert_eq!(h0.lines[1].content, "old");
        assert_eq!(h0.lines[2].kind, LineKind::Add);
        assert_eq!(h0.lines[2].content, "new");
        assert_eq!(a.hunks[1].lines.len(), 2);

        let b = &model.files[1];
        assert_eq!(b.path, "b.txt");
        assert_eq!(b.hunks[0].lines.len(), 2);
        assert_eq!(b.hunks[0].lines[0].kind, LineKind::Remove);
        assert_eq!(b.hunks[0].lines[1].kind, LineKind::Add);
    }

    #[test]
    fn empty_input_yields_no_files() {
        assert!(parse_unified_diff("").files.is_empty());
    }

    #[test]
    fn detects_binary_files() {
        let text = "\
diff --git a/img.png b/img.png
index 1111111..2222222 100644
Binary files a/img.png and b/img.png differ
";
        let model = parse_unified_diff(text);
        assert_eq!(model.files.len(), 1);
        assert_eq!(model.files[0].path, "img.png");
        assert!(model.files[0].binary);
        assert!(model.files[0].hunks.is_empty());
    }

    #[test]
    fn spaced_and_quoted_paths_resolved_from_header_lines() {
        // git leaves spaces unquoted on the `diff --git` line (ambiguous), but
        // the single-path `+++` header is unambiguous, so the path is exact.
        let spaced = "\
diff --git a/dir/my file.txt b/dir/my file.txt
--- a/dir/my file.txt
+++ b/dir/my file.txt
@@ -1 +1 @@
-a
+b
";
        assert_eq!(parse_unified_diff(spaced).files[0].path, "dir/my file.txt");

        // C-quoted header (special char) is decoded, including octal bytes.
        let quoted = "\
diff --git \"a/caf\\303\\251 \\\"x\\\".txt\" \"b/caf\\303\\251 \\\"x\\\".txt\"
--- \"a/caf\\303\\251 \\\"x\\\".txt\"
+++ \"b/caf\\303\\251 \\\"x\\\".txt\"
@@ -1 +1 @@
-a
+b
";
        assert_eq!(parse_unified_diff(quoted).files[0].path, "café \"x\".txt");

        // Deleted side: `+++ /dev/null` must fall back to the `---` path.
        let deleted = "\
diff --git a/gone file b/gone file
--- a/gone file
+++ /dev/null
@@ -1 +0,0 @@
-a
";
        assert_eq!(parse_unified_diff(deleted).files[0].path, "gone file");
    }

    #[test]
    fn git_show_commit_metadata_captured_as_preamble() {
        // `git show` prints the commit header before the diff; revu keeps it as
        // the model preamble (trailing blank lines trimmed) and still parses the
        // diff. A plain working-tree diff has no preamble.
        let show = "\
commit 8eb5964fe0952bfbff6556739249b2fa73f45bd0
Author: A B <a@b.com>
Date:   Sat Jun 27 15:33:42 2026 -0400

    chore: release v0.2.0

diff --git a/x b/x
--- a/x
+++ b/x
@@ -1 +1 @@
-a
+b
";
        let model = parse_unified_diff(show);
        assert_eq!(model.files.len(), 1);
        assert_eq!(model.files[0].path, "x");
        assert_eq!(
            model.preamble.first().unwrap(),
            &show.lines().next().unwrap()
        );
        assert!(model.preamble.iter().any(|l| l.contains("chore: release")));
        // No dangling blank line between message and diff.
        assert!(!model.preamble.last().unwrap().trim().is_empty());

        // Plain diff: nothing before `diff --git`, so no preamble.
        let plain = "diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -1 +1 @@\n-a\n+b\n";
        assert!(parse_unified_diff(plain).preamble.is_empty());
    }

    #[test]
    fn file_header_markers_are_not_mistaken_for_changes() {
        // The `--- a/x` / `+++ b/x` lines must not be parsed as remove/add lines.
        let text = "\
diff --git a/x b/x
--- a/x
+++ b/x
@@ -1 +1 @@
-a
+b
";
        let model = parse_unified_diff(text);
        assert_eq!(model.files[0].hunks[0].lines.len(), 2);
    }

    #[test]
    fn strip_sgr_removes_escapes_and_collects_codes() {
        let (clean, codes) = strip_sgr("\x1b[1;36m+moved\x1b[m");
        assert_eq!(clean, "+moved");
        assert_eq!(codes, vec![1, 36]);
        // Plain text yields itself and no codes.
        let (clean, codes) = strip_sgr("+normal");
        assert_eq!(clean, "+normal");
        assert!(codes.is_empty());
    }

    #[test]
    fn colored_parser_on_plain_input_matches_plain_parser() {
        // The colored parser must be a drop-in for plain (zero-ANSI) text: same
        // content, kinds, and all `moved = false`.
        let plain = parse_unified_diff(SAMPLE_PLAIN);
        let colored = parse_unified_diff_colored(SAMPLE_PLAIN);
        assert_eq!(plain, colored);
        for file in &colored.files {
            for hunk in &file.hunks {
                for dl in &hunk.lines {
                    assert!(!dl.moved, "plain input must never be moved: {dl:?}");
                }
            }
        }
    }

    const SAMPLE_PLAIN: &str = "\
diff --git a/a.txt b/a.txt
index 1111111..2222222 100644
--- a/a.txt
+++ b/a.txt
@@ -1,2 +1,2 @@
 keep
-old
+new
";

    #[test]
    fn colored_parser_detects_moved_lines() {
        // Hand-crafted colored diff: a normal change (green/red) plus a moved
        // block (cyan new-moved / magenta old-moved, git's zebra palette).
        let colored = concat!(
            "\x1b[1mdiff --git a/a.txt b/a.txt\x1b[m\n",
            "\x1b[36m@@ -1,4 +1,4 @@\x1b[m\n",
            "\x1b[32m+added normally\x1b[m\n",
            "\x1b[31m-removed normally\x1b[m\n",
            "\x1b[1;36m+moved in here\x1b[m\n",
            "\x1b[1;35m-moved out there\x1b[m\n",
        );
        let model = parse_unified_diff_colored(colored);
        let lines = &model.files[0].hunks[0].lines;
        assert_eq!(lines.len(), 4);

        assert_eq!(lines[0].kind, LineKind::Add);
        assert_eq!(lines[0].content, "added normally");
        assert!(!lines[0].moved, "green add must not be moved");

        assert_eq!(lines[1].kind, LineKind::Remove);
        assert!(!lines[1].moved, "red remove must not be moved");

        assert_eq!(lines[2].kind, LineKind::Add);
        assert_eq!(lines[2].content, "moved in here");
        assert!(lines[2].moved, "cyan add must be moved");

        assert_eq!(lines[3].kind, LineKind::Remove);
        assert_eq!(lines[3].content, "moved out there");
        assert!(lines[3].moved, "magenta remove must be moved");
    }
}
