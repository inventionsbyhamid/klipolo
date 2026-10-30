//! The app's logic that needs no Tauri types, so it tests anywhere (Linux included): the Meta credentials Settings
//! saves, the device id, and the one shared Meta adapter built from them. The Tauri commands (`main.rs` and its
//! modules) call into it.

pub mod credentials;
