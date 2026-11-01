//! The compiler: the model's tool calls, as `tools` read them, against the project snapshot the message's
//! context was built from, out as edits.
//! - Additions (`setFormat`, `setCaptions`, `fixCaptionWords`, `splitAt`, a trim that makes a clip longer, and the
//!   media: `addMusic`, `addSound`, `addText`, `addImage` and `makeTitleCard`'s title) are one `Transaction` the app
//!   applies at once: one undo step. An addition is judged by what its part does, not by its tool: one that takes
//!   something away (the model's own `removals`, as a format dropped with what was placed in it) is compiled again with
//!   the removals and proposed.
//! - Removals (`cutRange`, `keepOnly`, `removeSilences`, `removeFillers`, a trim that makes a clip shorter) are one
//!   `Proposal`: a `Transaction` for the project as the additions leave it, judged by the model's own review (what it
//!   takes away, on the timeline), applied as one undo step when the person says so.
//! - `findMoments` gives moments, `followSubject` and `blurObjects` jobs for their runners, `makeSticker` and
//!   `makeTitleCard`'s background jobs for a picture to be drawn (never an op until it exists), `askUser` questions,
//!   `undo` a flag. `cannotDo` is a refusal shown to the person, as is a tool that needs an analysis the clip doesn't
//!   have yet (`STILL_LISTENING`, `STILL_WATCHING`, `ANALYZE_FIRST`), filler words only roughly timed
//!   (`ROUGH_FILLERS`, `ROUGH_FILLERS_STAY`) and a removal with nothing to remove (`NO_FILLERS`, no pause that long);
//!   every other refusal goes back to the model with its reason.
//!
//! All calls compile on one working copy, in two phases: the additions in call order (the edits first, then the media
//! layers on what the edits made, so a trim later in the answer never shortens a picture placed before it), then the
//! removals in call order on what the additions made, as the app applies them. Each call's ops are one part of its phase's batch
//! (`apply_part`), tried on a copy and checked as the batch would end there (`finish_parts`): a call that fails is a
//! `Refusal` with its index, and the others still compile. The removals' working copy ends as `apply_ops` of the
//! proposal on the additions' result, so the review takes it as it is (`review_applied`) rather than applying again.
//! Each phase ends with the music's refit as one more part (`Phase::refit_music`, as `with_music_refit` folds it into a
//! hand edit): music that reached the video's end is cut or played on to the phase's new end, fading out there, in the
//! same undo step, and the proposal's review shows the song shortened.
//!
//! The model's times are the snapshot's timeline, as its context showed them. A range must stand on the context or the
//! message (`evidence`), else it is refused; it snaps to what it stands on, and is then mapped into the working copy by
//! content (`map`), so an earlier call in the answer never shifts what a later one names. A clip ref resolves through
//! the `Refs` the context was built with, never through the project's own order. `undo` doesn't mix with edits: the
//! edits were compiled against the project before it, so they are refused back to the model.
//!
//! Undo labels are message ids with values, worded by the UI: `chat.<tool>` (`chat.cutRange {startUs, endUs}`), and
//! `chat.andMore {first, more}` for a step of several calls. Splitting by concern: `additions`, `removals`, `trim`
//! (both), `media` (the library's music, sounds, words and pictures, and the pictures to draw), `map`.

mod additions;
mod map;
mod media;
mod removals;
mod trim;

use std::collections::{HashMap, HashSet};

use klipolo_model::coded::{CodedError, ErrorParam};
use klipolo_model::ids::Id;
use klipolo_model::media::music::refit_music_ops;
use klipolo_model::ops::{Op, Origin, Transaction};
use klipolo_model::project::{AssetKind, FormatKind};
use klipolo_model::risk::{Review, removals};
use klipolo_model::speech::{Speech, run_spans};
use klipolo_model::time::{RangeUs, Us};
use klipolo_model::timeline::{Attach, ClipSource};
use klipolo_model::{ModelError, Project, finish_parts};

use crate::context::{Refs, SceneLog, View, secs};
use crate::evidence::{Evidence, Unfounded};
use crate::library::Library;
use crate::sanitize;
use crate::tools::{Moment, PictureSize, Position, ToolCall};
use map::Map;
pub use media::{sticker_ops, sticker_placement};
use trim::Trim;

