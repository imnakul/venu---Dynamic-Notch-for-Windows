//! Per-user "launch on startup" support.
//!
//! Two mechanisms, picked by how Venu was installed:
//!
//! * **Microsoft Store (MSIX) build** — Windows' own `StartupTask`, declared in
//!   the package manifest with [`STORE_TASK_ID`]. It starts off; switching it
//!   on asks Windows, and if the user has turned Venu off in Settings › Apps ›
//!   Startup (or a policy has), Windows says no and Venu respects that rather
//!   than finding another way in. A packaged app has no business writing the
//!   Run key, so nothing below the next heading runs for it.
//! * **Portable exe and Inno Setup installer** — the registry, as follows.
//!
//! Venu registers itself under `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`,
//! so no elevation and no service is involved — the entry belongs to the
//! signed-in user and can be turned off here, in Task Manager's Startup list,
//! or by uninstalling.
//!
//! The registered command always points at the copy of `venu.exe` that last
//! wrote the entry and carries `--startup`, which makes a sign-in launch come
//! up quietly in the tray instead of opening the settings window over the
//! desktop. [`crate::main`] keeps the entry refreshed on every run so an
//! updated or moved binary never leaves a stale path behind.

use std::path::{Path, PathBuf};

use windows::ApplicationModel::{StartupTask, StartupTaskState};
use windows::Win32::System::WinRT::{RoInitialize, RoUninitialize, RO_INIT_MULTITHREADED};

use windows::core::{HSTRING, PCWSTR};
use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyW, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
    HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_SAM_FLAGS, REG_SZ,
};

/// The standard per-user autostart key. Explorer starts every value here at
/// sign-in.
const RUN_KEY_PATH: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

/// Task Manager's enable/disable state for Run-key entries lives next door.
/// Cleaning our value out too keeps the Startup list from showing a ghost.
const STARTUP_APPROVED_KEY_PATH: &str =
    r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";

/// The one Run-key value this app owns.
const VALUE_NAME: &str = "Venu";

/// Appended to the registered command so a sign-in launch starts quietly.
const QUIET_FLAG: &str = "--startup";

/// The startup task's id. Must match the `TaskId` of the
/// `windows.startupTask` extension in the Store package's AppxManifest.xml
/// (see docs/store-packaging.md).
pub const STORE_TASK_ID: &str = "VenuStartup";

/// What launch-on-startup ended up as after a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartupState {
    On,
    Off,
    /// The user switched Venu off in Windows' Startup settings. Only they can
    /// switch it back on, there.
    BlockedByUser,
    /// An administrator's policy decides.
    BlockedByPolicy,
}

impl StartupState {
    pub fn is_on(self) -> bool {
        self == StartupState::On
    }

    fn from_task(state: StartupTaskState) -> Self {
        match state {
            StartupTaskState::Enabled | StartupTaskState::EnabledByPolicy => StartupState::On,
            StartupTaskState::DisabledByUser => StartupState::BlockedByUser,
            StartupTaskState::DisabledByPolicy => StartupState::BlockedByPolicy,
            _ => StartupState::Off,
        }
    }
}

/// Whether startup is Windows' to manage: the Store build.
pub fn managed_by_windows() -> bool {
    crate::native_notifications::has_package_identity()
}

/// Run a WinRT call on a short-lived thread with its own multithreaded
/// apartment, so blocking on the async result never ties up — or assumes
/// anything about — the caller's thread (the Settings window's, at times).
fn on_winrt_thread<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    std::thread::Builder::new()
        .name("Venu startup task".to_owned())
        .spawn(move || {
            let initialized = unsafe { RoInitialize(RO_INIT_MULTITHREADED) }.is_ok();
            let out = f();
            if initialized {
                unsafe { RoUninitialize() };
            }
            out
        })
        .map_err(|e| format!("could not start the startup-task thread: {e}"))?
        .join()
        .map_err(|_| "the startup-task thread panicked".to_owned())?
}

fn store_task() -> Result<StartupTask, String> {
    StartupTask::GetAsync(&HSTRING::from(STORE_TASK_ID))
        .and_then(|op| op.get())
        .map_err(|e| format!("Windows has no startup task \"{STORE_TASK_ID}\" for Venu: {e}"))
}

/// The Store build's startup state as Windows sees it.
pub fn store_state() -> Result<StartupState, String> {
    on_winrt_thread(|| {
        let task = store_task()?;
        task.State()
            .map(StartupState::from_task)
            .map_err(|e| format!("could not read the startup task: {e}"))
    })
}

