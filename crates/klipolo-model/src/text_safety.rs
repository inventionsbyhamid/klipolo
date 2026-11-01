//! Invisible and bidirectional characters in agent-written text, shared by the MCP server and the library;
//! and the cleaning of a library version's name, description and tags, shared by the library, which
//! cleans before it saves, and the folder loader, which refuses what isn't already clean.

/// A library version's name, in characters.
pub const NAME_CHARS: usize = 40;
pub const DESCRIPTION_CHARS: usize = 200;
pub const MAX_TAGS: usize = 8;
pub const TAG_CHARS: usize = 24;
/// The app that made a version, as it names itself (`clientInfo.name`), and its licence id.
pub const MADE_BY_CHARS: usize = 60;

/// Characters that change no letter's look: zero-width ones, joiners, direction marks and overrides, the soft hyphen,
/// the byte-order mark, tags (Unicode's format characters, Cf); and the ones that draw nothing or only pick a glyph's
/// variant: variation selectors, the combining grapheme joiner, Hangul fillers and the blank Braille pattern. The MCP
/// server drops them from app names; the shader guard refuses them in code; the library drops them from names.
pub fn invisible(c: char) -> bool {
    matches!(c as u32,
        0xAD | 0x34F | 0x600..=0x605 | 0x61C | 0x6DD | 0x70F | 0x8E2 | 0x115F | 0x1160 | 0x180B..=0x180F
        | 0x200B..=0x200F | 0x202A..=0x202E | 0x2060..=0x2064 | 0x2066..=0x206F | 0x2800 | 0x3164
        | 0xFE00..=0xFE0F | 0xFEFF | 0xFFA0 | 0xFFF9..=0xFFFB | 0x110BD | 0x1BCA0..=0x1BCA3 | 0x1D173..=0x1D17A
        | 0xE0001 | 0xE0020..=0xE007F | 0xE0100..=0xE01EF)
}

/// `text` with control, invisible and bidirectional characters removed, every run of spaces and line breaks one space,
/// none at either end, and at most `max_chars` characters. Cleaning what is clean changes nothing, so a loader can
/// check a stored name by cleaning it again. Reads no further than it needs: a 1 MiB name costs what 40 characters do.
pub fn clean(text: &str, max_chars: usize) -> String {
    let mut out = String::new();
    let mut count = 0;
    let mut space = false;
    for c in text.chars() {
        if c.is_whitespace() {
            space = count > 0;
            continue;
        }
        if c.is_control() || invisible(c) {
            continue;
        }
        let needed = if space { 2 } else { 1 };
        if count + needed > max_chars {
            break;
        }
        if space {
            out.push(' ');
            space = false;
        }
        out.push(c);
        count += needed;
    }
    out
}

/// The invisible characters real text needs, which words put on screen keep: the zero-width joiner and non-joiner (an
/// emoji sequence such as 👩‍💻, the Persian `می‌شود`) and the variation selectors (an emoji's form, as in ❤️; an
/// ideograph's variant).
fn shaping(c: char) -> bool {
    matches!(c as u32, 0x200C | 0x200D | 0xFE00..=0xFE0F | 0xE0100..=0xE01EF)
}

/// What starts a new line: a line feed, a carriage return (with the line feed after it, one break), next line, and the
/// line and paragraph separators.
fn line_break(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\u{85}' | '\u{2028}' | '\u{2029}')
}

/// Words another app gives for the screen (a text layer's words, a title card's title), cleaned as `clean` cleans, but
/// keeping what real text needs: joiners and variation selectors (`shaping`), and line breaks, which a text layer draws
/// (each kind of break one `\n`; spaces beside a break, and breaks at either end, go). Control, other invisible and
/// bidirectional characters go; other runs of spaces are one space; at most `max_chars` characters, breaks counted.
/// Cleaning what is clean changes nothing.
pub fn clean_shown(text: &str, max_chars: usize) -> String {
    let mut out = String::new();
    let mut count = 0;
    let (mut space, mut breaks, mut after_cr) = (false, 0usize, false);
    for c in text.chars() {
        if line_break(c) {
            let joined = c == '\n' && after_cr;
            after_cr = c == '\r';
            if !joined && count > 0 {
                breaks += 1;
                space = false;
            }
            continue;
        }
        after_cr = false;
        if c.is_whitespace() {
            space = count > 0 && breaks == 0;
            continue;
        }
        if c.is_control() || (invisible(c) && !shaping(c)) {
            continue;
        }
        let before = if breaks > 0 {
            breaks
        } else {
            usize::from(space)
        };
        if count + before + 1 > max_chars {
            break;
        }
        if breaks > 0 {
            out.extend(std::iter::repeat_n('\n', breaks));
        } else if space {
            out.push(' ');
        }
        (space, breaks) = (false, 0);
        out.push(c);
        count += before + 1;
    }
    out
}

