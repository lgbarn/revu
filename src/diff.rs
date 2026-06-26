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
}

/// Parse unified diff text (as produced by `git diff`) into a [`DiffModel`].
///
/// Header lines that precede the first `@@` (`index`, `--- a/...`, `+++ b/...`,
/// `new file mode`, etc.) are consumed for the file path and otherwise ignored.
pub fn parse_unified_diff(text: &str) -> DiffModel {
    let mut files: Vec<FileDiff> = Vec::new();
    let mut cur_file: Option<FileDiff> = None;
    let mut cur_hunk: Option<Hunk> = None;

    for line in text.lines() {
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
                header: line.to_string(),
                lines: Vec::new(),
            });
        } else if let Some(hunk) = cur_hunk.as_mut() {
            // Inside a hunk, classify by the first character. The `---`/`+++`
            // file headers never reach here because they appear before the
            // first `@@`, when `cur_hunk` is still `None`.
            let (kind, content) = match line.chars().next() {
                Some('+') => (LineKind::Add, line[1..].to_string()),
                Some('-') => (LineKind::Remove, line[1..].to_string()),
                Some(' ') => (LineKind::Context, line[1..].to_string()),
                // e.g. "\ No newline at end of file" — keep as context.
                _ => (LineKind::Context, line.to_string()),
            };
            hunk.lines.push(DiffLine { kind, content });
        }
    }

    flush_hunk(&mut cur_file, &mut cur_hunk);
    if let Some(f) = cur_file.take() {
        files.push(f);
    }
    DiffModel { files }
}

fn flush_hunk(file: &mut Option<FileDiff>, hunk: &mut Option<Hunk>) {
    if let (Some(f), Some(h)) = (file.as_mut(), hunk.take()) {
        f.hunks.push(h);
    }
}

/// Extract the new-side path from the remainder of a `diff --git a/x b/x` line.
/// ponytail: assumes paths without spaces (the common case); quoted/space paths
/// are a later refinement (git emits `diff --git "a/p" "b/p"` with quoting).
fn parse_diff_git_path(rest: &str) -> String {
    match rest.split(' ').next_back() {
        Some(b) => b.strip_prefix("b/").unwrap_or(b).to_string(),
        None => rest.to_string(),
    }
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
}
