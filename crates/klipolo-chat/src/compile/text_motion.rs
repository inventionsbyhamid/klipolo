//! `animateText` on words of the additions' working copy: a text layer, or words already animated, become animated
//! words (`media::text_motion`), a `scene@1` motion graphic made from their words, their look and how they come in.
//! The layer keeps its id, place, length, keys and effects; its enter preset goes (the units come in instead), and so
//! does its exit when the units leave too. Before it is kept the scene is checked as any scene is (`check_scene`, the
//! check every motion graphic passes) and held to what Klipolo draws in time (`text_motion::problem`): a scene that
//! fails is refused back to the model with the reason, so it can ask for less.

use klipolo_model::Project;
use klipolo_model::coded::ErrorParam;
use klipolo_model::contrib::Catalog;
use klipolo_model::contrib::builtin;
use klipolo_model::ids::Id;
use klipolo_model::media::text::TEXT_FADE_US;
use klipolo_model::media::text_motion::{self, TextMotion, Unit};
use klipolo_model::motion::{self, MotionUse};
use klipolo_model::ops::{ItemPatch, Op};
use klipolo_model::scene::check_scene;
use klipolo_model::timeline::ItemContent;

use super::motion::drawn;
use super::{Edit, Refused, label, to_model};

/// `animateText` on the words `id` of `work`, named `said`: the layer as animated words, in one `UpdateItem`.
pub(super) fn animate_text(
    work: &Project,
    (id, said): (&Id, &str),
    m: &TextMotion,
) -> Result<Edit, Refused> {
    let item = drawn(work, (id, said), "animateText")?;
    let (text, style) = match &item.content {
        ItemContent::Text { text, style } => (text.clone(), style.clone()),
        ItemContent::Generator { source, params, .. } if source.as_str() == text_motion::SOURCE => {
            let (text, style, _) = text_motion::read(params);
            (text, style)
        }
        ItemContent::Generator { .. } => {
            return Err(to_model(format!(
                "animateText: {said} is a title template that moves its own way. Use addText for words to animate."
            )));
        }
        _ => {
            return Err(to_model(format!(
                "animateText: {said} is not words. Name words by their ref, such as t1."
            )));
        }
    };
    if let Some(why) = text_motion::problem(&text, m) {
        return Err(to_model(format!("animateText: {why}")));
    }
    let scene = text_motion::scene(&text, &style, m);
    if let Err(problems) = check_scene(&scene, Catalog::builtin()) {
        // Too many units blurring at once is what costs too much to draw; anything else is said as the check says it.
        let said = match problems.first() {
            Some(p) if p.code == "scene.blurringUnits" => format!(
                "too many units would blur at once for Klipolo to draw in time (at most {}). Use a longer stagger, a \
                 shorter sec, less blur, or by word.",
                klipolo_model::scene::SceneLimits::MAX_BLURRING_UNITS
            ),
            Some(p) if p.code.starts_with("scene.tooMany") => {
                format!("Klipolo can't draw this in time: {}", p.message)
            }
            Some(p) => p.message.clone(),
            None => "the words don't make a scene Klipolo draws.".to_owned(),
        };
        return Err(to_model(format!("animateText: {said}")));
    }
    let mut layout = item.layout.clone();
    if let Some(mut moves) = layout.motion.take() {
        moves.enter = None;
        if m.exit {
            moves.exit = None;
        }
        layout.motion = motion::kept(moves);
    }
    let content =
        ItemContent::generator(text_motion::source(), text_motion::params(&text, &style, m));
    Ok(Edit {
        ops: vec![Op::UpdateItem {
            id: item.id.clone(),
            patch: ItemPatch {
                layout: Some(layout),
                content: Some(content),
                ..ItemPatch::default()
            },
        }],
        label: label(
            "animateText",
            vec![(
                "by",
                ErrorParam::from(match m.unit {
                    Unit::Letter => "letter",
                    Unit::Word => "word",
                    Unit::Line => "line",
                }),
            )],
        ),
    })
}

/// `animateText` by none on the words `id` of `work`, named `said`: animated words become plain words again, with
/// their words and look, coming in and going out with a text layer's own fade where they have none.
pub(super) fn plain_words(work: &Project, (id, said): (&Id, &str)) -> Result<Edit, Refused> {
    let item = drawn(work, (id, said), "animateText")?;
    let (text, style) = match &item.content {
        ItemContent::Generator { source, params, .. } if source.as_str() == text_motion::SOURCE => {
            let (text, style, _) = text_motion::read(params);
            (text, style)
        }
        ItemContent::Text { .. } => {
            return Err(to_model(format!(
                "animateText: {said} is plain words already. Leave by none out."
            )));
        }
        _ => {
            return Err(to_model(format!(
                "animateText: {said} is not animated words, so by none has nothing to undo."
            )));
        }
    };
    let mut layout = item.layout.clone();
    let mut moves = layout.motion.take().unwrap_or_default();
    let fade = || MotionUse {
        preset: builtin::motion("fade"),
        params: Default::default(),
        duration_us: TEXT_FADE_US,
        ease: None,
    };
    moves.enter = moves.enter.or_else(|| Some(fade()));
    moves.exit = moves.exit.or_else(|| Some(fade()));
    moves.fit(item.duration_us);
    layout.motion = motion::kept(moves);
    Ok(Edit {
        ops: vec![Op::UpdateItem {
            id: item.id.clone(),
            patch: ItemPatch {
                layout: Some(layout),
                content: Some(ItemContent::Text { text, style }),
                ..ItemPatch::default()
            },
        }],
        label: label("animateText", vec![("by", ErrorParam::from("none"))]),
    })
}
