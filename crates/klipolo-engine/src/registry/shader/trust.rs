//! Who wrote a shader, and how that decides its text: a built-in is assembled as written, with whether it
//! animates read from its IR at registry build; a shader that isn't built in gets the output check and an entry that
//! returns through it (`untrusted`).

use klipolo_model::contrib::ContribDef;

use super::{EFFECT_ABI, PRELUDE, ParamLayout, SANITIZE, ShaderEffect, ShaderKind};
use crate::registry::guard;

/// Who wrote a shader, which decides how its text was assembled and checked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShaderTrust {
    /// Ships inside Klipolo: assembled as written.
    Builtin,
    /// Anything else (agent drafts, the library, project-embedded copies): it passed `guard::check`, and its source
    /// is the hardened module (the loop counter and the sanitising entry, written back out by naga).
    Checked,
}

impl ParamLayout {
    /// The prelude, `Params`, the output check for a checked shader, the body, and the entry, which for a checked
    /// shader returns through the output check. A checked body is closed with an empty declaration (`;`), so an
    /// attribute it leaves dangling can't attach to the host's `kl_fs`.
    pub(super) fn source(
        &self,
        def: &ContribDef,
        kind: ShaderKind,
        body: &str,
        trust: ShaderTrust,
    ) -> String {
        let entry = match kind {
            ShaderKind::Filter | ShaderKind::Generator => "effect",
            ShaderKind::Transition => "transition",
        };
        let (host, end, call) = match trust {
            ShaderTrust::Builtin => (String::new(), "", format!("{entry}(in.uv)")),
            ShaderTrust::Checked => (
                format!("{SANITIZE}\n"),
                "\n;",
                format!("kl_sanitize({entry}(in.uv))"),
            ),
        };
        format!(
            "{PRELUDE}\n{params}\n{host}// ---- {id} (effect ABI v{EFFECT_ABI}) ----\n{body}{end}\n\n@fragment\nfn kl_fs(in: KlVertex) -> @location(0) vec4<f32> {{\n    return {call};\n}}\n",
            params = self.wgsl(&def.params),
            id = def.id,
        )
    }
}

impl ShaderEffect {
    /// A built-in from the registry's tables: [`assemble`](Self::assemble), with whether it animates read from its
    /// IR (at registry build).
    pub(crate) fn builtin(def: &ContribDef, kind: ShaderKind, body: &str) -> Result<Self, String> {
        let mut effect = Self::assemble(def, kind, body)?;
        effect.animates = guard::animates(def, kind, &effect.source, body);
        Ok(effect)
    }
}
