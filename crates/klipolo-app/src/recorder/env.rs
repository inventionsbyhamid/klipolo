//! What the recorder works with and where its outcomes go: the app's (`state::AppSink`, `ProcessLaunch`, the main
//! window, the host clock) or a test's (`test_support`).

use std::path::PathBuf;
use std::sync::Arc;

use klipolo_capture::backend::IndicatorPlace;
use klipolo_capture::protocol::Event;
use klipolo_capture::session::{ImportTarget, SessionDir};
use klipolo_model::coded::CodedError;
use klipolo_platform::clock::Clock;

use super::launch::Launch;
use super::types::LeftOutcome;
use super::window::Window;

/// Where the recorder's outcomes go: the page and the take's landing, or a test's record.
pub trait Sink: Send + Sync {
    /// A worker's event, or the recorder's own, for the page (`recorder-event`), with the take it belongs to.
    fn event(&self, take: &str, ev: &Event);
    /// The take is complete in its folder (`recover`: finish it from its files first): it lands. Runs on the worker's
    /// reader thread, while the take is still the recorder's: no other take arms, and quitting waits, until it returns.
    /// Err with what the person is told when it didn't land in full (`capture.*`): for a take of this run, said on
    /// `recorder-event` already; for one an earlier run left behind (`Done.earlier_run`), left to `left` to say.
    fn finished(&self, done: &Done) -> Result<(), CodedError>;
    /// What became of take `take`, which an earlier run left behind, when it didn't land as a `recorder-imported`: for
    /// the page (`recorder-left`).
    fn left(&self, take: &str, outcome: LeftOutcome);
    /// A take is the recorder's (`held`, from arming until it has landed or ended) or none is: the engine's background
    /// media work waits meanwhile (`Hold::Recording`), so it never competes with the take.
    fn hold_media(&self, held: bool);
}

/// A take to land.
#[derive(Clone, Debug, PartialEq)]
pub struct Done {
    pub id: String,
    pub dir: SessionDir,
    /// The open project when it was armed, or a new one (`ImportTarget::New`).
    pub target: ImportTarget,
    /// That project's file, as it was open when the take was armed: the take lands there even when another project is
    /// open by then.
    pub project_file: Option<PathBuf>,
    /// The worker didn't finish it: repair it from its files first.
    pub recover: bool,
    /// What the person is told about how it ended, with the landed take (`capture.note.savedFromFiles` when the worker
    /// stopped while saving), when nothing has said it already.
    pub note: Option<CodedError>,
    /// An earlier run of Klipolo left it behind (it quit or crashed during the take), and it lands as Klipolo opens
    /// again (`left`): the page says it is from the last session.
    pub earlier_run: bool,
}

/// What the recorder works with.
pub struct Env {
    pub launch: Arc<dyn Launch>,
    pub clock: Arc<dyn Clock>,
    pub sink: Arc<dyn Sink>,
    /// Klipolo's window: hidden while a take records, shown again when it ends.
    pub window: Arc<dyn Window>,
    /// Where takes are made (`klipolo_platform::paths::recordings_root`); None: no videos folder.
    pub root: Option<PathBuf>,
    /// Where the recorder's item lives, for "Stop it from {place}".
    pub place: Box<dyn Fn() -> IndicatorPlace + Send + Sync>,
    /// A thread ticks the supervisor twice a second (the app); tests tick it themselves.
    pub ticker: bool,
}