/// What the person is told when a tool needs a clip's words and Klipolo is still hearing or timing them.
pub const STILL_LISTENING: &str =
    "Klipolo is still listening to this clip. Ask me again when its analysis is done.";
/// What the person is told when finding moments needs a clip's scene log and Klipolo is still making it (its words
/// are in, so cuts work already).
pub const STILL_WATCHING: &str =
    "Klipolo is still watching this clip. Ask me again when its analysis is done.";
/// What the person is told when a tool needs a clip's analysis and none was started.
pub const ANALYZE_FIRST: &str = "Analyze this clip first";
/// What the person is told when `removeFillers` finds no filler word.
pub const NO_FILLERS: &str = "There are no filler words to remove.";
/// What the person is told when a filler word is only roughly timed (word timing failed, stopped or ran out, so its
/// turn's words were spread over it by length): a cut at its guessed time would take real words. Said when the
/// preview's header offers Time words again for its clip (`Inputs::time_again`).
pub const ROUGH_FILLERS: &str = "Some words in this clip are only roughly timed, so I can't cut filler words exactly. \
                                 Press Time words again above the video, then ask me.";
/// `ROUGH_FILLERS` when the preview's header doesn't offer Time words again for the clip (timing again can't help,
/// as when its words don't match their sound, or its analysis isn't done): it names no button.
pub const ROUGH_FILLERS_STAY: &str =
    "Some words in this clip are only roughly timed, so I can't cut filler words exactly.";
/// The most pictures one answer draws (`makeSticker`, `makeTitleCard`): each drawing is paid for, so an answer asking
/// for more draws its first ones and refuses the rest (`TOO_MANY_PICTURES`).
pub const MAX_PICTURES: usize = 3;
/// What the person is told for each picture past the first `MAX_PICTURES` of an answer: a refusal shown to the person,
/// as `cannotDo` is.
pub const TOO_MANY_PICTURES: &str =
    "I draw up to 3 pictures per message, so I left out the rest. Ask me again for them.";

/// How far a file's analysis (transcript, loudness, scene log) is. A file missing from `Inputs::analysis` counts as
/// not started.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnalysisState {
    NotStarted,
    /// Its words are being heard or timed.
    Transcribing,
    /// Its words are in (heard and timed); its scene log is still being made.
    Watching,
    Done,
}

/// Everything besides the project that a message's calls compile against, all from the snapshot the message's context
/// was built from: the caller compiles against that same project.
#[derive(Clone, Debug, Default)]
pub struct Inputs {
    /// What the person wrote: the times in it count as evidence (`evidence::typed_times`).
    pub message: String,
    pub view: View,
    /// The refs the context named clips by.
    pub refs: Refs,
    /// Transcripts and loudness (`levels`) by asset id.
    pub speech: Speech,
    /// Scene logs by asset id.
    pub scenes: HashMap<String, SceneLog>,
    /// Each file's analysis, by asset id.
    pub analysis: HashMap<String, AnalysisState>,
    /// The files whose rough words the preview's header offers to time again (Time words again), by asset id: a
    /// refusal of their filler words points there (`ROUGH_FILLERS`), any other's doesn't (`ROUGH_FILLERS_STAY`).
    pub time_again: HashSet<String>,
    /// What the media tools name: the project's music, sounds and pictures and Klipolo's pack.
    pub library: Library,
}

/// What one message's calls compiled to.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Compiled {
    /// Applied at once, one undo step.
    pub additions: Option<Transaction>,
    /// Shown for the person to apply (one undo step) or discard, against the project once `additions` are applied.
    pub proposal: Option<Proposal>,
    /// The calls whose edits are in `proposal`, by call index, in call order. Every other call not refused is done.
    pub proposed_calls: Vec<usize>,
    /// Timeline ranges to point the person at, on the project once `additions` are applied.
    pub moments: Vec<Moment>,
    pub jobs: Vec<Job>,
    /// `askUser`'s questions, without dashes (`sanitize::reply`).
    pub questions: Vec<String>,
    /// The calls that compiled to nothing, by call index.
    pub refusals: Vec<Refusal>,
    /// The model asked to undo the last change.
    pub undo: bool,
    /// The additions that went through but changed nothing (a format the video already has), by call index.
    pub unchanged: Vec<usize>,
}

