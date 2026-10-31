//! The recorder's menu-bar item (`tray-icon`): the record symbol as a template image (`resources/status/dot*.png`,
//! tinted by the menu bar), the take's title ("● 12:34", `status_title`) and its menu (`menu_items`). Each action item
//! carries its `MenuAction::id`, so a click is paired with its action by id; plain lines are disabled items with ids
//! of their own. The menu is rebuilt only when its shape (which lines are actions) changes; otherwise each line's text
//! is set in place, so an open menu stays open and up to date. Start over comes last, set apart, with its key beside it
//! when the person set one.

use std::str::FromStr;

use objc2::{AllocAnyThread, MainThreadMarker};
use objc2_app_kit::{NSBitmapImageRep, NSCellImagePosition, NSImage};
use objc2_foundation::{NSData, NSSize};
use tray_icon::menu::accelerator::Accelerator;
use tray_icon::menu::{Menu, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{TrayIcon, TrayIconBuilder};

use crate::shell::{KeyBindings, MenuAction};
use crate::shell_view::{self, menu_shortcut};

const DOT: &[u8] = include_bytes!("../../../resources/status/dot.png");
const DOT_2X: &[u8] = include_bytes!("../../../resources/status/dot@2x.png");
/// The menu bar's icon size, in points.
const ICON_PT: f64 = 18.0;

pub(super) struct Item {
    tray: TrayIcon,
    menu: Menu,
    /// Each line's action (None: plain text), as the menu was built.
    shape: Vec<Option<MenuAction>>,
    rows: Vec<(MenuItem, String)>,
    title: String,
    /// Start over's key, shown beside it (`shell_view::menu_shortcut`); None when it isn't a key the menu can show.
    start_over: Option<Accelerator>,
}

impl Item {
    pub(super) fn new(mtm: MainThreadMarker, keys: &KeyBindings) -> Result<Self, String> {
        let menu = Menu::new();
        let tray = (TrayIconBuilder::new())
            .with_menu(Box::new(menu.clone()))
            .build()
            .map_err(|e| e.to_string())?;
        set_icon(&tray, mtm);
        // After the icon: tray-icon fits its click area to the item only through its own setters.
        let _ = tray.set_tooltip(Some("Klipolo"));
        Ok(Self {
            tray,
            menu,
            shape: Vec::new(),
            rows: Vec::new(),
            title: String::new(),
            start_over: menu_shortcut(MenuAction::StartOver, keys)
                .and_then(|key| Accelerator::from_str(key).ok()),
        })
    }

    /// Shows `title` beside the icon and `items` as the menu, changing only what differs.
    pub(super) fn show(&mut self, title: &str, items: &[shell_view::MenuItem]) {
        if title != self.title {
            self.tray.set_title((!title.is_empty()).then_some(title));
            title.clone_into(&mut self.title);
        }
        let shape: Vec<_> = items.iter().map(|item| item.action).collect();
        if shape != self.shape {
            return self.rebuild(items, shape);
        }
        for ((row, shown), item) in self.rows.iter_mut().zip(items) {
            if *shown != item.text {
                row.set_text(&item.text);
                item.text.clone_into(shown);
            }
        }
    }

    fn rebuild(&mut self, items: &[shell_view::MenuItem], shape: Vec<Option<MenuAction>>) {
        while self.menu.remove_at(0).is_some() {}
        self.rows.clear();
        for (i, item) in items.iter().enumerate() {
            let starts_actions = item.action.is_some() && i > 0 && shape[i - 1].is_none();
            // Start over, which discards the take, is set apart from Stop and save.
            if starts_actions || (i > 0 && item.action == Some(MenuAction::StartOver)) {
                let _ = self.menu.append(&PredefinedMenuItem::separator());
            }
            let id = item
                .action
                .map_or_else(|| format!("klipolo.line.{i}"), |a| a.id().to_owned());
            let key = (item.action == Some(MenuAction::StartOver))
                .then_some(self.start_over)
                .flatten();
            let row = MenuItem::with_id(MenuId::new(id), &item.text, item.action.is_some(), key);
            if let Err(error) = self.menu.append(&row) {
                tracing::warn!(%error, "shell: a menu line wasn't added");
            }
            self.rows.push((row, item.text.clone()));
        }
        self.shape = shape;
    }
}

/// The record symbol at 1x and 2x, as a template image beside the title.
fn set_icon(tray: &TrayIcon, mtm: MainThreadMarker) {
    let Some(button) = tray.ns_status_item().and_then(|item| item.button(mtm)) else {
        return;
    };
    let size = NSSize::new(ICON_PT, ICON_PT);
    let image = NSImage::initWithSize(NSImage::alloc(), size);
    for png in [DOT, DOT_2X] {
        if let Some(rep) = NSBitmapImageRep::imageRepWithData(&NSData::with_bytes(png)) {
            rep.setSize(size);
            image.addRepresentation(&rep);
        }
    }
    image.setTemplate(true);
    button.setImage(Some(&image));
    button.setImagePosition(NSCellImagePosition::ImageLeft);
}
