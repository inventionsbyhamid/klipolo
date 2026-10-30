//! A project plus bounded undo/redo. Each successful transaction is one undo step. A step holds the reverse patch
//!, not a copy of the project, and every edit, undo and redo reports the forward patch it made.

use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::apply::apply_ops;
use crate::coded::ErrorParam;
use crate::error::Result;
use crate::ops::Transaction;
use crate::patch::{self, diff, ProjectPatch};
use crate::project::Project;
use crate::validate::validate;

mod snapshot;
pub use snapshot::{Snapshot, SnapshotPatch};

/// Most undo steps kept; the oldest is dropped first.
pub const HISTORY_LIMIT: usize = 200;
/// The history's memory budget (≤ 64 MB for a 1-hour, 300-layer project).
pub const HISTORY_BYTES: usize = 64 << 20;
/// Steps kept while the system is short of memory (from the warning level).
pub const PRESSURE_STEPS: usize = 50;

/// How much history a [`Document`] keeps: at most `steps` undo and redo steps together, holding at most `bytes` (the
/// patches' JSON length). Apps' steps go first; the person's next undo and next redo are always kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HistoryLimits {
    pub steps: usize,
    pub bytes: usize,
}

impl Default for HistoryLimits {
    fn default() -> Self {
        Self {
            steps: HISTORY_LIMIT,
            bytes: HISTORY_BYTES,
        }
    }
}

impl HistoryLimits {
    pub fn under_pressure() -> Self {
        Self {
            steps: PRESSURE_STEPS,
            bytes: HISTORY_BYTES / 4,
        }
    }
}

/// What one change did: the revision it was made on, and how to get from there to now.
#[derive(Clone, Debug, PartialEq)]
pub struct Change {
    pub base_revision: u64,
    pub patch: ProjectPatch,
}

impl Change {
    /// Nothing changed at `revision`.
    pub fn none(revision: u64) -> Self {
        Self {
            base_revision: revision,
            patch: ProjectPatch::default(),
        }
    }
}

/// A label's values (`Transaction::params`).
pub type LabelParams = BTreeMap<String, ErrorParam>;

/// What the UI needs for its Undo/Redo menu items ("Undo Add title"): the labels are message ids, worded with their
/// params in the language shown.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct HistoryState {
    pub revision: u64,
    pub can_undo: bool,
    pub can_redo: bool,
    pub undo_label: Option<String>,
    pub redo_label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub undo_params: Option<LabelParams>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub redo_params: Option<LabelParams>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stack {
    Undo,
    Redo,
}

/// Who made a history step: the person (by hand, through chat, by applying a proposal
/// an app made, or by confirming a restore), or another app whose trust level applied its batch at once, by the name it
/// gave. It is stored on the step and kept through undo, redo and folds; what the apps' shares count, what goes first
/// when the history must shrink, and what may fold together all follow it. A step's label is presentation only.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum Maker {
    #[default]
    Person,
    App(String),
}

impl Maker {
    pub fn is_app(&self) -> bool {
        matches!(self, Self::App(_))
    }
}

/// One undo or redo step: the patch that takes the project back (or forward) over it, its label, and who made it.
#[derive(Debug)]
struct Step {
    label: String,
    params: Option<LabelParams>,
    patch: ProjectPatch,
    bytes: usize,
    /// Which change this is, the same while it moves between undo and redo: a later change has a larger number.
    seq: u64,
    /// What it may hold either way, undone or redone: the larger of its two patches, and a folded step at least both its
    /// parts. A [`Share`] is charged so (undoing an addition stores the added layers whole).
    charge: usize,
    /// Who made it: only steps of one maker ever fold together.
    maker: Maker,
    /// No fold ever joins this step with the one made right after it: a restore point was taken, or the person kept an
    /// app's changes, there.
    sealed: bool,
}

impl Step {
    /// `patch` as change `seq` by `maker`, whose patch the other way holds `other` bytes.
    fn new(
        (label, params): (String, Option<LabelParams>),
        patch: ProjectPatch,
        seq: u64,
        other: usize,
        maker: Maker,
    ) -> Self {
        let bytes = patch.approx_bytes() + overhead(&label, params.as_ref());
        Self {
            label,
            params,
            patch,
            bytes,
            seq,
            charge: bytes.max(other),
            maker,
            sealed: false,
        }
    }

