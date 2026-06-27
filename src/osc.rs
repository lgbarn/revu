//! OSC 52 clipboard copy: a one-shot terminal escape that sets the system
//! clipboard, written directly to the terminal (not through ratatui's cell
//! buffer), so it works over SSH and inside tmux with no clipboard daemon.
//!
//! Pure and dependency-free: a tiny base64 encoder (OSC 52 payloads are
//! base64) plus the escape builder and the line-range text extraction, all
//! unit-testable without a terminal.

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64 (with `=` padding). Hand-rolled to avoid a dependency — OSC
/// 52 is the only base64 user in revu and the inputs are small selections.
fn base64(input: &[u8]) -> String {
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(B64[((n >> 18) & 63) as usize] as char);
        out.push(B64[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            B64[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            B64[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

/// The OSC 52 escape that copies `text` to the clipboard (`c` = clipboard
/// selection). Terminals without OSC 52 support ignore it harmlessly.
pub fn osc52_copy(text: &str) -> String {
    format!("\x1b]52;c;{}\x1b\\", base64(text.as_bytes()))
}

/// The plain text of the inclusive line range `[a, b]` (order-independent),
/// joined by newlines. Indices are clamped to `lines`; an empty slice yields an
/// empty string. Used for line-granularity drag selection.
pub fn selected_lines_text(lines: &[String], a: usize, b: usize) -> String {
    if lines.is_empty() {
        return String::new();
    }
    let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
    let hi = hi.min(lines.len() - 1);
    let lo = lo.min(hi);
    lines[lo..=hi].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_rfc4648_vectors() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foob"), "Zm9vYg==");
        assert_eq!(base64(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn osc52_wraps_the_base64_payload() {
        assert_eq!(osc52_copy("foo"), "\x1b]52;c;Zm9v\x1b\\");
    }

    #[test]
    fn selected_lines_join_and_clamp() {
        let ls: Vec<String> = ["a", "b", "c", "d"].iter().map(|s| s.to_string()).collect();
        // Single line.
        assert_eq!(selected_lines_text(&ls, 1, 1), "b");
        // Forward range.
        assert_eq!(selected_lines_text(&ls, 1, 2), "b\nc");
        // Reversed range gives the same result.
        assert_eq!(selected_lines_text(&ls, 2, 1), "b\nc");
        // End clamps to the last line.
        assert_eq!(selected_lines_text(&ls, 2, 99), "c\nd");
        // Empty input.
        assert_eq!(selected_lines_text(&[], 0, 0), "");
    }
}
