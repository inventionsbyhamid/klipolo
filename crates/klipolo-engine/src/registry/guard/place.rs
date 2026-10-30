//! Where the author's body sits in the assembled source: the registry's own assembly with the output check
//! (`shader::SANITIZE`), and naga's errors mapped back to the body's lines (an error only the host's entry shows is
//! about the entry function; one in the generated `Params` is about a setting's key; one only the host's functions
//! show comes from a body's name that replaced a WGSL built-in they call).

use klipolo_model::contrib::ContribDef;
use naga::WithSpan;
use naga::front::wgsl::ParseError;
use naga::valid::ValidationError;

use super::{Diagnostic, hints, rules};
use crate::registry::shader::{EFFECT_ABI, PRELUDE, SANITIZE, ShaderKind};

/// The author's body inside the assembled source.
pub(crate) struct Placed<'a> {
    pub body: &'a str,
    /// The body's first byte in the source.
    pub start: usize,
    /// Where the host's code after `Params` (the output check, then the body's header comment) starts.
    pub host: usize,
}

impl Placed<'_> {
    /// A span's start as an offset in the body, when the body wrote it.
    pub fn offset(&self, span: naga::Span) -> Option<usize> {
        let r = span.to_range()?;
        (r.start >= self.start && r.start < self.start + self.body.len())
            .then(|| r.start - self.start)
    }

    fn after_body(&self, span: naga::Span) -> bool {
        span.to_range()
            .is_some_and(|r| r.start >= self.start + self.body.len())
    }

    /// In the host's own functions: the prelude or the output check.
    fn in_host(&self, span: naga::Span) -> bool {
        span.to_range().is_some_and(|r| {
            r.start < PRELUDE.len() || (r.start >= self.host && r.start < self.start)
        })
    }

    fn in_params(&self, span: naga::Span) -> bool {
        span.to_range()
            .is_some_and(|r| r.start > PRELUDE.len() && r.start < self.host)
    }
}

/// naga's parse error, where the author wrote it.
pub(super) fn parse_error(p: &Placed, kind: ShaderKind, e: &ParseError) -> Diagnostic {
    let spans: Vec<_> = e.labels().map(|(span, _)| span).collect();
    located(p, kind, "wgsl.parse", e.message().to_string(), &spans)
}

/// naga's validation error with its causes, where the author wrote it.
pub(super) fn validation_error(
    p: &Placed,
    kind: ShaderKind,
    e: &WithSpan<ValidationError>,
) -> Diagnostic {
    let mut message = e.as_inner().to_string();
    let mut cause = std::error::Error::source(e.as_inner());
    while let Some(c) = cause {
        message = format!("{message}: {c}");
        cause = c.source();
    }
    // naga adds the outer spans last: the most precise comes first here.
    let mut spans: Vec<_> = e.spans().map(|(span, _)| *span).collect();
    spans.reverse();
    located(p, kind, "wgsl.validate", message, &spans)
}

/// The registry's own text for a shader that isn't built in (`shader::untrusted`: `ShaderEffect::assemble`'s with the
/// output check), where the output check starts and where the body starts. The downsample rule is a check of the
/// definition, not of the text, so it's left to `ShaderEffect::assemble_untrusted`.
pub(super) fn assemble(
    def: &ContribDef,
    kind: ShaderKind,
    body: &str,
) -> Result<(String, usize, usize), Vec<Diagnostic>> {
    if !def.id.is_well_formed() {
        // The id is written into the source's header comment: only a well-formed one can't reach the code.
        // Quoted with escapes, and only its start: it's the agent's text.
        let start: String = def.id.as_str().chars().take(64).collect();
        return Err(vec![Diagnostic::error(
            "rule.params",
            format!("{start:?} isn't a contribution id."),
        )]);
    }
    let source = crate::registry::shader::untrusted_source(def, kind, body).map_err(|e| {
        vec![Diagnostic::error("rule.params", e)
            .hint("Settings for shaders are numbers, angles, bools, choices, points and colours, at most 16.")]
    })?;
    let check = format!("{SANITIZE}\n");
    match body_start(def, &source, body) {
        Some((marker, start)) if marker >= check.len() && source[..marker].ends_with(&check) => {
            let host = marker - check.len();
            Ok((source, host, start))
        }
        _ => Err(vec![Diagnostic::error(
            "rule.params",
            "The shader couldn't be assembled around this body.",
        )]),
    }
}

/// Where the body's header comment starts in `source`, and where the body starts after it.
pub(super) fn body_start(def: &ContribDef, source: &str, body: &str) -> Option<(usize, usize)> {
    let marker = format!("// ---- {} (effect ABI v{EFFECT_ABI}) ----\n", def.id);
    let at = source.find(&marker)?;
    let start = at + marker.len();
    source[start..].starts_with(body).then_some((at, start))
}

/// A naga error where the author wrote it. `spans` most precise first. One that only the host's entry shows is about
/// the entry function's signature; one in the generated `Params` is about a setting's key.
fn located(
    p: &Placed,
    kind: ShaderKind,
    code: &'static str,
    message: String,
    spans: &[naga::Span],
) -> Diagnostic {
    if let Some(offset) = spans.iter().find_map(|s| p.offset(*s)) {
        let d = Diagnostic::error(code, message).at(p.body, offset);
        let found = hints::hint(&d.message, d.source_line.as_deref().unwrap_or(""));
        return match found {
            Some(h) => d.hint(h),
            None => d,
        };
    }
    if spans.iter().any(|s| p.after_body(*s)) {
        return rules::entry(p.body, kind, Some(&message));
    }
    if spans.iter().any(|s| p.in_host(*s)) {
        return Diagnostic::error(
            "rule.shadow",
            format!("A name in the shader replaces one of WGSL's own that the host's code calls: {message}"),
        )
        .hint("Rename the function or constant named like a WGSL built-in, such as `max`, `clamp` or `pow`.");
    }
    if spans.iter().any(|s| p.in_params(*s)) {
        return Diagnostic::error(
            "rule.params",
            format!("A setting's key can't be used in WGSL: {message}"),
        )
        .hint(
            "Rename the setting: letters and digits, and not a WGSL word such as `loop` or `type`.",
        );
    }
    let d = Diagnostic::error(code, message);
    match hints::hint(&d.message, "") {
        Some(h) => d.hint(h),
        None => d,
    }
}
