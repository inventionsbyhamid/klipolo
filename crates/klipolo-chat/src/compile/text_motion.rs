//! `animateText` on words of the additions' working copy: a text layer, or words already animated, become animated
//! words (`media::text_motion`), a `scene@1` motion graphic made from their words, their look and how they come in.
//! The layer keeps its id, place, length, keys and effects; its enter preset goes (the units come in instead), and so
//! does its exit when the units leave too. Before it is kept the scene is checked as any scene is (`check_scene`, the
//! check every motion graphic passes) and held to what Klipolo draws in time (`text_motion::problem`): a scene that
//! fails is refused back to the model with the reason, so it can ask for less.

use klipolo_model::Project;
use klipolo_model::coded::ErrorParam;
use klipolo_model::contrib::Catalog;
use klipolo_model::ids::Id;
use klipolo_model::media::text_motion::{self, TextMotion, Unit};
use klipolo_model::motion;
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
        let first = problems
            .first()
            .map_or("it doesn't check", |p| p.message.as_str());
        return Err(to_model(format!(
            "animateText: Klipolo can't draw this in time: {first} Ask for less: fewer units at once, a shorter time, \
             or by word."
        )));
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
