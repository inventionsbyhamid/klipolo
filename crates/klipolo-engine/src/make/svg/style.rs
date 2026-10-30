//! What an SVG's `<style>` sheets cost usvg, counted from above before it runs.
//! simplecss makes a rule of each selector of a comma list (each with its own copy of the list's declarations); usvg
//! tests every rule against every element, every `<use>` copy included, and writes the declarations of those that
//! match; and each descendant combinator walks back through every ancestor, so a selector's tests grow as the depth
//! to the power of its descendant combinators. Read as simplecss reads a sheet, erring long: comments skipped; an
//! at-rule skipped to its `;` or past its whole block; a rule's selectors running to its `{` (quoted text and comments
//! skipped, as its selector parser does), its declarations to the first `}`, weighed to the later `}` its own quoted
//! text may hide.

/// Descendant combinators past this are lumped together, as too many.
const MANY: usize = 16;

/// The sheets of one SVG, weighed.
#[derive(Default)]
pub(super) struct Sheet {
    pub bytes: usize,
    /// Selectors: a rule each.
    pub selectors: usize,
    /// Per element: the declaration bytes the rules would write (each selector its block's)…
    writes: u64,
    /// …and the simple selectors to test, by their selector's descendant combinators.
    parts: [u64; MANY + 1],
}

/// The end of the comment or quoted text opened at `b[i]` (past it), if one opens there.
fn skip_quoted(b: &[u8], i: usize) -> Option<usize> {
    match b[i] {
        b'/' if b.get(i + 1) == Some(&b'*') => Some(
            b[i + 2..]
                .windows(2)
                .position(|w| w == b"*/")
                .map_or(b.len(), |p| i + 2 + p + 2),
        ),
        q @ (b'"' | b'\'') => {
            let mut j = i + 1;
            while j < b.len() && b[j] != q {
                j += 1 + usize::from(b[j] == b'\\');
            }
            Some((j + 1).min(b.len()))
        }
        _ => None,
    }
}

/// Where `until` first stands in `b` from `i`, comments and quoted text skipped when `quoted`; else the end.
fn find(b: &[u8], mut i: usize, until: u8, quoted: bool) -> usize {
    while i < b.len() && b[i] != until {
        i = match quoted.then(|| skip_quoted(b, i)).flatten() {
            Some(end) => end,
            None => i + 1,
        };
    }
    i
}

/// A selector's simple selectors (each compound one, and each class, id, attribute and pseudo-class in it) and its
/// descendant combinators (space between compounds, not beside `>`, `+` or `~`).
fn weigh(sel: &[u8]) -> (u64, usize) {
    let (mut parts, mut descendants, mut nested) = (0u64, 0usize, 0usize);
    let (mut in_compound, mut gap, mut explicit) = (false, false, false);
    for &c in sel {
        if nested > 0 {
            match c {
                b'[' | b'(' => nested += 1,
                b']' | b')' => nested -= 1,
                _ => {}
            }
            continue;
        }
        match c {
            b' ' | b'\t' | b'\n' | b'\r' | b'\x0c' => {
                gap |= in_compound;
                in_compound = false;
            }
            b'>' | b'+' | b'~' => (explicit, gap, in_compound) = (true, false, false),
            _ => {
                if !in_compound {
                    descendants += usize::from(gap && !explicit && parts > 0);
                    parts += 1;
                    (in_compound, gap, explicit) = (true, false, false);
                }
                parts += u64::from(matches!(c, b'.' | b'#' | b'[' | b':'));
                nested += usize::from(matches!(c, b'[' | b'('));
            }
        }
    }
    (parts.max(1), descendants)
}

impl Sheet {
    /// One `<style>`'s text, weighed into the sheet.
    pub fn read(&mut self, text: &str) {
        let b = text.as_bytes();
        self.bytes += b.len();
        let mut i = 0;
        while i < b.len() {
            if b[i].is_ascii_whitespace() {
                i += 1;
                continue;
            }
            if let Some(end) = skip_quoted(b, i).filter(|_| b[i] == b'/') {
                i = end;
                continue;
            }
            if b[i] == b'@' {
                while i < b.len() && b[i] != b';' && b[i] != b'{' {
                    i += 1;
                }
                if b.get(i) == Some(&b'{') {
                    let mut depth = 0usize;
                    while i < b.len() {
                        match b[i] {
                            b'{' => depth += 1,
                            b'}' => depth = depth.saturating_sub(1),
                            _ => {}
                        }
                        i += 1;
                        if depth == 0 {
                            break;
                        }
                    }
                } else {
                    i += 1;
                }
                continue;
            }
            let open = find(b, i, b'{', true);
            if open >= b.len() {
                // No block: simplecss keeps no rule without declarations.
                break;
            }
            let close = find(b, open, b'}', false);
            let block = find(b, open, b'}', true).max(close) - open;
            let mut selectors = 0u64;
            for sel in b[i..open].split(|&c| c == b',') {
                let (parts, descendants) = weigh(sel);
                self.parts[descendants.min(MANY)] += parts;
                selectors += 1;
            }
            self.selectors += selectors as usize;
            self.writes = self
                .writes
                .saturating_add(selectors.saturating_mul(block as u64));
            i = close + 1;
        }
    }

    /// The work one element `depth` ancestors deep at most costs: each simple selector tested along each way back
    /// through its ancestors, and each declaration written.
    pub fn per_element(&self, depth: usize) -> u64 {
        let ways = depth as u64 + 1;
        let mut work = self.writes;
        for (descendants, &parts) in self.parts.iter().enumerate() {
            let tries = if descendants >= MANY {
                u64::MAX
            } else {
                ways.saturating_pow(descendants as u32)
            };
            if parts > 0 {
                work = work.saturating_add(parts.saturating_mul(tries));
            }
        }
        work
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Selectors counted one a rule (comma lists split, at-rules and comments not rules), compound and simple selectors
    /// and descendant combinators weighed, blocks written once per selector.
    #[test]
    fn a_sheet_is_weighed_as_simplecss_reads_it() {
        assert_eq!(weigh(b"svg g > .a.b"), (5, 1));
        assert_eq!(weigh(b" x  g g "), (3, 2));
        assert_eq!(weigh(b"a + b ~ c"), (3, 0));
        assert_eq!(weigh(b"[x=\"a b\"] g"), (3, 1));
        let mut sheet = Sheet::default();
        sheet.read("/* a, b { */ @media print { a, b { fill: red } } .a, .b { fill: red } g g { x: \"}\" }");
        assert_eq!(sheet.selectors, 3);
        // `.a` and `.b` (2 parts each, no combinator), `g g` (2 parts, one); blocks of 12 bytes twice and 9 once.
        assert_eq!(sheet.per_element(0), 2 * 12 + 9 + 4 + 2);
        assert_eq!(sheet.per_element(9), 2 * 12 + 9 + 4 + 2 * 10);
    }
}
