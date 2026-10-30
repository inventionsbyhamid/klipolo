//! The text caps before naga sees the body, in linear passes: its size, NULs, lines, invisible characters outside
//! comments, bracket nesting with `else if` chains counted as nesting, and operators and selectors per statement.
//! naga parses operator, selector and `else if` chains iteratively but lowers, validates and writes them
//! recursively, one stack frame per link, so the last two caps keep it off deep recursion. Comments are
//! skipped exactly as naga's lexer skips them (a line comment ends at any line break, block comments nest), so what
//! this calls code is what naga reads as code. Direction characters, and line breaks other than `\n` and `\r\n`, are
//! refused everywhere, comments included (Trojan Source: an override in a comment, or a break naga ends a `//`
//! comment at but a viewer doesn't show, can make the code read differently from what naga compiles). The main
//! process runs it on its own: it builds at most `MAX_REPORTED` diagnostics, so even a body full of problems
//! costs one pass.

use klipolo_model::text_safety::invisible;

use super::chains::structure;
use super::names::names;
use super::{Diagnostic, MAX_LINE_CHARS, MAX_LINES, MAX_REPORTED, MAX_SOURCE_BYTES};

/// The pre-parse caps on an author's body: what the main process checks before anything else sees it.
pub fn scan(body: &str) -> Vec<Diagnostic> {
    if body.len() > MAX_SOURCE_BYTES {
        return vec![
            Diagnostic::error(
                "scan.size",
                format!(
                    "The shader is {} bytes; at most {MAX_SOURCE_BYTES} (32 KiB) are allowed.",
                    body.len()
                ),
            )
            .hint("Shorten it: share repeated code in a function and keep comments brief."),
        ];
    }
    let mut out = Vec::new();
    if let Some(at) = body.find('\0') {
        out.push(
            Diagnostic::error("scan.nul", "The shader contains a NUL character.").at(body, at),
        );
    }
    let lines = body.lines().count();
    if lines > MAX_LINES {
        out.push(
            Diagnostic::error(
                "scan.lines",
                format!("The shader has {lines} lines; at most {MAX_LINES} are allowed."),
            )
            .hint("Share repeated code in a function, or drop long comments."),
        );
    }
    let mut start = 0;
    for line in body.split('\n') {
        if out.len() >= MAX_REPORTED {
            break;
        }
        if let Some((i, _)) = line
            .trim_end_matches('\r')
            .char_indices()
            .nth(MAX_LINE_CHARS)
        {
            out.push(
                Diagnostic::error(
                    "scan.line",
                    format!("This line is longer than {MAX_LINE_CHARS} characters."),
                )
                .at(body, start + i)
                .hint("Break it over several lines."),
            );
        }
        start += line.len() + 1;
    }
    let mut chars = body.char_indices().peekable();
    // Each report locates itself in O(n): stop once there are enough to show.
    while let Some((at, c)) = chars.next().filter(|_| out.len() < MAX_REPORTED) {
        let lone_cr = c == '\r' && chars.peek().is_none_or(|&(_, n)| n != '\n');
        if direction(c) {
            out.push(
                Diagnostic::error(
                    "scan.invisible",
                    format!(
                        "A direction character (U+{:04X}) is in the shader.",
                        c as u32
                    ),
                )
                .at(body, at)
                .hint("Delete it: direction characters can make code read differently from what runs, so none are allowed, even in comments."),
            );
        } else if lone_cr || other_break(c) {
            out.push(
                Diagnostic::error(
                    "scan.newline",
                    format!(
                        "A line break other than a newline (U+{:04X}) is in the shader.",
                        c as u32
                    ),
                )
                .at(body, at)
                .hint("Use a plain newline: WGSL ends a `//` comment at this one, where a viewer may not show a break."),
            );
        }
    }
    let unclosed = code_chars(body, |at, c| {
        if invisible(c) && !direction(c) && out.len() < MAX_REPORTED {
            out.push(
                Diagnostic::error(
                    "scan.invisible",
                    format!(
                        "An invisible character (U+{:04X}) is in the code.",
                        c as u32
                    ),
                )
                .at(body, at)
                .hint("Delete it: only comments may hold invisible characters."),
            );
        }
    });
    match unclosed {
        Some(at) => out.push(
            Diagnostic::error("scan.comment", "This `/*` comment is never closed.").at(body, at),
        ),
        None => {
            let toks = tokens(body);
            names(body, &toks, &mut out);
            structure(body, &toks, &mut out);
        }
    }
    out.truncate(MAX_REPORTED);
    out
}

/// Unicode's bidirectional controls (Bidi_Control): the marks, embeddings, overrides and isolates that reorder how
/// text displays. All are `invisible` too.
fn direction(c: char) -> bool {
    matches!(c as u32, 0x61C | 0x200E | 0x200F | 0x202A..=0x202E | 0x2066..=0x2069)
}

/// naga's other line breaks: VT, FF, NEL and the line and paragraph separators (a lone `\r` is one too).
fn other_break(c: char) -> bool {
    matches!(
        c,
        '\u{000B}' | '\u{000C}' | '\u{0085}' | '\u{2028}' | '\u{2029}'
    )
}