fn store_enable() -> Result<StartupState, String> {
    on_winrt_thread(|| {
        let task = store_task()?;
        let state = task
            .State()
            .map_err(|e| format!("could not read the startup task: {e}"))?;
        // Asking again after the user said no would be ignored anyway, and
        // is not ours to ask.
        if state == StartupTaskState::DisabledByUser || state == StartupTaskState::DisabledByPolicy
        {
            return Ok(StartupState::from_task(state));
        }
        task.RequestEnableAsync()
            .and_then(|op| op.get())
            .map(StartupState::from_task)
            .map_err(|e| format!("Windows did not enable the startup task: {e}"))
    })
}

fn store_disable() -> Result<(), String> {
    on_winrt_thread(|| {
        store_task()?
            .Disable()
            .map_err(|e| format!("could not disable the startup task: {e}"))
    })
}

type RegResult<T> = Result<T, u32>;

/// The command line stored under the Run key for `exe`.
///
/// The path is quoted because install locations can contain spaces.
fn command_for(exe: &Path) -> String {
    format!("\"{}\" {QUIET_FLAG}", exe.display())
}

fn this_exe() -> Result<PathBuf, String> {
    std::env::current_exe().map_err(|e| format!("cannot resolve the running executable: {e}"))
}

/// Open a subkey of HKCU, run `f` with it, and always close it again.
fn with_key<T>(
    subkey: &str,
    rights: REG_SAM_FLAGS,
    f: impl FnOnce(HKEY) -> RegResult<T>,
) -> RegResult<T> {
    unsafe {
        let mut key = HKEY::default();
        let sub = HSTRING::from(subkey);
        let err = RegOpenKeyExW(HKEY_CURRENT_USER, PCWSTR(sub.as_ptr()), 0, rights, &mut key);
        if err != ERROR_SUCCESS {
            return Err(err.0);
        }
        let out = f(key);
        let _ = RegCloseKey(key);
        out
    }
}

