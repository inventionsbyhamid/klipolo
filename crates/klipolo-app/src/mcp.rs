//! Other apps edit the open project (`klipolo_mcp`). The app is the server's host (`AppHost`): over the
//! project session (the one apply path), chat's one proposal slot and the saved credentials. `McpState` runs the
//! server while "Let other apps edit with Klipolo" is on, and the page is told what apps did.
//!
//! - **Commits:** an app's batch commits through `project::commit_by_app` as one undo step the app made, under the
//!   slot's lock, taken before the project's: a proposal that waits, or is being applied, still refuses another's
//!   change, and the base, the person's redo and the history's room are checked in one step with the commit. An app
//!   undoes only its own newest step.
//! - **Revisions:** apps read one number that only grows across the project's openings (`Revisions`), so a revision
//!   read on one project never names another's: a new opening starts its document's revisions at 0 again.
//! - **Proposals:** an app's waits in chat's slot like chat's (`SlotGuard::propose`), on the same card. Apply and
//!   Discard are the person's (`chat_apply_proposal`, `chat_discard_proposal`), and what became of it is kept for the
//!   app. Moments an app points at never touch the slot.
//! - **Jobs that cost money** start only through `start_paid`, which answers at once: no key or code is `NoCredit`, an
//!   access code's credit below the estimate (or turned off) `LowCredit`. A picture is asked for as chat asks for one,
//!   its prompt cleaned and capped, and drawn when the page asks. Following a subject runs as chat's does
//!   (`follow_subject`): tracked, then landed as one undo step the app made; blurring answers as chat does.
//! - **Events:** `mcp-reply` (`ExternalReply`) for each change, proposal, moments or picture, told once every lock is
//!   let go; `mcp-apps` with the apps connected.

use std::io;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use klipolo_app::analysis::AnalysisState;
use klipolo_app::chat::follow::{NO_CROP, NO_ONE_THERE, phrase_for};
use klipolo_app::chat::{
    At, ChatReply, ChatSession, Decided, ExternalReply, Findings, Gone, PictureAsk, Projects,
    Waiting, inputs_for, picture_for,
};
use klipolo_app::credentials::{AiStatus, Credentials};
use klipolo_app::mcp_config::{config_json, needs_move};
use klipolo_app::prefs;
use klipolo_app::speaker::SpeakerEstimate;
use klipolo_chat::{Job, Moment, Proposal, ToolCall};
use klipolo_mcp::connection::{self, McpServer};
use klipolo_mcp::host::{Applied, Host, HostError, OpenProposal, ProposalOutcome, Snapshot};
use klipolo_mcp::{Mcp, words};
use klipolo_model::Transaction;
use klipolo_model::coded::{CodedError, ErrorParam};
use klipolo_model::text_safety;
use klipolo_model::time::Us;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::runtime::Handle;
use ts_rs::TS;

use crate::project::{AppRefused, ProjectSessionState, commit_by_app, undo_by_app};
use crate::view::ViewHandle;

#[cfg(test)]
mod tests;

/// The event each thing an app did goes out on, as an `ExternalReply`.
pub(crate) const REPLY_EVENT: &str = "mcp-reply";
/// The event the apps connected go out on, by name, sorted.
pub(crate) const APPS_EVENT: &str = "mcp-apps";
/// What Settings says when the server couldn't start listening.
pub(crate) const START_FAILED: &str = "Klipolo couldn't start listening for other apps. Try again.";
/// What Settings says when another Klipolo on this Mac already lets other apps in (it serves the connection file).
pub(crate) const ANOTHER_LISTENS: &str = "Another Klipolo on this Mac is already letting other apps in. Quit it, then turn this on again.";
/// The most characters of an app's picture prompt passed on to the picture maker (chat's tools ask for a short
/// phrase).
pub(crate) const PROMPT_CHARS: usize = 200;
/// How long `start_paid` waits for the relay to say what an access code's credit is.
const CREDIT_CHECK: Duration = Duration::from_secs(5);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What the page is told.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Told {
    Reply(Box<ExternalReply>),
    Apps(Vec<String>),
}

/// Where the page is told: the app's events, or a test's list.
pub(crate) type Tell = Arc<dyn Fn(Told) + Send + Sync>;

/// Where the host finds the project session: the app's managed state, or a test's own.
pub(crate) trait SessionOf: Send + Sync + 'static {
    fn session(&self) -> impl Deref<Target = ProjectSessionState> + '_;
}

