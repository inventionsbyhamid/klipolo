//! The chain caps: naga parses operator, selector and `else if` chains iteratively but lowers,
//! validates and writes them recursively, one stack frame per link, so the scan bounds them before naga sees the text.

use super::{Diagnostic, MAX_CHAIN, MAX_NESTING, MAX_REPORTED};

/// Over the code's tokens: bracket nesting plus the `else if` chains around each point (each `else if` one level: naga
/// recurses once per level), and operators and selectors (`.member`, `.xy`, `[i]`) between `;`, `{` and `}`. A type's
/// template brackets (`array<vec2<f32>, 64>`), the minus of a negative number, a number's point and an exponent's sign
/// (`1.5e-3`, `1.5e+3`, `1.e-3`) aren't links: naga doesn't recurse on them (reviews m-c, m7), so a
/// constant table fits. Brackets inside a template count like any other.
pub(super) fn structure(body: &str, toks: &[(usize, &str)], out: &mut Vec<Diagnostic>) {
    const OPERATORS: &str = "+-*/%&|^<>=!~";
    const RESETS: [&str; 6] = ["for", "while", "loop", "switch", "return", "discard"];
    let templates = templates(toks);
    let mut open: Vec<(char, usize)> = Vec::new();
    // `else if`s so far in the chain at each depth, and their sum.
    let mut chains: Vec<usize> = vec![0];
    let mut chained = 0;
    let mut links = 0;
    let (mut deep, mut long) = (false, false);
    for (i, &(at, t)) in toks.iter().enumerate() {
        if templates[i] {
            continue;
        }
        let here = chains.len() - 1;
        let value_before = i > 0
            && (toks[i - 1].1 == ")"
                || toks[i - 1].1 == "]"
                || toks[i - 1]
                    .1
                    .starts_with(|c: char| c == '_' || c.is_ascii_alphanumeric()));
        let number_after = toks
            .get(i + 1)
            .is_some_and(|n| n.1.starts_with(|c: char| c.is_ascii_digit()));
        let digits = |j: usize| toks[j].1.bytes().all(|b| b.is_ascii_digit());
        // `1.` is a number's point: naga reads it as part of the number.
        let point_after_digits = i > 0 && digits(i - 1);
        // `1.5e-3` lexes as `1`, `.`, `5e`, `-`, `3`, and `1.e-3` as `1`, `.`, `e`, `-`, `3`: a decimal mantissa
        // ending in `e` (never a hex number). A lone `e` counts only after `<digits>.`, since `s.e - 3` is a link.
        let exponent_before = i > 0 && {
            let w = toks[i - 1].1.as_bytes();
            matches!(w.last(), Some(b'e' | b'E'))
                && if w.len() >= 2 {
                    w[..w.len() - 1].iter().all(u8::is_ascii_digit)
                } else {
                    i >= 3 && toks[i - 2].1 == "." && digits(i - 3)
                }
        };
        let reset = |chains: &mut Vec<usize>, chained: &mut usize| {
            *chained -= chains[here];
            chains[here] = 0;
        };
        match t {
            "(" | "[" | "{" => {
                if t == "[" {
                    links += 1;
                } else if t == "{" {
                    links = 0;
                }
                open.push((t.chars().next().unwrap_or('('), at));
                chains.push(0);
            }
            ")" | "]" | "}" => {
                open.pop();
                if chains.len() > 1 {
                    chained -= chains.pop().unwrap_or(0);
                }
                if t == "}" {
                    links = 0;
                }
            }
            ";" => {
                links = 0;
                reset(&mut chains, &mut chained);
            }
            "if" if i > 0 && toks[i - 1].1 == "else" => {
                chains[here] += 1;
                chained += 1;
            }
            "if" => reset(&mut chains, &mut chained),
            // A member or swizzle: whatever follows that isn't a number's fraction (`.℘` too).
            "." if toks.get(i + 1).is_some() && !number_after && !point_after_digits => links += 1,
            "-" if (!value_before || exponent_before) && number_after => {}
            "+" if exponent_before && number_after => {}
            _ if RESETS.contains(&t) => reset(&mut chains, &mut chained),
            _ if t.len() == 1 && OPERATORS.contains(t) => links += 1,
            _ => {}
        }
        if open.len() + chained > MAX_NESTING && !deep && out.len() < MAX_REPORTED {
            deep = true;
            out.push(
                Diagnostic::error(
                    "scan.nesting",
                    format!(
                        "Brackets and `else if` chains nest more than {MAX_NESTING} deep here."
                    ),
                )
                .at(body, at)
                .hint("Split the expression with `let`s, move the inner part into a function, or use a `switch`."),
            );
        }
        if links > MAX_CHAIN && !long && out.len() < MAX_REPORTED {
            long = true;
            out.push(
                Diagnostic::error(
                    "scan.chain",
                    format!("This statement chains more than {MAX_CHAIN} operators or selectors."),
                )
                .at(body, at)
                .hint("Split it with `let`s."),
            );
        }
    }
    if let Some(&(c, at)) = open.last()
        && out.len() < MAX_REPORTED
    {
        out.push(
            Diagnostic::error("scan.brackets", format!("This `{c}` is never closed.")).at(body, at),
        );
    }
}

