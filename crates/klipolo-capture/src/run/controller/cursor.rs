//! A screen controller's cursor (`pipes::CursorFeed`): the sender a pipe or a source sends its events on, forwarded to
//! the core without ever blocking (a full queue drops positions and buttons, counted and logged, and holds state events
//! until there is room), the source's one start (inside the
//! take the person started: only from the run controller's `start`, after the Stop check), its pause and stop, and the
//! live switch to the cursor in the frames (`PipeCmd::CursorInFrames`), answered on this thread.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{RecvTimeoutError, SyncSender, TrySendError, sync_channel};
use std::time::Duration;

use klipolo_platform::clock::host_ns;
use klipolo_platform::qos::{self, Qos};

use super::super::pipes::{CursorFeed, Owned, SMALL_STACK};
use super::super::threads::Input;
use super::Controller;
use crate::cursor::{CURSOR_QUEUE, CursorEvent};

/// State events that may wait for room in the core's queue, how often they try again, and how long the last of them
/// may still try once the sender closed.
const HELD: usize = 256;
const HELD_RETRY: Duration = Duration::from_millis(10);
const LAST_FLUSH: Duration = Duration::from_millis(500);

/// A cursor sender whose events reach the core's queue with `try_send`, never blocking. A full queue drops a position
/// (`dropped` counts them); a state event (a button, a shape, visibility, the area, the clicks switch, shapes full, a
/// failure) is held, in order, and sent as soon as there is room: one lost would show a
/// held button, a wrong shape or the wrong clicks for the rest of the take. At most `HELD` wait (shapes are at most
/// `MAX_SHAPES`), and what is held when the sender closes still tries for up to `LAST_FLUSH`.
fn sender(core: SyncSender<Input>, dropped: Arc<AtomicU64>) -> SyncSender<CursorEvent> {
    let (tx, rx) = sync_channel(CURSOR_QUEUE);
    let spawned = std::thread::Builder::new()
        .name("klipolo-cursor-forward".into())
        .stack_size(SMALL_STACK)
        .spawn(move || {
            qos::set_current(Qos::UserInteractive);
            let mut held = VecDeque::new();
            loop {
                let next = if held.is_empty() {
                    rx.recv().map_err(|_| RecvTimeoutError::Disconnected)
                } else {
                    rx.recv_timeout(HELD_RETRY)
                };
                let ev = match next {
                    Ok(ev) => Some(ev),
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(RecvTimeoutError::Disconnected) => {
                        return flush(&core, &mut held, &dropped);
                    }
                };
                if !forward(&core, &mut held, ev, &dropped) {
                    return;
                }
            }
        });
    if let Err(e) = spawned {
        tracing::error!(error = %e, "pipes: the cursor's forwarder didn't start");
    }
    tx
}

/// The sender closed (the source stopped, a pipe restarted): what is held still tries, for up to `LAST_FLUSH`.
fn flush(core: &SyncSender<Input>, held: &mut VecDeque<CursorEvent>, dropped: &AtomicU64) {
    let until = std::time::Instant::now() + LAST_FLUSH;
    while !held.is_empty()
        && forward(core, held, None, dropped)
        && std::time::Instant::now() < until
    {
        if !held.is_empty() {
            std::thread::sleep(HELD_RETRY);
        }
    }
    if !held.is_empty() {
        dropped.fetch_add(held.len() as u64, Ordering::Relaxed);
    }
}

/// The state events held for room, then `ev`; false once the core is gone.
fn forward(
    core: &SyncSender<Input>,
    held: &mut VecDeque<CursorEvent>,
    ev: Option<CursorEvent>,
    dropped: &AtomicU64,
) -> bool {
    while let Some(state) = held.pop_front() {
        match core.try_send(Input::Cursor(state)) {
            Ok(()) => {}
            Err(TrySendError::Full(Input::Cursor(state))) => {
                held.push_front(state);
                break;
            }
            Err(TrySendError::Full(_)) => {}
            Err(TrySendError::Disconnected(_)) => return false,
        }
    }
    let Some(ev) = ev else {
        return true;
    };
    let state = !matches!(ev, CursorEvent::Move { .. } | CursorEvent::Beat { .. });
    // Behind held state, a state event waits its turn; a position goes now if there is room.
    if state && !held.is_empty() {
        return hold(held, ev, dropped);
    }
    match core.try_send(Input::Cursor(ev)) {
        Ok(()) => true,
        Err(TrySendError::Full(Input::Cursor(ev))) if state => hold(held, ev, dropped),
        Err(TrySendError::Full(_)) => {
            dropped.fetch_add(1, Ordering::Relaxed);
            true
        }
        Err(TrySendError::Disconnected(_)) => false,
    }
}

fn hold(held: &mut VecDeque<CursorEvent>, ev: CursorEvent, dropped: &AtomicU64) -> bool {
    if held.len() < HELD {
        held.push_back(ev);
    } else {
        dropped.fetch_add(1, Ordering::Relaxed);
    }
    true
}