/// Removals for the person to approve.
#[derive(Clone, Debug, PartialEq)]
pub struct Proposal {
    pub tx: Transaction,
    /// `tx` judged against the project once the additions are applied: `after` is what Apply commits.
    pub review: Review,
    /// How much footage goes: the length of `ranges`.
    pub removed_us: Us,
    /// The main-track footage that goes, on the timeline once the additions are applied (red on the timeline).
    pub ranges: Vec<RangeUs>,
}

/// Work for a runner: tracking or blurring what one short noun phrase names, or a picture for an image maker to draw
/// (Muse Image in the app). A picture's job is never an op until the picture exists: the app places it when it comes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Job {
    Follow {
        description: String,
    },
    Blur {
        description: String,
    },
    /// A sticker to draw and place over its footage (`sticker_placement`).
    MakeSticker(MakeSticker),
    /// A background about `prompt`, with no lettering, to go under the title layer `title_id`, where that layer is
    /// when the picture comes (it moves with its clip); dropped if the layer is gone. `format` is the shape to draw:
    /// the primary format as the message's additions leave it. A format change still waiting for approval isn't in
    /// it, so the app reads the project's primary format again when it starts drawing.
    MakeTitleBackground {
        prompt: String,
        format: FormatKind,
        title_id: Id,
    },
}

/// A sticker of `prompt` to draw, cut out of its background, and show for `duration_us` at `size` and `position`
/// (`sticker_ops`) over the footage `anchor` names: a clip and a moment of its source, as an attached layer holds its
/// place (`query::anchor_at`), so it moves with that footage while the picture is drawn. `at_us` is where that footage
/// played on the project as the message's additions left it, and the start to use when `anchor` is None (past the main
/// track's end). When the picture comes, `sticker_placement` says where and how long on the project as it is then, or
/// None to drop the job (that footage was cut, or the start is now at the video's end).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MakeSticker {
    pub prompt: String,
    pub at_us: Us,
    pub duration_us: Us,
    pub position: Position,
    pub size: PictureSize,
    pub anchor: Option<Attach>,
}

/// A call that compiled to nothing, and why, in one plain sentence on one line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refusal {
    pub call_index: usize,
    pub reason: String,
    pub audience: Audience,
    /// The clip the refusal is about, by its ref in the context (`c4`): set when a clip isn't analyzed yet or its
    /// fillers are only roughly timed, so the person sees which one `STILL_LISTENING`, `STILL_WATCHING`, `ANALYZE_FIRST`
    /// or `ROUGH_FILLERS` (`ROUGH_FILLERS_STAY`) means (the app names it after "this clip").
    pub clip: Option<String>,
}

/// Who a refusal's reason is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Audience {
    /// Sent back to the model as the call's result, for it to try again or explain.
    Model,
    /// Shown to the person as it is, never retried: `cannotDo` (without dashes, `sanitize::reply`), and a clip that
    /// isn't analyzed yet.
    Person,
}

/// Compiles `calls` against `project`, the snapshot `inputs` come from.
pub fn compile(project: &Project, inputs: &Inputs, calls: &[ToolCall]) -> Compiled {
    Compiler::new(project, inputs, false).run(calls)
}

/// Compiles another app's `calls` (over MCP) as `compile` does chat's, but for the words they put on screen: an app
/// acts for its own person, so `addText`'s words and `makeTitleCard`'s title are taken as it gave them, cleaned and
/// within what a text layer holds (`media::apps_words`), where chat's model may only quote the person.
pub fn compile_for_app(project: &Project, inputs: &Inputs, calls: &[ToolCall]) -> Compiled {
    Compiler::new(project, inputs, true).run(calls)
}

/// One call's ops and its undo label.
struct Edit {
    ops: Vec<Op>,
    label: CodedError,
}

/// Why a call compiled to nothing.
struct Refused {
    reason: String,
    audience: Audience,
    clip: Option<String>,
}

/// Records that call `call_index` compiled to nothing.
fn refuse(out: &mut Compiled, call_index: usize, r: Refused) {
    out.refusals.push(Refusal {
        call_index,
        reason: r.reason,
        audience: r.audience,
        clip: r.clip,
    });
}

