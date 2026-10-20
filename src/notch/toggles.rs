//! The Quick Toggles slide: one-click switches for things people reach into
//! Settings or the taskbar for many times a day.
//!
//! Reading a switch's state is a COM call or a registry read, so the states
//! are cached and refreshed at most a few times a second, and only while the
//! slide is on screen.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::{
    eCapture, eConsole, eRender, EDataFlow, IMMDeviceEnumerator, MMDeviceEnumerator,
};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_ALL, CLSCTX_INPROC_SERVER};
use windows::Win32::System::Power::{
    SetThreadExecutionState, ES_CONTINUOUS, ES_DISPLAY_REQUIRED, ES_SYSTEM_REQUIRED,
};
use windows::Win32::System::Registry::{
    RegGetValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_DWORD, RRF_RT_REG_DWORD,
};
use windows::Win32::UI::WindowsAndMessaging::{
    SendMessageTimeoutW, HWND_BROADCAST, SMTO_ABORTIFHUNG, WM_SETTINGCHANGE,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Toggle {
    Mic,
    Sound,
    DarkMode,
    KeepAwake,
}

impl Toggle {
    pub const ALL: [Toggle; 4] = [
        Toggle::Mic,
        Toggle::Sound,
        Toggle::DarkMode,
        Toggle::KeepAwake,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Toggle::Mic => "Microphone",
            Toggle::Sound => "Sound",
            Toggle::DarkMode => "Dark mode",
            Toggle::KeepAwake => "Keep awake",
        }
    }

    /// A Segoe MDL2 Assets glyph for the tile, on and off.
    pub fn glyph(self, on: bool) -> &'static str {
        match (self, on) {
            (Toggle::Mic, true) => "\u{E720}",
            (Toggle::Mic, false) => "\u{EC54}",
            (Toggle::Sound, true) => "\u{E767}",
            (Toggle::Sound, false) => "\u{E74F}",
            (Toggle::DarkMode, _) => "\u{E708}",
            (Toggle::KeepAwake, _) => "\u{E890}",
        }
    }
}

/// Whether each switch is on. "On" is the state the tile lights up for: the
/// microphone and sound are on when *not* muted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct States {
    pub mic: Option<bool>,
    pub sound: Option<bool>,
    pub dark: Option<bool>,
    pub awake: bool,
}

impl States {
    pub fn get(&self, toggle: Toggle) -> Option<bool> {
        match toggle {
            Toggle::Mic => self.mic,
            Toggle::Sound => self.sound,
            Toggle::DarkMode => self.dark,
            Toggle::KeepAwake => Some(self.awake),
        }
    }
}

static KEEP_AWAKE: AtomicBool = AtomicBool::new(false);
static CACHE: Mutex<Option<(Instant, States)>> = Mutex::new(None);

const REFRESH: Duration = Duration::from_millis(700);

/// The switches' states, at most [`REFRESH`] old.
pub fn states() -> States {
    let mut cache = CACHE.lock();
    if let Some((at, states)) = *cache {
        if at.elapsed() < REFRESH {
            return states;
        }
    }
    let states = States {
        mic: endpoint(eCapture).and_then(|v| unsafe { v.GetMute() }.ok().map(|m| !m.as_bool())),
        sound: endpoint(eRender).and_then(|v| unsafe { v.GetMute() }.ok().map(|m| !m.as_bool())),
        dark: apps_use_light_theme().map(|light| !light),
        awake: KEEP_AWAKE.load(Ordering::Relaxed),
    };
    *cache = Some((Instant::now(), states));
    states
}

/// Flip one switch. Runs on the overlay thread, where COM is already up.
pub fn flip(toggle: Toggle) {
    let now = states();
    match toggle {
        Toggle::Mic | Toggle::Sound => {
            let flow = if toggle == Toggle::Mic {
                eCapture
            } else {
                eRender
            };
            if let (Some(volume), Some(on)) = (endpoint(flow), now.get(toggle)) {
                unsafe {
                    let _ = volume.SetMute(on, std::ptr::null());
                }
            }
        }
        Toggle::DarkMode => {
            if let Some(dark) = now.dark {
                set_light_theme(dark);
            }
        }
        Toggle::KeepAwake => {
            let awake = !KEEP_AWAKE.load(Ordering::Relaxed);
            KEEP_AWAKE.store(awake, Ordering::Relaxed);
            // Applies to the calling thread for as long as it lives, which
            // for the overlay thread is as long as Venu runs.
            unsafe {
                SetThreadExecutionState(if awake {
                    ES_CONTINUOUS | ES_SYSTEM_REQUIRED | ES_DISPLAY_REQUIRED
                } else {
                    ES_CONTINUOUS
                });
            }
        }
    }
    *CACHE.lock() = None;
}

fn endpoint(flow: EDataFlow) -> Option<IAudioEndpointVolume> {
    unsafe {
        let devices: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_INPROC_SERVER).ok()?;
        let device = devices.GetDefaultAudioEndpoint(flow, eConsole).ok()?;
        device
            .Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None)
            .ok()
    }
}

const PERSONALIZE: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize");

fn apps_use_light_theme() -> Option<bool> {
    let mut value = 0u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            PERSONALIZE,
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut value as *mut u32 as *mut _),
            Some(&mut size),
        )
        .ok()
        .ok()?;
    }
    Some(value != 0)
}

/// Switch apps and the shell together, the way the Settings app does, then
/// tell running windows to repaint in the new colours.
fn set_light_theme(light: bool) {
    let value: u32 = light as u32;
    for name in [w!("AppsUseLightTheme"), w!("SystemUsesLightTheme")] {
        unsafe {
            let _ = RegSetKeyValueW(
                HKEY_CURRENT_USER,
                PERSONALIZE,
                name,
                REG_DWORD.0,
                Some(&value as *const u32 as *const _),
                std::mem::size_of::<u32>() as u32,
            );
        }
    }
    // The broadcast waits on every top-level window, so it goes on a thread
    // of its own rather than stalling the notch.
    std::thread::spawn(|| unsafe {
        let _ = SendMessageTimeoutW(
            HWND_BROADCAST,
            WM_SETTINGCHANGE,
            WPARAM(0),
            LPARAM(w!("ImmersiveColorSet").as_ptr() as isize),
            SMTO_ABORTIFHUNG,
            2000,
            None,
        );
    });
}
