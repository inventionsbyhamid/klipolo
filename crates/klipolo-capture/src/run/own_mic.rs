//! The run controller's look for the person's own mic: no pipe looks for it in two cases, so the run controller does,
//! once a second, in the OS's registry (`Backend::mics`, which opens nothing), for as long as the take runs.
//! - **On a fallback** (swapped to another mic). `DeviceBack` goes to the core once their mic has been listed for 3 s
//!   straight (`mic::Presence`), and at most once per 30 s (`mic::SWITCH_BACK_EVERY_NS`) after the last switch back, so a
//!   mic that flaps, or stays listed while it can't stream, never churns runs: the take stays on the fallback until it
//!   settles. The first flap, or a switch held back, is a mild `capture.note.micFlapping`.
//! - **After a start on their own mic failed** with any error (USB audio listed a moment before it can stream, or an OS
//!   error). The first look waits `RETRY_AFTER`, past the core's 3 s fallback, so a mic that can't
//!   stream gives way to a fallback rather than being retried in a loop. Then `DeviceBack` goes the first time it is
//!   listed (a restart, not a switch back: no presence wait), and at most once per `RETRY_AFTER`.

use std::time::{Duration, Instant};

use crate::english::code;
use crate::heal::MIC_FALLBACK_NS;
use crate::mic::{Presence, SWITCH_BACK_EVERY_NS, Seen};
use crate::pipe::{PipeError, PipeEvent};

/// How often the person's own mic is looked for.
const LOOK_EVERY: Duration = Duration::from_secs(1);
/// After a failed start on the person's own mic: how long until it is tried again, at the soonest.
const RETRY_AFTER: Duration = Duration::from_nanos(MIC_FALLBACK_NS + 1_000_000_000);

/// A mic source's own device, what its pipe records on, and the look for it (the module doc).
pub(super) struct OwnMic {
    own: String,
    on_own: bool,
    look: Option<Look>,
    last_back: Option<Instant>,
}

/// Which case the look is for (the module doc).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    SwitchBack,
    Retry,
}

struct Look {
    kind: Kind,
    base: Instant,
    next: Instant,
    presence: Presence,
    noted: bool,
}

impl Look {
    fn new(kind: Kind, now: Instant) -> Self {
        let next = match kind {
            Kind::SwitchBack => now,
            Kind::Retry => now + RETRY_AFTER,
        };
        Self {
            kind,
            base: now,
            next,
            presence: Presence::default(),
            noted: false,
        }
    }

    /// The mild note, the first time only.
    fn flapping(&mut self) -> Option<PipeEvent> {
        let first = !std::mem::replace(&mut self.noted, true);
        first.then(|| PipeEvent::Note(code!("capture.note.micFlapping").into()))
    }
}

impl OwnMic {
    pub(super) fn new(own: String) -> Self {
        Self {
            own,
            on_own: true,
            look: None,
            last_back: None,
        }
    }

    /// The pipe is swapped to `device`: on another mic, the switch-back look starts; back on the person's own, it ends.
    pub(super) fn swapped(&mut self, device: &str, now: Instant) {
        self.on_own = device == self.own;
        if self.on_own {
            (self.look, self.last_back) = (None, Some(now));
        } else {
            self.look = Some(Look::new(Kind::SwitchBack, now));
        }
    }

    /// A start's outcome (`failed`: its error). On the person's own mic, a start that failed, whatever the error, is
    /// looked after again (the module doc); one that started needs no look.
    pub(super) fn started(&mut self, failed: Option<&PipeError>, now: Instant) {
        if !self.on_own {
            return;
        }
        self.look = failed.map(|_| Look::new(Kind::Retry, now));
    }

    /// What to tell the core now, if anything (the module doc). `listed(id)` asks the registry, only when a look is due.
    pub(super) fn look(
        &mut self,
        now: Instant,
        listed: impl FnOnce(&str) -> bool,
    ) -> Option<PipeEvent> {
        let look = self.look.as_mut()?;
        if now < look.next {
            return None;
        }
        look.next = now + LOOK_EVERY;
        if look.kind == Kind::Retry {
            if !listed(&self.own) {
                return None;
            }
            look.next = now + RETRY_AFTER;
            return Some(PipeEvent::DeviceBack);
        }
        let at = u64::try_from(now.duration_since(look.base).as_nanos()).unwrap_or(u64::MAX);
        match look.presence.look(listed(&self.own), at) {
            Seen::Present => {
                let every = Duration::from_nanos(SWITCH_BACK_EVERY_NS);
                if self
                    .last_back
                    .is_some_and(|t| now.duration_since(t) < every)
                {
                    return look.flapping();
                }
                look.presence.reset();
                self.last_back = Some(now);
                Some(PipeEvent::DeviceBack)
            }
            Seen::Flapped => look.flapping(),
            Seen::Gone | Seen::Settling => None,
        }
    }
}
