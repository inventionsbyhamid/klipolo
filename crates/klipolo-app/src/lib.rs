//! The app's logic that needs no Tauri types, so it tests anywhere (Linux included): the Meta credentials Settings
//! saves, the device id, and the one shared Meta adapter built from them; and the pictures made for a project. The Tauri
//! commands (`main.rs` and its modules) call into it.

pub mod credentials;

/// Pictures made for a project: stickers (cut out of their white background) and title cards drawn by an
/// `ImageMaker`, saved in the project's media folder. Importing files is the binary's own `media` module
/// (`src/media.rs`), which this shares a folder with.
pub mod media {
    pub mod generated;
    pub mod media_folder;
    pub mod sticker;
}
