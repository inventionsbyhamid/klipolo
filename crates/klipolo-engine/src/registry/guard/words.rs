//! Rules read from the body's words. naga's WGSL writer, which makes the text a device compiles
//! (`harden`), drops `@diagnostic(…)` filters, so what it writes wouldn't validate as the checked text did
//! (`rule.diagnostic`). Names the writer would rewrite (`rule.name`) are refused earlier, by the scan
//! (`names`), so the main process refuses them too.

use super::scan::tokens;
use super::{Diagnostic, MAX_REPORTED, Placed};

pub(super) fn check(p: &Placed) -> Vec<Diagnostic> {
    let words = tokens(p.body);
    let mut out = Vec::new();
    for (i, &(_, word)) in words.iter().enumerate().skip(1) {
        if out.len() >= MAX_REPORTED {
            break;
        }
        if word == "diagnostic" && words[i - 1].1 == "@" {
            out.push(
                Diagnostic::error(
                    "rule.diagnostic",
                    "`@diagnostic(…)` filters aren't kept when the host writes the shader out for the GPU.",
                )
                .at(p.body, words[i - 1].0)
                .hint("Remove it. Inside a loop, sample with `kl_sample_a`, `kl_sample_b` or `textureSampleLevel`."),
            );
        }
    }
    out
}
