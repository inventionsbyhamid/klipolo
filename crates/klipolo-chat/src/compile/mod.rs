//! The compiler: the model's tool calls, as `tools` read them, against the project snapshot the message's context was
//! context was built from, out as edits.
//! - Additions (`setFormat`, `setCaptions`, `fixCaptionWords`, `splitAt`, a trim that makes a clip longer) are one
//!   `Transaction` the app applies at once: one undo step.
//! - Removals (`cutRange`, `keepOnly`, `removeSilences`, `removeFillers`, a trim that makes a clip shorter) are one
//!   `Proposal`: a `Transaction` for the project as the additions leave it, judged by the model's own review (what it
//!   takes away, on the timeline), applied as one undo step when the person says so.
//! - `findMoments` gives moments, `followSubject` and `blurObjects` jobs for their runners, `askUser` questions, `undo`
//!   a flag. `cannotDo` is a refusal shown to the person, as is a tool that needs an analysis the clip doesn't have yet
//!   (`STILL_LISTENING`, `ANALYZE_FIRST`); every other refusal goes back to the model with its reason.
//!
//! All calls compile on one working copy, in two phases: the additions in call order, then the removals in call order
//! on what the additions made, as the app applies them. Each call's ops are one part of its phase's batch
//! (`apply_part`), tried on a copy and checked as the batch would end there (`finish_parts`): a call that fails is a
//! `Refusal` with its index, and the others still compile. The removals' working copy ends as `apply_ops` of the
//! proposal on the additions' result, so the review takes it as it is (`review_applied`) rather than applying again.
//!
//! The model's times are the snapshot's timeline, as its context showed them. A range must stand on the context or the
//! message (`evidence`), else it is refused; it snaps to what it stands on, and is then mapped into the working copy by
//! content (`map`), so an earlier call in the answer never shifts what a later one names. A clip ref resolves through
//! the `Refs` the context was built with, never through the project's own order. `undo` doesn't mix with edits: the
//! edits were compiled against the project before it, so they are refused back to the model.
//!
//! Undo labels are message ids with values, worded by the UI: `chat.<tool>` (`chat.cutRange {startUs, endUs}`), and
//! `chat.andMore {first, more}` for a step of several calls. Splitting by concern: `additions`, `removals`, `trim`
//! (both), `map`.

mod map;
mod removals;

use std::collections::HashMap;

use klipolo_model::coded::{CodedError, ErrorParam};
use klipolo_model::ops::{Op, Origin, Transaction};
use klipolo_model::risk::Review;
use klipolo_model::speech::Speech;
use klipolo_model::time::{RangeUs, Us};
use klipolo_model::{ModelError, Project, finish_parts};

use crate::context::{Refs, SceneLog, View, secs};
use crate::evidence::{Evidence, Unfounded};
use crate::tools::{Moment, ToolCall};
use map::Map;

/// What the person is told when a tool needs a clip's analysis and Klipolo is still making it.
pub const STILL_LISTENING: &str = "Klipolo is still listening to this clip";
/// What the person is told when a tool needs a clip's analysis and none was started.
pub const ANALYZE_FIRST: &str = "Analyze this clip first";

/// How far a file's analysis (transcript, loudness, scene log) is. A file missing from `Inputs::analysis` counts as
/// not started.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnalysisState {
    NotStarted,
    Transcribing,
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
}

/// What one message's calls compiled to.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Compiled {
    /// Applied at once, one undo step.
    pub additions: Option<Transaction>,
    /// Shown for the person to apply (one undo step) or discard, against the project once `additions` are applied.
    pub proposal: Option<Proposal>,
    /// Timeline ranges to point the person at, on the project once `additions` are applied.
    pub moments: Vec<Moment>,
    pub jobs: Vec<Job>,
    /// `askUser`'s questions.
    pub questions: Vec<String>,
    /// The calls that compiled to nothing, by call index.
    pub refusals: Vec<Refusal>,
    /// The model asked to undo the last change.
    pub undo: bool,
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

/// Work for a runner that tracks or blurs (Plan 5), named by one short noun phrase.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Job {
    Follow { description: String },
    Blur { description: String },
}

/// A call that compiled to nothing, and why, in one plain sentence on one line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refusal {
    pub call_index: usize,
    pub reason: String,
    pub audience: Audience,
}

