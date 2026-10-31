//! The self-check's fallback (`clicks: sampled`): `NSEvent.mouseLocation` and `pressedMouseButtons` at
//! 240 Hz on their own thread, stamped with the host clock at each sample (clicks within ±4.2 ms). It sends a move when
//! the place changed and each button's edges; the sampler's beat covers a still pointer. While paused it sends nothing,
//! and its first sample after a resume only sets the buttons it compares against (a press during the pause is no
//! edge).

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc::SyncSender;
use std::thread::JoinHandle;
use std::time::Duration;

use klipolo_platform::clock::host_ns;
use klipolo_platform::qos::{self, Qos};
use objc2::rc::autoreleasepool;
use objc2_app_kit::NSEvent;

use super::Link;
use crate::cursor::CursorEvent;
use crate::platform::common::cursor_map::to_area;
use crate::platform::common::cursor_pace::POLL_NS;

/// The buttons it follows: left, right, middle and five others (`pressedMouseButtons`' bits, the file's `btn`).
const BUTTONS: u8 = 8;
const PAUSED_LOOK: Duration = Duration::from_millis(50);

pub(super) fn spawn(
    out: SyncSender<CursorEvent>,
    link: Arc<Link>,
) -> std::io::Result<JoinHandle<()>> {
    std::thread::Builder::new()
        .name("klipolo-cursor-poll".into())
        .stack_size(256 * 1024)
        .spawn(move || run(&out, &link))
}

fn run(out: &SyncSender<CursorEvent>, link: &Link) {
    // Its samples are the clicks' times: it mustn't be the first thread the system lets wait.
    qos::set_current(Qos::UserInitiated);
    let (mut last, mut buttons) = (None, None::<usize>);
    let mut next = host_ns();
    while !link.stop.load(Ordering::Acquire) {
        if link.paused.load(Ordering::Acquire) {
            buttons = None;
            std::thread::sleep(PAUSED_LOOK);
            next = host_ns();
            continue;
        }
        let at = host_ns();
        let (place, pressed) =
            autoreleasepool(|_| (link.pointer(), NSEvent::pressedMouseButtons()));
        let (x, y) = to_area(place, link.area.get());
        if last != Some(place) {
            last = Some(place);
            link.send(out, CursorEvent::Move { host_ns: at, x, y });
        }
        if let Some(before) = buttons {
            for btn in (0..BUTTONS).filter(|b| (before ^ pressed) >> b & 1 == 1) {
                let down = pressed >> btn & 1 == 1;
                let ev = CursorEvent::Button {
                    host_ns: at,
                    btn,
                    down,
                    x,
                    y,
                };
                link.send(out, ev);
            }
        }
        buttons = Some(pressed);
        // On its own schedule; a late pass starts it again rather than catching up in a burst.
        next += POLL_NS;
        let now = host_ns();
        if next <= now {
            next = now + POLL_NS;
        }
        std::thread::sleep(Duration::from_nanos(next - now));
    }
}
