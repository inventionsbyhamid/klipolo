//! The app's logic that needs no Tauri types, so it tests anywhere (Linux included). The Tauri commands (`main.rs` and
//! its modules) call into it.

/// Pictures made for a project: Muse Image stickers, cut out of their white background. Importing files is the
/// binary's own `media` module (`src/media.rs`), which this shares a folder with.
pub mod media {
    pub mod sticker;
}