/// The first character in `text` that would hide, reorder or break what is shown: a control
/// character, a line or paragraph separator, or an invisible one; but not what real text uses: one variation selector
/// right after a visible character (`❤️`, `1️⃣`), and a zero-width joiner or non-joiner between two visible characters
/// (`👩‍💻`, the Persian `می‌شود`). A run of selectors or joiners is refused. Text that is checked, never cleaned (a
/// definition's settings, which its key covers).
pub fn hidden(text: &str) -> Option<char> {
    let visible = |c: char| !c.is_whitespace() && !c.is_control() && !invisible(c);
    let mut prev: Option<char> = None;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        let next = chars.peek().copied();
        let shown = match c {
            '\u{FE0E}' | '\u{FE0F}' => prev.is_some_and(visible),
            '\u{200C}' | '\u{200D}' => {
                prev.is_some_and(|p| visible(p) || p == '\u{FE0F}') && next.is_some_and(visible)
            }
            '\u{2028}' | '\u{2029}' => false,
            _ => !c.is_control() && !invisible(c),
        };
        if !shown {
            return Some(c);
        }
        prev = Some(c);
    }
    None
}

/// At most `MAX_TAGS` tags, each cleaned to `TAG_CHARS`, the empty ones left out.
pub fn clean_tags(tags: &[String]) -> Vec<String> {
    tags.iter()
        .map(|t| clean(t, TAG_CHARS))
        .filter(|t| !t.is_empty())
        .take(MAX_TAGS)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direction_overrides_and_zero_widths_are_invisible_letters_are_not() {
        for c in [
            '\u{202E}',
            '\u{2066}',
            '\u{200B}',
            '\u{200D}',
            '\u{FEFF}',
            '\u{E0041}',
            '\u{FE0F}',
        ] {
            assert!(invisible(c), "{:04X}", c as u32);
        }
        for c in ['a', 'é', ' ', '\n', 'ß', '中', '😀'] {
            assert!(!invisible(c), "{:04X}", c as u32);
        }
    }

    /// Words an app puts on screen keep what real text needs: joiners and variation selectors, and line breaks (a text
    /// layer draws them), each kind of break one line break; control, invisible and bidirectional characters go, and
    /// spaces as `clean` folds them, none beside a line break.
    #[test]
    fn words_to_show_keep_joiners_selectors_and_line_breaks() {
        for kept in [
            "\u{1F469}\u{200D}\u{1F4BB}",
            "\u{645}\u{6CC}\u{200C}\u{634}\u{648}\u{62F}",
            "\u{2764}\u{FE0F}",
            "\u{2764}\u{FE0E}",
            "\u{845B}\u{E0100}",
            "\u{845B}\u{E01EF}",
            "one\ntwo",
        ] {
            assert_eq!(clean_shown(kept, 100), kept, "{kept:?}");
        }
        assert_eq!(
            clean_shown(" a \r\n b\rc\u{2028}d\u{2029}e\u{85}f\u{B}g ", 100),
            "a\nb\nc\nd\ne\nf g"
        );
        assert_eq!(clean_shown("a\u{202E}b\u{200B}c\u{7}\u{FEFF}", 100), "abc");
        assert_eq!(clean_shown("a   b\t\tc", 100), "a b c");
        assert_eq!(clean_shown("\n\n a \n\n", 100), "a");
        assert_eq!(clean_shown("a\n\n\nb", 100), "a\n\n\nb");
        assert_eq!(clean_shown("abcdef", 3), "abc");
        assert_eq!(clean_shown("ab cd", 3), "ab");
        // Cleaning what is clean changes nothing.
        let once = clean_shown(" \u{1F469}\u{200D}\u{1F4BB} a\r\nb ", 100);
        assert_eq!(clean_shown(&once, 100), once);
    }

    /// Where a joiner may sit. Between two visible characters it is real text (an emoji
    /// sequence, a Persian word); doubled, first, last or beside a space it hides something.
    #[test]
    fn a_joiner_passes_only_between_two_visible_characters() {
        for real in ["👩\u{200D}💻", "می\u{200C}شود", "❤\u{FE0F}\u{200D}🔥"] {
            assert_eq!(hidden(real), None, "{real:?}");
        }
        for hiding in [
            "a\u{200D}\u{200D}b",
            "\u{200D}a",
            "a\u{200C}",
            "a\u{200D} b",
            "a \u{200C}b",
        ] {
            assert!(hidden(hiding).is_some(), "{hiding:?}");
        }
    }
}
