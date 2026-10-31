//! What an app may do in a while, and what of the undo history apps may hold.

use klipolo_mcp::limits::undo_shares;
use klipolo_model::history::{HistoryLimits, Whose};

#[test]
fn undo_shares_are_a_quarter_and_a_half() {
    let [app, apps] = undo_shares("Agent", HistoryLimits::default());
    assert_eq!(
        (app.of, app.steps, app.bytes),
        (Whose::App("Agent"), 50, 16 << 20)
    );
    assert_eq!(
        (apps.of, apps.steps, apps.bytes),
        (Whose::Apps, 100, 32 << 20)
    );
    // Under memory pressure the history keeps less, and so do the shares.
    let [app, apps] = undo_shares("Agent", HistoryLimits::under_pressure());
    assert_eq!((app.steps, app.bytes), (12, 4 << 20));
    assert_eq!((apps.steps, apps.bytes), (25, 8 << 20));
}
