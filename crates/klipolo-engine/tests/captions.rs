//! Captions in the frame plan (24 and 26; pure planning, no GPU): the page on screen as a `Layer::Caption`
//! over everything else, its words' state from its style (the frame), its style's face and place.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use klipolo_engine::plan::captions::page_at_lead;
use klipolo_engine::plan::{CaptionLayer, Layer, PlanContext, frame_plan};
use klipolo_model::captions::{Captions, Pages, captions};
use klipolo_model::contrib::ContribId;
use klipolo_model::project::{Asset, AssetKind, FormatKind, Project};
use klipolo_model::speech::{Speech, Transcript, Word, WordKind};
use klipolo_model::time::secs_to_us as s;
use klipolo_model::timeline::{Clip, ClipSource};

/// A landscape project of one 10 s clip, its captions on in the built-in `style` (`caption.loud`).
fn project(style: &str) -> Project {
    let mut p = Project::new("Captions");
    p.captions.enabled = true;
    p.captions.style = Some(ContribId::builtin(style));
    p.assets = vec![Asset {
        id: "v".into(),
        kind: AssetKind::Video,
        path: "/media/v.mp4".into(),
        name: "v".into(),
        duration_us: s(10.0),
        width: 1920,
        height: 1080,
        fps: 30.0,
        rotation: None,
        has_audio: true,
        audio_lead_us: None,
    }];
    p.timeline.main = vec![Clip::new(ClipSource::Asset { id: "v".into() }, 0, s(10.0))];
    klipolo_model::apply_ops(&p, &[]).unwrap()
}

/// One landscape page from 1 s to 2 s: "Hello" said at 1.0–1.3 s, "world" at 1.4–1.8 s.
fn hello_pages() -> Pages {
    Pages::one_page(
        "v",
        s(1.0),
        s(2.0),
        &[("Hello", s(1.0), s(1.3)), ("world", s(1.4), s(1.8))],
    )
}

fn landscape(pages: Pages) -> Arc<Captions> {
    Arc::new(Captions {
        style: ContribId::builtin("caption.bold"),
        pages: BTreeMap::from([(FormatKind::Landscape, Arc::new(pages))]),
    })
}

fn hello_world() -> Arc<Captions> {
    landscape(hello_pages())
}

fn caption(ctx: &PlanContext, at: f64) -> Option<Box<CaptionLayer>> {
    frame_plan(ctx, s(at), FormatKind::Landscape)
        .layers
        .into_iter()
        .rev()
        .find_map(|l| match l {
            Layer::Caption(c) => Some(c),
            _ => None,
        })
}

#[test]
fn the_page_on_screen_is_drawn_last() {
    let p = Arc::new(project("caption.bold"));
    let ctx = PlanContext::new(p.clone()).with_captions(hello_world());
    let c = caption(&ctx, 1.5).expect("a caption at 1.5 s");
    assert_eq!(c.shown_text(), "Hello world");
    assert_eq!((c.format, c.page), (FormatKind::Landscape, 0));
    // Bold in landscape: lines at most 0.60 of the width, centred at 0.80.
    assert_eq!(c.max_width, 0.6);
    let (x, y) = c.anchor.centre(&c.style, c.format, (0.5, 0.1));
    assert_eq!((x, y), (0.5, 0.8));
    assert!(
        c.frame.words.iter().all(|w| w.visible),
        "both words said by 1.5 s"
    );
    assert!(caption(&ctx, 3.0).is_none(), "no page then");
    let plan = frame_plan(&ctx, s(1.5), FormatKind::Landscape);
    assert!(
        matches!(plan.layers.last(), Some(Layer::Caption(_))),
        "over every overlay"
    );
}