/// Who a refusal's reason is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Audience {
    /// Sent back to the model as the call's result, for it to try again or explain.
    Model,
    /// Shown to the person as it is, never retried: `cannotDo`, and a clip that isn't analyzed yet.
    Person,
}

/// Compiles `calls` against `project`, the snapshot `inputs` come from.
pub fn compile(project: &Project, inputs: &Inputs, calls: &[ToolCall]) -> Compiled {
    let c = Compiler {
        snapshot: project,
        evidence: Evidence::new(project, inputs),
        map: Map::new(project),
    };
    c.run(calls)
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
}

/// Records that call `call_index` compiled to nothing.
fn refuse(out: &mut Compiled, call_index: usize, r: Refused) {
    out.refusals.push(Refusal {
        call_index,
        reason: r.reason,
        audience: r.audience,
    });
}

/// A reason for the model.
fn to_model(reason: String) -> Refused {
    Refused {
        reason,
        audience: Audience::Model,
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
    labels: Vec<CodedError>,
}

impl Phase {
    fn new(start: Project) -> Self {
        Self {
            work: start.clone(),
            done: start,
            ops: Vec::new(),
            labels: Vec::new(),
        }
    }

    /// Adds `edit` if its ops apply and the batch still ends valid; else the working copy stays as it was. An edit
    /// with no ops (a format the video already has) adds nothing.
    fn add(&mut self, edit: Edit) -> Result<(), ModelError> {
        if edit.ops.is_empty() {
            return Ok(());
        }
        let mut work = self.work.clone();
        klipolo_model::apply_part(&mut work, &edit.ops)?;
        let mut done = work.clone();
        finish_parts(&mut done)?;
        self.work = work;
        self.done = done;
        self.ops.extend(edit.ops);
        self.labels.push(edit.label);
        Ok(())
    }

    /// The phase's ops as one chat transaction, labelled for its first call and how many more.
    fn transaction(&self) -> Option<Transaction> {
        let (label, params) = match self.labels.as_slice() {
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

/// A call waiting for the removals' phase.
enum Removal<'c> {
    Call(&'c ToolCall),
}

struct Compiler<'a> {
    snapshot: &'a Project,
    evidence: Evidence,
    map: Map<'a>,
}

impl Compiler<'_> {
    fn run(&self, calls: &[ToolCall]) -> Compiled {
        let mut out = Compiled::default();
        // In call order: the additions compile now; removals wait for the project the additions make.
        let mut adding = Phase::new(self.snapshot.clone());
        let mut waiting: Vec<(usize, Removal)> = Vec::new();
        for (i, call) in calls.iter().enumerate() {
            let addition = match call {
                ToolCall::CutRange { .. } | ToolCall::KeepOnly { .. } => {
                    waiting.push((i, Removal::Call(call)));
                    continue;
                }
                other => Err(to_model(format!("{}: not available yet.", other.name()))),
            };
            let added = addition.and_then(|edit| {
                (adding.add(edit)).map_err(|e| to_model(format!("{}: {e}", call.name())))
            });
            if let Err(r) = added {
                refuse(&mut out, i, r);
            }
        }

        // The project as the additions leave it: what the app applies them to, and what the proposal starts from.
        let with_additions = adding.done.clone();
        // The removals in call order, on top.
        let mut removing = Phase::new(with_additions);
        for (i, removal) in waiting {
            let work = &removing.work;
            let (call_name, edit) = match removal {
                Removal::Call(call) => (call.name(), removals::compile(self, work, call)),
            };
            let added = edit.and_then(|edit| {
                (removing.add(edit)).map_err(|e| to_model(format!("{call_name}: {e}")))
            });
            if let Err(r) = added {
                refuse(&mut out, i, r);
            }
        }

        out.additions = adding.transaction();
        if let Some(tx) = removing.transaction() {
            let review = tx.review_applied(&adding.done, removing.done);
            let ranges = review.removals.ranges.clone();
            out.proposal = Some(Proposal {
                removed_us: ranges.iter().map(RangeUs::len).sum(),
                ranges,
                tx,
                review,
            });
        }
        out.refusals.sort_by_key(|r| r.call_index);
        out
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