/// For a (re)start of the pipe: the pipe's own cursor sender (a `Pipe` feed), or the source's first start (a `Source`
/// feed, which then goes on across the pipe's restarts). A source that can't start is a failed one: the watchdog
/// burns the cursor in.
pub(super) fn wire(
    feed: &mut CursorFeed,
    core: &SyncSender<Input>,
    dropped: &Arc<AtomicU64>,
) -> Option<SyncSender<CursorEvent>> {
    match feed {
        CursorFeed::None => None,
        CursorFeed::Pipe => Some(sender(core.clone(), dropped.clone())),
        CursorFeed::Source { source, started } => {
            if !*started {
                *started = true;
                if let Err(e) = source.start(sender(core.clone(), dropped.clone())) {
                    tracing::warn!(error = %e, "pipes: the cursor's source didn't start");
                    let _ = core.send(Input::Cursor(CursorEvent::Failed));
                }
            }
            None
        }
    }
}

impl Controller {
    /// The pipe's counters for the core's tick, with the cursor events dropped; a growing count is logged.
    pub(super) fn publish_stats(&mut self) {
        let Owned::Video { pipe, .. } = &self.owned else {
            return;
        };
        let mut stats = pipe.stats();
        stats.cursor_dropped = self.cursor_dropped.load(Ordering::Relaxed);
        if stats.cursor_dropped > self.cursor_dropped_told {
            self.cursor_dropped_told = stats.cursor_dropped;
            tracing::warn!(
                cursor_dropped = stats.cursor_dropped,
                "pipes: cursor events were dropped (the core's queue was full)"
            );
        }
        *self
            .stats
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = stats;
    }

    /// `PipeCmd::CursorInFrames`: the pipe shows the cursor from now (or from its next start), and the core hears how.
    pub(super) fn cursor_in_frames(&mut self) {
        let ok = match &mut self.owned {
            Owned::Video { pipe, .. } => pipe.set_cursor_in_frames(true).is_ok(),
            Owned::Audio(_) => false,
        };
        let _ = self.core.send(Input::CursorInFrames {
            ok,
            host_ns: host_ns(),
        });
    }

    pub(super) fn cursor_paused(&mut self, paused: bool) {
        if let Owned::Video {
            cursor:
                CursorFeed::Source {
                    source,
                    started: true,
                },
            ..
        } = &mut self.owned
        {
            source.set_paused(paused);
        }
    }

    /// The take's end (not a restart): the source stops.
    pub(super) fn stop_cursor(&mut self) {
        if let Owned::Video {
            cursor: CursorFeed::Source { source, .. },
            ..
        } = &mut self.owned
        {
            source.stop();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;
    use crate::cursor::ShapeImage;

    /// A full core queue never loses a state event (a shape, visibility, a button, the
    /// clicks switch), even once the source's sender closes; moves drop.
    #[test]
    fn state_events_wait_for_room_in_a_full_core_queue_and_moves_drop() {
        let (core, inputs) = sync_channel(1);
        core.send(Input::ParentGone).unwrap();
        let dropped = Arc::new(AtomicU64::new(0));
        let tx = sender(core, dropped.clone());
        let mv = |host_ns| CursorEvent::Move {
            host_ns,
            x: 0.5,
            y: 0.5,
        };
        let button = |host_ns, down| CursorEvent::Button {
            host_ns,
            btn: 0,
            down,
            x: 0.5,
            y: 0.5,
        };
        let image = ShapeImage {
            px: (1, 1),
            rgba: vec![0; 4],
            hotspot_px: (0.0, 0.0),
            pts: 1.0,
            sys: None,
        };
        let state = [
            CursorEvent::NewShape {
                host_ns: 2,
                key: 9,
                image,
            },
            CursorEvent::Visible {
                host_ns: 3,
                on: false,
            },
            button(3, true),
            button(4, false),
            CursorEvent::ClicksSampled { host_ns: 4 },
            CursorEvent::ShapesFull { host_ns: 5 },
        ];
        tx.send(mv(1)).unwrap();
        for ev in state.clone() {
            tx.send(ev).unwrap();
        }
        tx.send(mv(6)).unwrap();
        let until = Instant::now() + Duration::from_secs(5);
        while dropped.load(Ordering::Relaxed) < 2 && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(5));
        }
        // The source stops while everything is still held: what's held still goes.
        drop(tx);
        std::thread::sleep(Duration::from_millis(50));
        assert!(matches!(inputs.recv().unwrap(), Input::ParentGone));
        let got: Vec<CursorEvent> = (0..state.len())
            .filter_map(|_| match inputs.recv_timeout(Duration::from_secs(2)) {
                Ok(Input::Cursor(ev)) => Some(ev),
                _ => None,
            })
            .collect();
        assert_eq!(got, state, "every state event, in order");
        assert_eq!(dropped.load(Ordering::Relaxed), 2, "the moves");
    }
}