/// Calls `code` with each character outside comments and its byte offset. Returns where an unclosed block comment
/// starts, if one does.
pub(super) fn code_chars(body: &str, mut code: impl FnMut(usize, char)) -> Option<usize> {
    let mut chars = body.char_indices().peekable();
    while let Some((at, c)) = chars.next() {
        if c != '/' {
            code(at, c);
            continue;
        }
        match chars.peek().map(|&(_, n)| n) {
            Some('/') => {
                // naga: `chars.position(is_comment_end)`.
                for (_, c) in chars.by_ref() {
                    if matches!(
                        c,
                        '\u{000a}'..='\u{000d}' | '\u{0085}' | '\u{2028}' | '\u{2029}'
                    ) {
                        break;
                    }
                }
            }
            Some('*') => {
                chars.next();
                // naga's own walk: a `*/` or a `/*` is only seen when its first character didn't end a pair.
                let (mut depth, mut prev) = (1, None);
                let mut closed = false;
                for (_, c) in chars.by_ref() {
                    match (prev, c) {
                        (Some('*'), '/') => {
                            prev = None;
                            depth -= 1;
                            if depth == 0 {
                                closed = true;
                                break;
                            }
                        }
                        (Some('/'), '*') => {
                            prev = None;
                            depth += 1;
                        }
                        _ => prev = Some(c),
                    }
                }
                if !closed {
                    return Some(at);
                }
            }
            _ => code(at, c),
        }
    }
    None
}

/// The code's words (identifiers, keywords, numbers) and punctuation, with their byte offsets: what the reserved-name
/// rule reads. A word is a run of `_`, ASCII letters and digits and non-ASCII characters other than spaces, which
/// covers every identifier naga accepts.
pub(super) fn tokens(body: &str) -> Vec<(usize, &str)> {
    let mut out: Vec<(usize, &str)> = Vec::new();
    let mut word: Option<usize> = None;
    let mut last_end = 0;
    let word_char =
        |c: char| c == '_' || c.is_ascii_alphanumeric() || (!c.is_ascii() && !c.is_whitespace());
    code_chars(body, |at, c| {
        // A comment between two characters ends a word too.
        if let Some(s) = word
            && (!word_char(c) || at != last_end)
        {
            out.push((s, &body[s..last_end]));
            word = None;
        }
        if word_char(c) {
            word.get_or_insert(at);
        } else if !c.is_whitespace() {
            out.push((at, &body[at..at + c.len_utf8()]));
        }
        last_end = at + c.len_utf8();
    });
    if let Some(s) = word {
        out.push((s, &body[s..last_end]));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comments_are_skipped_as_naga_skips_them() {
        let codes = |s: &str| scan(s).iter().map(|d| d.code).collect::<Vec<_>>();
        assert!(
            codes("// a \u{200B} note\nlet a = 1; /* nested /* \u{200B} */ still */").is_empty()
        );
        assert_eq!(codes("/* /* */ let a = (1"), ["scan.comment"]);
        assert_eq!(codes("let a = (1;"), ["scan.brackets"]);
        assert_eq!(
            codes("// x\u{2028}let a\u{200B} = 1;"),
            ["scan.newline", "scan.invisible"],
            "U+2028 ends a line comment"
        );
        let words: Vec<_> = tokens("var<private> kl/* */x: f32; // kly")
            .into_iter()
            .map(|(_, t)| t)
            .collect();
        assert_eq!(
            words,
            ["var", "<", "private", ">", "kl", "x", ":", "f32", ";"]
        );
    }

    #[test]
    fn direction_characters_are_refused_in_comments_too() {
        // Trojan Source: a direction override in a comment can make the code after it read differently from what
        // naga compiles, so comments may hold none.
        let codes = |s: &str| scan(s).iter().map(|d| d.code).collect::<Vec<_>>();
        for c in [
            '\u{061C}', '\u{200E}', '\u{200F}', '\u{202A}', '\u{202B}', '\u{202C}', '\u{202D}',
            '\u{202E}', '\u{2066}', '\u{2067}', '\u{2068}', '\u{2069}',
        ] {
            assert!(invisible(c), "U+{:04X} is invisible too", c as u32);
            for text in [
                format!("// a {c} note\nlet a = 1;"),
                format!("let a = 1; /* {c} */"),
                format!("/* nested /* {c} */ still */"),
                format!("/* never closed {c}"),
            ] {
                assert!(
                    codes(&text).contains(&"scan.invisible"),
                    "U+{:04X} in {text:?}",
                    c as u32
                );
            }
        }
        let d = scan("let a = 1;\n/*\u{202E} } \u{2066}if (admin) {\u{2069} \u{2066} */");
        assert_eq!(d.len(), 4, "{d:?}");
        assert!(d.iter().all(|d| d.code == "scan.invisible"));
        assert_eq!((d[0].line, d[0].column), (2, 3));
        // One report for one character in the code, not one per rule.
        assert_eq!(codes("let a\u{202E} = 1;"), ["scan.invisible"]);
        // Other invisible characters stay allowed in comments.
        assert!(codes("// \u{200B}\nlet a = 1; /* \u{FEFF} */").is_empty());
    }

    #[test]
    fn line_breaks_other_than_a_newline_are_refused() {
        // naga ends a `//` comment at each of them, where a viewer may not break the line: code could sit on what
        // reads as the comment's line.
        let codes = |s: &str| scan(s).iter().map(|d| d.code).collect::<Vec<_>>();
        for c in [
            '\u{000B}', '\u{000C}', '\r', '\u{0085}', '\u{2028}', '\u{2029}',
        ] {
            for text in [
                format!("// a note{c}let a = 1;"),
                format!("let a = 1;{c}let b = 2;"),
                format!("/* {c} */"),
            ] {
                assert_eq!(
                    codes(&text),
                    ["scan.newline"],
                    "U+{:04X} in {text:?}",
                    c as u32
                );
            }
        }
        assert!(codes("// a\r\nlet a = 1;\n// b\nlet b = 2;\r\n").is_empty());
        let d = scan("let a = 1;\n// x\u{2028}let b = 2;");
        assert_eq!((d[0].line, d[0].column), (2, 5));
    }
}