#[test]
fn karaoke_colours_the_word_being_said() {
    let yellow = [255, 212, 0];
    let ctx = PlanContext::new(Arc::new(project("caption.karaoke"))).with_captions(hello_world());
    let words = |at: f64| caption(&ctx, at).unwrap().frame.words;
    assert_eq!(
        words(1.5)[1].fill,
        yellow,
        "\"world\" is said at 1.5 s: its accent, #FFD400"
    );
    assert_eq!(words(1.5)[0].fill, [255, 255, 255]);
    // "Hello" (1.0–1.3 s) stays lit 150 ms after it ends, until the next word's lead.
    assert_eq!(words(1.32)[0].fill, yellow);
    assert!(
        (words(1.32)[1].fill_alpha - 0.55).abs() < 1e-6,
        "\"world\" still to come"
    );
    assert_eq!(
        words(1.36)[1].fill,
        mix_at(1.36 - 1.35),
        "\"world\" lights up its style's 50 ms lead before it is said, over 80 ms"
    );
    assert_eq!(words(1.36)[0].fill, [255, 255, 255]);
}

/// Karaoke's spoken-word colour `secs` into its 80 ms fade from white to #FFD400.
fn mix_at(secs: f64) -> [u8; 3] {
    let k = (secs / 0.08).clamp(0.0, 1.0);
    let m = |a: f64, b: f64| (a + (b - a) * k).round() as u8;
    [m(255.0, 255.0), m(255.0, 212.0), m(255.0, 0.0)]
}

/// A word-reveal style's page (Bold) shows from its first word's start less the lead, never before the page before
/// it ends.
#[test]
fn a_page_shows_from_its_lead_but_never_over_the_page_before() {
    let pages = Pages::one_page("v", s(1.0), s(2.0), &[("Hi", s(1.0), s(1.3))]);
    assert_eq!(page_at_lead(&pages, s(0.96), s(0.05)), Some(0));
    assert_eq!(page_at_lead(&pages, s(0.94), s(0.05)), None);
    let mut two = pages.clone();
    let mut second = two.pages[0];
    (second.start_us, second.end_us, second.index) = (s(2.02), s(3.0), 1);
    two.pages.push(second);
    assert_eq!(
        page_at_lead(&two, s(1.98), s(0.05)),
        Some(0),
        "the page before is still up"
    );
    assert_eq!(
        page_at_lead(&two, s(2.0), s(0.05)),
        Some(1),
        "from the end of the one before"
    );
}

/// The interim layer draws the page's words that aren't hidden, with a `\n` where the person started a line, and
/// colours the word being said in that text.
#[test]
fn the_page_draws_its_shown_words_and_the_persons_line_breaks() {
    let mut pages = Pages::one_page(
        "v",
        s(1.0),
        s(3.0),
        &[
            ("um", s(1.0), s(1.2)),
            ("so", s(1.3), s(1.5)),
            ("this", s(1.6), s(1.9)),
            ("works", s(2.0), s(2.4)),
        ],
    );
    let words = Arc::make_mut(&mut pages.words);
    words[0].hidden = true;
    words[3].line_break = true;
    let ctx =
        PlanContext::new(Arc::new(project("caption.karaoke"))).with_captions(landscape(pages));
    let c = caption(&ctx, 2.1).unwrap();
    assert_eq!(c.shown_text(), "so this\nworks");
    assert!(!c.frame.words[0].visible, "\"um\" is hidden");
    assert_eq!(c.frame.words[3].fill, [255, 212, 0], "\"works\" is said");
}

#[test]
fn captions_off_draw_nothing_and_a_context_builds_pages_from_speech() {
    let mut p = project("caption.clean");
    p.captions.enabled = false;
    let ctx = PlanContext::new(Arc::new(p.clone())).with_captions(hello_world());
    assert!(caption(&ctx, 1.5).is_none());
    p.captions.enabled = true;
    let words = vec![Word {
        text: "Hi".into(),
        start_us: s(1.0),
        end_us: s(1.2),
        confidence: 0.9,
        kind: WordKind::Word,
    }];
    let t = Transcript {
        key: "k".into(),
        language: "en".into(),
        model: "m".into(),
        covered_us: s(10.0),
        complete: true,
        words,
    };
    let speech = Speech {
        transcripts: HashMap::from([("v".to_owned(), Arc::new(t))]),
        ..Speech::default()
    };
    let ctx = PlanContext::new(Arc::new(p.clone())).with_captions(Arc::new(captions(&p, &speech)));
    assert_eq!(
        caption(&ctx, 1.1).map(|c| c.shown_text()),
        Some("Hi".into())
    );
}

