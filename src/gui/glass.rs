//! Frosted glass behind the settings window.
//!
//! The window is created transparent; this asks Windows to blur what is
//! behind it, and [`super::theme`] paints translucent tints on top. The blur
//! is done by the compositor, so it costs this process nothing per frame.
//!
//! It uses the accent policy (`SetWindowCompositionAttribute`) rather than
//! the documented system backdrop: the documented acrylic drops to a flat
//! colour whenever the window loses focus, and a window that floats beside
//! the thing being tuned is unfocused most of the time. The accent policy
//! keeps blurring regardless, on Windows 10 as well as 11.

use std::ffi::c_void;

use windows::core::{s, w};
use windows::Win32::Foundation::{BOOL, HWND};
use windows::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE,
    DWMWCP_ROUND, DWM_WINDOW_CORNER_PREFERENCE,
};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};

/// `WCA_ACCENT_POLICY`.
const ACCENT_POLICY: u32 = 19;
/// `ACCENT_ENABLE_ACRYLICBLURBEHIND`: blur plus the acrylic noise and
/// luminosity layers.
const ACCENT_ACRYLIC: u32 = 4;

#[repr(C)]
struct AccentPolicy {
    state: u32,
    flags: u32,
    /// `0xAABBGGRR`. Kept nearly clear: the real tint is painted by egui, so
    /// changing the glass strength is a repaint rather than another call here.
    gradient: u32,
    animation: u32,
}

#[repr(C)]
struct CompositionData {
    attribute: u32,
    data: *mut c_void,
    size: usize,
}

type SetWindowCompositionAttribute = unsafe extern "system" fn(HWND, *mut CompositionData) -> BOOL;

/// Blur behind `hwnd`, round its corners and give it a dark frame. Returns
/// whether the blur took; if it did not, the window has to be painted solid.
pub fn apply(hwnd: HWND) -> bool {
    unsafe {
        // Cosmetic, and fine to fail on Windows 10, which has neither.
        let dark = BOOL(1);
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            &dark as *const _ as *const c_void,
            std::mem::size_of::<BOOL>() as u32,
        );
        let round: DWM_WINDOW_CORNER_PREFERENCE = DWMWCP_ROUND;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &round as *const _ as *const c_void,
            std::mem::size_of::<DWM_WINDOW_CORNER_PREFERENCE>() as u32,
        );

        // Undocumented, so it is looked up rather than linked.
        let Ok(user32) = GetModuleHandleW(w!("user32.dll")) else {
            return false;
        };
        let Some(proc) = GetProcAddress(user32, s!("SetWindowCompositionAttribute")) else {
            return false;
        };
        let set: SetWindowCompositionAttribute = std::mem::transmute(proc);

        let mut policy = AccentPolicy {
            state: ACCENT_ACRYLIC,
            flags: 0,
            gradient: 0x10_16_10_10,
            animation: 0,
        };
        let mut data = CompositionData {
            attribute: ACCENT_POLICY,
            data: &mut policy as *mut _ as *mut c_void,
            size: std::mem::size_of::<AccentPolicy>(),
        };
        set(hwnd, &mut data).as_bool()
    }
}
