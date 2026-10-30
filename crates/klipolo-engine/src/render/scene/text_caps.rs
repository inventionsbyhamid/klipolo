//! What a scene text may hold before it is shaped: a scene's
//! text comes from projects people share, so it is held first, cheaply and plainly: at most `room` characters (500 a
//! node), each grapheme cluster at most [`MAX_CLUSTER_CHARS`] characters (a base with 10,000 combining marks keeps 31
//! of them), only the first 64 KiB looked at (a 1 MB string costs a 1 MB scan at most), and every paragraph
//! separator read as a newline. Whatever is left out is said (`frame.sceneTextCut`), never a hang or a panic.

use std::ops::Range;

use unicode_segmentation::UnicodeSegmentation;

/// The most characters one grapheme cluster keeps: Unicode's stream-safe limit of 30 marks, its base and a joiner.
pub const MAX_CLUSTER_CHARS: usize = 32;
/// The most pieces one laid-out text draws (a node's 500 characters, and its spans' cuts).
pub const MAX_PIECES: usize = 512;
/// The most bytes of one part looked at: 500 clusters of 32 four-byte characters.
const MAX_SCAN_BYTES: usize = 64 * 1024;

/// `parts` (each text with its look) as one text of at most `room` characters, each part's bytes in it, and whether
/// anything was cut. Paragraph separators (CR, CR LF, NEL, U+2028, U+2029, FS, GS, RS) become newlines, one character
/// each (a CR before LF a space), so the shaper and the cut agree on every byte.
pub(super) fn capped(parts: &[(&str, u8)], room: usize) -> (String, Vec<(Range<usize>, u8)>, bool) {
    let mut out = String::new();
    let mut spans = Vec::with_capacity(parts.len());
    let (mut chars, mut cut) = (0usize, false);
    for &(part, look) in parts {
        let start = out.len();
        let mut end = part.len().min(MAX_SCAN_BYTES);
        while !part.is_char_boundary(end) {
            end -= 1;
        }
        cut |= end < part.len();
        let part = &part[..end];
        'clusters: for cluster in part.graphemes(true) {
            if chars >= room {
                cut = true;
                break;
            }
            let mut kept = 0;
            let mut it = cluster.chars().peekable();
            while let Some(c) = it.next() {
                if kept >= MAX_CLUSTER_CHARS {
                    cut = true;
                    break;
                }
                if chars >= room {
                    cut = true;
                    break 'clusters;
                }
                out.push(match c {
                    '\r' if it.peek() == Some(&'\n') => ' ',
                    '\r' | '\u{85}' | '\u{2028}' | '\u{2029}' | '\u{1c}'..='\u{1e}' => '\n',
                    c => c,
                });
                kept += 1;
                chars += 1;
            }
        }
        spans.push((start..out.len(), look));
    }
    (out, spans, cut)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pile_of_marks_and_a_megabyte_are_held_plainly() {
        let pile = format!("a{}", "\u{301}".repeat(10_000));
        let (text, _, cut) = capped(&[(&pile, 0)], 500);
        assert!(cut);
        assert_eq!(text.chars().count(), MAX_CLUSTER_CHARS);
        let big = "x".repeat(1 << 20);
        let started = std::time::Instant::now();
        let (text, _, cut) = capped(&[(&big, 0)], 500);
        assert!(cut && text.len() == 500);
        assert!(
            started.elapsed().as_millis() < 500,
            "{:?}",
            started.elapsed()
        );
        let (text, spans, cut) = capped(&[("ab\r\ncd\u{2029}e", 0), ("fg", 1)], 500);
        assert!(!cut);
        assert_eq!(text, "ab \ncd\nefg");
        assert_eq!(spans, vec![(0..8, 0), (8..10, 1)]);
    }
}
