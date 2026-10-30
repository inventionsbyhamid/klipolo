//! What `CheckScene` and a scene's `Verify` read from the session's `in/`: the SVGs a request brought, each
//! a file holding its name, a line feed and its text, at most [`MAX_SVGS`](super::svgs::MAX_SVGS) of at most 1 MiB; and
//! `contribs.json`, the library and project generators the scene's fills name, with this computer's verdicts and
//! envelopes (`ContribEntry`: definition and source, never hardened text). Each generator is checked and
//! hardened again here, drawn as its verdict says (at half size when it runs there) and clamped to its envelope, as the
//! preview draws it; one that doesn't assemble is left out of the catalogue, so a fill naming it is refused.

use std::path::Path;
use std::sync::Arc;

use klipolo_model::contrib::{Catalog, ContribKind, Resolution};

use super::checked::Fills;
use super::svgs::MAX_SVGS;
use crate::make::frames::ContribEntry;
use crate::make::frames::project::read;
use crate::make::protocol::InputName;
use crate::make::svg::MAX_SVG_BYTES;
use crate::registry::{ShaderEffect, ShaderKind};

/// `contribs.json` at most, and its entries.
const MAX_CONTRIBS_BYTES: u64 = 4 << 20;
const MAX_CONTRIBS: usize = 64;
/// An SVG's name at most (the Make tool's own cap).
const MAX_NAME_BYTES: usize = 256;

/// The SVGs `names` hold in `inputs`: (its name, its text) each.
pub fn read_svgs(inputs: &Path, names: &[InputName]) -> Result<Vec<(String, Vec<u8>)>, String> {
    if names.len() > MAX_SVGS {
        return Err(format!("a scene brings at most {MAX_SVGS} SVGs"));
    }
    names
        .iter()
        .map(|n| {
            let bytes = read(
                &inputs.join(n.as_str()),
                (MAX_SVG_BYTES + MAX_NAME_BYTES + 1) as u64,
            )?;
            let split = bytes
                .iter()
                .position(|&b| b == b'\n')
                .filter(|&at| at <= MAX_NAME_BYTES)
                .ok_or("an SVG input has no name line")?;
            let name = std::str::from_utf8(&bytes[..split])
                .map_err(|_| "an SVG's name isn't text")?
                .to_owned();
            Ok((name, bytes[split + 1..].to_vec()))
        })
        .collect()
}

/// The generators `name` in `inputs` hands over (None: none), as the fills of a scene draw them.
pub fn read_fills(inputs: &Path, name: Option<&InputName>) -> Result<Fills, String> {
    let entries: Vec<ContribEntry> = match name {
        Some(name) => {
            serde_json::from_slice(&read(&inputs.join(name.as_str()), MAX_CONTRIBS_BYTES)?)
                .map_err(|e| format!("the contributions aren't valid: {e}"))?
        }
        None => Vec::new(),
    };
    if entries.len() > MAX_CONTRIBS {
        return Err(format!("more than {MAX_CONTRIBS} contributions"));
    }
    let mut catalog = Catalog::builtin().clone();
    let mut shaders = Vec::new();
    for e in entries {
        if e.def.kind != ContribKind::Generator || e.def.id.is_builtin() {
            continue;
        }
        let Ok(mut shader) =
            ShaderEffect::assemble_untrusted(&e.def, ShaderKind::Generator, &e.body)
        else {
            continue;
        };
        if e.runs_at.half {
            shader.resolution = Resolution::Half;
        }
        shader.envelope = Some(Arc::new(e.envelope));
        catalog = catalog.with(e.def.clone());
        shaders.push((e.def, shader));
    }
    Ok(Fills { catalog, shaders })
}
