//! What Delete… may do on Windows: recycle, never destroy. The Shell tells a progress sink
//! before each delete whether it will recycle the item (`TSF_DELETE_RECYCLE_IF_POSSIBLE` in `PreDeleteItem`'s flags);
//! one that won't (too big for the Recycle Bin, a drive without one) is refused. Compiled on Windows, and everywhere for
//! its test.

/// `TSF_DELETE_RECYCLE_IF_POSSIBLE` (`TRANSFER_SOURCE_FLAGS`): the Shell will put the item in the Recycle Bin.
const RECYCLE_IF_POSSIBLE: u32 = 0x80;

/// Whether the delete the Shell is about to make recycles the item (true), or would destroy it (false: refused).
pub(crate) fn recycles(flags: u32) -> bool {
    flags & RECYCLE_IF_POSSIBLE != 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_delete_that_recycles_goes_through() {
        assert!(recycles(RECYCLE_IF_POSSIBLE));
        assert!(recycles(RECYCLE_IF_POSSIBLE | 0x1));
        assert!(
            !recycles(0),
            "too big for the Recycle Bin: a permanent delete"
        );
        assert!(!recycles(0x1 | 0x40));
    }
}
