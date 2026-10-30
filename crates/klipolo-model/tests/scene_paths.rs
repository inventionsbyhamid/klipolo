//! Paths in `scene@1`: a path's `viewBox` and fit are checked, a table drawn by a polyline
//! is a chart that rises with its values, a motion path is a table of at most 256 points walked by length, and a path's
//! own bounds are its curves' (not their control points').

use std::sync::Arc;

use klipolo_model::contrib::Catalog;
use klipolo_model::scene::eval::{motion_path, MOTION_SAMPLES};
use klipolo_model::scene::instance::SceneInstance;
use klipolo_model::scene::path::{bounds, parse_d};
use klipolo_model::scene::timeline::{compile, CompiledScene};
use klipolo_model::scene::units::parse_color;
use klipolo_model::scene::{check_scene, Points, Scene};

fn scene(json: &str) -> Arc<CompiledScene> {
    Arc::new(compile(&Scene::from_stored(json.as_bytes()).unwrap()))
}

fn codes(json: &str) -> Vec<(String, &'static str)> {
    let s = Scene::from_stored(json.as_bytes()).unwrap_or_else(|p| panic!("{p:#?}"));
    match check_scene(&s, Catalog::builtin()) {
        Ok(_) => Vec::new(),
        Err(problems) => problems.into_iter().map(|p| (p.path, p.code)).collect(),
    }
}

/// A path takes a `viewBox` (four finite numbers, a width and height above 0) and `"xMidYMid meet"` or
/// `"none"`, and so does a motion path; `strokeScale` and `fillRule` are a path's. A stroked path can go unfilled.
#[test]
fn a_path_takes_a_view_box_and_how_it_fits() {
    let ok = r##"{"scene":1,"duration":2,"nodes":[
      {"type":"path","id":"p","d":"M0 0 L10 10","viewBox":[0,0,10,10],"preserveAspectRatio":"none",
       "strokeScale":false,"fillRule":"evenodd","fill":"none","stroke":"#FFFFFF"},
      {"type":"ellipse","box":[0,0,0.1,0.1],
       "motionPath":{"path":"M0 0 L1 1","viewBox":[0,0,1,1],"preserveAspectRatio":"xMidYMid meet"}}]}"##;
    assert_eq!(codes(ok), Vec::<(String, &str)>::new());
    let flat = ok.replace("[0,0,10,10]", "[0,0,10,0]");
    assert_eq!(
        codes(&flat),
        [("/nodes/0/viewBox".to_owned(), "scene.value")]
    );
    let motion = ok.replace("[0,0,1,1]", "[0,0,-1,1]");
    assert_eq!(
        codes(&motion),
        [("/nodes/1/motionPath/viewBox".to_owned(), "scene.value")]
    );
    // Only those two fits; and a viewBox is a path's, not a rect's.
    assert!(Scene::from_stored(ok.replace("\"none\"", "\"xMinYMin slice\"").as_bytes()).is_err());
    let on_rect = r##"{"scene":1,"duration":2,"nodes":[{"type":"rect","viewBox":[0,0,1,1]}]}"##;
    assert!(Scene::from_stored(on_rect.as_bytes()).is_err());
    assert_eq!(parse_color("none"), Some([0.0; 4]));
    assert_eq!(parse_color("Transparent"), Some([0.0; 4]));
}

/// The plan's polyline rule: a table setting's rows are spread across the box (`x = i / (n − 1)`) and rise with their
/// first number (`y = 1 − (v − min) / (max − min)`); a one-column table is enough. Written points stay as written.
#[test]
fn a_polyline_from_a_table_is_a_chart_of_its_values() {
    let json = r##"{"scene":1,"duration":2,"params":[{"key":"data","label":"Data","type":"table","columns":["v"],
      "maxRows":8,"default":[{"label":"a","values":[1]},{"label":"b","values":[3]},{"label":"c","values":[2]}]}],
      "nodes":[{"type":"polyline","id":"p","points":"$data","stroke":"#FFFFFF","smooth":true}]}"##;
    assert_eq!(codes(json), Vec::<(String, &str)>::new());
    let s = scene(json);
    let inst = SceneInstance::new(&s, &Default::default());
    let p = inst.instances_of(s.find("p").unwrap()).next().unwrap();
    assert_eq!(
        inst.points_of(p, &Points::Bind("$data".into())),
        vec![[0.0, 1.0], [0.5, 0.0], [1.0, 0.5]]
    );
    assert_eq!(
        inst.points_of(p, &Points::List(vec![[0.25, 0.75]])),
        vec![[0.25, 0.75]]
    );
}