/// The types naga parses a template after (`parse/mod.rs`), by exact name: an author's `texture_x` is a value. Not
/// `texture_1d_array`, `texture_multisampled_2d_array` or `texture_storage_1d_array`: naga 25 doesn't reserve them, so
/// they can be a member's name, and a body can't bind a texture anyway.
const TEMPLATED: [&str; 34] = [
    "vec2",
    "vec3",
    "vec4",
    "mat2x2",
    "mat2x3",
    "mat2x4",
    "mat3x2",
    "mat3x3",
    "mat3x4",
    "mat4x2",
    "mat4x3",
    "mat4x4",
    "array",
    "atomic",
    "ptr",
    "bitcast",
    "binding_array",
    "sampler",
    "sampler_comparison",
    "texture_1d",
    "texture_2d",
    "texture_2d_array",
    "texture_3d",
    "texture_cube",
    "texture_cube_array",
    "texture_multisampled_2d",
    "texture_depth_2d",
    "texture_depth_2d_array",
    "texture_depth_cube",
    "texture_depth_cube_array",
    "texture_storage_1d",
    "texture_storage_2d",
    "texture_storage_2d_array",
    "texture_storage_3d",
];

/// The `<` and `>` of types' templates: a `<` right after a templated type's name, to the `>` that closes it outside
/// any bracket opened within (a `>` inside parentheses is an operator, as naga reads it), within 64 tokens. Only these
/// two tokens are marked: brackets inside a template still count, so a template can't forge the nesting.
fn templates(toks: &[(usize, &str)]) -> Vec<bool> {
    let mut skip = vec![false; toks.len()];
    let typed = |j: usize| j > 0 && TEMPLATED.contains(&toks[j - 1].1);
    let mut i = 1;
    while i < toks.len() {
        if toks[i].1 == "<" && typed(i) {
            let (mut angles, mut brackets, mut end) = (Vec::new(), 0usize, None);
            for (j, t) in toks.iter().enumerate().skip(i).take(64) {
                match t.1 {
                    "(" | "[" | "{" => brackets += 1,
                    ")" | "]" | "}" if brackets == 0 => break,
                    ")" | "]" | "}" => brackets -= 1,
                    ";" => break,
                    "<" if brackets == 0 && typed(j) => angles.push(j),
                    ">" if brackets == 0 => {
                        angles.push(j);
                        let opened = angles.iter().filter(|&&a| toks[a].1 == "<").count();
                        if angles.len() == 2 * opened {
                            end = Some(j);
                            break;
                        }
                    }
                    _ => {}
                }
            }
            if let Some(end) = end {
                angles.iter().for_each(|&a| skip[a] = true);
                i = end;
            }
        }
        i += 1;
    }
    skip
}