/// A reason for the model.
fn to_model(reason: String) -> Refused {
    Refused {
        reason,
        audience: Audience::Model,
        clip: None,
    }
}

/// A sentence for the person: a fact about the project the model couldn't change by trying again.
fn to_person(reason: String) -> Refused {
    Refused {
        reason,
        audience: Audience::Person,
        clip: None,
    }
}

/// `chat.<tool>` with its values.
fn label(tool: &str, params: Vec<(&str, ErrorParam)>) -> CodedError {
    CodedError {
        code: format!("chat.{tool}"),
        params: (params.into_iter())
            .map(|(k, v)| (k.to_owned(), v))
            .collect(),
    }
}

/// A time in microseconds as a label's value.
fn us(t: Us) -> ErrorParam {
    ErrorParam::Number(t as f64)
}

/// One phase's batch: its calls' ops applied part by part to the working copy.
struct Phase {
    /// The parts so far, the batch not finished: what the next call's ops meet.
    work: Project,
    /// `work` as the batch would end here (transitions refitted, validated): the phase's result.
    done: Project,
    ops: Vec<Op>,
    /// Each kept edit's label, with its call's index in the answer.
    labels: Vec<(usize, CodedError)>,
    /// The calls whose edits held no ops: they went through and changed nothing.
    unchanged: Vec<usize>,
}

impl Phase {
    fn new(start: Project) -> Self {
        Self {
            work: start.clone(),
            done: start,
            ops: Vec::new(),
            labels: Vec::new(),
            unchanged: Vec::new(),
        }
    }

    /// `edit` tried on a copy of the working copy: its ops applied as one part, and the batch still ending valid. The
    /// phase stays as it was until `keep`.
    fn try_edit(&self, edit: Edit) -> Result<Tried, ModelError> {
        if edit.ops.is_empty() {
            return Ok(Tried {
                edit,
                after: None,
                removes: false,
            });
        }
        let mut work = self.work.clone();
        klipolo_model::apply_part(&mut work, &edit.ops)?;
        let mut done = work.clone();
        finish_parts(&mut done)?;
        let removes = !removals(&self.done, &done).is_empty();
        Ok(Tried {
            edit,
            after: Some((work, done)),
            removes,
        })
    }

    /// Makes the tried edit of call `call` part of the batch. An edit with no ops (a format the video already has) adds
    /// nothing, and the call is noted as having changed nothing.
    fn keep(&mut self, tried: Tried, call: usize) {
        let Some((work, done)) = tried.after else {
            self.unchanged.push(call);
            return;
        };
        self.work = work;
        self.done = done;
        self.ops.extend(tried.edit.ops);
        self.labels.push((call, tried.edit.label));
    }

    /// `try_edit`, then `keep`.
    fn add(&mut self, edit: Edit, call: usize) -> Result<(), ModelError> {
        let tried = self.try_edit(edit)?;
        self.keep(tried, call);
        Ok(())
    }

    /// The phase's ops as one chat transaction, labelled for the first of its calls in the answer (the order the
    /// person asked in, whatever order they were placed in) and how many more.
    fn transaction(&self) -> Option<Transaction> {
        let mut labels = self.labels.clone();
        labels.sort_by_key(|(call, _)| *call);
        let labels: Vec<CodedError> = labels.into_iter().map(|(_, label)| label).collect();
        let (label, params) = match labels.as_slice() {
            [] => return None,
            [one] => (one.code.clone(), one.params.clone()),
            [first, rest @ ..] => (
                "chat.andMore".to_owned(),
                [
                    (
                        "first".to_owned(),
                        ErrorParam::Coded(Box::new(first.clone())),
                    ),
                    ("more".to_owned(), ErrorParam::from(rest.len())),
                ]
                .into(),
            ),
        };
        Some(Transaction {
            label,
            params: (!params.is_empty()).then_some(params),
            ops: self.ops.clone(),
            origin: Some(Origin::Chat),
        })
    }
}

/// An edit tried on a phase's working copy (`Phase::try_edit`).
struct Tried {
    edit: Edit,
    /// The working copy with it, and the batch as it would end there; None for an edit with no ops.
    after: Option<(Project, Project)>,
    /// It takes away something the batch had before it (the model's own `removals`).
    removes: bool,
}

