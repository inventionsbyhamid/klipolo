//! `animate` on the additions' working copy, and the entrance `addText` gives its words. Chat's words are the model's
//! built-in motion presets (`contrib::builtin::MOTION_PRESETS` for an enter or exit, `MOTION_LOOPS` for an emphasis),
//! drawn by the engine in the preview and the export alike, so the layer's card shows and edits what chat set:
//! - an enter or exit is the preset of its name (`blur` is `blur-in`; `fromLeft` and `toLeft` are `slide` with `from`
//!   `left`), lasting `ENTER_US` or `EXIT_US` (bounce and blur a little longer, typewriter by the words' length), its
//!   ease the preset's own;
//! - an emphasis is the loop of its name at `EMPHASIS_STRENGTH`, one cycle `EMPHASIS_US` (a shake quicker);
//! - every one at most the layer's length (`Motion::fit`), and none takes it away; what the call leaves null stays.
//!
//! The layer is one the context names by its ref (`t1` words, `i1` a picture, `v1` a video layer), or `last`: the layer
//! the call before it in the answer added (an item already on the working copy, or a sticker still to be drawn, whose
//! job then carries the animation, `MakeSticker::motion`). A sound from the library plays where the animation starts:
//! as the layer comes in, or as it goes out when the call sets only its exit; as `addSound` places one, tied to the
//! footage under it, as the layer is.

use std::collections::BTreeMap;

use klipolo_model::Project;
use klipolo_model::coded::ErrorParam;
use klipolo_model::contrib::builtin;
use klipolo_model::ids::Id;
use klipolo_model::media::sound::add_sound_ops;
use klipolo_model::motion::{self, Motion, MotionUse};
use klipolo_model::ops::{ItemPatch, Op};
use klipolo_model::params::ParamValue;
use klipolo_model::time::Us;
use klipolo_model::timeline::{Item, ItemContent};

use super::{Compiler, Edit, Refused, label, media, to_model};
use crate::library::{LibraryItem, MediaKind};
use crate::tools::{Emphasis, Entrance, Exit};

/// How long an enter takes.
pub const ENTER_US: Us = 400_000;
/// How long an exit takes.
pub const EXIT_US: Us = 300_000;
/// One cycle of an emphasis.
pub const EMPHASIS_US: Us = 800_000;
/// An emphasis's strength (the presets' own is 1, at most 2): a pulse grows by 6 %, a wiggle turns 4.5 degrees.
pub const EMPHASIS_STRENGTH: f64 = 1.5;
/// A typewriter's time per letter, and its least and most.
const TYPE_US: Us = 50_000;
const TYPE_MIN_US: Us = 500_000;
const TYPE_MAX_US: Us = 2_000_000;

/// What one `animate` call sets: None keeps it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Change {
    pub enter: Option<Entrance>,
    pub exit: Option<Exit>,
    pub emphasis: Option<Emphasis>,
}

impl Change {
    /// Whether its sound plays as the layer goes out: only its exit is set.
    fn sound_at_exit(&self) -> bool {
        self.enter.is_none()
            && self.emphasis.is_none()
            && self.exit.is_some_and(|e| e != Exit::None)
    }
}

fn preset(word: &str, from: Option<&str>, duration_us: Us) -> MotionUse {
    let mut params = BTreeMap::new();
    if let Some(from) = from {
        params.insert("from".to_owned(), ParamValue::Text(from.to_owned()));
    }
    MotionUse {
        preset: builtin::motion(word),
        params,
        duration_us,
        ease: None,
    }
}

/// The preset `e` is for a layer reading `text` (None for anything but words), or None for none.
pub(crate) fn enter_use(e: Entrance, text: Option<&str>) -> Option<MotionUse> {
    Some(match e {
        Entrance::Pop => preset("pop", None, ENTER_US),
        Entrance::Fade => preset("fade", None, ENTER_US),
        Entrance::Zoom => preset("zoom", None, ENTER_US),
        Entrance::Bounce => preset("bounce", None, 600_000),
        Entrance::Blur => preset("blur-in", None, 500_000),
        Entrance::Typewriter => {
            let letters = text.map_or(0, |t| t.chars().count()) as Us;
            preset(
                "typewriter",
                None,
                (letters * TYPE_US).clamp(TYPE_MIN_US, TYPE_MAX_US),
            )
        }
        // A template of its own, not a preset: `addText` makes it.
        Entrance::Kinetic => return None,
        Entrance::FromLeft => preset("slide", Some("left"), ENTER_US),
        Entrance::FromRight => preset("slide", Some("right"), ENTER_US),
        Entrance::FromTop => preset("slide", Some("top"), ENTER_US),
        Entrance::FromBottom => preset("slide", Some("bottom"), ENTER_US),
        Entrance::None => return None,
    })
}

pub(crate) fn exit_use(e: Exit) -> Option<MotionUse> {
    Some(match e {
        Exit::Pop => preset("pop", None, EXIT_US),
        Exit::Fade => preset("fade", None, EXIT_US),
        Exit::Zoom => preset("zoom", None, EXIT_US),
        Exit::ToLeft => preset("slide", Some("left"), EXIT_US),
        Exit::ToRight => preset("slide", Some("right"), EXIT_US),
        Exit::ToTop => preset("slide", Some("top"), EXIT_US),
        Exit::ToBottom => preset("slide", Some("bottom"), EXIT_US),
        Exit::None => return None,
    })
}

