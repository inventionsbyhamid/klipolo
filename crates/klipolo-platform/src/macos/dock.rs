//! The Dock icon's menu on macOS (`dock::install`): two methods added to the app's own `NSApplication` delegate class
//! (Tauri's, which has neither), `applicationDockMenu:`, which AppKit asks each time the person opens the menu, and the
//! items' action. The menu is built from the app's items then, so it always shows the take as it is.

use std::cell::RefCell;
use std::ffi::CStr;
use std::sync::OnceLock;

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Imp, Sel};
use objc2::{MainThreadMarker, sel};
use objc2_app_kit::{NSApplication, NSMenu, NSMenuItem};
use objc2_foundation::NSString;

use crate::dock::{DockClick, DockItems};

/// The app's items and click, set once by `install`.
static MENU: OnceLock<(DockItems, DockClick)> = OnceLock::new();

thread_local! {
    /// The ids of the menu last shown, by each item's tag.
    static SHOWN: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

pub(crate) fn install_dock_menu(items: DockItems, click: DockClick) {
    let Some(mtm) = MainThreadMarker::new() else {
        tracing::warn!("dock: not on the main thread, so the Dock icon has no recorder menu");
        return;
    };
    if MENU.set((items, click)).is_err() {
        tracing::warn!("dock: its menu was installed already");
        return;
    }
    let app = NSApplication::sharedApplication(mtm);
    let Some(owner) = app.delegate() else {
        tracing::warn!("dock: the app has no delegate yet, so the Dock icon has no recorder menu");
        return;
    };
    let delegate: &AnyObject = owner.as_ref();
    let class = delegate.class();
    // SAFETY: each function's signature is the selector's (`@@:@`: an object back for one object; `v@:@`: nothing back
    // for one object), as AppKit calls them; adding a method a class lacks is the runtime's documented use.
    let added = unsafe {
        let menu = std::mem::transmute::<
            extern "C-unwind" fn(&AnyObject, Sel, *mut AnyObject) -> *mut NSMenu,
            Imp,
        >(dock_menu);
        let clicked = std::mem::transmute::<
            extern "C-unwind" fn(&AnyObject, Sel, *mut NSMenuItem),
            Imp,
        >(item_clicked);
        (
            add(class, sel!(applicationDockMenu:), menu, c"@@:@"),
            add(class, sel!(klipoloDockItem:), clicked, c"v@:@"),
        )
    };
    // AppKit may note which optional methods a delegate answers when it is set, and tao set it before these were added:
    // set again (nil, then the same object, which tao keeps alive), it is asked afresh. On the main loop's next turn,
    // so it never runs inside the launch notification tao's setup is called from.
    drop(owner);
    cidre::dispatch::Queue::main().async_mut(|| {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let app = NSApplication::sharedApplication(mtm);
        if let Some(owner) = app.delegate() {
            app.setDelegate(None);
            app.setDelegate(Some(&owner));
        }
    });
    tracing::info!(
        menu = added.0,
        action = added.1,
        "dock: the recorder's menu is in place"
    );
}

/// `class_addMethod`: false when the class has the method already (it is never replaced).
///
/// # Safety
/// `imp` must have the C signature `types` describes.
unsafe fn add(class: &AnyClass, name: Sel, imp: Imp, types: &CStr) -> bool {
    let class = std::ptr::from_ref(class).cast_mut();
    // SAFETY: the caller's; `class` is a registered class.
    unsafe { objc2::ffi::class_addMethod(class, name, imp, types.as_ptr()).as_bool() }
}

/// `applicationDockMenu:`: the app's items now, each sending `klipoloDockItem:` to the delegate with its tag; nil (the
/// OS's own menu alone) when there are none.
extern "C-unwind" fn dock_menu(this: &AnyObject, _: Sel, _sender: *mut AnyObject) -> *mut NSMenu {
    let (Some((items, _)), Some(mtm)) = (MENU.get(), MainThreadMarker::new()) else {
        return std::ptr::null_mut();
    };
    let items = items();
    let ids = items.iter().map(|i| i.id.clone()).collect();
    SHOWN.with(|shown| *shown.borrow_mut() = ids);
    if items.is_empty() {
        return std::ptr::null_mut();
    }
    let menu = NSMenu::new(mtm);
    menu.setAutoenablesItems(false);
    for (tag, item) in items.iter().enumerate() {
        // SAFETY: `klipoloDockItem:` is the delegate's (`install_dock_menu`), and the target is the delegate.
        let row = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                mtm.alloc(),
                &NSString::from_str(&item.title),
                Some(sel!(klipoloDockItem:)),
                &NSString::new(),
            )
        };
        // SAFETY: the delegate outlives the menu (AppKit keeps it for the app's life).
        unsafe { row.setTarget(Some(this)) };
        row.setTag(tag as isize);
        row.setEnabled(true);
        menu.addItem(&row);
    }
    Retained::autorelease_return(menu)
}

/// An item's action: its id goes to the app's click.
extern "C-unwind" fn item_clicked(_this: &AnyObject, _: Sel, sender: *mut NSMenuItem) {
    // SAFETY: AppKit sends the clicked item itself, alive for the call.
    let Some(sender) = (unsafe { sender.as_ref() }) else {
        return;
    };
    let tag = usize::try_from(sender.tag()).ok();
    let id = SHOWN.with(|shown| tag.and_then(|t| shown.borrow().get(t).cloned()));
    if let (Some(id), Some((_, click))) = (id, MENU.get()) {
        click(&id);
    }
}
