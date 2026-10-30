//! Whole-project validation. Every refusal is a model code (`messages`), worded by the UI.

use std::collections::{BTreeMap, HashSet};

use crate::coded::CodedError;
use crate::contrib::{AssetPath, Catalog, ContribDef, ContribId, ContribKind};
use crate::error::{invalid, invalid_with, not_found, ModelError, Result};
use crate::font::font_ok;
use crate::layout::{BoxN, Layout};
use crate::messages::{code, msg};
use crate::motion::Blend;
use crate::params::ParamValue;
use crate::project::{AssetKind, BackgroundKind, Project, PROJECT_VERSION};
use crate::query::{makes_sound, source_duration_us};
use crate::speech::{fix_text_blank, fix_text_ok, WordFix};
use crate::time::{drifted, unscaled, Us, MAX_TIME_US, MIN_DURATION_US, OVERLAP_TOLERANCE_US};
use crate::timeline::{
    Clip, ClipSource, EffectInstance, ItemContent, Look, TextStyle, Track, TrackKind, ZoomTarget,
};
use crate::transitions;

mod recording;
pub(crate) use recording::validate_recording;
use recording::{check_centre, validate_mix};
mod generator;
pub use generator::check_padding;

pub const MAX_ASSETS: usize = 2000;
pub const MAX_CLIPS: usize = 5000;
pub const MAX_TRACKS: usize = 50;
pub const MAX_ITEMS: usize = 5000;
pub const MAX_LUTS: usize = 4;
/// About one per word of a two-hour take: a project's fixes are one list, sent whole when one changes.
pub const MAX_WORD_FIXES: usize = 20_000;
/// About one per word of a two-hour take, as fixes: the looks are one list too, sent whole when one changes.
pub const MAX_WORD_LOOKS: usize = 20_000;
pub const MAX_EFFECTS: usize = 8;
const MAX_PARAMS: usize = 32;
const MAX_REQUIRES: usize = 32;
pub(crate) const MAX_NAME: usize = 200;
/// How far the person may shift a recording source to fix its sync.
pub const MAX_NUDGE_US: Us = 5_000_000;
/// The largest clock correction a run may carry (0.15 %: real devices drift tens of ppm). Sync searches ±1,200 ppm,
/// past recorders at 48.048 and 47.952 kHz (±1,000 ppm). The same cap clamps capture import's drift
/// (`klipolo-capture`'s `import.rs`): widening it there is harmless, since a take's manifest caps drift at 1,000.
pub const MAX_DRIFT_PPM: f64 = 1_500.0;
/// The furthest a sound layer ducks under speech (`ItemContent::Audio::duck`): 30 dB down is all but silent under a
/// voice.
pub const DEEPEST_DUCK_DB: f64 = -30.0;
/// The most channels a sound file plays as (the engine folds a file with more into two), so the channels a source
/// can play centred from (`SourceMix::centre_from`).
pub const PLAYED_CHANNELS: u8 = 2;
const MAX_TEXT: usize = 2000;
/// Rounding slack when comparing against source durations.
const SOURCE_SLACK_US: Us = 1_000;

/// `Ok` when `ok`, else the code. A message with values goes through [`check_with`].
fn check(ok: bool, code: &'static str) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(invalid(code))
    }
}

/// [`check`] with a message built only when the check fails: validation runs on every edit and every dry-run frame
/// of a drag, and a valid project must not build thousands of messages it never shows.
fn check_with(ok: bool, message: impl FnOnce() -> CodedError) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(invalid_with(message()))
    }
}

fn in_range(v: f64, lo: f64, hi: f64) -> bool {
    v.is_finite() && v >= lo && v <= hi
}

/// A stored time the model accepts (0 to 24 h). Checked before any arithmetic uses the value.
fn valid_time(t: Us) -> bool {
    (0..=MAX_TIME_US).contains(&t)
}

pub(crate) fn valid_color(c: &str) -> bool {
    let hex = c.strip_prefix('#').unwrap_or("");
    (hex.len() == 6 || hex.len() == 8) && hex.chars().all(|ch| ch.is_ascii_hexdigit())
}

