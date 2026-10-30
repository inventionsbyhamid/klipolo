//! Scene layers in the plan: a generator layer naming a scene plans a scene layer at scene time, a still
//! one draws its settled pose (the end of `in`, or before its exit when it has no segments), and its settings reach
//! the layer once, shared while they don't change.

mod support;
use klipolo_engine::plan::{Layer, frame_plan};
use support::*;

#[test]
fn a_generator_layer_naming_a_scene_plans_a_scene_layer_at_scene_time() {
    let (p, registry) = project_with_scene("lt", LOWER_THIRD, 0.0, 12.0); // scene: duration 4, in 1, out 0.5
    let ctx = ctx_with(p, registry);
    let at = |s| match &frame_plan(&ctx, secs(s), klipolo_model::project::FormatKind::Landscape)
        .layers[0]
    {
        Layer::Scene(l) => l.t,
        other => panic!("{other:?}"),
    };
    assert_eq!(at(0.5), 0.5);
    assert!((at(11.75) - 3.75).abs() < 1e-6);
}

#[test]
fn a_still_scene_draws_its_settled_pose() {
    let (mut p, registry) = project_with_scene("lt", LOWER_THIRD, 0.0, 12.0);
    set_still(&mut p, "lt", true);
    let ctx = ctx_with(p, registry);
    for s in [0.1, 6.0, 11.9] {
        let Layer::Scene(l) =
            &frame_plan(&ctx, secs(s), klipolo_model::project::FormatKind::Portrait).layers[0]
        else {
            panic!()
        };
        assert_eq!(l.t, 1.0, "the end of `in`");
    }
}

/// A still layer of a scene without segments draws it before its exit
/// (its hold point), never the blank end of a fade-out.
#[test]
fn a_still_scene_without_segments_draws_before_its_exit() {
    const FADES: &str = r##"{"scene":1,"duration":4,"nodes":[{"type":"rect","id":"r","box":[0,0,1,1],"fill":"#FFFFFF"}],
      "timeline":[{"from":"#r","at":0,"dur":0.5,"opacity":0},{"to":"#r","at":3,"dur":1,"opacity":0}]}"##;
    let (mut p, registry) = project_with_scene("fade", FADES, 0.0, 6.0);
    set_still(&mut p, "fade", true);
    let ctx = ctx_with(p, registry);
    let Layer::Scene(l) = &frame_plan(
        &ctx,
        secs(5.0),
        klipolo_model::project::FormatKind::Landscape,
    )
    .layers[0] else {
        panic!()
    };
    assert!(
        l.t < 3.0 && l.t > 2.99,
        "just before the exit at 3 s: {}",
        l.t
    );
}

/// The layer's settings are shared while they don't change (no copy per frame), and their hash changes with them.
#[test]
fn a_scene_layers_settings_are_shared_until_they_change() {
    let (mut p, registry) = project_with_scene("lt", LOWER_THIRD, 0.0, 12.0);
    let plan = |ctx: &klipolo_engine::plan::PlanContext| match frame_plan(
        ctx,
        secs(2.0),
        klipolo_model::project::FormatKind::Landscape,
    )
    .layers
    .remove(0)
    {
        Layer::Scene(l) => l,
        other => panic!("{other:?}"),
    };
    let ctx = ctx_with(p.clone(), registry.clone());
    let (a, b) = (plan(&ctx), plan(&ctx));
    assert!(std::sync::Arc::ptr_eq(&a.params, &b.params));
    assert_eq!(a.params_hash, b.params_hash);
    if let klipolo_model::timeline::ItemContent::Generator { params, .. } =
        &mut scene_item(&mut p, "lt").content
    {
        params.insert(
            "accent".into(),
            klipolo_model::params::ParamValue::Text("#00FF00".into()),
        );
    }
    let ctx = ctx_with(p, registry);
    let c = plan(&ctx);
    assert_ne!(c.params_hash, a.params_hash);
}

