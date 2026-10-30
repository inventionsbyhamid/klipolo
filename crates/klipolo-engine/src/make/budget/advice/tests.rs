use super::*;

/// A figure from the 24 ms stop is where it stopped, so it reads "over 24.0 ms"; a
/// half-size figure says half size; a measured one is quoted as it is.
#[test]
fn the_over_advice_says_where_a_stop_ended_it() {
    let b = Budget::DEFAULT;
    let half = over_advice(Decided::Half(OVER_AT_PREVIEW_MS), true, &b).to_lowercase();
    assert!(
        half.contains("over 24.0 ms a frame at half size against 6.0 ms"),
        "{half}"
    );
    let stopped = over_advice(Decided::Preview(30.8), false, &b).to_lowercase();
    assert!(
        stopped.contains("over 24.0 ms a frame at preview size against 2.0 ms"),
        "{stopped}"
    );
    let measured = over_advice(Decided::Preview(3.6), false, &b);
    assert!(
        measured.starts_with("3.6 ms a frame at preview size against 2.0 ms"),
        "{measured}"
    );
}