fn valid_box(b: &BoxN) -> bool {
    in_range(b.x, -2.0, 3.0)
        && in_range(b.y, -2.0, 3.0)
        && in_range(b.w, 1e-4, 4.0)
        && in_range(b.h, 1e-4, 4.0)
}

/// The codes a placed thing's problems are told with, one set per owner (the owner word is translated too).
struct LayoutCodes {
    placement: &'static str,
    rotation: &'static str,
    opacity: &'static str,
    radius: &'static str,
}

const LAYER_LAYOUT: LayoutCodes = LayoutCodes {
    placement: code!("model.layer.placement"),
    rotation: code!("model.layer.rotation"),
    opacity: code!("model.layer.opacity"),
    radius: code!("model.layer.radius"),
};
const CAMERA_LAYOUT: LayoutCodes = LayoutCodes {
    placement: code!("model.camera.placement"),
    rotation: code!("model.camera.rotation"),
    opacity: code!("model.camera.opacity"),
    radius: code!("model.camera.radius"),
};

fn check_layout(l: &Layout, codes: &LayoutCodes) -> Result<()> {
    check(
        valid_box(&l.rect) && l.overrides.values().all(valid_box),
        codes.placement,
    )?;
    check(in_range(l.rotation, -360.0, 360.0), codes.rotation)?;
    check(in_range(l.opacity, 0.0, 1.0), codes.opacity)?;
    check(in_range(l.radius, 0.0, 0.5), codes.radius)
}

/// Motion and blend: on visual layers (any blend but normal also needs a picture of its own, so not on an adjustment
/// adjustment layer), and keys only on the camera bubble. `len_us` is a layer's
/// length. A layer's presets are checked against the catalogue: a built-in's settings against its schema, while an
/// id this build doesn't know is kept.
fn check_motion(p: &Project, layout: &Layout, owner: Owner, len_us: Us) -> Result<()> {
    let moving = layout.motion.as_ref().is_some_and(|m| !m.is_empty());
    let blended = layout.blend_mode() != Blend::Normal;
    let problem = match owner {
        Owner::Layer { visual: false, .. } => {
            return check(!moving && !blended, code!("model.motion.notVisual"));
        }
        Owner::Layer {
            adjustment, text, ..
        } => {
            check(
                !(adjustment && blended),
                code!("model.motion.blendNeedsPicture"),
            )?;
            layout
                .motion
                .as_ref()
                .and_then(|m| crate::motion::problem(m, Some(len_us), text))
        }
        Owner::Camera => {
            check(!blended, code!("model.motion.cameraBlend"))?;
            layout
                .motion
                .as_ref()
                .and_then(|m| crate::motion::problem(m, None, false))
        }
    };
    if let Some(c) = problem {
        return Err(invalid(c));
    }
    let uses = layout
        .motion
        .iter()
        .flat_map(|m| [&m.enter, &m.exit, &m.looping]);
    for u in uses.flatten() {
        let def = check_contrib(&u.preset, ContribKind::Motion)?;
        check_params(p, def, &u.params, &MOTION_PARAMS)?;
    }
    Ok(())
}

/// Whose layout `check_motion` checks.
#[derive(Clone, Copy)]
enum Owner {
    Layer {
        visual: bool,
        adjustment: bool,
        text: bool,
    },
    Camera,
}

/// A contribution kind's codes: an id that isn't well formed, and a built-in of another kind.
fn contrib_codes(kind: ContribKind) -> (&'static str, &'static str) {
    match kind {
        ContribKind::Effect => (code!("model.effect.badId"), code!("model.effect.wrongKind")),
        ContribKind::Look => (code!("model.look.badId"), code!("model.look.wrongKind")),
        ContribKind::Transition => (
            code!("model.transition.badId"),
            code!("model.transition.wrongKind"),
        ),
        // `motion::problem` refuses an id that isn't well formed first, with the same code.
        ContribKind::Motion => (
            code!("model.motion.badPreset"),
            code!("model.motion.wrongKind"),
        ),
        ContribKind::AudioEffect => (
            code!("model.audio.effectBadId"),
            code!("model.audio.effectWrongKind"),
        ),
        // Scenes are shown by generator layers, so they are told as generators.
        ContribKind::Generator | ContribKind::Scene => (
            code!("model.layer.generatorBadId"),
            code!("model.layer.generatorWrongKind"),
        ),
        ContribKind::CaptionStyle => (
            code!("model.captions.styleBadId"),
            code!("model.captions.styleWrongKind"),
        ),
    }
}