#[test]
fn a_caption_draws_in_its_styles_face_at_its_styles_place() {
    let p = Arc::new(project("caption.loud"));
    let ctx = PlanContext::new(p.clone()).with_captions(hello_world());
    let c = caption(&ctx, 1.5).expect("a caption at 1.5 s");
    assert_eq!(*c.font, klipolo_model::font::FontRef::bundled("Bangers"));
    assert_eq!(
        c.anchor.centre(&c.style, c.format, (0.5, 0.1)).1,
        0.72,
        "Loud's landscape centre"
    );
    // The person's own place wins, and a look on a box takes the box's colour and its text the contrast.
    let mut placed = project("caption.boxed");
    placed.captions.place.insert(FormatKind::Landscape, 0.3);
    placed.captions.variant = Some("dark".into());
    let ctx = PlanContext::new(Arc::new(placed)).with_captions(hello_world());
    let c = caption(&ctx, 1.5).unwrap();
    assert_eq!(c.anchor.centre(&c.style, c.format, (0.5, 0.1)).1, 0.3);
    assert_eq!(c.frame.line_boxes, Some(([0, 0, 0], 1.0)), "the dark box");
    assert_eq!(c.frame.words[0].fill, [255, 255, 255], "white words on it");
}

/// A style this build doesn't have (a pack that isn't installed) draws in Clean, and the frame names its id with its
/// kind: a pack's effect whose name starts `caption.` is still a missing effect.
#[test]
fn an_unknown_style_draws_in_clean_and_the_frame_names_it() {
    use klipolo_engine::plan::MissingContrib;
    use klipolo_model::contrib::ContribKind;
    let mut p = project("caption.bold");
    p.captions.style = Some(ContribId::from("com.acme.pack/caption.neon"));
    p.timeline.main[0].effects = vec![klipolo_model::timeline::EffectInstance::new(
        "com.acme.fx/caption.glow",
    )];
    let ctx = PlanContext::new(Arc::new(p)).with_captions(hello_world());
    let c = caption(&ctx, 1.5).unwrap();
    assert_eq!(*c.font, klipolo_model::font::FontRef::bundled("Inter"));
    let plan = frame_plan(&ctx, s(1.5), FormatKind::Landscape);
    let missing = |kind, id: &str| MissingContrib {
        kind,
        id: id.into(),
        checking: false,
    };
    assert_eq!(
        plan.missing,
        [
            missing(ContribKind::Effect, "com.acme.fx/caption.glow"),
            missing(ContribKind::CaptionStyle, "com.acme.pack/caption.neon"),
        ]
    );
    assert_eq!(
        frame_plan(&ctx, s(3.0), FormatKind::Landscape).missing,
        [missing(ContribKind::Effect, "com.acme.fx/caption.glow")],
        "the style is said only while a caption shows"
    );
    // The note names the style and the one drawn instead (its catalogue name).
    let note = klipolo_engine::api::caption_style_missing("com.acme.pack/caption.neon");
    let notice = klipolo_engine::api::frame_notice(&note).expect("a coded note");
    assert_eq!(
        notice,
        klipolo_engine::api::CodedError::new("frame.captionStyleMissing")
            .with("style", "com.acme.pack/caption.neon")
            .with("fallback", "klipolo.builtin/caption.clean")
    );
    assert_eq!(
        klipolo_engine::api::frame_notice("A LUT couldn't be read."),
        None
    );
}

/// The resolved style is kept from one revision to the next while the caption settings and formats stay.
#[test]
fn a_new_revision_keeps_the_resolved_style_until_the_captions_change() {
    let p = Arc::new(project("caption.loud"));
    let ctx = PlanContext::new(p.clone());
    let mut renamed = (*p).clone();
    renamed.name = "Renamed".into();
    let next = PlanContext::reusing(Arc::new(renamed.clone()), &ctx);
    assert!(Arc::ptr_eq(&ctx.caption_style, &next.caption_style));
    renamed.captions.accent = Some("#3DFF5C".into());
    let recoloured = PlanContext::reusing(Arc::new(renamed), &next);
    assert!(!Arc::ptr_eq(&next.caption_style, &recoloured.caption_style));
    assert_eq!(recoloured.caption_style.style.accent, "#3DFF5C");
}
