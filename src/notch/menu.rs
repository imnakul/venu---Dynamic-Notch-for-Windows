//! The notch's right-click menu.
//!
//! It is the standard Windows popup menu rather than something painted: it
//! costs nothing while closed, follows the system theme, works with the
//! keyboard and screen readers, and closes itself the way every other menu
//! on the desktop does.
//!
//! `TrackPopupMenu` runs a modal loop until the menu closes, so it is never
//! called while the config lock is held: the notch asks for a menu from its
//! tick, and the manager shows it after releasing the lock (see
//! `NotchManager::tick`).

use windows::core::{HSTRING, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, FindWindowW, PostMessageW, SetForegroundWindow,
    TrackPopupMenu, MENU_ITEM_FLAGS, MF_CHECKED, MF_GRAYED, MF_SEPARATOR, MF_STRING, TPM_LEFTALIGN,
    TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON, TPM_TOPALIGN, WM_NULL,
};

/// Something the menu can ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuCommand {
    Settings,
    TogglePin,
    PauseHover,
    ResumeHover,
    NextPage,
    PreviousPage,
    ToggleClickThrough,
    Close,
    Quit,
    /// Shelf tile actions, by path, so a shelf edited while the menu was up
    /// can never redirect the action to a different item.
    Open(String),
    CopyFile(String),
    CopyPath(String),
    Reveal(String),
    Pin(String),
    Unpin(String),
    ClearClipboard,
}

/// One row of the menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuEntry {
    Item {
        command: MenuCommand,
        label: String,
        checked: bool,
        enabled: bool,
    },
    Separator,
}

impl MenuEntry {
    pub fn item(command: MenuCommand, label: &str) -> Self {
        MenuEntry::Item {
            command,
            label: label.to_owned(),
            checked: false,
            enabled: true,
        }
    }

    pub fn check(command: MenuCommand, label: &str, checked: bool) -> Self {
        MenuEntry::Item {
            command,
            label: label.to_owned(),
            checked,
            enabled: true,
        }
    }
}

/// A menu the notch wants shown at a screen point.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuRequest {
    pub at: (i32, i32),
    pub entries: Vec<MenuEntry>,
}

/// The window that owns the menu. The tray's hidden window can take the
/// foreground, which a popup menu needs in order to close when the user
/// clicks elsewhere; the notch itself never activates.
fn owner(fallback: HWND) -> HWND {
    let title = HSTRING::from("VenuTray");
    unsafe { FindWindowW(None, PCWSTR(title.as_ptr())) }
        .ok()
        .filter(|hwnd| !hwnd.is_invalid())
        .unwrap_or(fallback)
}

/// Show the menu and wait for a choice. `None` if it was dismissed.
pub fn show(request: &MenuRequest, fallback_owner: HWND) -> Option<MenuCommand> {
    let commands: Vec<&MenuCommand> = request
        .entries
        .iter()
        .filter_map(|entry| match entry {
            MenuEntry::Item { command, .. } => Some(command),
            MenuEntry::Separator => None,
        })
        .collect();
    if commands.is_empty() {
        return None;
    }

    unsafe {
        let menu = CreatePopupMenu().ok()?;
        // Ids are 1-based positions among the items; 0 means "dismissed".
        let mut id = 0usize;
        for entry in &request.entries {
            match entry {
                MenuEntry::Separator => {
                    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
                }
                MenuEntry::Item {
                    label,
                    checked,
                    enabled,
                    ..
                } => {
                    id += 1;
                    let mut flags: MENU_ITEM_FLAGS = MF_STRING;
                    if *checked {
                        flags |= MF_CHECKED;
                    }
                    if !*enabled {
                        flags |= MF_GRAYED;
                    }
                    let text = HSTRING::from(label.as_str());
                    let _ = AppendMenuW(menu, flags, id, PCWSTR(text.as_ptr()));
                }
            }
        }

        let hwnd = owner(fallback_owner);
        let _ = SetForegroundWindow(hwnd);
        let chosen = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON | TPM_LEFTALIGN | TPM_TOPALIGN,
            request.at.0,
            request.at.1,
            0,
            hwnd,
            None,
        );
        // Documented companion to SetForegroundWindow + TrackPopupMenu: lets
        // the menu close properly on the next click elsewhere.
        let _ = PostMessageW(hwnd, WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);

        let index = chosen.0 as usize;
        if index == 0 {
            return None;
        }
        commands.get(index - 1).map(|command| (*command).clone())
    }
}