/// A well-formed id, of the right kind when this build knows it. Unknown ids are kept (not installed, or newer).
pub(crate) fn check_contrib(
    id: &ContribId,
    kind: ContribKind,
) -> Result<Option<&'static ContribDef>> {
    let (bad_id, wrong_kind) = contrib_codes(kind);
    check_with(id.is_well_formed(), || {
        CodedError::new(bad_id).with("id", id.as_str())
    })?;
    match Catalog::builtin().get(id) {
        Some(def) => {
            check_with(def.kind == kind, || {
                CodedError::new(wrong_kind).with("name", def.id.as_str())
            })?;
            Ok(Some(def))
        }
        None => Ok(None),
    }
}

/// The codes the settings of an owner's contribution are told with.
struct ParamCodes {
    too_many: &'static str,
    bad_name: &'static str,
    bad_value: &'static str,
}

const EFFECT_PARAMS: ParamCodes = ParamCodes {
    too_many: code!("model.effect.tooManySettings"),
    bad_name: code!("model.effect.settingName"),
    bad_value: code!("model.effect.badSetting"),
};
const TRANSITION_PARAMS: ParamCodes = ParamCodes {
    too_many: code!("model.transition.tooManySettings"),
    bad_name: code!("model.transition.settingName"),
    bad_value: code!("model.transition.badSetting"),
};
const MOTION_PARAMS: ParamCodes = ParamCodes {
    too_many: code!("model.motion.tooManySettings"),
    bad_name: code!("model.motion.settingName"),
    bad_value: code!("model.motion.badSetting"),
};
const GENERATOR_PARAMS: ParamCodes = ParamCodes {
    too_many: code!("model.layer.generatorTooManySettings"),
    bad_name: code!("model.layer.generatorSettingName"),
    bad_value: code!("model.layer.generatorBadSetting"),
};

/// Values the definition describes are checked against it; the rest only need to be sane. `codes` names the owner.
fn check_params(
    p: &Project,
    def: Option<&ContribDef>,
    params: &BTreeMap<String, ParamValue>,
    codes: &ParamCodes,
) -> Result<()> {
    check(params.len() <= MAX_PARAMS, codes.too_many)?;
    for (key, value) in params {
        check(!key.is_empty() && key.len() <= 64, codes.bad_name)?;
        match def.and_then(|d| d.param(key).map(|spec| (d, spec))) {
            // The contribution and key name the setting for the UI, which words its label itself.
            Some((d, spec)) => spec.check(value, p).map_err(|e| {
                invalid_with(
                    e.with("contribution", d.id.as_str())
                        .with("key", key.as_str()),
                )
            })?,
            None => check(value.is_sane(), codes.bad_value)?,
        }
    }
    Ok(())
}

fn validate_effects(p: &Project, effects: &[EffectInstance]) -> Result<()> {
    validate_stack(p, effects, ContribKind::Effect)
}

/// An effect stack of `kind` contributions: pictures' effects, or a sound source's audio effects.
fn validate_stack(p: &Project, effects: &[EffectInstance], kind: ContribKind) -> Result<()> {
    check(effects.len() <= MAX_EFFECTS, code!("model.effect.tooMany"))?;
    let mut ids = HashSet::new();
    for e in effects {
        check(
            !e.id.is_empty() && e.id.len() <= 64 && ids.insert(e.id.as_str()),
            code!("model.effect.duplicateId"),
        )?;
        let def = check_contrib(&e.effect, kind)?;
        check_params(p, def, &e.params, &EFFECT_PARAMS)?;
    }
    Ok(())
}

fn unique<'a>(seen: &mut HashSet<&'a str>, id: &'a str) -> Result<()> {
    check(!id.is_empty(), code!("model.project.missingId"))?;
    if seen.insert(id) {
        Ok(())
    } else {
        Err(invalid_with(msg!("model.project.duplicateId", id = id)))
    }
}

