//! The scene cases `scenes.rs` measures: a kinetic title, a blur-in title and three lower thirds, a long title, and an
//! animated scene without text.

/// The lower third the scene tests draw (`tests/support/scene.rs`), its title and role shadowed when `shadow`, its card
/// faded (an offscreen group, laid back as a quad) when `faded`.
fn lower_third(shadow: bool, faded: bool) -> String {
    let sh = if shadow {
        r#","shadow":{"x":3,"y":3,"blur":10,"color":"rgba(0, 0, 0, 0.6)"}"#
    } else {
        ""
    };
    let fade = if faded { r#","opacity":0.9"# } else { "" };
    format!(
        r##"{{"scene":1,"duration":4,"segments":{{"in":1,"out":0.5}},
  "params":[{{"key":"accent","label":"Accent","type":"color","default":"#FFB000"}},
    {{"key":"title","label":"Name","type":"text","default":"Ana Lee","maxLen":500}},
    {{"key":"role","label":"Role","type":"text","default":"Product designer","maxLen":80}}],
  "nodes":[{{"id":"card","type":"group","box":[0.04,0.62,0.6,0.26]{fade},"children":[
    {{"id":"plate","type":"rect","box":[0,0,1,1],"fill":"rgba(20, 20, 28, 0.85)","radius":12}},
    {{"id":"tab","type":"rect","box":[0,0,0.012,1],"fill":"$accent","radius":3,"origin":"50% 100%",
     "animate":{{"scaleY":[{{"t":0,"v":0}},{{"t":0.4,"v":1,"ease":"power3.out"}}]}}}},
    {{"id":"title","class":"line","type":"text","text":"$title","box":[0.05,0.08,0.9,0.4],"size":48,"weight":700,
     "fill":"#FFFFFF","fit":"shrink"{sh}}},
    {{"id":"role","class":"line","type":"text","text":"$role","box":[0.05,0.58,0.6,0.3],"size":30,"weight":500,
     "fill":"#8FA6BF","fit":"shrink"{sh}}},
    {{"id":"rule","type":"line","box":[0.05,0.52,0.9,0],"stroke":"$accent","strokeWidth":4,"cap":"round"}}]}}],
  "timeline":[{{"set":"#card","clip":"inset(0 100% 0 0)"}},
    {{"to":"#card","at":0.1,"dur":0.55,"ease":"power3.out","clip":"inset(0 0% 0 0)"}},
    {{"from":".line","at":0.34,"dur":0.5,"ease":"power3.out","y":"22px","opacity":0,"stagger":0.1}},
    {{"to":"#card","at":"out","dur":0.35,"ease":"power2.in","y":"18px","opacity":0}}]}}"##
    )
}

/// "RISE UP", split by letter, each rising 60 px and fading in 0.05 s after the one before (the tests'
/// `KINETIC_LETTERS`).
const KINETIC_LETTERS: &str = r##"{"scene":1,"duration":2,"nodes":[
  {"id":"t","type":"text","text":"RISE UP","split":"chars","box":[0.05,0.3,0.9,0.4],"size":150,"weight":800,
   "fill":"#FFD400","align":"center"}],
  "timeline":[{"from":"#t","at":0,"dur":0.4,"y":"60px","opacity":0,"stagger":0.05,"ease":"power3.out"}]}"##;

/// A kinetic title by words: each word rises, fades and sharpens from a 24-pixel blur in turn.
const KINETIC_WORDS: &str = r##"{"scene":1,"duration":2,"nodes":[
  {"id":"t","type":"text","text":"MAKE EVERY FRAME COUNT","split":"words","box":[0.05,0.3,0.9,0.4],"size":120,
   "weight":800,"fill":"#FFFFFF","align":"center"}],
  "timeline":[{"from":"#t","at":0,"dur":0.5,"y":"40px","opacity":0,"blur":24,"stagger":0.08,
    "ease":"power3.out"}]}"##;

/// Twelve letters, each fading in and sharpening from a 24-pixel blur over 0.4 s, 0.05 s after the one before (the
/// a waiting letter isn't drawn, so at most twelve blur at once).
const BLUR_LETTERS: &str = r##"{"scene":1,"duration":2,"nodes":[
  {"id":"t","type":"text","text":"BLURRED TITLE","split":"chars","box":[0.05,0.3,0.9,0.4],"size":150,"weight":800,
   "fill":"#FFFFFF","align":"center"}],
  "timeline":[{"from":"#t","at":0,"dur":0.4,"blur":24,"opacity":0,"stagger":0.05}]}"##;

