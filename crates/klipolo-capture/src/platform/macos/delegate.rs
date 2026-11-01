//! The worker's application delegate.
//! - **Launched:** the menu-bar item is made once AppKit's loop runs (`Face::launched`), as tray-icon asks so the item
//!   works over fullscreen apps, the shell's main case.
//! - **A system quit** (logout, restart, shutdown, Activity Monitor's Quit): AppKit asks `applicationShouldTerminate:`,
//!   and its own answer would exit with 0 at once, mid-take. The answer is "later": the session gets the menu's own
//!   Stop, and the process ends with the session's own exit code once it has ended (`SessionEnd`), within
//!   `quit_wait`: 10 s for a take (past that its journal is durable and recovery finishes it on the next launch, exit
//!   1), at once without one. AppKit is never asked to end the process, so its exit 0 never replaces the session's code.
//!   `SuddenTerminationDisabled` (the take's activity) makes macOS ask rather than kill.
//!
//!   While it waits, AppKit runs a nested loop inside `terminate:`. The wait is watched by a run-loop timer in the
//!   common modes, which that loop serves, never by the main dispatch queue: a `terminate:` called from a main-queue
//!   block holds that queue until it returns (seen in the smoke run). 20 looks a second, for at most `quit_wait`.

use std::cell::Cell;
use std::sync::mpsc::{SyncSender, TrySendError};
use std::time::{Duration, Instant};

use cidre::cf;
use objc2::rc::Retained;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{NSApplication, NSApplicationDelegate, NSApplicationTerminateReply};
use objc2_foundation::{NSNotification, NSObject, NSObjectProtocol};

use super::shell::with_face;
use crate::shell::{MenuAction, SessionEnd, ShellInput, WorkerState};
use crate::shell_view::{QUIT_OVERDUE_CODE, quit_wait};

/// How often the system quit's wait looks at the session's end, in seconds.
const QUIT_LOOK_EVERY_S: f64 = 0.05;

pub(super) struct Ivars {
    input: SyncSender<ShellInput>,
    end: SessionEnd,
    /// The session's latest state (`Face::show`): how long a system quit waits.
    state: Cell<Option<WorkerState>>,
    /// A system quit is under way: the session's `Quit` ends the process instead of AppKit's loop.
    quitting: Cell<bool>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements, and `Delegate` doesn't implement `Drop`.
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[name = "KlipoloWorkerDelegate"]
    #[ivars = Ivars]
    pub(super) struct Delegate;

    // SAFETY: `NSObjectProtocol` has no safety requirements.
    unsafe impl NSObjectProtocol for Delegate {}

    // SAFETY: `NSApplicationDelegate` has no safety requirements; the signatures are AppKit's.
    unsafe impl NSApplicationDelegate for Delegate {
        #[unsafe(method(applicationDidFinishLaunching:))]
        fn did_finish_launching(&self, _notification: &NSNotification) {
            with_face(|face| face.launched());
        }

        #[unsafe(method(applicationShouldTerminate:))]
        fn should_terminate(&self, _sender: &NSApplication) -> NSApplicationTerminateReply {
            self.system_quit();
            NSApplicationTerminateReply::TerminateLater
        }
    }
);

impl Delegate {
    pub(super) fn new(
        mtm: MainThreadMarker,
        input: SyncSender<ShellInput>,
        end: SessionEnd,
    ) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(Ivars {
            input,
            end,
            state: Cell::new(None),
            quitting: Cell::new(false),
        });
        // SAFETY: NSObject's `init` has this signature.
        unsafe { msg_send![super(this), init] }
    }

    pub(super) fn seen(&self, state: WorkerState) {
        self.ivars().state.set(Some(state));
    }

    pub(super) fn quitting(&self) -> bool {
        self.ivars().quitting.get()
    }

    /// The system asks to quit: once, the session gets the menu's Stop (never blocking AppKit's thread), and a
    /// run-loop timer watches for its end within the bound.
    fn system_quit(&self) {
        let iv = self.ivars();
        if let Some(code) = iv.end.wait(Duration::ZERO) {
            exit_with(code);
        }
        if iv.quitting.replace(true) {
            return;
        }
        let wait = quit_wait(iv.state.get());
        tracing::info!(state = ?iv.state.get(), ?wait, "shell: the system asks to quit; the take stops and saves first");
        if let Err(TrySendError::Full(stop)) = iv.input.try_send(ShellInput::Menu(MenuAction::Stop))
        {
            let input = iv.input.clone();
            std::thread::spawn(move || input.send(stop));
        }
        let (end, until) = (iv.end.clone(), Instant::now() + wait);
        let first = cf::abs_time_current() + QUIT_LOOK_EVERY_S;
        let look = cf::RunLoopTimer::with_handler(
            first,
            QUIT_LOOK_EVERY_S,
            cf::OptionFlags::NONE,
            0,
            move |_| match end.wait(Duration::ZERO) {
                Some(code) => exit_with(code),
                None if Instant::now() >= until => {
                    tracing::warn!(
                        "shell: the take didn't finish in time; its journal is durable, recovery finishes it"
                    );
                    exit_with(QUIT_OVERDUE_CODE)
                }
                None => {}
            },
        );
        cf::RunLoop::main().add_timer(&look, cf::RunLoopMode::common());
    }
}

/// Ends the process with the session's code: the shortcuts and the take's activity end with it.
fn exit_with(code: i32) -> ! {
    tracing::info!(code, "shell: the worker ends for the system's quit");
    std::process::exit(code)
}
