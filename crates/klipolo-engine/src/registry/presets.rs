//! The shader presets' bodies and the classes they claim, and the motion graphic
//! folder's `effect.wgsl` beside the definition the model's catalogue reads (`klipolo_model::presets`), so both read the
//! same folder; the registry assembles them as built-ins under `klipolo.builtin/<local id>` (`build_builtin`). Every
//! preset also passes the rules agents' shaders meet when given an agent's id (a test holds it), and claims a cost
//! class that its measurement on the Intel Mac must meet. The claims follow the class each measures on that floor
//! Normal, except Dust and bokeh (Heavy, at half size).

use super::ShaderKind;
use crate::make::budget::CostClass;

/// `(local id, effect.wgsl)`.
macro_rules! body {
    ($local:literal) => {
        (
            $local,
            include_str!(concat!(
                "../../../klipolo-model/presets/",
                $local,
                "/effect.wgsl"
            )),
        )
    };
}

/// The shader presets' bodies, in `klipolo_model::presets::SHADER_PRESETS`'s order.
pub const SHADER_BODIES: [(&str, &str); 8] = [
    body!("generator.aurora"),
    body!("generator.waves"),
    body!("generator.dust"),
    body!("generator.grain"),
    body!("generator.light-leak"),
    body!("effect.sparkle"),
    body!("effect.glow"),
    body!("effect.drop-shadow"),
];

/// The class each shader preset claims, which its measurement on the Intel Mac must meet: the class it
/// measures on the Iris Plus. A generator's figure there includes compositing
/// its own picture, so even the built-in gradient reads Normal; Sparkle's glints, and Glow's and Drop shadow's blurs
/// past the layer's box, read Normal too. Dust and bokeh read Heavy, at the half size it always draws at.
pub const SHADER_CLAIMED: [(&str, CostClass); 8] = [
    ("generator.aurora", CostClass::Normal),
    ("generator.waves", CostClass::Normal),
    ("generator.dust", CostClass::Heavy),
    ("generator.grain", CostClass::Normal),
    ("generator.light-leak", CostClass::Normal),
    ("effect.sparkle", CostClass::Normal),
    ("effect.glow", CostClass::Normal),
    ("effect.drop-shadow", CostClass::Normal),
];

/// The class each motion graphic preset claims.
pub const SCENE_CLAIMED: [(&str, CostClass); 0] = [];

/// Every preset's claimed class: the shader presets', then the motion graphics'.
pub fn claimed() -> impl Iterator<Item = (&'static str, CostClass)> {
    SHADER_CLAIMED.into_iter().chain(SCENE_CLAIMED)
}

/// The body of the shader preset `local` (`generator.waves`), if there is one.
pub fn body(local: &str) -> Option<&'static str> {
    SHADER_BODIES
        .iter()
        .find(|(id, _)| *id == local)
        .map(|(_, body)| *body)
}

/// What a shader preset draws as: its local id's kind (`generator.` or `effect.`).
pub(super) fn kind_of(local: &str) -> ShaderKind {
    if local.starts_with("generator.") {
        ShaderKind::Generator
    } else {
        ShaderKind::Filter
    }
}