    /// This step the other way, undone or redone: `patch` takes the project back over it, and the patch it held comes
    /// back. Its label, number, maker, seal and charge stay.
    fn turned(mut self, patch: ProjectPatch) -> (Self, ProjectPatch) {
        let held = std::mem::replace(&mut self.patch, patch);
        self.bytes = self.patch.approx_bytes() + overhead(&self.label, self.params.as_ref());
        self.charge = self.bytes.max(self.charge);
        (self, held)
    }
}

/// What a label and its params add to a patch's bytes.
fn overhead(label: &str, params: Option<&LabelParams>) -> usize {
    label.len() + params.map_or(0, params_bytes)
}

/// Where the history was at a moment, to tell later which steps were made since: the newest
/// undo step then, and how many undo steps had been dropped (the limits, or a cleared history).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryMark {
    pub top: Option<u64>,
    pub dropped: u64,
}

/// One undo step as callers see it: its label and params (presentation only) and who made it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StepView<'a> {
    pub label: &'a str,
    pub params: Option<&'a LabelParams>,
    pub maker: &'a Maker,
}

impl<'a> From<&'a Step> for StepView<'a> {
    fn from(s: &'a Step) -> Self {
        Self {
            label: &s.label,
            params: s.params.as_ref(),
            maker: &s.maker,
        }
    }
}

/// Which steps a [`Share`] covers, by who made them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Whose<'a> {
    /// One app's, by the name it gave.
    App(&'a str),
    /// Every app's together.
    Apps,
}

impl Whose<'_> {
    pub fn covers(self, maker: &Maker) -> bool {
        match (self, maker) {
            (_, Maker::Person) => false,
            (Whose::Apps, Maker::App(_)) => true,
            (Whose::App(name), Maker::App(app)) => app == name,
        }
    }
}

/// A share of the history that one app's steps, or all apps' together, may hold: at most `steps` of them, and `bytes`,
/// each charged at the larger of its undo and redo patches, a folded step at the sum of its parts (MCP security
/// rules: counted by maker, never by label).
pub struct Share<'a> {
    pub of: Whose<'a>,
    pub steps: usize,
    pub bytes: usize,
}

/// How a step folded from one app's own steps is labelled, by the app's name ("<app>'s earlier changes"): set by the
/// host ([`Document::set_folded_label`]). Presentation only.
pub type FoldedLabel = fn(&str) -> (String, Option<LabelParams>);

/// An app's batch to commit ([`Document::commit_tx_keeping`]).
pub struct ByApp<'a> {
    /// The app, by the name it gave: the step's maker.
    pub app: &'a str,
    /// What its steps, and all apps' together, may hold.
    pub shares: &'a [Share<'a>],
    /// A restore point was taken just before this batch, where the history is now: no fold ever joins the newest undo
    /// step with this one, so the point's mark stays reachable.
    pub after_point: bool,
}

/// Why [`Document::commit_tx_keeping`] didn't commit. The project it was given comes back; the history is as it was.
#[derive(Debug)]
pub enum Refused {
    /// It would push out a step another made: the person's, or another app's.
    PushesOut(Box<Project>),
    /// The steps of `shares[n]`, this one included, would hold more than that share.
    OverShare(usize, Box<Project>),
}

/// Why making room for an app's step failed, before the project is handed back.
enum NoRoom {
    PushesOut,
    OverShare(usize),
}

/// What making room for an app's step did to the undo stack, so a refused commit puts all of it back.
enum Undone {
    /// Steps `at` and `at + 1` folded into one, at `at`.
    Folded {
        at: usize,
        far: Box<Step>,
        near: Box<Step>,
    },
    /// The oldest undo step went.
    Dropped(Box<Step>),
}

/// About what a label's params hold, for the history's memory budget.
fn params_bytes(params: &LabelParams) -> usize {
    fn value(v: &ErrorParam) -> usize {
        match v {
            ErrorParam::Number(_) => 8,
            ErrorParam::Text(t) => t.len(),
            ErrorParam::Coded(e) => e.code.len() + params_bytes(&e.params),
        }
    }
    params.iter().map(|(k, v)| k.len() + value(v)).sum()
}