impl SessionOf for AppHandle {
    fn session(&self) -> impl Deref<Target = ProjectSessionState> + '_ {
        self.state::<ProjectSessionState>()
    }
}

impl SessionOf for Arc<ProjectSessionState> {
    fn session(&self) -> impl Deref<Target = ProjectSessionState> + '_ {
        &**self
    }
}

/// The revisions apps read: the open document's revision on top of a base that only grows with each opening, so the
/// number read on one opening (a project, or the same one opened again) never stands for another's, whose own
/// revisions start at 0 again. The first opening seen starts at 1; an opening older than the newest seen reads 0, which
/// stands for none.
#[derive(Default)]
pub(crate) struct Revisions(Mutex<Epoch>);

#[derive(Default)]
struct Epoch {
    /// The newest opening seen, and the number its revision 0 reads as.
    opening: u64,
    base: u64,
    /// The largest number read so far.
    top: u64,
}

impl Revisions {
    /// The number apps read for the project at `at`.
    pub(crate) fn of(&self, at: &At) -> u64 {
        let mut epoch = lock(&self.0);
        if at.opening < epoch.opening {
            return 0;
        }
        if at.opening > epoch.opening {
            let base = epoch.top + 1;
            *epoch = Epoch {
                opening: at.opening,
                base,
                top: base,
            };
        }
        let read = epoch.base.saturating_add(at.revision);
        epoch.top = epoch.top.max(read);
        read
    }
}

/// The app, as the MCP server's host.
pub(crate) struct AppHost<S> {
    projects: S,
    chat: Arc<ChatSession>,
    findings: Arc<dyn Findings>,
    credentials: Arc<Credentials>,
    view: ViewHandle,
    tell: Tell,
    revisions: Revisions,
    /// The app's runtime, to ask the relay for an access code's credit from the server's blocking thread.
    rt: Handle,
}

impl<S: SessionOf> AppHost<S> {
    pub(crate) fn new(
        projects: S,
        chat: Arc<ChatSession>,
        findings: Arc<dyn Findings>,
        credentials: Arc<Credentials>,
        view: ViewHandle,
        tell: Tell,
        rt: Handle,
    ) -> Self {
        Self {
            projects,
            chat,
            findings,
            credentials,
            view,
            tell,
            revisions: Revisions::default(),
            rt,
        }
    }

    /// Tells the page what `client` did on the project's `opening`.
    fn told(&self, client: &str, opening: u64, reply: ChatReply) {
        (self.tell)(Told::Reply(Box::new(ExternalReply {
            client: client.to_owned(),
            opening,
            reply,
        })));
    }

    /// Where the open project stands now, for a reply's moments and pictures.
    fn at_now(&self) -> Option<At> {
        Projects::now(&*self.projects.session()).map(|now| now.at)
    }

    /// What following `description` costs on the open project as it is: None without a speaker following or a project.
    fn follow_estimate(&self, description: &str) -> Option<SpeakerEstimate> {
        let speaker = self.chat.speaker()?;
        let now = Projects::now(&*self.projects.session())?;
        let p = &now.project;
        let ids: Vec<String> = p.timeline.main.iter().map(|c| c.id.clone()).collect();
        let phrase = phrase_for(description);
        Some(
            self.rt
                .block_on(speaker.estimate(p, &ids, &p.settings.formats, &phrase)),
        )
    }