/// One unsplit line of 40 characters, the whole node sharpening from a 24-pixel blur over 0.4 s.
const BLUR_LINE: &str = r##"{"scene":1,"duration":2,"nodes":[
  {"id":"t","type":"text","text":"Forty characters on one line, blurring!!","box":[0.03,0.4,0.94,0.2],"size":64,
   "weight":700,"fill":"#FFFFFF","align":"center"}],
  "timeline":[{"from":"#t","at":0,"dur":0.4,"blur":24}]}"##;

/// 500 letters split by letter, each rising 30 px and fading in 4 ms after the one before (a long title's
/// draws and paints a frame, and its drawer's CPU).
fn many_letters() -> String {
    let text = "Motion graphics in Klipolo draw every letter as its own piece. ".repeat(9);
    let text: String = text.chars().take(500).collect();
    format!(
        r##"{{"scene":1,"duration":3,"nodes":[
  {{"id":"t","type":"text","text":"{text}","split":"chars","box":[0.03,0.05,0.94,0.9],"size":40,"weight":600,
   "fill":"#FFFFFF"}}],
  "timeline":[{{"from":"#t","at":0,"dur":0.3,"y":"30px","opacity":0,"stagger":0.004,"ease":"power2.out"}}]}}"##
    )
}

/// A thousand dots in a grid of 40 × 25 whose rows rise and fall (the tests' `DOT_GRID`: an animated scene with no text,
/// and what its first frame packs).
const DOT_GRID: &str = r##"{"scene":1,"duration":4,"nodes":[
  {"type":"repeat","id":"rows","times":25,"gap":[0,0.04],"children":[
    {"type":"repeat","times":40,"gap":[0.025,0],"children":[
      {"type":"ellipse","class":"dot","box":[0.005,0.005,0.012,0.02],"fill":"#7FD1FF"}]}]}],
  "timeline":[{"to":".dot","at":0,"dur":1,"y":"12px","yoyo":true,"repeat":-1,"ease":"sine.inOut",
    "stagger":{"each":0.002}}]}"##;

/// One case: its scene, the starts of its layers, and the frames measured (seconds), by phase.
pub(super) struct Case {
    pub name: &'static str,
    pub json: String,
    pub starts: &'static [f64],
    pub phases: Vec<(&'static str, Vec<f64>)>,
}

/// Frames `from`, `from + step`, … (`n` of them).
fn frames(from: f64, step: f64, n: usize) -> Vec<f64> {
    (0..n).map(|i| from + step * i as f64).collect()
}

pub(super) fn cases() -> Vec<Case> {
    let blur = |name, json: &str| Case {
        name,
        json: json.to_string(),
        starts: &[0.0],
        phases: vec![
            ("frames 0–15", frames(0.0, 1.0 / 30.0, 16)),
            ("after", frames(0.6, 1.0 / 30.0, 30)),
        ],
    };
    let thirds = |name, shadow, faded| Case {
        name,
        json: lower_third(shadow, faded),
        starts: &[0.0, 0.1, 0.2],
        phases: vec![
            ("animating", frames(0.25, 1.0 / 30.0, 24)),
            ("holding", frames(1.6, 1.0 / 30.0, 24)),
        ],
    };
    vec![
        Case {
            name: "dot grid (no text)",
            json: DOT_GRID.to_string(),
            starts: &[0.0],
            phases: vec![("moving", frames(0.1, 1.0 / 30.0, 24))],
        },
        Case {
            name: "kinetic title: letters rising",
            json: KINETIC_LETTERS.to_string(),
            starts: &[0.0],
            phases: vec![("moving", frames(0.0, 1.0 / 30.0, 24))],
        },
        Case {
            name: "500 letters rising one by one",
            json: many_letters(),
            starts: &[0.0],
            phases: vec![("moving", frames(0.2, 1.0 / 30.0, 24))],
        },
        blur("kinetic title: words blurring in", KINETIC_WORDS),
        blur("blur-in: 12 letters", BLUR_LETTERS),
        blur("blur-in: node blur, 40-char line", BLUR_LINE),
        thirds("3 lower thirds: plain", false, false),
        thirds("3 lower thirds: shadowed title, role", true, false),
        thirds("3 lower thirds: in a faded group", false, true),
    ]
}
