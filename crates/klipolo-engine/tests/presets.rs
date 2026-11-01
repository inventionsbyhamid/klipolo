//! The shipped shader presets: data folders in exactly the shape a library version
//! has, listed where the model and the engine read them, and each one passing the rules agents' code meets when it is
//! given an agent's id: the guard, the hardening and the loader's definition checks, small by construction (loops of
//! constant bound ≤ 8, at most 9 samples a pixel). The class each claims is the one it measures on the Intel Mac.

use klipolo_engine::make::budget::CostClass;
use klipolo_engine::registry::pack::{check_definition, names_clean, setting_text_clean};
use klipolo_engine::registry::presets::{SCENE_BODIES, SHADER_BODIES, SHADER_CLAIMED, claimed};
use klipolo_engine::registry::{Registry, ShaderEffect, ShaderKind, guard};
use klipolo_model::contrib::ContribId;

#[test]
fn every_preset_folder_is_listed_and_the_list_matches_the_folders() {
    let dir = format!("{}/../klipolo-model/presets", env!("CARGO_MANIFEST_DIR"));
    let mut on_disk: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    on_disk.sort();
    let mut listed: Vec<&str> = klipolo_model::presets::SHADER_PRESETS
        .iter()
        .map(|(id, _)| *id)
        .chain(
            klipolo_model::presets::SCENE_PRESETS
                .iter()
                .map(|(id, _)| *id),
        )
        .collect();
    listed.sort();
    assert_eq!(on_disk, listed);
    // The engine's bodies and claims name the model's shader presets, in its order.
    let model: Vec<&str> = klipolo_model::presets::SHADER_PRESETS
        .iter()
        .map(|(id, _)| *id)
        .collect();
    assert_eq!(SHADER_BODIES.map(|(id, _)| id), *model);
    assert_eq!(SHADER_CLAIMED.map(|(id, _)| id), *model);
    // And its scenes the model's motion graphic presets, in its order.
    let scenes: Vec<&str> = klipolo_model::presets::SCENE_PRESETS
        .iter()
        .map(|(id, _)| *id)
        .collect();
    assert_eq!(SCENE_BODIES.map(|(id, _)| id), *scenes);
    // Each claims the class it measures on the floor GPU, the Intel Iris Plus:
    // Normal (a generator's figure there includes compositing its own picture; the filters' glints and blurs read
    // Normal too), except Dust and bokeh (Heavy, at half size). `claimed()` starts with these.
    use CostClass::{Heavy, Normal};
    let want = [
        ("generator.aurora", Normal),
        ("generator.waves", Normal),
        ("generator.dust", Heavy),
        ("generator.grain", Normal),
        ("generator.light-leak", Normal),
        ("effect.sparkle", Normal),
        ("effect.glow", Normal),
        ("effect.drop-shadow", Normal),
    ];
    assert_eq!(SHADER_CLAIMED, want);
    assert!(claimed().take(want.len()).eq(want));
}

#[test]
fn every_shader_preset_also_passes_the_rules_agents_code_meets() {
    let reg = Registry::builtin();
    for (local, body) in SHADER_BODIES {
        let id = ContribId::builtin(local);
        let def = reg.catalog().get(&id).unwrap();
        let kind = if local.starts_with("generator.") {
            ShaderKind::Generator
        } else {
            ShaderKind::Filter
        };
        let mut as_agent = def.clone();
        as_agent.id = format!("lib.k3f9q2ma/{local}.v1").as_str().into();
        ShaderEffect::assemble_untrusted(&as_agent, kind, body)
            .unwrap_or_else(|d| panic!("{local}: {d:?}"));
        let facts = guard::check(&as_agent, kind, body).unwrap().facts;
        assert!(
            facts.ops_per_pixel < 20_000,
            "{local}: small by construction ({} operations a pixel)",
            facts.ops_per_pixel
        );
        assert!(
            facts.samples_per_pixel <= 9,
            "{local}: {} samples a pixel",
            facts.samples_per_pixel
        );
        assert!(
            facts
                .loops
                .iter()
                .all(|l| l.max_trips.is_some_and(|n| n <= 8)),
            "{local}: loops of constant bound ≤ 8: {:?}",
            facts.loops
        );
        // The definition as the loader reads a library version's `contrib.json`.
        names_clean(&as_agent).unwrap_or_else(|f| panic!("{local}: its {f} isn't clean"));
        setting_text_clean(&as_agent).unwrap_or_else(|k| panic!("{local}: setting {k}"));
        check_definition(&as_agent).unwrap_or_else(|e| panic!("{local}: {e}"));
        // And the built-in draws it as written.
        let shader = reg
            .shader(&id)
            .unwrap_or_else(|| panic!("{local} is drawn"));
        assert_eq!(shader.kind, kind, "{local}");
        assert_eq!(shader.animates, facts.animates, "{local}");
    }
}