    /// Follows `description` in every clip that can, in every format whose Fill crops it, as chat's `followSubject`
    /// does, and lands it as one undo step `client` made (told to the page). Waits for the tracking: SAM answers a
    /// short in under a minute. Refused with the sentence chat would say.
    fn follow_subject(&self, client: &str, description: &str) -> Result<(), HostError> {
        let speaker = self.chat.speaker().ok_or(HostError::NoCredit)?;
        let estimate = self.follow_estimate(description).ok_or(HostError::Moved)?;
        if estimate.clips.is_empty() {
            return Err(HostError::Refused(NO_CROP.to_owned()));
        }
        if let Some(refusal) = estimate.refusal {
            return Err(HostError::Refused(refusal));
        }
        let now = Projects::now(&*self.projects.session()).ok_or(HostError::Moved)?;
        let p = now.project.clone();
        let ids: Vec<String> = p.timeline.main.iter().map(|c| c.id.clone()).collect();
        let phrase = phrase_for(description);
        let followed = (self.rt)
            .block_on(speaker.follow(&p, &ids, &p.settings.formats, &phrase))
            .map_err(HostError::Refused)?;
        let tx = {
            let session = self.projects.session();
            let ops = Projects::now(&*session).map_or_else(Vec::new, |n| followed.ops(&n.project));
            if ops.is_empty() {
                return Err(HostError::Refused(NO_ONE_THERE.to_owned()));
            }
            Transaction {
                label: "chat.followSubject".into(),
                params: Some(
                    [("description".to_owned(), ErrorParam::from(phrase.as_str()))].into(),
                ),
                ops,
                origin: None,
            }
        };
        let done = {
            let mut slot = self.chat.slot();
            if someone_else(slot.waiting().as_ref(), client) {
                return Err(HostError::Moved);
            }
            let session = self.projects.session();
            let done = commit_by_app(&session, client, &tx, |_| true).map_err(host_error)?;
            let _ = slot.settle();
            done
        };
        let applied = reply()
            .applied(done.patch, undo_label(&tx))
            .on_revision(done.at.revision);
        self.told(client, done.at.opening, applied);
        Ok(())
    }

    /// Whether an access code's credit, as the relay says now, is below `usd` (or the code is turned off). Waits at
    /// most `CREDIT_CHECK`; a relay that doesn't say in time refuses the picture itself when the credit is gone.
    fn credit_short(&self, usd: f64) -> bool {
        let asked = self.rt.block_on(async {
            tokio::time::timeout(CREDIT_CHECK, self.credentials.status()).await
        });
        match asked {
            Ok(status) => short(&status, usd),
            Err(_) => {
                tracing::warn!(
                    "the relay didn't say the credit in time; the picture goes to the relay"
                );
                false
            }
        }
    }
}

/// Whether `status` shows an access code turned off, or with less credit left than `usd`.
fn short(status: &AiStatus, usd: f64) -> bool {
    let left = status.budget_usd.zip(status.spent_usd).map(|(b, s)| b - s);
    status.active == Some(false) || left.is_some_and(|left| left < usd)
}

/// Whether `waiting` is a proposal someone else made: Klipolo's chat, or another app.
fn someone_else(waiting: Option<&Waiting>, client: &str) -> bool {
    waiting.is_some_and(|w| w.from.as_deref() != Some(client))
}

/// A reply of the app's, with an id of its own (the page made none).
fn reply() -> ChatReply {
    ChatReply::new(uuid::Uuid::new_v4().to_string(), "")
}

/// `tx`'s undo label, as its step names it (an app's carries its name as `client`).
fn undo_label(tx: &Transaction) -> CodedError {
    CodedError {
        code: tx.label.clone(),
        params: tx.params.clone().unwrap_or_default(),
    }
}

fn host_error(refused: AppRefused) -> HostError {
    match refused {
        AppRefused::NoProject => HostError::NoProject,
        AppRefused::Moved => HostError::Moved,
        AppRefused::ReadOnly => HostError::ReadOnly,
        AppRefused::Removes => HostError::Refused(words::REMOVES.to_owned()),
        AppRefused::RedoWaits => HostError::RedoWaits,
        AppRefused::PushesOut => HostError::PushesOut,
        AppRefused::OverShare => HostError::OverShare,
        AppRefused::NotYours => HostError::NotYours,
        AppRefused::NothingToUndo => HostError::NothingToUndo,
        AppRefused::Invalid(why) => HostError::Refused(why),
    }
}

/// Why a proposal went, as an app reads it.
fn gone_said(gone: Gone) -> &'static str {
    match gone {
        Gone::Changed => words::DROPPED_CHANGED,
        Gone::Retimed => words::DROPPED_RETIMED,
        Gone::Replaced => words::DROPPED_REPLACED,
        Gone::NewChat => words::DROPPED_NEW_CHAT,
        Gone::Closed => words::DROPPED_CLOSED,
    }
}

fn secs(us: Us) -> f64 {
    us as f64 / 1e6
}

/// `ask` with its prompt as an app may give one to the picture maker: control, invisible and bidirectional characters
/// dropped, at most `PROMPT_CHARS`; None when nothing is left.
fn capped(ask: PictureAsk) -> Option<PictureAsk> {
    let clean = |prompt: &str| {
        Some(text_safety::clean(prompt, PROMPT_CHARS)).filter(|prompt| !prompt.is_empty())
    };
    Some(match ask {
        PictureAsk::Sticker {
            prompt,
            place,
            placement,
        } => PictureAsk::Sticker {
            prompt: clean(&prompt)?,
            place,
            placement,
        },
        PictureAsk::TitleBackground { prompt, title_id } => PictureAsk::TitleBackground {
            prompt: clean(&prompt)?,
            title_id,
        },
    })
}

