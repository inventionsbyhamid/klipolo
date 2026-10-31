//! The app's logic that needs no Tauri types, so it tests anywhere (Linux included): the Meta credentials Settings
//! saves, the device id, and the one shared Meta adapter built from them; the analysis of imported clips; the caption
//! pages the Captions panel reads; the shapes a chat message's reply takes; the pictures made for a project; and
//! export's slot, start and run. The Tauri commands (`main.rs` and its modules) call into it.

pub mod analysis;
pub mod captions;
pub mod chat;
pub mod credentials;
pub mod export;

/// Pictures made for a project: stickers (cut out of their white background) and title card backgrounds drawn by an
/// `ImageMaker`, saved in the project's media folder. Importing files is the binary's own `media` module
/// (`src/media.rs`), which this shares a folder with.
pub mod media {
    pub mod generated;
    pub mod media_folder;
    pub mod sticker;
}
