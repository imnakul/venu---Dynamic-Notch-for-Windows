//! Global shortcut plumbing that does not touch Win32: key names to virtual
//! key codes, and modifier flags for `RegisterHotKey`. Kept pure so it can be
//! tested on any platform.

use std::sync::atomic::{AtomicU8, Ordering};

use crate::config::Hotkey;

/// What became of a shortcut when the notch registered it, for Settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyStatus {
    /// Not set, or not registered yet.
    Off,
    Active,
    /// Windows refused it, nearly always because another app owns it.
    InUse,
}

static STATUS: [AtomicU8; 3] = [AtomicU8::new(0), AtomicU8::new(0), AtomicU8::new(0)];

pub fn set_status(index: usize, status: HotkeyStatus) {
    if let Some(slot) = STATUS.get(index) {
        let code = match status {
            HotkeyStatus::Off => 0,
            HotkeyStatus::Active => 1,
            HotkeyStatus::InUse => 2,
        };
        slot.store(code, Ordering::Release);
    }
}

/// The shortcut at `index`, in [`crate::config::InputConfig::hotkeys`] order.
pub fn status(index: usize) -> HotkeyStatus {
    match STATUS.get(index).map(|s| s.load(Ordering::Acquire)) {
        Some(1) => HotkeyStatus::Active,
        Some(2) => HotkeyStatus::InUse,
        _ => HotkeyStatus::Off,
    }
}

/// `RegisterHotKey` modifier flags.
const MOD_ALT: u32 = 0x0001;
const MOD_CONTROL: u32 = 0x0002;
const MOD_SHIFT: u32 = 0x0004;
const MOD_WIN: u32 = 0x0008;
/// Holding the keys down fires once, not on every auto-repeat.
const MOD_NOREPEAT: u32 = 0x4000;

/// Virtual key code for a key named the way egui names it, or `None` for a
/// key that cannot be a global shortcut here.
pub fn vk_for(name: &str) -> Option<u32> {
    let bytes = name.as_bytes();
    if bytes.len() == 1 {
        let c = bytes[0].to_ascii_uppercase();
        // Letters and digits are their own ASCII codes as virtual keys.
        if c.is_ascii_uppercase() || c.is_ascii_digit() {
            return Some(c as u32);
        }
    }
    if let Some(n) = name.strip_prefix('F').and_then(|n| n.parse::<u32>().ok()) {
        if (1..=24).contains(&n) {
            return Some(0x70 + n - 1);
        }
    }
    let vk = match name {
        "Space" => 0x20,
        "Enter" => 0x0D,
        "Tab" => 0x09,
        "Backspace" => 0x08,
        "Escape" => 0x1B,
        "Insert" => 0x2D,
        "Delete" => 0x2E,
        "Home" => 0x24,
        "End" => 0x23,
        "PageUp" => 0x21,
        "PageDown" => 0x22,
        "Left" => 0x25,
        "Up" => 0x26,
        "Right" => 0x27,
        "Down" => 0x28,
        "Semicolon" => 0xBA,
        "Equals" | "Plus" => 0xBB,
        "Comma" => 0xBC,
        "Minus" => 0xBD,
        "Period" => 0xBE,
        "Slash" | "Questionmark" => 0xBF,
        "Backtick" => 0xC0,
        "OpenBracket" => 0xDB,
        "Backslash" | "Pipe" => 0xDC,
        "CloseBracket" => 0xDD,
        "Quote" => 0xDE,
        _ => return None,
    };
    Some(vk)
}

/// `(modifiers, virtual key)` for `RegisterHotKey`, or `None` when the
/// shortcut has no modifier or names an unknown key.
pub fn registration(hotkey: &Hotkey) -> Option<(u32, u32)> {
    if !hotkey.has_modifier() {
        return None;
    }
    let vk = vk_for(&hotkey.key)?;
    let mut mods = MOD_NOREPEAT;
    if hotkey.ctrl {
        mods |= MOD_CONTROL;
    }
    if hotkey.alt {
        mods |= MOD_ALT;
    }
    if hotkey.shift {
        mods |= MOD_SHIFT;
    }
    if hotkey.win {
        mods |= MOD_WIN;
    }
    Some((mods, vk))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_digits_and_function_keys_map_to_their_codes() {
        assert_eq!(vk_for("N"), Some(0x4E));
        assert_eq!(vk_for("n"), Some(0x4E));
        assert_eq!(vk_for("7"), Some(0x37));
        assert_eq!(vk_for("F1"), Some(0x70));
        assert_eq!(vk_for("F24"), Some(0x87));
        assert_eq!(vk_for("F25"), None);
        assert_eq!(vk_for("Space"), Some(0x20));
        assert_eq!(vk_for("Copy"), None);
    }

    #[test]
    fn a_shortcut_needs_a_modifier_and_a_known_key() {
        let ctrl_alt_n = Hotkey::ctrl_alt("N");
        assert_eq!(
            registration(&ctrl_alt_n),
            Some((MOD_CONTROL | MOD_ALT | MOD_NOREPEAT, 0x4E))
        );
        let bare = Hotkey {
            ctrl: false,
            alt: false,
            shift: false,
            win: false,
            key: "N".into(),
        };
        assert_eq!(registration(&bare), None);
        assert_eq!(registration(&Hotkey::ctrl_alt("Paste")), None);
        assert_eq!(ctrl_alt_n.label(), "Ctrl + Alt + N");
    }
}