pub fn validate(p: &Project) -> Result<()> {
    if p.version != PROJECT_VERSION {
        return Err(ModelError::UnsupportedVersion(p.version));
    }
    check(
        !p.name.trim().is_empty() && p.name.chars().count() <= MAX_NAME,
        code!("model.project.name"),
    )?;
    validate_settings(p)?;
    check(
        p.assets.len() <= MAX_ASSETS,
        code!("model.project.tooManyMedia"),
    )?;
    check(
        p.timeline.main.len() <= MAX_CLIPS,
        code!("model.project.tooManyClips"),
    )?;
    check(
        p.timeline.tracks.len() <= MAX_TRACKS,
        code!("model.project.tooManyTracks"),
    )?;
    let item_count: usize = p.timeline.tracks.iter().map(|t| t.items.len()).sum();
    check(
        item_count <= MAX_ITEMS,
        code!("model.project.tooManyLayers"),
    )?;
    check(p.luts.len() <= MAX_LUTS, code!("model.project.tooManyLuts"))?;
    check(
        p.word_fixes.len() <= MAX_WORD_FIXES,
        code!("model.project.tooManyFixes"),
    )?;
    check(
        p.word_looks.len() <= MAX_WORD_LOOKS,
        code!("model.wordLook.tooMany"),
    )?;
    check(
        p.requires.len() <= MAX_REQUIRES
            && p.requires.iter().all(|r| !r.is_empty() && r.len() <= 64),
        code!("model.project.requirements"),
    )?;

    // Sized once: growing it would rehash about a dozen times and briefly hold two tables.
    let id_count = 1
        + p.assets.len()
        + p.luts.len()
        + p.recordings.len()
        + p.timeline.main.len()
        + p.timeline.tracks.len()
        + item_count;
    let mut ids: HashSet<&str> = HashSet::with_capacity(id_count);
    unique(&mut ids, &p.id)?;
    for a in &p.assets {
        unique(&mut ids, &a.id)?;
        check_with(!a.path.is_empty(), || {
            msg!("model.media.noPath", name = a.name.as_str())
        })?;
        check_with(AssetPath::parse(&a.path).is_some(), || {
            msg!("model.media.badPath", name = a.name.as_str())
        })?;
        check_with(
            valid_time(a.duration_us) && a.fps.is_finite() && a.fps >= 0.0,
            || msg!("model.media.timing", name = a.name.as_str()),
        )?;
        check_with(a.rotation.is_none_or(|r| r < 360 && r % 90 == 0), || {
            msg!("model.media.rotation", name = a.name.as_str())
        })?;
        if a.kind != AssetKind::Audio {
            check_with(a.width > 0 && a.height > 0, || {
                msg!("model.media.size", name = a.name.as_str())
            })?;
        }
    }
    for lut in &p.luts {
        unique(&mut ids, &lut.id)?;
        check(
            !lut.name.trim().is_empty() && lut.name.chars().count() <= MAX_NAME,
            code!("model.lut.name"),
        )?;
        check_with(!lut.path.trim().is_empty(), || {
            msg!("model.lut.noFile", name = lut.name.as_str())
        })?;
        check_with(AssetPath::parse(&lut.path).is_some(), || {
            msg!("model.lut.badFile", name = lut.name.as_str())
        })?;
    }
    for r in &p.recordings {
        unique(&mut ids, &r.id)?;
        validate_recording(p, r)?;
    }
    for clip in &p.timeline.main {
        unique(&mut ids, &clip.id)?;
        validate_clip(p, clip)?;
    }
    validate_transitions(p)?;
    validate_background(p)?;
    p.cursor.check()?;
    crate::captions::validate_settings(p)?;
    for track in &p.timeline.tracks {
        unique(&mut ids, &track.id)?;
        for item in &track.items {
            unique(&mut ids, &item.id)?;
        }
        validate_track(p, track)?;
    }
    validate_word_fixes(p)?;
    crate::captions::looks::validate_word_looks(p)
}