/// The open project. Every edit goes through [`Document::apply`]; `revision` increases on each
/// change (apply, undo, redo, replace) so observers can tell the project changed.
///
/// The project is held in an `Arc`, made once per revision: the engine and the background save share it
/// ([`Document::shared`]) instead of each taking a copy.
#[derive(Debug)]
pub struct Document {
    project: Arc<Project>,
    revision: u64,
    /// Oldest first; the next undo is at the back.
    undo: VecDeque<Step>,
    /// Furthest first; the next redo is at the back.
    redo: VecDeque<Step>,
    /// Both stacks' `bytes`, summed.
    bytes: usize,
    limits: HistoryLimits,
    /// The next change's [`Step::seq`].
    next_seq: u64,
    /// Undo steps dropped so far (the limits, a cleared history).
    dropped: u64,
    /// How a folded app step is labelled ([`Document::set_folded_label`]); without one, it keeps the newer part's.
    folded: Option<FoldedLabel>,
}

impl Document {
    pub fn new(project: Project) -> Result<Self> {
        validate(&project)?;
        Ok(Self {
            project: Arc::new(project),
            revision: 0,
            undo: VecDeque::new(),
            redo: VecDeque::new(),
            bytes: 0,
            limits: HistoryLimits::default(),
            next_seq: 0,
            dropped: 0,
            folded: None,
        })
    }

    /// How a step folded from one app's own steps is labelled ("<app>'s earlier changes", by the app's name).
    pub fn set_folded_label(&mut self, label: FoldedLabel) {
        self.folded = Some(label);
    }

    pub fn project(&self) -> &Project {
        &self.project
    }