/// A call waiting for the removals' phase.
enum Removal<'c> {
    /// A removal tool.
    Call(&'c ToolCall),
    /// A trim that makes a clip shorter.
    Shorter(Trim),
    /// An addition tool whose edit takes something away (a format dropped with what was placed in it).
    Moved(&'c ToolCall),
    /// A trim that makes a clip longer and takes something away.
    Longer(Trim),
}

struct Compiler<'a> {
    snapshot: &'a Project,
    inputs: &'a Inputs,
    evidence: Evidence,
    map: Map<'a>,
    /// The calls are another app's (`compile_for_app`), not chat's model's.
    by_app: bool,
}

impl<'a> Compiler<'a> {
    fn new(snapshot: &'a Project, inputs: &'a Inputs, by_app: bool) -> Compiler<'a> {
        Compiler {
            snapshot,
            inputs,
            evidence: Evidence::new(
                snapshot,
                &inputs.speech,
                &inputs.scenes,
                &inputs.view,
                &inputs.message,
            ),
            map: Map::new(snapshot),
            by_app,
        }
    }
}

impl Compiler<'_> {
    fn run(&self, calls: &[ToolCall]) -> Compiled {
        let mut out = Compiled {
            undo: calls.iter().any(|c| matches!(c, ToolCall::Undo)),
            ..Compiled::default()
        };
        // In call order: the edits that are additions compile now; media layers go on the project they make, then
        // removals, moments and pictures to draw wait for the project all the additions make, so nothing later in the
        // answer moves or shortens what came before it.
        let mut adding = Phase::new(self.snapshot.clone());
        let mut waiting: Vec<(usize, Removal)> = Vec::new();
        let mut moments: Vec<(usize, &[Moment])> = Vec::new();
        let mut layers: Vec<(usize, &ToolCall)> = Vec::new();
        let mut jobs: Vec<(usize, Job)> = Vec::new();
        let mut stickers: Vec<(usize, &ToolCall)> = Vec::new();
        let mut titles: Vec<(usize, &str, Id)> = Vec::new();
        // Once a format change is proposed, the later ones in the answer follow it there, so they apply in call order.
        let mut format_proposed = false;
        // The pictures asked for so far: past `MAX_PICTURES`, the rest are refused before anything is drawn.
        let mut pictures = 0;
        for (i, call) in calls.iter().enumerate() {
            if out.undo && is_edit(call) {
                let reason = format!(
                    "{}: not done, because undo is in the same answer. Call undo on its own, then the other changes.",
                    call.name()
                );
                refuse(&mut out, i, to_model(reason));
                continue;
            }
            if matches!(
                call,
                ToolCall::MakeSticker { .. } | ToolCall::MakeTitleCard { .. }
            ) {
                pictures += 1;
                if pictures > MAX_PICTURES {
                    let said = Refused {
                        reason: TOO_MANY_PICTURES.to_owned(),
                        audience: Audience::Person,
                        clip: None,
                    };
                    refuse(&mut out, i, said);
                    continue;
                }
            }
            if let Some(said) = self.missing_analysis(call) {
                refuse(&mut out, i, said);
                continue;
            }
            if format_proposed && matches!(call, ToolCall::SetFormat { .. }) {
                waiting.push((i, Removal::Moved(call)));
                continue;
            }
            let addition = match call {
                ToolCall::Undo => continue,
                ToolCall::AskUser { question } => {
                    out.questions.push(sanitize::reply(question));
                    continue;
                }
                ToolCall::CannotDo { reason } => {
                    let said = Refused {
                        reason: sanitize::reply(reason),
                        audience: Audience::Person,
                        clip: None,
                    };
                    refuse(&mut out, i, said);
                    continue;
                }
                ToolCall::FollowSubject { description } => {
                    let description = description.clone();
                    jobs.push((i, Job::Follow { description }));
                    continue;
                }
                ToolCall::BlurObjects { description } => {
                    let description = description.clone();
                    jobs.push((i, Job::Blur { description }));
                    continue;
                }
                ToolCall::FindMoments { moments: found, .. } => {
                    moments.push((i, found));
                    continue;
                }
                ToolCall::MakeSticker { .. } => {
                    stickers.push((i, call));
                    continue;
                }
                ToolCall::AddMusic { .. }
                | ToolCall::AddSound { .. }
                | ToolCall::AddText { .. }
                | ToolCall::AddImage { .. }
                | ToolCall::MakeTitleCard { .. } => {
                    layers.push((i, call));
                    continue;
                }
                ToolCall::CutRange { .. }
                | ToolCall::KeepOnly { .. }
                | ToolCall::RemoveSilences { .. }
                | ToolCall::RemoveFillers => {
                    waiting.push((i, Removal::Call(call)));
                    continue;
                }
                ToolCall::TrimClip { clip, edge, to_us } => {
                    match trim::plan(self, clip, *edge, *to_us) {
                        Ok(shorter) if shorter.removed.is_some() => {
                            waiting.push((i, Removal::Shorter(shorter)));
                            continue;
                        }
                        Ok(longer) => (
                            trim::lengthen(self, &adding.work, &longer),
                            Removal::Longer(longer),
                        ),
                        Err(r) => (Err(r), Removal::Moved(call)),
                    }
                }
                ToolCall::SplitAt { .. }
                | ToolCall::SetFormat { .. }
                | ToolCall::SetCaptions { .. }
                | ToolCall::FixCaptionWords { .. } => {
                    (self.addition(&adding.work, call), Removal::Moved(call))
                }
            };
            let (edit, if_it_removes) = addition;
            let tried = edit.and_then(|edit| {
                (adding.try_edit(edit)).map_err(|e| to_model(format!("{}: {e}", call.name())))
            });
            match tried {
                // Judged by what it does, not by its tool: proposed with the removals.
                Ok(tried) if tried.removes => {
                    format_proposed |= matches!(call, ToolCall::SetFormat { .. });
                    waiting.push((i, if_it_removes));
                }
                Ok(tried) => adding.keep(tried, i),
                Err(r) => refuse(&mut out, i, r),
            }
        }

        // The media layers, in call order, on the project the other additions make: where one starts and how long it
        // plays (cut at the video's end) don't depend on whether a trim or a split came before it or after it.
        for (i, call) in layers {
            let tried = match call {
                ToolCall::MakeTitleCard {
                    title,
                    at_us,
                    duration_us,
                    ..
                } => media::make_title_card(self, &adding.work, title, (*at_us, *duration_us))
                    .map(|(edit, title_id)| (edit, Some(title_id))),
                _ => self.addition(&adding.work, call).map(|edit| (edit, None)),
            }
            .and_then(|(edit, title_id)| {
                let tried = (adding.try_edit(edit))
                    .map_err(|e| to_model(format!("{}: {e}", call.name())))?;
                Ok((tried, title_id))
            });
            match tried {
                // A layer takes nothing away; if one ever did, it would be proposed like any other.
                Ok((tried, _)) if tried.removes => waiting.push((i, Removal::Moved(call))),
                Ok((tried, title_id)) => {
                    adding.keep(tried, i);
                    // The title is in: its background is drawn for it.
                    if let (Some(title_id), ToolCall::MakeTitleCard { prompt, .. }) =
                        (title_id, call)
                    {
                        titles.push((i, prompt.as_str(), title_id));
                    }
                }
                Err(r) => refuse(&mut out, i, r),
            }
        }
        waiting.sort_by_key(|(i, _)| *i);
        // A longer video plays the music that reached its end on to the new end.
        adding.refit_music(self.snapshot);

        // The project as the additions leave it: what the app applies them to, and what the proposal starts from.
        let with_additions = adding.done.clone();
        for (i, found) in moments {
            match self.moments(&with_additions, found) {
                Ok(m) => out.moments.extend(m),
                Err(r) => refuse(&mut out, i, r),
            }
        }
        for (i, call) in stickers {
            let ToolCall::MakeSticker {
                prompt,
                position,
                at_us,
                duration_us,
            } = call
            else {
                continue;
            };
            let times = (*at_us, *duration_us);
            match media::make_sticker(self, &with_additions, prompt, *position, times) {
                Ok(job) => jobs.push((i, job)),
                Err(r) => refuse(&mut out, i, r),
            }
        }
        for (i, prompt, title_id) in titles {
            jobs.push((
                i,
                media::title_background(prompt, &with_additions, title_id),
            ));
        }
        jobs.sort_by_key(|(i, _)| *i);
        out.jobs = jobs.into_iter().map(|(_, job)| job).collect();
        // The removals in call order, on top.
        let mut removing = Phase::new(with_additions);
        for (i, removal) in waiting {
            let work = &removing.work;
            let (call_name, edit) = match removal {
                Removal::Call(call) => (call.name(), removals::compile(self, work, call)),
                Removal::Shorter(t) => ("trimClip", trim::shorten(self, work, &t)),
                Removal::Moved(call) => (call.name(), self.addition(work, call)),
                Removal::Longer(t) => ("trimClip", trim::lengthen(self, work, &t)),
            };
            let added = edit.and_then(|edit| {
                (removing.add(edit, i)).map_err(|e| to_model(format!("{call_name}: {e}")))
            });
            match added {
                Ok(()) => out.proposed_calls.push(i),
                Err(r) => refuse(&mut out, i, r),
            }
        }
        // A shorter video cuts the music that reached its end there, fading out: Apply takes both in one step.
        removing.refit_music(&adding.done);

        out.additions = adding.transaction();
        out.unchanged = adding.unchanged.clone();
        out.unchanged.sort_unstable();
        match removing.transaction() {
            Some(tx) => {
                let review = tx.review_applied(&adding.done, removing.done);
                let ranges = review.removals.ranges.clone();
                out.proposal = Some(Proposal {
                    removed_us: ranges.iter().map(RangeUs::len).sum(),
                    ranges,
                    tx,
                    review,
                });
            }
            // Removals whose edits held no ops propose nothing.
            None => out.proposed_calls.clear(),
        }
        out.refusals.sort_by_key(|r| r.call_index);
        out
    }

    /// An addition tool's edit on `work` (a trim is planned apart, `trim::plan`).
    fn addition(&self, work: &Project, call: &ToolCall) -> Result<Edit, Refused> {
        match call {
            ToolCall::SplitAt { at_us } => additions::split_at(self, work, *at_us),
            ToolCall::SetFormat {
                format,
                keep_others,
            } => Ok(additions::set_format(work, *format, *keep_others)),
            ToolCall::SetCaptions { on, style } => Ok(additions::set_captions(work, *on, *style)),
            ToolCall::FixCaptionWords { find, replace } => {
                additions::fix_caption_words(self, work, find, replace)
            }
            ToolCall::AddMusic {
                file,
                volume,
                duck,
                at_us,
            } => media::add_music(self, work, file, (*volume, *duck), *at_us),
            ToolCall::AddSound { name, at_us } => media::add_sound(self, work, name, at_us),
            ToolCall::AddText {
                text,
                preset,
                at_us,
                duration_us,
            } => media::add_text(self, work, text, *preset, (*at_us, *duration_us)),
            ToolCall::AddImage {
                file,
                size,
                position,
                at_us,
                duration_us,
            } => media::add_image(self, work, file, (*size, *position), (*at_us, *duration_us)),
            other => Err(to_model(format!("{}: not an addition.", other.name()))),
        }
    }

    /// The person's refusal when `call` needs an analysis some clip on the main track lacks: its voice files' words for
    /// removing fillers and pauses (a scene log still being made doesn't hold them up), and its picture's scene log too
    /// for finding moments. Not started anywhere wins over still being made, since analyzing is something the person
    /// can do, and words still to come over a scene log; the refusal names the first clip it is about.
    fn missing_analysis(&self, call: &ToolCall) -> Option<Refused> {
        let pictures = match call {
            ToolCall::RemoveFillers | ToolCall::RemoveSilences { .. } => false,
            ToolCall::FindMoments { .. } => true,
            _ => return None,
        };
        let p = self.snapshot;
        let spans = run_spans(p);
        let mut voices: HashMap<&str, Vec<&str>> = HashMap::new();
        for s in &spans {
            voices
                .entry(s.clip.as_str())
                .or_default()
                .push(s.asset.as_str());
        }
        let state =
            |id: &str| (self.inputs.analysis.get(id).copied()).unwrap_or(AnalysisState::NotStarted);
        // The first clip, in timeline order, with a file never analyzed, the first whose words are still to come, and
        // the first whose scene log is.
        let (mut not_started, mut transcribing, mut watching) = (None, None, None);
        for clip in &p.timeline.main {
            let mut files = voices.get(clip.id.as_str()).cloned().unwrap_or_default();
            if pictures
                && let ClipSource::Asset { id } = &clip.source
                && p.asset(id).is_some_and(|a| a.kind == AssetKind::Video)
            {
                files.push(id.as_str());
            }
            for file in files {
                match state(file) {
                    AnalysisState::NotStarted => not_started = not_started.or(Some(&clip.id)),
                    AnalysisState::Transcribing => transcribing = transcribing.or(Some(&clip.id)),
                    AnalysisState::Watching if pictures => watching = watching.or(Some(&clip.id)),
                    AnalysisState::Watching | AnalysisState::Done => {}
                }
            }
        }
        let (said, clip) = match (not_started, transcribing, watching) {
            (Some(clip), ..) => (ANALYZE_FIRST, clip),
            (None, Some(clip), _) => (STILL_LISTENING, clip),
            (None, None, Some(clip)) => (STILL_WATCHING, clip),
            (None, None, None) => return None,
        };
        Some(Refused {
            reason: said.to_owned(),
            audience: Audience::Person,
            clip: self.inputs.refs.ref_of(clip),
        })
    }

    /// `findMoments`' ranges, each standing on the evidence, where they play once the additions are applied.
    fn moments(&self, with_additions: &Project, found: &[Moment]) -> Result<Vec<Moment>, Refused> {
        let mut out = Vec::new();
        for (k, m) in found.iter().enumerate() {
            let range = self.checked(
                "findMoments",
                (
                    &format!("ranges[{k}].startSec"),
                    &format!("ranges[{k}].endSec"),
                ),
                m.range,
            )?;
            out.extend(
                (self.map.range(with_additions, range).into_iter()).map(|range| Moment {
                    range,
                    label: m.label.clone(),
                }),
            );
        }
        Ok(out)
    }

    /// `r` as the evidence snaps it, or the reason it stands on nothing, naming the call's fields.
    fn checked(
        &self,
        tool: &str,
        (start, end): (&str, &str),
        r: RangeUs,
    ) -> Result<RangeUs, Refused> {
        self.evidence.range(r).map_err(|miss| {
            to_model(match miss {
                Unfounded::Start => unfounded(tool, start, r.start),
                Unfounded::End => unfounded(tool, end, r.end),
                Unfounded::Empty(at) => format!(
                    "{tool}: {start} and {end} both fall on {} s, so nothing lies between them.",
                    secs(at)
                ),
            })
        })
    }
}

