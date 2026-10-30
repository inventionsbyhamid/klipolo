//! The scene cache's eviction order.

use super::*;

/// When room is needed, a drawn entry that left the screen goes first, then one prepared ahead,
/// and one on screen (drawn last frame) last; within a class, the oldest first.
#[test]
fn entries_go_off_screen_first_then_prepared_then_on_screen() {
    let now = 100;
    let mut entries = [
        ("on screen", evict_rank(true, 99, now)),
        ("prepared long ago", evict_rank(false, 10, now)),
        ("prepared just now", evict_rank(false, 98, now)),
        ("off screen", evict_rank(true, 50, now)),
        ("off screen longer", evict_rank(true, 20, now)),
    ];
    entries.sort_by_key(|(_, rank)| *rank);
    let order: Vec<&str> = entries.iter().map(|(name, _)| *name).collect();
    assert_eq!(
        order,
        [
            "off screen longer",
            "off screen",
            "prepared long ago",
            "prepared just now",
            "on screen"
        ]
    );
}