    /// The project of this revision, shared (no copy): what the engine shows and the save writes.
    pub fn shared(&self) -> Arc<Project> {
        Arc::clone(&self.project)
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Apply `tx` as one undo step labelled `tx.label`, made by the person. A failing batch returns its error and a
    /// batch that changes nothing returns the current revision; neither records a step.
    pub fn apply(&mut self, tx: &Transaction) -> Result<u64> {
        self.apply_change(tx).map(|_| self.revision)
    }

    /// [`Document::apply`], returning what changed.
    pub fn apply_change(&mut self, tx: &Transaction) -> Result<Change> {
        if tx.ops.is_empty() {
            return Ok(Change::none(self.revision));
        }
        let next = apply_ops(&self.project, &tx.ops)?;
        Ok(self.commit_tx(tx, next))
    }

    /// Record `next` as one undo step made by the person, labelled with `tx`'s label and params, without applying any
    /// ops: chat commits the project its review made, and a proposal the person applied is theirs, whoever made it.
    /// `next` must be what [`apply_ops`] made from the current project with `tx.ops`, so it is valid. An
    /// unchanged project records nothing.
    pub fn commit_tx(&mut self, tx: &Transaction, next: Project) -> Change {
        self.record((tx.label.clone(), tx.params.clone()), next)
    }

    /// An app's batch, which its level applies at once, as one undo step it made (`by.app`). Room is made only from the
    /// app's own steps: past a share's steps, its own oldest steps that sit next to each other fold into one (labelled
    /// as [`Document::set_folded_label`] says, charged both its parts, never across a seal); past the history's limits,
    /// judged with every app step at its charge, its own steps fold (by count) or its own oldest step
    /// goes (at the oldest end). It is refused when making room
    /// would push out another's step (the person's or another app's, in undo or, since any commit drops them, in redo)
    /// or would make a share it belongs to hold more than its steps or bytes (bytes are never
    /// made room for). A refused commit changes nothing, its folds included, and `next`
    /// comes back. O(steps × shares), plus a patch composed per fold.
    pub fn commit_tx_keeping(
        &mut self,
        tx: &Transaction,
        next: Project,
        by: &ByApp<'_>,
    ) -> std::result::Result<Change, Refused> {
        let forward = diff(&self.project, &next);
        if forward.is_empty() && next == *self.project {
            return Ok(Change::none(self.revision));
        }
        let maker = Maker::App(by.app.to_owned());
        let refused = |why: NoRoom, next: Project| match why {
            NoRoom::PushesOut => Refused::PushesOut(Box::new(next)),
            NoRoom::OverShare(n) => Refused::OverShare(n, Box::new(next)),
        };
        if self.redo.iter().any(|s| s.maker != maker) {
            return Err(refused(NoRoom::PushesOut, next));
        }
        let other = forward.approx_bytes() + overhead(&tx.label, tx.params.as_ref());
        let step = Step::new(
            (tx.label.clone(), tx.params.clone()),
            diff(&next, &self.project),
            self.next_seq,
            other,
            maker,
        );
        let mut undone = Vec::new();
        if let Err(why) = self.make_room(&step, by.shares, &mut undone) {
            self.put_back(undone);
            return Err(refused(why, next));
        }
        self.project = Arc::new(next);
        self.clear_redo();
        if by.after_point {
            self.seal();
        }
        self.next_seq += 1;
        self.push(Stack::Undo, step);
        self.revision += 1;
        Ok(Change {
            base_revision: self.revision - 1,
            patch: forward,
        })
    }

    /// Makes room for `step`, an app's, from that app's own undo steps only, recording each change in `undone`: first its
    /// shares, then the limits as they will be once the redo steps go and `step` is the newest undo step.
    fn make_room(
        &mut self,
        step: &Step,
        shares: &[Share<'_>],
        undone: &mut Vec<Undone>,
    ) -> std::result::Result<(), NoRoom> {
        // Bytes over a share can't be made room for: a folded step is charged both its parts.
        for (n, share) in shares.iter().enumerate() {
            if let Some((_, held)) = self.held(share, step) {
                if held + step.charge > share.bytes {
                    return Err(NoRoom::OverShare(n));
                }
            }
        }
        // Steps over a share: the app's own oldest adjacent steps fold, when folding can make room for every share.
        let over = |doc: &Self| {
            shares.iter().position(|share| {
                doc.held(share, step)
                    .is_some_and(|(count, _)| count + 1 > share.steps)
            })
        };
        let need = shares
            .iter()
            .filter_map(|share| {
                let (count, _) = self.held(share, step)?;
                Some((count + 1).saturating_sub(share.steps))
            })
            .max()
            .unwrap_or(0);
        if need > self.pairs(&step.maker) {
            return Err(over(self).map_or(NoRoom::PushesOut, NoRoom::OverShare));
        }
        for _ in 0..need {
            let at = self.pair(Stack::Undo, Some(&step.maker));
            let Some(at) = at else { break };
            let (far, near) = self.fold(Stack::Undo, at);
            undone.push(Undone::Folded {
                at,
                far: Box::new(far),
                near: Box::new(near),
            });
        }
        if let Some(n) = over(self) {
            return Err(NoRoom::OverShare(n));
        }
        // The limits, as the history will be once the redo steps go and `step` is the newest undo step, with every app
        // step at its charge: what it holds once undone or redone, so undoing it later never needs room the person's steps
        // hold. The app's own steps fold (too many steps) or go from the oldest end; another's never.
        let reserved = |doc: &Self| {
            let held: usize = doc
                .undo
                .iter()
                .map(|s| if s.maker.is_app() { s.charge } else { s.bytes })
                .sum();
            held + step.charge
        };
        loop {
            let count = self.undo.len() + 1 > self.limits.steps;
            if !count && reserved(self) <= self.limits.bytes {
                return Ok(());
            }
            if count {
                if let Some(at) = self.pair(Stack::Undo, Some(&step.maker)) {
                    let (far, near) = self.fold(Stack::Undo, at);
                    undone.push(Undone::Folded {
                        at,
                        far: Box::new(far),
                        near: Box::new(near),
                    });
                    continue;
                }
            }
            if self.undo.front().is_some_and(|s| s.maker == step.maker) {
                if let Some(old) = self.drop_front(Stack::Undo) {
                    undone.push(Undone::Dropped(Box::new(old)));
                    continue;
                }
            }
            return Err(NoRoom::PushesOut);
        }
    }

    /// Puts back what [`Document::make_room`] did, newest first: the history is as it was.
    fn put_back(&mut self, undone: Vec<Undone>) {
        for change in undone.into_iter().rev() {
            match change {
                Undone::Folded { at, far, near } => {
                    let merged = std::mem::replace(&mut self.undo[at], *far);
                    self.bytes = self.bytes + self.undo[at].bytes + near.bytes - merged.bytes;
                    self.undo.insert(at + 1, *near);
                }
                Undone::Dropped(old) => {
                    self.bytes += old.bytes;
                    self.dropped -= 1;
                    self.undo.push_front(*old);
                }
            }
        }
    }

    /// How many undo steps `share` holds, and their charge, when `step` belongs to it (else None). O(steps).
    fn held(&self, share: &Share<'_>, step: &Step) -> Option<(usize, usize)> {
        share.of.covers(&step.maker).then(|| {
            self.undo
                .iter()
                .filter(|s| share.of.covers(&s.maker))
                .fold((0, 0), |(n, b), s| (n + 1, b + s.charge))
        })
    }

    /// How many times `maker`'s undo steps could fold: pairs of its steps next to each other, no seal between.
    fn pairs(&self, maker: &Maker) -> usize {
        self.undo
            .iter()
            .zip(self.undo.iter().skip(1))
            .filter(|(a, b)| a.maker == *maker && b.maker == *maker && !a.sealed)
            .count()
    }

    /// Where in `stack` the first two steps of one app sit next to each other with no seal between (`maker`'s only, when
    /// given): the oldest pair in undo, the furthest (the newest made) in redo. Never the person's.
    fn pair(&self, stack: Stack, maker: Option<&Maker>) -> Option<usize> {
        let steps = self.stack(stack);
        (0..steps.len().saturating_sub(1)).find(|&i| {
            let (a, b) = (&steps[i], &steps[i + 1]);
            // The older of the two in time: a seal after it keeps them apart.
            let older = match stack {
                Stack::Undo => a,
                Stack::Redo => b,
            };
            a.maker.is_app()
                && a.maker == b.maker
                && maker.is_none_or(|m| *m == a.maker)
                && !older.sealed
        })
    }

    fn stack(&self, stack: Stack) -> &VecDeque<Step> {
        match stack {
            Stack::Undo => &self.undo,
            Stack::Redo => &self.redo,
        }
    }

    /// Folds steps `at` and `at + 1` of `stack` (one maker's, next to each other) into one at `at`, labelled as the host
    /// says, charged both its parts; the two come back. Undoing (or redoing) it does the nearer one first, then the
    /// further. It keeps the newer one's number and seal, so a mark at the newer one still reaches it. O(patch).
    fn fold(&mut self, stack: Stack, at: usize) -> (Step, Step) {
        let folded = self.folded;
        let steps = match stack {
            Stack::Undo => &mut self.undo,
            Stack::Redo => &mut self.redo,
        };
        let near = steps.remove(at + 1).expect("a step after `at`");
        let far = &steps[at];
        // In time, the newer of the two: the nearer in undo, the further in redo.
        let newer = match stack {
            Stack::Undo => &near,
            Stack::Redo => far,
        };
        let label = match (folded, &newer.maker) {
            (Some(label), Maker::App(app)) => label(app),
            _ => (newer.label.clone(), newer.params.clone()),
        };
        let mut merged = Step::new(
            label,
            patch::compose(&near.patch, &far.patch),
            newer.seq,
            far.charge + near.charge,
            newer.maker.clone(),
        );
        merged.sealed = newer.sealed;
        let added = merged.bytes;
        let far = std::mem::replace(&mut steps[at], merged);
        self.bytes = self.bytes + added - far.bytes - near.bytes;
        (far, near)
    }

    /// The step at the far end of `stack` goes: the oldest undo step (counted as dropped, for marks) or the furthest redo.
    fn drop_front(&mut self, stack: Stack) -> Option<Step> {
        let old = match stack {
            Stack::Undo => self.undo.pop_front()?,
            Stack::Redo => self.redo.pop_front()?,
        };
        self.bytes -= old.bytes;
        if stack == Stack::Undo {
            self.dropped += 1;
        }
        Some(old)
    }

    /// The person kept an app's changes here (or a restore point was taken): no fold ever joins the newest undo step with
    /// a later one, so Undo never takes back past this state in one step, and a mark here stays reachable.
    pub fn seal(&mut self) {
        if let Some(top) = self.undo.back_mut() {
            top.sealed = true;
        }
    }

    /// Where the history is now, for [`Document::since`].
    pub fn mark(&self) -> HistoryMark {
        HistoryMark {
            top: self.undo.back().map(|s| s.seq),
            dropped: self.dropped,
        }
    }

    /// The undo steps made since `mark` (this document's), oldest first; None when the history no longer reaches back to
    /// it (the newest step then was undone, dropped or folded into a later one, or with none then, any undo step was
    /// dropped since). Undoing these exactly gives the project as it was at `mark`. O(steps).
    pub fn since(&self, mark: HistoryMark) -> Option<Vec<StepView<'_>>> {
        let from = match mark.top {
            Some(top) => self.undo.iter().position(|s| s.seq == top)? + 1,
            None if self.dropped == mark.dropped => 0,
            None => return None,
        };
        Some(self.undo.range(from..).map(StepView::from).collect())
    }

    /// Each undo step's label and params, oldest first.
    pub fn undo_labels(&self) -> impl Iterator<Item = (&str, Option<&LabelParams>)> {
        self.undo
            .iter()
            .map(|s| (s.label.as_str(), s.params.as_ref()))
    }

    /// Each undo step, oldest first.
    pub fn undo_view(&self) -> impl Iterator<Item = StepView<'_>> {
        self.undo.iter().map(StepView::from)
    }