fn set_value(value_name: &str, data: &str) -> RegResult<()> {
    // A clean Windows profile (including GitHub Actions runners) may not have
    // the standard Run key yet. Create/open it for writes instead of assuming
    // Explorer has already created it. RegCreateKeyW is idempotent for an
    // existing key and gives us the write access needed for RegSetValueExW.
    unsafe {
        let mut key = HKEY::default();
        let sub = HSTRING::from(RUN_KEY_PATH);
        let err = RegCreateKeyW(HKEY_CURRENT_USER, PCWSTR(sub.as_ptr()), &mut key);
        if err != ERROR_SUCCESS {
            return Err(err.0);
        }

        let name = HSTRING::from(value_name);
        let mut wide: Vec<u16> = data.encode_utf16().collect();
        wide.push(0);
        let mut bytes = Vec::with_capacity(wide.len() * 2);
        for unit in &wide {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        let err = RegSetValueExW(key, PCWSTR(name.as_ptr()), 0, REG_SZ, Some(&bytes));
        let _ = RegCloseKey(key);
        if err == ERROR_SUCCESS {
            Ok(())
        } else {
            Err(err.0)
        }
    }
}

fn value_exists(value_name: &str) -> RegResult<bool> {
    with_key(RUN_KEY_PATH, KEY_QUERY_VALUE, |key| unsafe {
        let name = HSTRING::from(value_name);
        let err = RegQueryValueExW(key, PCWSTR(name.as_ptr()), None, None, None, None);
        match err {
            ERROR_SUCCESS => Ok(true),
            ERROR_FILE_NOT_FOUND => Ok(false),
            other => Err(other.0),
        }
    })
}

fn query_value(value_name: &str) -> RegResult<Option<String>> {
    with_key(RUN_KEY_PATH, KEY_QUERY_VALUE, |key| unsafe {
        let name = HSTRING::from(value_name);

        // Size probe first, then one read into a buffer of exactly that size.
        let mut len: u32 = 0;
        let err = RegQueryValueExW(key, PCWSTR(name.as_ptr()), None, None, None, Some(&mut len));
        if err == ERROR_FILE_NOT_FOUND {
            return Ok(None);
        }
        if err != ERROR_SUCCESS {
            return Err(err.0);
        }

        let mut buf = vec![0u8; len.max(2) as usize];
        let mut got = len;
        let err = RegQueryValueExW(
            key,
            PCWSTR(name.as_ptr()),
            None,
            None,
            Some(buf.as_mut_ptr()),
            Some(&mut got),
        );
        if err != ERROR_SUCCESS {
            return Err(err.0);
        }

        let bytes = &buf[..(got as usize).min(buf.len())];
        let wide: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .take_while(|&u| u != 0)
            .collect();
        Ok(Some(String::from_utf16_lossy(&wide)))
    })
}

fn remove_value(subkey: &str, value_name: &str) -> RegResult<()> {
    with_key(subkey, KEY_SET_VALUE, |key| unsafe {
        let name = HSTRING::from(value_name);
        let err = RegDeleteValueW(key, PCWSTR(name.as_ptr()));
        if err == ERROR_SUCCESS || err == ERROR_FILE_NOT_FOUND {
            Ok(())
        } else {
            Err(err.0)
        }
    })
}

/// Whether a `Venu` entry currently exists under the Run key. Always false
/// for the Store build, which never uses it.
pub fn is_registered() -> bool {
    !managed_by_windows() && value_exists(VALUE_NAME).unwrap_or(false)
}

/// The command the autostart entry will run, if one is registered. `None`
/// for the Store build: Windows starts the packaged app itself.
pub fn registered_command() -> Option<String> {
    if managed_by_windows() {
        return None;
    }
    query_value(VALUE_NAME).ok().flatten()
}

/// Switch launch-on-startup on. The registry build writes the entry so
/// sign-in starts *this* binary quietly; the Store build asks Windows, which
/// may answer that the user or a policy has it switched off.
pub fn enable() -> Result<StartupState, String> {
    if managed_by_windows() {
        return store_enable();
    }
    let exe = this_exe()?;
    set_value(VALUE_NAME, &command_for(&exe))
        .map(|()| StartupState::On)
        .map_err(|e| format!("could not write the autostart registry entry (Win32 error {e})"))
}

/// Switch launch-on-startup off: remove the entry (and the Task Manager
/// approval leftover), or disable the Store startup task.
pub fn disable() -> Result<(), String> {
    if managed_by_windows() {
        return store_disable();
    }
    remove_value(RUN_KEY_PATH, VALUE_NAME)
        .map_err(|e| format!("could not remove the autostart registry entry (Win32 error {e})"))?;
    // Best effort: the approval entry is Explorer bookkeeping and may not exist.
    let _ = remove_value(STARTUP_APPROVED_KEY_PATH, VALUE_NAME);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_VALUE: &str = "VenuTest";

    #[test]
    fn startup_task_states_map_to_what_the_toggle_can_show() {
        assert_eq!(
            StartupState::from_task(StartupTaskState::Enabled),
            StartupState::On
        );
        assert_eq!(
            StartupState::from_task(StartupTaskState::EnabledByPolicy),
            StartupState::On
        );
        assert_eq!(
            StartupState::from_task(StartupTaskState::Disabled),
            StartupState::Off
        );
        assert_eq!(
            StartupState::from_task(StartupTaskState::DisabledByUser),
            StartupState::BlockedByUser
        );
        assert_eq!(
            StartupState::from_task(StartupTaskState::DisabledByPolicy),
            StartupState::BlockedByPolicy
        );
    }

    #[test]
    fn command_is_quoted_and_quiet() {
        let cmd = command_for(Path::new(r"C:\Program Files\Venu\venu.exe"));
        assert_eq!(cmd, r#""C:\Program Files\Venu\venu.exe" --startup"#);
    }

    #[test]
    fn registry_roundtrip_uses_a_scratch_value() {
        // Deliberately not the real `Venu` value: the test writes, reads and
        // removes a scratch entry, so running `cargo test` never flips the
        // user's actual autostart state.
        set_value(TEST_VALUE, r#""C:\somewhere\venu.exe" --startup"#)
            .expect("write under HKCU Run");
        assert!(matches!(value_exists(TEST_VALUE), Ok(true)));
        assert_eq!(
            query_value(TEST_VALUE).expect("query under HKCU Run"),
            Some(r#""C:\somewhere\venu.exe" --startup"#.to_string())
        );
        remove_value(RUN_KEY_PATH, TEST_VALUE).expect("remove scratch value");
        assert!(matches!(value_exists(TEST_VALUE), Ok(false)));
        // Removing something already gone is not an error.
        assert!(remove_value(RUN_KEY_PATH, TEST_VALUE).is_ok());
    }
}
