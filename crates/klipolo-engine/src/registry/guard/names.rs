//! Names naga's WGSL writer would rewrite (checked in the scan, so the rule holds before naga parses). The writer,
//! which makes the text a device compiles (`harden`), keeps only ASCII letters, digits and single underscores in a name
//! and drops a trailing `_`, so `fn max_` or `fn maäx` would come back as `fn max` and take WGSL's own `max` calls in
//! that text. And a non-ASCII name such as `℘` isn't counted as a selector by every reader: refusing it before the
//! parse keeps member chains in the scan's sight.

use klipolo_model::text_safety::invisible;

use super::{Diagnostic, MAX_REPORTED};

pub(super) fn names(body: &str, toks: &[(usize, &str)], out: &mut Vec<Diagnostic>) {
    let mut seen: Vec<&str> = Vec::new();
    for &(at, word) in toks {
        if out.len() >= MAX_REPORTED {
            break;
        }
        // A word with an invisible character is `scan.invisible`'s.
        if rewritten(word) && !word.chars().any(invisible) && !seen.contains(&word) {
            seen.push(word);
            out.push(
                Diagnostic::error(
                    "rule.name",
                    format!(
                        "`{word}` would change when the host writes the shader out for the GPU, and could then name one \
                         of WGSL's own functions."
                    ),
                )
                .at(body, at)
                .hint("Use ASCII letters, digits and single underscores, not ending in `_`."),
            );
        }
    }
}

/// A name naga's writer rewrites (`proc::Namer::sanitize`): one with a character other than an ASCII letter, digit or
/// `_`, two underscores in a row, a trailing `_`, or the WGSL writer's reserved `_naga` prefix (`_nagafoo` comes back
/// as `gen__nagafoo`). WGSL's phony `_` is no name. Words that start with a digit are numbers.
fn rewritten(word: &str) -> bool {
    let name = word.chars().next().is_some_and(|c| {
        !c.is_ascii_digit() && (c == '_' || c.is_ascii_alphabetic() || !c.is_ascii())
    });
    name && word != "_"
        && (!word.is_ascii()
            || word.ends_with('_')
            || word.contains("__")
            || word.starts_with("_naga"))
}

#[cfg(test)]
mod tests {
    use super::rewritten;

    #[test]
    fn names_the_writer_keeps_and_those_it_rewrites() {
        for kept in [
            "max",
            "_half",
            "x1",
            "my_max",
            "_",
            "0x7f800000u",
            "1e3",
            "naga",
            "_nag",
        ] {
            assert!(!rewritten(kept), "{kept}");
        }
        for changed in [
            "max_", "maäx", "x__y", "größe", "__x", "a_", "℘", "_naga", "_nagafoo",
        ] {
            assert!(rewritten(changed), "{changed}");
        }
    }
}