impl<S: SessionOf> Host for AppHost<S> {
    fn snapshot(&self) -> Option<Snapshot> {
        let (now, pack) = {
            let session = self.projects.session();
            (Projects::now(&*session)?, Projects::pack(&*session))
        };
        // No message: an app's words are never the person's typed times. The person's live view is.
        let (inputs, turns) = inputs_for(
            &now.project,
            &*self.findings,
            "",
            self.view.chat_view(),
            pack,
        );
        Some(Snapshot {
            revision: self.revisions.of(&now.at),
            project: now.project,
            inputs,
            turns,
        })
    }

    fn open_proposal(&self) -> Option<OpenProposal> {
        let waiting = self.chat.slot().waiting()?;
        Some(OpenProposal {
            base_revision: self.revisions.of(&waiting.at),
            id: waiting.id,
            from: waiting.from,
        })
    }

    fn apply(&self, client: &str, base: u64, tx: &Transaction) -> Result<Applied, HostError> {
        let mut slot = self.chat.slot();
        if someone_else(slot.waiting().as_ref(), client) {
            return Err(HostError::Moved);
        }
        let done = {
            let session = self.projects.session();
            commit_by_app(&session, client, tx, |at| self.revisions.of(at) == base)
        }
        .map_err(host_error)?;
        // The change drops the proposal waiting, which can only be the app's own: the caller proposes its removals
        // again only when it was.
        let dropped = slot.settle();
        drop(slot);
        let revision = self.revisions.of(&done.at);
        if revision != base {
            let applied = reply()
                .applied(done.patch, undo_label(tx))
                .on_revision(done.at.revision);
            self.told(client, done.at.opening, applied);
        }
        Ok(Applied { revision, dropped })
    }

    fn propose(
        &self,
        client: &str,
        base: u64,
        calls: &[ToolCall],
        proposal: Proposal,
        replacing: Option<&str>,
    ) -> Result<String, HostError> {
        let mut slot = self.chat.slot();
        let waiting = slot.waiting();
        if someone_else(waiting.as_ref(), client)
            || waiting.as_ref().map(|w| w.id.as_str()) != replacing
        {
            return Err(HostError::Moved);
        }
        let now = Projects::now(&*self.projects.session()).ok_or(HostError::NoProject)?;
        if now.read_only {
            return Err(HostError::ReadOnly);
        }
        if self.revisions.of(&now.at) != base {
            return Err(HostError::Moved);
        }
        let tools: Vec<&str> = calls.iter().map(ToolCall::name).collect();
        let card = slot.propose(client, &now, &proposal, &tools);
        drop(slot);
        let id = card.id().to_owned();
        let proposed = reply().proposal(card).on_revision(now.at.revision);
        self.told(client, now.at.opening, proposed);
        Ok(id)
    }

    fn outcome(&self, client: &str) -> ProposalOutcome {
        let mut slot = self.chat.slot();
        if let Some(w) = slot.waiting() {
            return ProposalOutcome::Waiting {
                id: w.id,
                from: w.from.unwrap_or_else(|| words::KLIPOLO_CHAT.to_owned()),
                title: w.title,
                removed_sec: secs(w.removed_us),
                ranges: (w.ranges.iter())
                    .map(|r| [secs(r.start), secs(r.end)])
                    .collect(),
            };
        }
        match slot.decided(client) {
            None => ProposalOutcome::None,
            Some((id, Decided::Applied)) => ProposalOutcome::Applied { id },
            Some((id, Decided::Discarded)) => ProposalOutcome::Discarded { id },
            Some((id, Decided::Dropped(gone))) => ProposalOutcome::Dropped {
                id,
                why: gone_said(gone).to_owned(),
            },
        }
    }

    fn undo_own(&self, client: &str) -> Result<Applied, HostError> {
        let mut slot = self.chat.slot();
        if someone_else(slot.waiting().as_ref(), client) {
            return Err(HostError::Moved);
        }
        let done = undo_by_app(&self.projects.session(), client).map_err(host_error)?;
        let dropped = slot.settle();
        drop(slot);
        let undone = reply().undone(done.patch).on_revision(done.at.revision);
        self.told(client, done.at.opening, undone);
        Ok(Applied {
            revision: self.revisions.of(&done.at),
            dropped,
        })
    }