/// A scene of `$copies` dots in a row (a repeat's `times`, 1–400): what its measurement bounds.
const DOTS: &str = r##"{"scene":1,"duration":2,
  "params":[{"key":"copies","label":"Copies","type":"int","min":1,"max":400,"default":3}],
  "nodes":[{"type":"repeat","id":"row","times":"$copies","gap":[0.002,0],"children":[
    {"type":"ellipse","box":[0,0.4,0.002,0.02],"fill":"#FFFFFF"}]}]}"##;

/// A library scene this computer hasn't checked is in the catalogue only. It is never parsed in
/// this process, draws as nothing, and is said as being checked (never as a missing motion graphic). With its verdict it
/// is compiled once and drawn; a rebuilt registry shares that compiled scene, so the drawer's instance stays.
#[test]
fn an_unverified_scene_draws_nothing_until_checked() {
    use klipolo_engine::registry::pack_scene::CompiledScenes;
    use klipolo_model::contrib::ContribKind;
    let c = loaded_scene("scene.lower-third.v1", "Lower third", LOWER_THIRD);
    let id = c.def.id.clone();
    let scenes = CompiledScenes::new();
    let p = project_showing(&id, 4.0, &[]);
    let landscape = klipolo_model::project::FormatKind::Landscape;
    let unchecked = snapshot_registry(&c, None, &scenes);
    assert!(unchecked.catalog().get(&id).is_some() && unchecked.scene(&id).is_none());
    let plan = frame_plan(&ctx_with(p.clone(), unchecked), secs(1.5), landscape);
    assert!(plan.layers.is_empty(), "{:?}", plan.layers);
    assert_eq!(plan.missing.len(), 1);
    assert!(
        plan.missing[0].checking && plan.missing[0].kind == ContribKind::Scene,
        "said as being checked: {:?}",
        plan.missing
    );
    assert_eq!(scenes.compiled(), 0, "never parsed before its verdict");
    let verdict = light_verdict(Default::default());
    let checked = snapshot_registry(&c, Some(&verdict), &scenes);
    let plan = frame_plan(&ctx_with(p.clone(), checked.clone()), secs(1.5), landscape);
    assert!(
        matches!(plan.layers[..], [Layer::Scene(_)]),
        "{:?}",
        plan.layers
    );
    assert!(plan.missing.is_empty(), "{:?}", plan.missing);
    let again = snapshot_registry(&c, Some(&verdict), &scenes);
    assert!(std::sync::Arc::ptr_eq(
        checked.scene(&id).unwrap(),
        again.scene(&id).unwrap()
    ));
    assert_eq!(scenes.compiled(), 1, "compiled once, for its key");
}

/// A verified scene's settings are drawn inside what its verdict measured. A layer asking 400 copies
/// of a scene measured up to 120 draws 120 and is said as being checked (its re-measure is the app's); one inside
/// draws as it asks, unsaid.
#[test]
fn a_scene_param_past_its_envelope_is_clamped() {
    use klipolo_engine::registry::pack_scene::CompiledScenes;
    use klipolo_model::params::ParamValue;
    let c = loaded_scene("scene.dots.v1", "Dots", DOTS);
    let scenes = CompiledScenes::new();
    let verdict = light_verdict(range_envelope("copies", 1.0, 120.0));
    let registry = snapshot_registry(&c, Some(&verdict), &scenes);
    let landscape = klipolo_model::project::FormatKind::Landscape;
    for (asked, drawn, said) in [(400.0, 120.0, true), (100.0, 100.0, false)] {
        let p = project_showing(&c.def.id, 2.0, &[("copies", asked)]);
        let plan = frame_plan(&ctx_with(p, registry.clone()), secs(0.5), landscape);
        let Layer::Scene(l) = &plan.layers[0] else {
            panic!("{:?}", plan.layers)
        };
        assert_eq!(l.params.get("copies"), Some(&ParamValue::Number(drawn)));
        assert_eq!(
            plan.missing.iter().any(|m| m.checking),
            said,
            "{:?}",
            plan.missing
        );
        let inst = klipolo_model::scene::instance::SceneInstance::new(&l.scene, &l.params);
        // The repeat's dot (compiled node 1: each node comes before its children).
        assert_eq!(
            inst.instances_of(1).count(),
            drawn as usize,
            "the instance draws what was measured"
        );
    }
}
