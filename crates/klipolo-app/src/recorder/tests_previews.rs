//! The Record dialog's previews: in a test run none starts. Fake workers and a fake page only: nothing captures and
//! nothing asks.

use super::state::RecorderState;
use super::test_previews::*;

#[test]
fn test_run_starts_no_preview() {
    let state = RecorderState::build(true, || panic!("a test run makes no recorder"));
    let page = std::sync::Arc::new(Page::default());
    let refused = state.previews(&everything(), page).unwrap_err();
    assert_eq!(refused.code, "capture.error.testRun");
    // The stop has nothing to stop.
    state.previews_off();
}