    fn show_moments(&self, client: &str, moments: &[Moment]) {
        let Some(at) = self.at_now() else {
            return;
        };
        let mut shown = reply().on_revision(at.revision);
        for moment in moments {
            shown = shown.moment(moment.range, &moment.label);
        }
        self.told(client, at.opening, shown);
    }

    fn estimate_usd(&self, job: &Job) -> f64 {
        match job {
            Job::MakeSticker(_) | Job::MakeTitleBackground { .. } => {
                klipolo_meta::pricing::image_usd(1)
            }
            Job::Follow { description } => self.follow_estimate(description).map_or(0.0, |e| e.usd),
            // Not in Klipolo yet: nothing runs, nothing is spent.
            Job::Blur { .. } => 0.0,
        }
    }

    fn start_paid(&self, client: &str, job: &Job) -> Result<(), HostError> {
        if let Job::Follow { description } = job
            && self.chat.speaker().is_some()
        {
            return self.follow_subject(client, description);
        }
        let ask = picture_for(job).map_err(|coming| HostError::Refused(coming.to_owned()))?;
        let ask = capped(ask).ok_or_else(|| HostError::Refused(words::NO_PROMPT.to_owned()))?;
        // No key, and no access code this Mac registered: nothing can pay.
        let Some(meta) = self.credentials.meta() else {
            return Err(HostError::NoCredit);
        };
        if meta.client().is_relay() && self.credit_short(self.estimate_usd(job)) {
            return Err(HostError::LowCredit);
        }
        let picture = self.chat.ask_picture(ask);
        let at = self.at_now();
        let asked = (reply().picture(picture)).on_revision(at.as_ref().map_or(0, |at| at.revision));
        self.told(client, at.map_or(0, |at| at.opening), asked);
        Ok(())
    }

    fn connected(&self, clients: &[String]) {
        (self.tell)(Told::Apps(clients.to_vec()));
    }
}

/// What Settings shows about other apps' access.
#[derive(Clone, Debug, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct McpStatus {
    /// Klipolo listens for other apps now.
    pub enabled: bool,
    /// The apps with a session open, by name, sorted: one that quit without saying so stays listed until its session
    /// goes unused for 10 minutes (`klipolo_mcp::session::IDLE`), so this reads as "recently", not "now".
    pub connected: Vec<String>,
    /// What "Copy config" puts on the clipboard (`mcp_config::config_json`).
    pub config: String,
    /// Klipolo runs from its disk image or a translocated copy: it has to move to Applications before the config can
    /// name it.
    pub needs_move: bool,
    /// Off because another Klipolo on this Mac lets other apps in now (it serves the connection file, a dev build
    /// beside the installed app): this one can't until that one quits.
    pub another_listens: bool,
}

/// The MCP server for the app's run: one `Mcp` (its paces, its allowance for jobs and an app's waiting proposal last
/// the run), listening while the person has other apps' access on.
pub(crate) struct McpState {
    mcp: Arc<Mcp>,
    /// Klipolo's support folder (`connection::support_dir`): the connection file and `prefs.json`. None without one.
    dir: Option<PathBuf>,
    rt: Handle,
    server: Mutex<Option<McpServer>>,
}

impl McpState {
    pub(crate) fn new(mcp: Arc<Mcp>, dir: Option<PathBuf>, rt: Handle) -> Self {
        Self {
            mcp,
            dir,
            rt,
            server: Mutex::new(None),
        }
    }

