//! The app's logic that needs no Tauri types, so it tests anywhere (Linux included): the Meta credentials Settings
//! saves, the device id, and the one shared Meta adapter built from them; and the caption pages the Captions panel
//! reads. The Tauri commands (`main.rs` and its modules) call into it.

pub mod captions;
pub mod credentials;
