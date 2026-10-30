//! Padding: a filter may draw a declared margin outside its layer's box, so the
//! layer's effect target and its placed quad grow by it.

mod support;
use klipolo_engine::plan::{Layer, frame_plan, padded_dest};
use klipolo_engine::registry::{ShaderEffect, ShaderKind};
use klipolo_model::contrib::{ContribDef, ContribKind, PaddingSpec};
use klipolo_model::params::{ParamKind, ParamSpec};
use support::*;

#[test]
fn a_padded_filter_grows_its_layer_and_the_margins_add_up() {
    let (p, registry) = project_with_padded_filters(&[("glow", 16.0), ("shadow", 8.0)]);
    let plan = frame_plan(
        &ctx_with(p, registry),
        secs(0.5),
        klipolo_model::project::FormatKind::Landscape,
    );
    let Layer::Image(img) = &plan.layers[0] else {
        panic!("{:?}", plan.layers)
    };
    assert_eq!(img.pad_px, 24.0);
    let grown = padded_dest(img.dest, img.pad_px, (1920, 1080), 1.0);
    assert!((grown.x - (img.dest.x - 24.0 / 1920.0)).abs() < 1e-9);
    assert!((grown.h - (img.dest.h + 48.0 / 1080.0)).abs() < 1e-9);
}

/// Every kind of layer that carries filters takes their margin; a generator's own pass adds none (padding is a
/// filter's), and a layer whose filters aren't padded doesn't grow.
#[test]
fn text_and_generator_layers_take_their_filters_margin() {
    let (mut p, registry) = project_with_padded_filters(&[("glow", 16.0)]);
    let filters = p.timeline.tracks[0].items[0].effects.clone();
    let mut title = item("t", 0.0, 2.0, bx(0.1, 0.1, 0.5, 0.2), text("Hello"));
    title.effects = filters.clone();
    let mut gradient = item(
        "g",
        0.0,
        2.0,
        bx(0.5, 0.5, 0.2, 0.2),
        klipolo_model::timeline::ItemContent::generator(
            klipolo_model::contrib::builtin::GENERATOR_GRADIENT.into(),
            Default::default(),
        ),
    );
    gradient.effects = filters;
    let plain = item("plain", 0.0, 2.0, bx(0.0, 0.0, 0.1, 0.1), text("No glow"));
    p.timeline.tracks.push(overlay_track("t", vec![title]));
    p.timeline.tracks.push(overlay_track("g", vec![gradient]));
    p.timeline.tracks.push(overlay_track("p", vec![plain]));
    let plan = frame_plan(
        &ctx_with(p, registry),
        secs(0.5),
        klipolo_model::project::FormatKind::Landscape,
    );
    let pads: Vec<f64> = plan
        .layers
        .iter()
        .map(|l| match l {
            Layer::Image(i) => i.pad_px,
            Layer::Text(t) => t.pad_px,
            Layer::Generator(g) => {
                assert_eq!(g.source.pad_px, 0.0);
                g.pad_px
            }
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(pads, [16.0, 16.0, 16.0, 0.0]);
}

/// Only a filter may declare padding, within 0 to 256 pixels, growing with a number setting.
#[test]
fn padding_is_a_filters_within_range_and_grows_with_a_number() {
    let body = "fn effect(uv: vec2<f32>) -> vec4<f32> { return vec4<f32>(uv, 0.0, 1.0); }";
    let refused = |def: &ContribDef, kind| match ShaderEffect::assemble_untrusted(def, kind, body) {
        Ok(_) => panic!("{:?} was accepted", def.padding),
        Err(d) => d[0].code,
    };
    let pad = |px: f32, max_px: f32, param: Option<&str>| PaddingSpec {
        px,
        param: param.map(Into::into),
        per_unit: 2.0,
        max_px,
    };
    let generator = ContribDef {
        padding: Some(pad(8.0, 64.0, None)),
        ..ContribDef::new("com.test.fx/generator.pad", ContribKind::Generator, "Pad")
    };
    assert_eq!(refused(&generator, ShaderKind::Generator), "rule.padding");
    let mut filter = padded_filter_def("pad", Some(8.0));
    filter.padding = Some(pad(8.0, 300.0, None));
    assert_eq!(refused(&filter, ShaderKind::Filter), "rule.padding");
    filter.padding = Some(pad(f32::NAN, 64.0, None));
    assert_eq!(refused(&filter, ShaderKind::Filter), "rule.padding");
    filter.padding = Some(pad(8.0, 64.0, Some("radius")));
    assert_eq!(
        refused(&filter, ShaderKind::Filter),
        "rule.padding",
        "no such setting"
    );
    filter.params = vec![ParamSpec {
        key: "radius".into(),
        label: "Radius".into(),
        visible_when: None,
        kind: ParamKind::Float {
            min: 0.0,
            max: 20.0,
            default: 4.0,
            step: None,
            unit: None,
        },
    }];
    let fx = ShaderEffect::assemble_untrusted(&filter, ShaderKind::Filter, body).unwrap();
    assert_eq!(fx.padding, filter.padding);
}