    /// Record `next` as one undo step made by the person, labelled `label`, without applying any ops again. `next` must
    /// be what [`apply_ops`] made from the current project (the caller already ran it, to save it or return it first),
    /// so it is valid. An unchanged project records nothing.
    pub fn commit(&mut self, label: &str, next: Project) -> u64 {
        self.commit_change(label, next);
        self.revision
    }

    /// [`Document::commit`], returning what changed. The label has no params.
    pub fn commit_change(&mut self, label: &str, next: Project) -> Change {
        self.record((label.to_string(), None), next)
    }

    fn record(&mut self, label: (String, Option<LabelParams>), next: Project) -> Change {
        let forward = diff(&self.project, &next);
        // The whole project is compared only when the patch is empty (a patch doesn't carry `id` or `version`).
        if forward.is_empty() && next == *self.project {
            return Change::none(self.revision);
        }
        let back = diff(&next, &self.project);
        let other = forward.approx_bytes() + overhead(&label.0, label.1.as_ref());
        self.project = Arc::new(next);
        self.clear_redo();
        let step = Step::new(label, back, self.next_seq, other, Maker::Person);
        self.next_seq += 1;
        self.push(Stack::Undo, step);
        self.revision += 1;
        Change {
            base_revision: self.revision - 1,
            patch: forward,
        }
    }