/// Word fixes: each on a file of the project, over a range of valid times, reading one line of text; sorted by
/// file and start and never overlapping on one file, as the speech map's one-pointer walk needs. One media lookup per
/// file the fixes name: O(fixes + files named × media).
fn validate_word_fixes(p: &Project) -> Result<()> {
    let mut last: Option<&WordFix> = None;
    for f in p.word_fixes.iter() {
        let same_file = last.is_some_and(|l| l.asset_id == f.asset_id);
        if !same_file && p.asset(&f.asset_id).is_none() {
            return Err(not_found("media", &f.asset_id));
        }
        check(
            valid_time(f.start_us) && valid_time(f.end_us),
            code!("model.project.timeOutside"),
        )?;
        check(f.start_us < f.end_us, code!("model.fix.range"))?;
        check(!fix_text_blank(&f.text), code!("model.fix.blank"))?;
        check(fix_text_ok(&f.text), code!("model.fix.text"))?;
        check(
            last.is_none_or(|l| {
                if same_file {
                    l.end_us <= f.start_us
                } else {
                    l.asset_id < f.asset_id
                }
            }),
            code!("model.fix.overlap"),
        )?;
        last = Some(f);
    }
    Ok(())
}

fn validate_settings(p: &Project) -> Result<()> {
    let s = &p.settings;
    check(
        [24, 25, 30, 50, 60].contains(&s.fps),
        code!("model.settings.fps"),
    )?;
    check(
        !s.formats.is_empty() && s.formats.len() <= 3,
        code!("model.settings.formatCount"),
    )?;
    let distinct: HashSet<_> = s.formats.iter().collect();
    check(
        distinct.len() == s.formats.len(),
        code!("model.settings.duplicateFormat"),
    )?;
    check(
        s.formats.contains(&s.primary_format),
        code!("model.settings.primary"),
    )
}

fn validate_look(p: &Project, look: &Look) -> Result<()> {
    check(
        in_range(look.strength, 0.0, 1.0) && in_range(look.lut_strength, 0.0, 1.0),
        code!("model.look.strength"),
    )?;
    if let Some(lut) = &look.lut_id {
        check(
            p.luts.iter().any(|l| &l.id == lut),
            code!("model.look.missingLut"),
        )?;
    }
    if let Some(id) = &look.preset {
        check_contrib(id, ContribKind::Look)?;
    }
    Ok(())
}

fn validate_clip(p: &Project, c: &Clip) -> Result<()> {
    let source_len = source_duration_us(p, &c.source)
        .ok_or_else(|| invalid(code!("model.clip.missingMedia")))?;
    check(
        valid_time(c.source_in_us)
            && valid_time(c.source_out_us)
            && c.source_in_us < c.source_out_us
            && c.source_out_us <= source_len.saturating_add(SOURCE_SLACK_US),
        code!("model.clip.outsideSource"),
    )?;
    check(in_range(c.speed, 0.25, 4.0), code!("model.clip.speed"))?;
    check(in_range(c.volume, 0.0, 2.0), code!("model.clip.volume"))?;
    let duration = c.duration_us();
    check(duration >= MIN_DURATION_US, code!("model.clip.tooShort"))?;
    check(
        valid_time(c.fade_in_us)
            && valid_time(c.fade_out_us)
            && c.fade_in_us.saturating_add(c.fade_out_us) <= duration,
        code!("model.clip.fades"),
    )?;
    check(
        c.framing
            .values()
            .all(|f| in_range(f.focus_x, 0.0, 1.0) && in_range(f.focus_y, 0.0, 1.0)),
        code!("model.clip.focus"),
    )?;
    validate_look(p, &c.look)?;
    validate_effects(p, &c.effects)?;
    crate::cursor_style::check_clip(c)?;
    let recording = match &c.source {
        ClipSource::Recording { id } => p.recording(id),
        ClipSource::Asset { .. } => None,
    };
    if let Some(camera) = &c.camera {
        // A bubble's camera, or a camera-only recording's footage (its mirror).
        check(
            recording.is_some_and(|r| r.camera().is_some() || r.camera_only()),
            code!("model.camera.noCamera"),
        )?;
        check_layout(&camera.bubble, &CAMERA_LAYOUT)?;
        check_motion(p, &camera.bubble, Owner::Camera, 0)?;
    }
    for (source, mix) in &c.source_mix {
        // A sound source, or a picture source whose files carry sound (an imported camera's own sound).
        let s = recording.and_then(|r| r.source(source));
        check(
            s.is_some_and(|s| makes_sound(p, s)),
            code!("model.clip.sourceMix"),
        )?;
        if let Some(s) = s {
            validate_mix(p, s, mix)?;
        }
    }
    Ok(())
}

