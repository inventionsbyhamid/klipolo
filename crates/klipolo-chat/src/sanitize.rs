//! Text the model writes, made fit to show the person: Klipolo never shows an em or en dash (the copy rules). The
//! system prompt asks for none; this catches those that slip through. Use it on everything the model writes for the
//! person: replies, `askUser` questions, `cannotDo` reasons.

/// The figure dash, the en dash, the em dash and the horizontal bar.
const DASHES: [char; 4] = ['\u{2012}', '\u{2013}', '\u{2014}', '\u{2015}'];

/// `text` with every dash in `DASHES` replaced as it reads: a hyphen between digits ("10–20" reads "10-20", the spaces
/// around it dropped) and for an unspaced en or figure dash inside a word ("pre–roll"); else a comma, as a dash between
/// clauses reads ("Done — want more?" reads "Done, want more?"). A dash at either end goes, and one after other
/// punctuation leaves only a space.
pub fn reply(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        let dash = chars[i];
        if !DASHES.contains(&dash) {
            out.push(dash);
            i += 1;
            continue;
        }
        let spaced_before = out.ends_with(char::is_whitespace);
        let mut next = i + 1;
        while next < chars.len() && chars[next].is_whitespace() {
            next += 1;
        }
        let spaced_after = next > i + 1;
        out.truncate(out.trim_end().len());
        let before = out.chars().last();
        let after = chars.get(next).copied();
        let between = |test: fn(&char) -> bool| {
            before.is_some_and(|b| test(&b)) && after.is_some_and(|a| test(&a))
        };
        let in_word = !spaced_before
            && !spaced_after
            && matches!(dash, '\u{2012}' | '\u{2013}')
            && between(|c| c.is_alphanumeric());
        if between(char::is_ascii_digit) || in_word {
            out.push('-');
        } else if before.is_none() || after.is_none() {
            // At either end: nothing to join.
        } else if out.ends_with([',', ';', ':', '.', '!', '?', '(']) {
            out.push(' ');
        } else {
            out.push_str(", ");
        }
        i = next;
    }
    out
}