    pub fn undo(&mut self) -> Option<u64> {
        self.undo_change().map(|_| self.revision)
    }

    /// [`Document::undo`], returning what changed.
    pub fn undo_change(&mut self) -> Option<Change> {
        let step = self.undo.pop_back()?;
        self.bytes -= step.bytes;
        let Ok(previous) = patch::apply(&self.project, &step.patch) else {
            // Can't happen for patches this document made; stay consistent rather than half-undo.
            self.clear_history();
            return None;
        };
        let forward = diff(&previous, &self.project);
        self.project = Arc::new(previous);
        let (redo, patch) = step.turned(forward);
        self.push(Stack::Redo, redo);
        self.revision += 1;
        Some(Change {
            base_revision: self.revision - 1,
            patch,
        })
    }

    pub fn redo(&mut self) -> Option<u64> {
        self.redo_change().map(|_| self.revision)
    }

    /// [`Document::redo`], returning what changed.
    pub fn redo_change(&mut self) -> Option<Change> {
        let step = self.redo.pop_back()?;
        self.bytes -= step.bytes;
        let Ok(next) = patch::apply(&self.project, &step.patch) else {
            self.clear_history();
            return None;
        };
        let back = diff(&next, &self.project);
        self.project = Arc::new(next);
        let (undo, patch) = step.turned(back);
        self.push(Stack::Undo, undo);
        self.revision += 1;
        Some(Change {
            base_revision: self.revision - 1,
            patch,
        })
    }

    /// Swap in a whole project (open, revert). Validates it and clears history.
    pub fn replace(&mut self, project: Project) -> Result<u64> {
        validate(&project)?;
        self.project = Arc::new(project);
        self.clear_history();
        self.revision += 1;
        Ok(self.revision)
    }

