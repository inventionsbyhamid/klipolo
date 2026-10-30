//! A hardened module that no longer validates, refused where the author can find it.
//! The transformed module keeps the checked text's spans, so naga's most precise span inside the body gives the line.
//! The message is naga's cause chain, never the assembled source.

use std::error::Error;

use naga::WithSpan;
use naga::valid::{FunctionError, ValidationError};

use crate::registry::guard::{Checked, Diagnostic};

/// What closes the body in the checked text (`shader::trust`): the empty declaration, then the host's entry.
const BODY_END: &str = "\n;\n\n@fragment\n";

pub(super) fn validation_failed(checked: &Checked, e: &WithSpan<ValidationError>) -> Diagnostic {
    let mut message = e.as_inner().to_string();
    let mut cause = Error::source(e.as_inner());
    while let Some(c) = cause {
        message = format!("{message}: {c}");
        cause = c.source();
    }
    let d = Diagnostic::error(
        "harden.validate",
        format!("The shader doesn't validate once the host's loop counter is added: {message}"),
    );
    let start: usize = checked
        .source
        .split_inclusive('\n')
        .take(checked.body_line as usize - 1)
        .map(str::len)
        .sum();
    let end = checked
        .source
        .rfind(BODY_END)
        .unwrap_or(checked.source.len())
        .max(start);
    // naga adds the outer spans last: the most precise comes first here.
    let mut spans: Vec<_> = e.spans().filter_map(|(span, _)| span.to_range()).collect();
    spans.reverse();
    let d = match spans.iter().find(|r| r.start >= start && r.start < end) {
        Some(r) => d.at(&checked.source[start..end], r.start - start),
        None => d,
    };
    if non_uniform(e.as_inner()) {
        d.hint(
            "Inside a loop, sample with `kl_sample_a`/`kl_sample_b` (or `textureSampleLevel`) and use no `dpdx`, \
             `dpdy` or `fwidth`: the host's loop counter can end any loop early.",
        )
    } else {
        d
    }
}

/// Whether naga refused the module for a derivative or an implicit-level sample in non-uniform control flow.
fn non_uniform(e: &ValidationError) -> bool {
    let mut cause = Error::source(e);
    while let Some(c) = cause {
        if matches!(
            c.downcast_ref::<FunctionError>(),
            Some(FunctionError::NonUniformControlFlow(..))
        ) {
            return true;
        }
        cause = c.source();
    }
    false
}

#[cfg(test)]
mod tests {
    use naga::valid::{Capabilities, ValidationFlags, Validator};

    use super::non_uniform;

    fn validate(wgsl: &str) -> Result<(), Box<naga::valid::ValidationError>> {
        let m = naga::front::wgsl::parse_str(wgsl).unwrap();
        Validator::new(ValidationFlags::all(), Capabilities::empty())
            .validate(&m)
            .map(drop)
            .map_err(|e| Box::new(e.into_inner()))
    }

    /// naga 25 turns off derivative and implicit-level uniformity for fragment shaders altogether
    /// (`DISABLE_UNIFORMITY_REQ_FOR_FRAGMENT_STAGE`), so no fragment module it validates can show the hint's case, and
    /// `FunctionError::NonUniformControlFlow` can't be built outside naga. This pins both halves: the case validates
    /// today (a naga that checks it turns this red, and the hint's own case becomes testable), and any other
    /// refusal gets no hint.
    #[test]
    fn the_uniformity_hint_is_for_uniformity_only() {
        let sample = "@group(0) @binding(0) var t: texture_2d<f32>;
@group(0) @binding(1) var s: sampler;
@fragment fn main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    if (uv.x > 0.5) { return textureSample(t, s, uv); }
    return vec4<f32>(0.0);
}";
        assert!(validate(sample).is_ok());
        let types = "fn f() -> f32 { return vec2<f32>(1.0); }";
        assert!(!non_uniform(&validate(types).unwrap_err()));
    }
}