/// Why a time stands on nothing.
fn unfounded(tool: &str, field: &str, t: Us) -> String {
    format!(
        "{tool}: {field} {} is not within 0.3 s of a word, a pause, a scene, a clip's start or end, the selection, the \
         playhead, the video's start or end, or a time the person said. Use a time from the context.",
        secs(t)
    )
}

/// Whether `call` changes the edit (now or through a runner).
fn is_edit(call: &ToolCall) -> bool {
    !matches!(
        call,
        ToolCall::Undo
            | ToolCall::AskUser { .. }
            | ToolCall::CannotDo { .. }
            | ToolCall::FindMoments { .. }
    )
}

impl Phase {
    /// Fits the music that reached the video's end before the phase (`before`) to where the phase leaves it
    /// (`refit_music_ops`): one more part of the batch, so the phase's one undo step takes back both and its result
    /// is what `apply_ops` makes of all its ops. A refit the batch refuses (a bug) is left out, as `with_music_refit`
    /// leaves it, so the edits still go through.
    fn refit_music(&mut self, before: &Project) {
        let refit = refit_music_ops(before, &self.done);
        if refit.is_empty() {
            return;
        }
        let mut work = self.work.clone();
        if klipolo_model::apply_part(&mut work, &refit).is_err() {
            return;
        }
        let mut done = work.clone();
        if finish_parts(&mut done).is_err() {
            return;
        }
        self.work = work;
        self.done = done;
        self.ops.extend(refit);
    }
}