/// A motion path is flattened once into at most 256 points; halfway along a quarter circle is its middle, heading
/// along it; a long path is resampled evenly to 256 points, its length kept; `"#id"` rides that path node's `d`.
#[test]
fn a_motion_path_is_walked_by_length() {
    let s = scene(
        r##"{"scene":1,"duration":2,"nodes":[
          {"type":"path","id":"arc","d":"M100 0 A100 100 0 0 1 0 100","stroke":"#FFFFFF"},
          {"type":"ellipse","id":"own","box":[0,0,0.1,0.1],"motionPath":{"path":"M100 0 A100 100 0 0 1 0 100"}},
          {"type":"ellipse","id":"rides","box":[0,0,0.1,0.1],"motionPath":{"path":"#arc","autoRotate":true}}]}"##,
    );
    let inst = SceneInstance::new(&s, &Default::default());
    for id in ["own", "rides"] {
        let node = inst.instances_of(s.find(id).unwrap()).next().unwrap();
        let (x, y, heading) = motion_path(&inst, node, 0.5).unwrap();
        let half = 100.0 * std::f64::consts::FRAC_1_SQRT_2;
        assert!(
            (x - half).abs() < 0.05 && (y - half).abs() < 0.05,
            "{id}: ({x}, {y})"
        );
        // Clockwise (y down) from (100, 0) to (0, 100): at 45° it heads down and left, 135°.
        assert!((heading - 135.0).abs() < 1.5, "{id}: {heading}");
        let (x, y, _) = motion_path(&inst, node, 0.0).unwrap();
        assert!((x - 100.0).abs() < 1e-9 && y.abs() < 1e-9);
        let table = inst.motion_table(node).unwrap();
        assert!(table.len() <= MOTION_SAMPLES);
        assert!((table.length() - 50.0 * std::f64::consts::PI).abs() < 0.1);
    }
    let arc = inst.instances_of(s.find("arc").unwrap()).next().unwrap();
    assert!(motion_path(&inst, arc, 0.5).is_none());
    // 2,000 lines zigzagging along x: resampled to 256 points evenly along its length.
    let mut d = String::from("M0 0");
    for i in 1..=2000 {
        d.push_str(&format!(" L{} {}", i, (i % 2) * 2));
    }
    let long = format!(
        r##"{{"scene":1,"duration":2,"nodes":[{{"type":"rect","id":"r","motionPath":{{"path":"{d}"}}}}]}}"##
    );
    let s = scene(&long);
    let inst = SceneInstance::new(&s, &Default::default());
    let r = inst.instances_of(s.find("r").unwrap()).next().unwrap();
    let table = inst.motion_table(r).unwrap();
    assert_eq!(table.len(), MOTION_SAMPLES);
    let exact = 2000.0 * 5f64.sqrt();
    assert!(
        (table.length() - exact).abs() / exact < 0.01,
        "{}",
        table.length()
    );
}

/// A curve's bounds hold the curve, not its control points: the path fits its box by what it draws.
#[test]
fn a_paths_bounds_are_its_curves() {
    let b = bounds(&parse_d("M0 0 C0 100 100 100 100 0").unwrap()).unwrap();
    assert!((b[3] - 75.0).abs() < 1e-9, "{b:?}");
    let b = bounds(&parse_d("M10 20 Q60 120 110 20 Z").unwrap()).unwrap();
    assert_eq!([b[0], b[1], b[2]], [10.0, 20.0, 100.0]);
    assert!((b[3] - 50.0).abs() < 1e-9);
}

/// `autoRotate`'s heading turns smoothly along a path cut into few chords a curve (here a circle of 200 arcs, one
/// chord each, 1.8° apart): from one sample to the next it never jumps by a whole chord's turn.
#[test]
fn a_motion_paths_heading_turns_smoothly() {
    let mut d = String::from("M100 0");
    for i in 1..=200 {
        let a = f64::from(i) * std::f64::consts::TAU / 200.0;
        d.push_str(&format!(
            " A100 100 0 0 1 {:.6} {:.6}",
            100.0 * a.cos(),
            100.0 * a.sin()
        ));
    }
    let json = format!(
        r##"{{"scene":1,"duration":2,"nodes":[{{"type":"rect","id":"r","motionPath":{{"path":"{d}"}}}}]}}"##
    );
    let s = scene(&json);
    let inst = SceneInstance::new(&s, &Default::default());
    let r = inst.instances_of(s.find("r").unwrap()).next().unwrap();
    let heading = |p: f64| motion_path(&inst, r, p).unwrap().2;
    let mut worst = 0.0f64;
    for i in 0..399 {
        let (a, b) = (
            heading(f64::from(i) / 400.0),
            heading(f64::from(i + 1) / 400.0),
        );
        worst = worst.max(((b - a + 540.0).rem_euclid(360.0) - 180.0).abs());
    }
    assert!(
        worst < 1.3,
        "the heading jumps by {worst}° between samples 0.9° apart"
    );
}