/// Transitions as `transitions::normalize` leaves them: between two clips, within range and footage (each cut
/// checked with the previous transition's share of the clip), a transition contribution, no fades at the cut.
fn validate_transitions(p: &Project) -> Result<()> {
    let main = &p.timeline.main;
    let mut incoming_after: Us = 0;
    for (i, c) in main.iter().enumerate() {
        let Some(t) = &c.transition_out else {
            incoming_after = 0;
            continue;
        };
        check(i + 1 < main.len(), code!("model.transition.noNextClip"))?;
        check(
            (transitions::MIN_TRANSITION_US..=transitions::MAX_TRANSITION_US)
                .contains(&t.duration_us),
            code!("model.transition.length"),
        )?;
        let def = check_contrib(&t.effect, ContribKind::Transition)?;
        check_params(p, def, &t.params, &TRANSITION_PARAMS)?;
        check(
            t.duration_us <= transitions::fit_us(p, i, incoming_after, 0),
            code!("model.transition.tooLong"),
        )?;
        check(
            c.fade_out_us == 0 && main[i + 1].fade_in_us == 0,
            code!("model.transition.fades"),
        )?;
        incoming_after = transitions::split(t.duration_us).1;
    }
    Ok(())
}

fn validate_background(p: &Project) -> Result<()> {
    let b = &p.background;
    check(
        valid_color(&b.color) && b.color_end.as_deref().is_none_or(valid_color),
        code!("model.background.color"),
    )?;
    check(
        in_range(b.padding, 0.0, 0.4),
        code!("model.background.padding"),
    )?;
    if b.kind == BackgroundKind::Image {
        let asset = b.asset_id.as_deref().and_then(|id| p.asset(id));
        check(
            asset.is_some_and(|a| a.kind == AssetKind::Image),
            code!("model.background.missingImage"),
        )?;
    }
    Ok(())
}

fn validate_text_style(s: &TextStyle) -> Result<()> {
    check(!s.font.family.trim().is_empty(), code!("model.text.font"))?;
    check(font_ok(&s.font), code!("model.text.fontSource"))?;
    check(in_range(s.size, 4.0, 400.0), code!("model.text.size"))?;
    check((100..=900).contains(&s.weight), code!("model.text.weight"))?;
    check(
        valid_color(&s.color) && s.background.as_deref().is_none_or(valid_color),
        code!("model.text.color"),
    )?;
    // The outline is in the same pixels as the size and scales with it: up to a quarter of the largest size.
    if let Some(o) = &s.outline {
        check(
            valid_color(&o.color) && in_range(o.width, 0.0, 100.0),
            code!("model.text.outline"),
        )?;
    }
    Ok(())
}

fn validate_track(p: &Project, track: &Track) -> Result<()> {
    check(track.name.chars().count() <= 100, code!("model.track.name"))?;
    // Solo picks what plays and shows; a zoom track is neither (the main track, which it moves, always plays).
    check(
        !track.solo || track.kind != TrackKind::Zoom,
        code!("model.track.zoomSolo"),
    )?;
    let mut previous_end: Option<Us> = None;
    for item in &track.items {
        check(item.start_us >= 0, code!("model.layer.beforeStart"))?;
        check(
            item.duration_us >= MIN_DURATION_US,
            code!("model.layer.tooShort"),
        )?;
        check(
            item.start_us <= MAX_TIME_US && item.duration_us <= MAX_TIME_US,
            code!("model.layer.time"),
        )?;
        check(
            item.content.track_kind() == track.kind,
            code!("model.layer.wrongTrackKind"),
        )?;
        if let Some(end) = previous_end {
            check_with(
                item.start_us.saturating_add(OVERLAP_TOLERANCE_US) >= end,
                || msg!("model.track.overlap", track = track.name.as_str()),
            )?;
        }
        previous_end = Some(item.end_us());
        check_layout(&item.layout, &LAYER_LAYOUT)?;
        let owner = Owner::Layer {
            visual: item.content.is_visual(),
            adjustment: matches!(item.content, ItemContent::Adjustment),
            text: matches!(item.content, ItemContent::Text { .. }),
        };
        check_motion(p, &item.layout, owner, item.duration_us)?;
        if let Some(attach) = &item.attach {
            check(valid_time(attach.source_us), code!("model.layer.time"))?;
            check(
                source_duration_us(p, &attach.source).is_some(),
                code!("model.layer.attachedMissing"),
            )?;
        }
        validate_content(p, &item.content, item.duration_us)?;
        // A sound layer's stack holds audio effects (a separated recording source keeps its mix's); a picture
        // layer's, effects; a zoom has none.
        if matches!(item.content, ItemContent::Audio { .. }) {
            validate_stack(p, &item.effects, ContribKind::AudioEffect)?;
        } else {
            check(
                item.effects.is_empty() || item.content.is_visual(),
                code!("model.layer.noEffects"),
            )?;
            validate_effects(p, &item.effects)?;
        }
    }
    Ok(())
}

