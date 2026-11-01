//! The app's Dock icon menu: while a take records, the Dock icon offers the recorder's own actions, so the
//! person can stop, pause or mark a moment when the menu-bar item is hidden behind the notch or a fullscreen app's menu
//! bar. The app says what the menu holds each time the person opens it, and what a click does.
//! - macOS: `applicationDockMenu:` on the app's own `NSApplication` delegate (Tauri's), added once at start;
//! - Windows (later): the live analogue is the taskbar button's thumbnail toolbar
//!   (`ITaskbarList3::ThumbBarAddButtons`), which can offer Stop and Pause while a take runs (a jump list's entries are
//!   static); until it is built, the recorder's notification area item is the way out there, and this does nothing;
//! - Linux: launcher actions (`.desktop` actions) are static; nothing yet.

/// One line of the Dock menu: the id a click hands back, and its words in the person's language.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DockItem {
    pub id: String,
    pub title: String,
}

/// What the menu holds now: asked on the main thread each time the person opens it, so it is never stale. Empty: the
/// OS's own menu alone.
pub type DockItems = Box<dyn Fn() -> Vec<DockItem> + Send + Sync>;
/// A click on the item with this id, on the main thread.
pub type DockClick = Box<dyn Fn(&str) + Send + Sync>;

/// Gives the app's Dock icon its menu, once. macOS: on the main thread, after the app's delegate is set (Tauri's
/// `setup`); a second call, or one off the main thread, is logged and does nothing. Other OSes: nothing (the module doc).
pub fn install(items: DockItems, click: DockClick) {
    crate::imp::install_dock_menu(items, click);
}