pub(crate) fn emphasis_use(e: Emphasis) -> Option<MotionUse> {
    let (word, cycle) = match e {
        Emphasis::Pulse => ("pulse", EMPHASIS_US),
        Emphasis::Shake => ("shake", 600_000),
        Emphasis::Wiggle => ("wiggle", EMPHASIS_US),
        Emphasis::None => return None,
    };
    let mut u = preset(word, None, cycle);
    u.params
        .insert("strength".to_owned(), ParamValue::Number(EMPHASIS_STRENGTH));
    Some(u)
}

/// `now` with what `change` sets, for a layer `len_us` long reading `text` (words only), fitted to it; None when
/// nothing is left.
pub(crate) fn animated(
    now: Option<Motion>,
    change: &Change,
    len_us: Us,
    text: Option<&str>,
) -> Option<Motion> {
    let mut m = now.unwrap_or_default();
    if let Some(e) = change.enter {
        m.enter = enter_use(e, text);
    }
    if let Some(e) = change.exit {
        m.exit = exit_use(e);
    }
    if let Some(e) = change.emphasis {
        m.looping = emphasis_use(e);
    }
    m.fit(len_us);
    motion::kept(m)
}

/// Where a sound plays on a layer from `start_us`, `len_us` long, moving as `m`: as it comes in, or as it goes out
/// when `at_exit` (the call set only its exit) and it has one.
pub(crate) fn sound_start(at_exit: bool, start_us: Us, len_us: Us, m: Option<&Motion>) -> Us {
    match m.and_then(|m| m.exit.as_ref()) {
        Some(exit) if at_exit => start_us + len_us - exit.duration_us.min(len_us),
        _ => start_us,
    }
}

/// The words a layer reads, for a typewriter's length.
fn words_of(item: &Item) -> Option<&str> {
    match &item.content {
        ItemContent::Text { text, .. } => Some(text),
        _ => None,
    }
}

/// What a layer is, as the undo label says it: animated words are words.
pub(super) fn noun(item: &Item) -> &'static str {
    match &item.content {
        ItemContent::Text { .. } => "words",
        ItemContent::Generator { source, .. }
            if source.as_str() == klipolo_model::media::text_motion::SOURCE =>
        {
            "words"
        }
        ItemContent::Generator { .. } => "title",
        ItemContent::Video { .. } => "video",
        _ => "picture",
    }
}

/// The sound `name` names in the library, for `animate`.
pub(crate) fn sound<'l>(c: &'l Compiler, name: &str) -> Result<&'l LibraryItem, Refused> {
    let item = media::pick(c, "animate", name, &[MediaKind::Sound])?;
    if !item.asset.has_audio {
        return Err(to_model(format!(
            "animate: {:?} has no sound to play.",
            item.name
        )));
    }
    Ok(item)
}

/// `animate` on the layer `id` of `work`, named `said` (its ref, or `last`): its new motion as one `UpdateItem`, and the
/// sound, if any, where the animation starts.
pub(super) fn animate(
    c: &Compiler,
    work: &Project,
    (id, said): (&Id, &str),
    change: &Change,
    sound_name: Option<&str>,
) -> Result<Edit, Refused> {
    let Some(item) = (work.timeline.tracks.iter())
        .flat_map(|t| &t.items)
        .find(|i| &i.id == id)
    else {
        return Err(to_model(format!("animate: {said} is gone.")));
    };
    if !item.content.is_visual() || matches!(item.content, ItemContent::Adjustment) {
        return Err(to_model(format!(
            "animate: {said} draws nothing to animate."
        )));
    }
    if change.enter == Some(Entrance::Kinetic) {
        return Err(to_model(
            "animate: kinetic is a title of its own: add the words with addText and animation kinetic.".to_owned(),
        ));
    }
    if change.enter == Some(Entrance::Typewriter) && words_of(item).is_none() {
        return Err(to_model(format!(
            "animate: typewriter is for words. Use another enter for {said}."
        )));
    }
    let sound = sound_name.map(|name| sound(c, name)).transpose()?;
    let motion = animated(
        item.layout.motion.clone(),
        change,
        item.duration_us,
        words_of(item),
    );
    let mut layout = item.layout.clone();
    layout.motion = motion;
    let mut ops = Vec::new();
    if layout != item.layout {
        ops.push(Op::UpdateItem {
            id: item.id.clone(),
            patch: ItemPatch {
                layout: Some(layout.clone()),
                ..ItemPatch::default()
            },
        });
    }
    let mut params = vec![("layer", ErrorParam::from(noun(item)))];
    if let Some(sound) = sound {
        let at = sound_start(
            change.sound_at_exit(),
            item.start_us,
            item.duration_us,
            layout.motion.as_ref(),
        );
        // The same sound this answer put there already (an `addSound` on the same word) plays for this one too.
        if !media::sound_near(c, work, &sound.asset.id, at) {
            ops.extend(
                add_sound_ops(work, &sound.asset, at)
                    .map_err(|e| to_model(format!("animate: {e}")))?,
            );
            params.push(("sound", ErrorParam::from(sound.name.as_str())));
        }
    }
    Ok(Edit {
        ops,
        label: label("animate", params),
    })
}
