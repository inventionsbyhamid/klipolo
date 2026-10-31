//! `PacketGate`: how every video pipe sends its encoded packets, so a dropped packet never leaves the
//! rest of its GOP referring to it.

use std::sync::mpsc::{SyncSender, TrySendError};

use crate::media::EncodedPacket;

/// Every video pipe sends its encoded packets through this. Once a packet can't be sent (a full
/// channel), it drops every non-key packet until the next keyframe, so a missing P-frame never leaves the rest of its
/// GOP referring to it. `send` returns `WantKeyframe` after a drop; the pipe then forces its encoder's next frame to a
/// keyframe. All drops count in `dropped_channel`.
///
/// A keyframe that doesn't fit either is a new drop: the frames after it would refer to it. It asks again only once
/// 2 s of packet time have passed since the last ask, so a channel that stays full never turns the encoder all-intra;
/// meanwhile the 1 s GOP's own keyframe ends the wait. A closed channel (the session is gone) is `Closed`: the pipe
/// stops, and nothing is asked or counted.
pub struct PacketGate {
    tx: SyncSender<EncodedPacket>,
    awaiting_key: bool,
    /// The packet time of the last `WantKeyframe`.
    asked_at: Option<u64>,
    closed: bool,
    dropped: u64,
}

/// The least packet time between two keyframe requests.
pub const KEYFRAME_ASK_NS: u64 = 2_000_000_000;

/// What became of a packet given to `PacketGate::send`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sent {
    Sent,
    Dropped,
    /// Dropped, and the encoder's next frame must be a keyframe.
    WantKeyframe,
    /// The receiver is gone: stop the pipe.
    Closed,
}

impl PacketGate {
    pub fn new(tx: SyncSender<EncodedPacket>) -> Self {
        Self {
            tx,
            awaiting_key: false,
            asked_at: None,
            closed: false,
            dropped: 0,
        }
    }

    /// Never blocks: O(1), no allocation.
    pub fn send(&mut self, p: EncodedPacket) -> Sent {
        if self.closed {
            return Sent::Closed;
        }
        if self.awaiting_key && !p.keyframe {
            self.dropped += 1;
            return Sent::Dropped;
        }
        let at = p.host_ns;
        match self.tx.try_send(p) {
            Ok(()) => {
                self.awaiting_key = false;
                Sent::Sent
            }
            Err(TrySendError::Disconnected(_)) => {
                self.closed = true;
                Sent::Closed
            }
            Err(TrySendError::Full(_)) => {
                self.dropped += 1;
                self.awaiting_key = true;
                if self
                    .asked_at
                    .is_some_and(|last| at < last.saturating_add(KEYFRAME_ASK_NS))
                {
                    return Sent::Dropped;
                }
                self.asked_at = Some(at);
                Sent::WantKeyframe
            }
        }
    }

    /// Every packet dropped so far.
    pub fn dropped(&self) -> u64 {
        self.dropped
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::{Receiver, sync_channel};

    use super::*;

    fn packet(n: u64, keyframe: bool) -> EncodedPacket {
        EncodedPacket {
            host_ns: n,
            keyframe,
            data: vec![],
        }
    }

    fn drain(rx: &Receiver<EncodedPacket>) -> Vec<u64> {
        rx.try_iter().map(|p| p.host_ns).collect()
    }

    #[test]
    fn after_a_failed_send_only_a_keyframe_goes_next_and_one_is_asked_for() {
        let (tx, rx) = sync_channel(2);
        let mut gate = PacketGate::new(tx);
        assert_eq!(gate.send(packet(0, true)), Sent::Sent);
        assert_eq!(gate.send(packet(1, false)), Sent::Sent);
        // The channel is full: this P-frame is lost, so the rest of its GOP must not follow it.
        assert_eq!(gate.send(packet(2, false)), Sent::WantKeyframe);
        assert_eq!(drain(&rx), [0, 1]);
        assert_eq!(
            gate.send(packet(3, false)),
            Sent::Dropped,
            "room again, but it refers to frame 2"
        );
        assert_eq!(gate.send(packet(4, false)), Sent::Dropped);
        assert_eq!(
            gate.send(packet(5, true)),
            Sent::Sent,
            "the asked-for keyframe"
        );
        assert_eq!(gate.send(packet(6, false)), Sent::Sent);
        assert_eq!(drain(&rx), [5, 6]);
        assert_eq!(gate.dropped(), 3, "every drop counts");
    }

    #[test]
    fn a_keyframe_that_doesnt_fit_asks_again_but_at_most_every_two_seconds() {
        let ms = |t: u64, keyframe| EncodedPacket {
            host_ns: t * 1_000_000,
            keyframe,
            data: vec![],
        };
        let (tx, rx) = sync_channel(1);
        let mut gate = PacketGate::new(tx);
        assert_eq!(gate.send(ms(0, true)), Sent::Sent);
        assert_eq!(gate.send(ms(16, false)), Sent::WantKeyframe);
        // The channel stays full: the forced keyframe is lost too, but asking again so soon would make the encoder
        // all-intra. The GOP's own keyframe comes within a second.
        assert_eq!(gate.send(ms(33, true)), Sent::Dropped);
        assert_eq!(gate.send(ms(50, false)), Sent::Dropped);
        assert_eq!(gate.send(ms(1_000, true)), Sent::Dropped, "still full");
        assert_eq!(
            gate.send(ms(2_016, true)),
            Sent::WantKeyframe,
            "2 s after the last ask"
        );
        assert_eq!(drain(&rx), [0]);
        assert_eq!(gate.send(ms(2_033, true)), Sent::Sent);
        assert_eq!(gate.dropped(), 5);
    }

    #[test]
    fn a_closed_channel_is_closed_not_full() {
        let (tx, rx) = sync_channel(4);
        let mut gate = PacketGate::new(tx);
        assert_eq!(gate.send(packet(0, true)), Sent::Sent);
        drop(rx);
        assert_eq!(
            gate.send(packet(1, false)),
            Sent::Closed,
            "the session is gone"
        );
        assert_eq!(
            gate.send(packet(2, true)),
            Sent::Closed,
            "no keyframe is asked for"
        );
        assert_eq!(gate.dropped(), 0, "nothing counts as a drop");
    }
}
