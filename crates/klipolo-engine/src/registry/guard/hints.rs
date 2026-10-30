//! Hints for naga's errors: what a GLSL or Shadertoy habit becomes in WGSL on Klipolo's effect ABI, matched on
//! naga's message or on the author's line. The first rule that matches gives the hint.

type Rule = (fn(&str, &str) -> bool, &'static str);

const HINTS: [Rule; 13] = [
    (
        |m, l| unknown(m, "mod") || calls(l, "mod"),
        "WGSL: use `x % y` (truncated) or `x - y * floor(x / y)` (GLSL's mod)",
    ),
    (
        |m, l| m.contains("wrong number of arguments") && calls(l, "atan"),
        "WGSL's two-argument arctangent is `atan2(y, x)`",
    ),
    (
        |m, l| {
            ["texture", "texture2D", "texelFetch"]
                .iter()
                .any(|f| unknown(m, f) || calls(l, f))
        },
        "Read the layer with `kl_sample_a(uv)` (a transition's incoming layer with `kl_sample_b(uv)`)",
    ),
    (
        |m, _| unknown(m, "iTime"),
        "Use `kl.time`: seconds since the layer started",
    ),
    (
        |m, _| unknown(m, "iResolution"),
        "Use `kl.size`: this pass's size in pixels",
    ),
    (
        |m, _| unknown(m, "fragCoord") || unknown(m, "gl_FragCoord"),
        "Use `uv * kl.size`: `uv` runs 0–1 from the top left",
    ),
    (
        |_, l| l.contains("#define"),
        "WGSL has no preprocessor: use `const NAME: f32 = …;`",
    ),
    (
        |_, l| starts_with_word(l.trim_start(), "uniform"),
        "Settings arrive in `params`, generated from your schema",
    ),
    (
        |m, l| glsl_type(m, l),
        "WGSL types are `f32`, `i32`, `u32`, `bool`, `vec2<f32>` and `mat3x3<f32>`; declare with `let x: f32 = …;` or `var`",
    ),
    (
        |m, l| m.contains("left-hand side of assignment") && swizzle_assign(l),
        "WGSL can't assign to a swizzle: build a new vector, as in `c = vec4<f32>(rgb, c.a);`",
    ),
    (
        |m, _| m.contains("left-hand side of assignment"),
        "Only a `var` can change: `let` values are fixed",
    ),
    (
        |m, _| int_and_float(m),
        "WGSL has no implicit int→float: write `f32(i)` (and `i32(x)` the other way)",
    ),
    (
        |m, _| m.starts_with("redefinition of `kl") || m.starts_with("redefinition of `Kl"),
        "Names starting with `kl` are the host's: pick another name",
    ),
];

/// Every hint, in the order they are tried: the authoring guide's GLSL → WGSL table.
pub fn table() -> impl Iterator<Item = &'static str> {
    HINTS.iter().map(|&(_, h)| h)
}

pub(super) fn hint(message: &str, line: &str) -> Option<&'static str> {
    HINTS
        .iter()
        .find(|(matches, _)| matches(message, line))
        .map(|&(_, h)| h)
}

/// naga's "no definition in scope for identifier: `name`".
fn unknown(message: &str, name: &str) -> bool {
    message
        .strip_prefix("no definition in scope for identifier: ")
        .is_some_and(|rest| rest.trim_matches('`') == name)
}

fn word_char(c: char) -> bool {
    c == '_' || c.is_alphanumeric()
}

/// `name(` as a word of its own on the line.
fn calls(line: &str, name: &str) -> bool {
    line.match_indices(name).any(|(i, _)| {
        let before = line[..i].chars().next_back();
        let after = line[i + name.len()..].trim_start();
        !before.is_some_and(word_char) && after.starts_with('(')
    })
}

fn starts_with_word(s: &str, word: &str) -> bool {
    s.strip_prefix(word)
        .is_some_and(|rest| rest.starts_with(char::is_whitespace))
}

/// A GLSL declaration (`float x`, `vec3 c`) on the line, or naga not knowing a GLSL type name.
fn glsl_type(message: &str, line: &str) -> bool {
    const TYPES: [&str; 13] = [
        "float", "int", "uint", "vec2", "vec3", "vec4", "ivec2", "ivec3", "ivec4", "mat2", "mat3",
        "mat4", "half",
    ];
    let line = line.trim_start();
    let line = line.strip_prefix("const ").unwrap_or(line);
    TYPES.iter().any(|t| {
        unknown(message, t)
            || message.trim_end_matches('`').ends_with(&format!("`{t}"))
            || line.strip_prefix(t).is_some_and(|rest| {
                rest.starts_with(' ')
                    && rest
                        .trim_start()
                        .starts_with(|c: char| c.is_alphabetic() || c == '_')
            })
    })
}

/// `v.xyz = …` (or `+=` and the like): a swizzle of two or more components on the left of an assignment.
fn swizzle_assign(line: &str) -> bool {
    line.match_indices('.').any(|(i, _)| {
        let rest = &line[i + 1..];
        let n = rest.chars().take_while(|c| "xyzwrgba".contains(*c)).count();
        let after = rest[n..].trim_start();
        let after = after.trim_start_matches(['+', '-', '*', '/', '%', '&', '|', '^']);
        n >= 2 && after.starts_with('=') && !after.starts_with("==")
    })
}

/// naga's messages for a number of one kind where another is needed.
fn int_and_float(m: &str) -> bool {
    let float = m.contains("f32") || m.contains("kind: Float");
    let int = m.contains("i32")
        || m.contains("u32")
        || m.contains("kind: Sint")
        || m.contains("kind: Uint");
    float && int
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glsl_habits_get_their_wgsl() {
        let h = |m: &str, l: &str| hint(m, l).unwrap_or("");
        assert!(
            h(
                "no definition in scope for identifier: `mod`",
                "x = mod(a, b);"
            )
            .contains("floor(x / y)")
        );
        assert!(h("no definition in scope for identifier: `iTime`", "").contains("kl.time"));
        assert!(
            h(
                "wrong number of arguments: expected 1, found 2",
                "let a = atan(y, x);"
            )
            .contains("atan2")
        );
        assert!(h("expected ..., found \"x\"", "    float x = 1.0;").contains("vec2<f32>"));
        assert!(h("invalid left-hand side of assignment", "c.rgb *= 0.5;").contains("swizzle"));
        assert!(h("automatic conversions cannot convert `i32` to `f32`", "").contains("f32(i)"));
        assert_eq!(hint("expected `;`, found \"}\"", "return x"), None);
    }
}