fn validate_content(p: &Project, content: &ItemContent, duration: Us) -> Result<()> {
    match content {
        ItemContent::Video {
            asset_id,
            source_in_us,
            speed,
            volume,
            ..
        } => {
            let asset = p
                .asset(asset_id)
                .filter(|a| a.kind == AssetKind::Video)
                .ok_or_else(|| invalid(code!("model.layer.videoMissing")))?;
            check(in_range(*speed, 0.25, 4.0), code!("model.layer.speed"))?;
            check(in_range(*volume, 0.0, 2.0), code!("model.layer.volume"))?;
            check(
                valid_time(*source_in_us)
                    && source_in_us.saturating_add(unscaled(duration, *speed))
                        <= asset.duration_us.saturating_add(SOURCE_SLACK_US),
                code!("model.layer.videoOutside"),
            )
        }
        ItemContent::Image { asset_id } => check(
            p.asset(asset_id)
                .is_some_and(|a| a.kind == AssetKind::Image),
            code!("model.layer.imageMissing"),
        ),
        ItemContent::Text { text, style } => {
            check(!text.trim().is_empty(), code!("model.text.empty"))?;
            check(
                text.chars().count() <= MAX_TEXT,
                code!("model.text.tooLong"),
            )?;
            validate_text_style(style)
        }
        ItemContent::Audio {
            asset_id,
            source_in_us,
            volume,
            fade_in_us,
            fade_out_us,
            effect,
            drift_ppm,
            centre_from,
            duck,
            muted: _,
            tie: _,
        } => {
            if let Some(id) = effect {
                check_contrib(id, ContribKind::AudioEffect)?;
            }
            if let Some(db) = duck {
                check(
                    in_range(*db, DEEPEST_DUCK_DB, 0.0),
                    code!("model.audio.duck"),
                )?;
            }
            check_centre(*centre_from)?;
            let asset = p
                .asset(asset_id)
                .filter(|a| a.has_audio)
                .ok_or_else(|| invalid(code!("model.audio.missing")))?;
            check(in_range(*volume, 0.0, 2.0), code!("model.layer.volume"))?;
            check(
                in_range(*drift_ppm, -MAX_DRIFT_PPM, MAX_DRIFT_PPM),
                code!("model.recording.drift"),
            )?;
            // The file's own time it plays: its length on the timeline at the file's rate.
            let footage = drifted(duration, *drift_ppm);
            check(
                valid_time(*source_in_us)
                    && source_in_us.saturating_add(footage)
                        <= asset.duration_us.saturating_add(SOURCE_SLACK_US),
                code!("model.audio.outside"),
            )?;
            check(
                valid_time(*fade_in_us)
                    && valid_time(*fade_out_us)
                    && fade_in_us.saturating_add(*fade_out_us) <= duration,
                code!("model.audio.fades"),
            )
        }
        ItemContent::Zoom { target, scale } => {
            check(in_range(*scale, 1.0, 5.0), code!("model.layer.zoom"))?;
            if let ZoomTarget::Rect { rect } = target {
                check(valid_box(rect), code!("model.layer.zoomArea"))?;
            }
            Ok(())
        }
        ItemContent::Generator {
            source,
            params,
            still: _,
            offset_us,
            span_us,
            seed_from: _,
        } => generator::check_generator(
            p,
            generator::Generator {
                source,
                params,
                offset_us: *offset_us,
                span_us: *span_us,
            },
            duration,
        ),
        ItemContent::Adjustment => Ok(()),
    }
}
