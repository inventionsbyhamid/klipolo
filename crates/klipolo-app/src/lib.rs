//! The app's logic that needs no Tauri types, so it tests anywhere (Linux included): the Meta credentials Settings
//! saves, the device id, and the one shared Meta adapter built from them; the analysis of imported clips; the caption
//! pages the Captions panel reads; how loud a recorded sound is on each side (`channels`); chat with the open project;
//! the pictures made for a project; export's slot, start and run; the app's own settings file (`prefs`), the window's
//! colour for the theme it keeps (`appearance`), what Settings copies for another app to start Klipolo's MCP bridge
//! (`mcp_config`), and the library's favorites and recently used items (`library_marks`). The Tauri commands
//! (`main.rs` and its modules) call into it.

pub mod analysis;
pub mod appearance;
pub mod captions;
pub mod channels;
pub mod chat;
pub mod credentials;
pub mod export;
pub mod library_marks;
pub mod mcp_config;
pub mod prefs;

/// Pictures made for a project: stickers (cut out of their white background) and title card backgrounds drawn by an
/// `ImageMaker`, saved in the project's media folder. Importing files is the binary's own `media` module
/// (`src/media.rs`), which this shares a folder with.
pub mod media {
    pub mod generated;
    pub mod media_folder;
    pub mod sticker;
}
