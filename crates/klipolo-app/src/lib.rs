//! The app's logic that needs no Tauri types, so it tests anywhere (Linux included): the Meta credentials Settings
//! saves, the device id, and the one shared Meta adapter built from them; the caption pages the Captions panel reads;
//! and the shapes a chat message's reply takes. The Tauri commands (`main.rs` and its modules) call into it.

pub mod captions;
pub mod chat;
pub mod credentials;