    /// At launch: listening again when the person left other apps' access on, unless another Klipolo lets them in
    /// now (it stays off for this run, and Settings says why); else no connection file is left behind (a Klipolo that
    /// crashed), unless another Klipolo running now wrote it (a dev build beside the installed app).
    pub(crate) fn at_launch(&self) {
        let Some(dir) = &self.dir else { return };
        if prefs::load(dir).mcp_enabled {
            match self.start() {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    tracing::info!(
                        "another Klipolo lets other apps in: this one stays off for this run"
                    );
                }
                Err(error) => tracing::warn!(%error, "the MCP server couldn't start at launch"),
            }
        } else {
            remove_left(dir);
        }
    }

    /// The switch: on starts listening then saves it, off stops (the connection file goes) then saves it. A server
    /// that can't start leaves access off and the saved switch as it was, with what Settings says: another Klipolo on
    /// this Mac letting apps in is said as such.
    pub(crate) fn set_enabled(&self, on: bool) -> Result<McpStatus, String> {
        let Some(dir) = self.dir.clone() else {
            return Err(START_FAILED.to_owned());
        };
        if on {
            self.start().map_err(|error| {
                if error.kind() == io::ErrorKind::AlreadyExists {
                    tracing::info!("another Klipolo lets other apps in: this one stays off");
                    return ANOTHER_LISTENS.to_owned();
                }
                tracing::warn!(%error, "the MCP server couldn't start");
                START_FAILED.to_owned()
            })?;
        } else {
            self.stop();
        }
        if let Err(error) = prefs::update(&dir, |p| p.mcp_enabled = on) {
            tracing::warn!(%error, "the MCP switch couldn't be saved");
        }
        Ok(self.status())
    }

    /// Listens, unless it already does; `AlreadyExists` while another Klipolo serves the connection file.
    fn start(&self) -> io::Result<()> {
        let dir = (self.dir.as_deref()).ok_or_else(|| io::Error::other("no support folder"))?;
        let mut server = lock(&self.server);
        if server.is_none() {
            *server = Some(McpServer::start(&self.rt, Arc::clone(&self.mcp), dir)?);
        }
        Ok(())
    }

    /// Stops listening; the connection file goes. The server is taken out of the lock before it stops: stopping ends
    /// its sessions, which tells the host the apps connected now.
    pub(crate) fn stop(&self) {
        let running = lock(&self.server).take();
        drop(running);
    }

    pub(crate) fn status(&self) -> McpStatus {
        let connected = lock(&self.server).as_ref().map(McpServer::connected);
        // Asked only while this one doesn't listen, with the lock let go.
        let another_listens = connected.is_none()
            && (self.dir.as_deref()).is_some_and(|dir| connection::served(dir).is_some());
        let exe = std::env::current_exe().unwrap_or_default();
        McpStatus {
            enabled: connected.is_some(),
            connected: connected.unwrap_or_default(),
            config: config_json(&exe),
            needs_move: needs_move(&exe),
            another_listens,
        }
    }

    /// Where it listens now, while it does.
    #[cfg(test)]
    pub(crate) fn port(&self) -> Option<u16> {
        lock(&self.server).as_ref().map(McpServer::port)
    }
}

/// Deletes a connection file left in `dir`, unless another Klipolo that is running wrote it.
fn remove_left(dir: &Path) {
    let theirs = connection::read(dir).is_some_and(|file| {
        file.pid != std::process::id() && klipolo_platform::process::is_alive(file.pid)
    });
    if !theirs {
        connection::remove(dir);
    }
}

/// The app's MCP server, its host over the open project, chat's slot and the credentials, telling the page through
/// events; listening at launch when the person left it on.
pub(crate) fn load(
    app: &AppHandle,
    chat: Arc<ChatSession>,
    analysis: Arc<AnalysisState>,
    credentials: Arc<Credentials>,
    view: ViewHandle,
) -> McpState {
    let events = app.clone();
    let tell: Tell = Arc::new(move |told| {
        let sent = match told {
            Told::Reply(reply) => events.emit(REPLY_EVENT, *reply),
            Told::Apps(apps) => events.emit(APPS_EVENT, apps),
        };
        if let Err(error) = sent {
            tracing::warn!(%error, "an MCP event couldn't be sent");
        }
    });
    let rt = tauri::async_runtime::handle().inner().clone();
    let host = AppHost::new(
        app.clone(),
        chat,
        analysis,
        credentials,
        view,
        tell,
        rt.clone(),
    );
    let state = McpState::new(
        Arc::new(Mcp::new(Arc::new(host))),
        connection::support_dir(),
        rt,
    );
    state.at_launch();
    state
}

/// Whether other apps may edit now, who is connected, and the config to copy.
#[tauri::command(async)]
pub(crate) fn mcp_status(state: State<'_, McpState>) -> McpStatus {
    state.status()
}

/// Turns other apps' access on or off; a sentence when it couldn't start listening.
#[tauri::command(async)]
pub(crate) fn mcp_set_enabled(state: State<'_, McpState>, on: bool) -> Result<McpStatus, String> {
    state.set_enabled(on)
}
