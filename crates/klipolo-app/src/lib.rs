//! The app's logic that needs no Tauri types, so it tests anywhere (Linux included): the Meta credentials Settings
//! saves, the device id, the one shared Meta adapter built from them, and the analysis of imported clips. The Tauri
//! commands (`main.rs` and its modules) call into it.

pub mod analysis;
pub mod credentials;
