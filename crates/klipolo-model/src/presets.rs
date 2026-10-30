//! The shipped shader and motion graphic presets: data folders under `presets/<local id>/`, in
//! exactly the shape a version in a person's library has (`contrib.json` beside `effect.wgsl` or `scene.json`), embedded
//! at build time. Their ids are `klipolo.builtin/<local id>`. The model's built-in catalogue reads each `contrib.json`
//! (`contrib::builtin`), and the engine reads the same folders' `effect.wgsl` (`klipolo_engine::registry::presets`), so
//! both read the same bytes. `SHADER_PRESETS` are the shader effects, `SCENE_PRESETS` the motion graphics.

/// `(local id, contrib.json)`.
macro_rules! preset {
    ($local:literal) => {
        (
            $local,
            include_str!(concat!("../presets/", $local, "/contrib.json")),
        )
    };
}

/// The shader presets' definitions: generators Aurora, Waves, Dust and bokeh, Film grain and Light leak; filters
/// Sparkle, Glow and Drop shadow. The engine's `SHADER_BODIES` lists the same, in this order (a test holds them equal).
pub const SHADER_PRESETS: [(&str, &str); 8] = [
    preset!("generator.aurora"),
    preset!("generator.waves"),
    preset!("generator.dust"),
    preset!("generator.grain"),
    preset!("generator.light-leak"),
    preset!("effect.sparkle"),
    preset!("effect.glow"),
    preset!("effect.drop-shadow"),
];

/// The motion graphic presets' definitions.
pub const SCENE_PRESETS: [(&str, &str); 0] = [];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contrib::{Catalog, ContribDef, ContribId, ContribKind, Resolution};
    use crate::motion::Blend;

    /// Each `contrib.json` is a definition exactly as Klipolo writes one (so no field is misspelt or left over), under
    /// its folder's id, made by Klipolo as original work, and in the built-in catalogue as written. `UPDATE_GOLDEN=1`
    /// rewrites a file in that form.
    #[test]
    fn every_preset_definition_is_written_as_klipolo_writes_one() {
        for (local, json) in SHADER_PRESETS.iter().chain(&SCENE_PRESETS) {
            let def: ContribDef =
                serde_json::from_str(json).unwrap_or_else(|e| panic!("{local}: {e}"));
            assert_eq!(def.id, ContribId::builtin(local), "{local}");
            let written = serde_json::to_string_pretty(&def).unwrap() + "\n";
            if std::env::var_os("UPDATE_GOLDEN").is_some() {
                let path = format!(
                    "{}/presets/{local}/contrib.json",
                    env!("CARGO_MANIFEST_DIR")
                );
                std::fs::write(path, &written).unwrap();
                // The compiled-in text is the old one until the next build.
                continue;
            }
            assert_eq!(*json, written, "{local}: not as Klipolo writes it");
            let made = def.provenance.as_ref().unwrap();
            assert_eq!(
                (made.made_by.as_str(), made.license.as_str()),
                ("Klipolo", "original")
            );
            assert!(
                !def.description.is_empty() && !def.tags.is_empty(),
                "{local}"
            );
            assert_eq!(Catalog::builtin().get(&def.id), Some(&def), "{local}");
        }
    }

    /// What each must keep: Waves, Aurora and Dust at half size (never `auto`), Film grain stepped at
    /// 24 fps under overlay, Light leak under screen, Sparkle, Glow and Drop shadow padded by their size, radius and
    /// distance.
    #[test]
    fn the_presets_say_how_they_run() {
        let def = |local: &str| {
            Catalog::builtin()
                .get(&ContribId::builtin(local))
                .unwrap()
                .clone()
        };
        for local in ["generator.waves", "generator.aurora", "generator.dust"] {
            assert_eq!(def(local).resolution, Some(Resolution::Half), "{local}");
        }
        for (local, _) in SHADER_PRESETS {
            assert_ne!(def(local).resolution, Some(Resolution::Auto), "{local}");
            let kind = if local.starts_with("generator.") {
                ContribKind::Generator
            } else {
                ContribKind::Effect
            };
            assert_eq!(def(local).kind, kind, "{local}");
        }
        let grain = def("generator.grain");
        assert_eq!((grain.fps, grain.blend), (Some(24), Some(Blend::Overlay)));
        assert_eq!(def("generator.light-leak").blend, Some(Blend::Screen));
        for (local, param, per_unit, max_px) in [
            ("effect.sparkle", "size", 1.0, 48.0),
            ("effect.glow", "radius", 1.0, 64.0),
            ("effect.drop-shadow", "distance", 1.5, 96.0),
        ] {
            let pad = def(local).padding.unwrap();
            assert_eq!(
                (pad.px, pad.param.as_deref(), pad.per_unit, pad.max_px),
                (0.0, Some(param), per_unit, max_px),
                "{local}"
            );
        }
    }
}