    /// New limits apply at once ([`Document::enforce`]'s order: apps' steps first).
    pub fn set_limits(&mut self, limits: HistoryLimits) {
        self.limits = limits;
        self.enforce();
    }

    pub fn undo_steps(&self) -> usize {
        self.undo.len()
    }

    /// The undo steps whose maker `mine` accepts (an app's own steps, for its share of the history). O(steps).
    pub fn undo_steps_where(&self, mine: impl Fn(&Maker) -> bool) -> usize {
        self.undo.iter().filter(|s| mine(&s.maker)).count()
    }

    pub fn redo_steps(&self) -> usize {
        self.redo.len()
    }

    /// What the undo and redo steps hold: their patches' [`ProjectPatch::approx_bytes`] plus their labels and params.
    pub fn history_bytes(&self) -> usize {
        self.bytes
    }

    pub fn history(&self) -> HistoryState {
        HistoryState {
            revision: self.revision,
            can_undo: !self.undo.is_empty(),
            can_redo: !self.redo.is_empty(),
            undo_label: self.undo.back().map(|s| s.label.clone()),
            redo_label: self.redo.back().map(|s| s.label.clone()),
            undo_params: self.undo.back().and_then(|s| s.params.clone()),
            redo_params: self.redo.back().and_then(|s| s.params.clone()),
        }
    }

    fn push(&mut self, stack: Stack, step: Step) {
        self.bytes += step.bytes;
        match stack {
            Stack::Undo => self.undo.push_back(step),
            Stack::Redo => self.redo.push_back(step),
        }
        self.enforce();
    }

    /// Folds or drops steps until undo and redo together fit the limits:
    /// 1. apps' steps go first, those in redo (the newest made first), then those in undo (the oldest first): one folds
    ///    into the same app's step next to it (never across a seal), for too many steps or for bytes (a folded patch holds
    ///    each item once, so steps that touch the same items shrink); where none can, one at the far
    ///    end of its stack goes;
    /// 2. then the person's: the step furthest from the current revision, the oldest undo or the furthest redo, whichever
    ///    stack is longer (undo on a tie), but their redo only once no app's step is left; the person's next undo and
    ///    next redo always stay.
    ///
    /// A fold never mixes makers: where only that would make room, the far end goes instead (an app's step alone between
    /// the person's goes once it reaches the oldest end). What is left still chains from the current project.
    fn enforce(&mut self) {
        loop {
            let count = self.undo.len() + self.redo.len() > self.limits.steps;
            if !count && self.bytes <= self.limits.bytes {
                return;
            }
            let shed =
                self.shed_apps(Stack::Redo) || self.shed_apps(Stack::Undo) || self.shed_persons();
            if !shed {
                return;
            }
        }
    }

    /// One app's step of `stack` goes: a pair of one app's steps folds; else one at the far end goes. True when one did.
    fn shed_apps(&mut self, stack: Stack) -> bool {
        if let Some(at) = self.pair(stack, None) {
            self.fold(stack, at);
            return true;
        }
        let far = self.stack(stack).front().is_some_and(|s| s.maker.is_app());
        far && self.drop_front(stack).is_some()
    }

    /// One of the person's steps goes, furthest from the current revision first, but their redo only once no app's step
    /// is left (one can be left alone between the person's undo steps until it reaches the oldest end). True when one
    /// did.
    fn shed_persons(&mut self) -> bool {
        let (undo, redo) = (self.undo.len(), self.redo.len());
        let apps = self.undo.iter().chain(&self.redo).any(|s| s.maker.is_app());
        let stack = if undo > 1 && (apps || redo <= 1 || redo <= undo) {
            Stack::Undo
        } else if redo > 1 {
            Stack::Redo
        } else {
            return false;
        };
        self.drop_front(stack).is_some()
    }

    fn clear_redo(&mut self) {
        self.bytes -= self.redo.iter().map(|s| s.bytes).sum::<usize>();
        self.redo.clear();
    }

    fn clear_history(&mut self) {
        // At least one: a failed undo already took its step off the stack.
        self.dropped += self.undo.len() as u64 + 1;
        self.undo.clear();
        self.redo.clear();
        self.bytes = 0;
    }
}
