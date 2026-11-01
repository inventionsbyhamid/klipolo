//! The macOS mouse monitor's event mask, built here once for every monitor Klipolo makes: the
//! mouse's buttons, drags and moves, never a key. Apple documents the Accessibility requirement for key events only,
//! so a mouse-only monitor needs no permission. Pure, built on every OS, so a Linux run checks it.

/// AppKit's NSEventType bits for the mouse: left, right and other down/up/dragged, and moved. No scroll.
pub const NS_MOUSE_MASK: u64 = (1 << 1)
    | (1 << 2)
    | (1 << 3)
    | (1 << 4)
    | (1 << 5)
    | (1 << 6)
    | (1 << 7)
    | (1 << 25)
    | (1 << 26)
    | (1 << 27);

/// keyDown (10), keyUp (11), flagsChanged (12): never in a mask Klipolo builds.
pub const NS_KEY_BITS: u64 = (1 << 10) | (1 << 11) | (1 << 12);

/// The mask every mouse monitor uses.
pub fn mouse_mask() -> u64 {
    NS_MOUSE_MASK & !NS_KEY_BITS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mask_has_every_mouse_bit_and_no_key_bit() {
        // leftDown, leftUp, rightDown, rightUp, mouseMoved, leftDragged, rightDragged, otherDown, otherUp, otherDragged.
        for bit in [1, 2, 3, 4, 5, 6, 7, 25, 26, 27] {
            assert_ne!(mouse_mask() & 1 << bit, 0, "bit {bit}");
        }
        assert_eq!(mouse_mask() & NS_KEY_BITS, 0);
        // scrollWheel (22) isn't asked for either.
        assert_eq!(mouse_mask() & 1 << 22, 0);
    }

    #[test]
    fn the_mask_is_the_value_c9_inlined() {
        assert_eq!(mouse_mask(), 0x0E00_00FE);
    }
}
