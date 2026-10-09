#![cfg(target_os = "windows")]

use std::process::{Child, Command};
use std::thread;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{BOOL, HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible, PostMessageW, WM_CLOSE,
    WM_LBUTTONUP, WM_USER,
};

const TRAY_TITLE: &str = "VenuTray";
const SETTINGS_TITLE: &str = "Venu - Settings";
// Must stay in sync with WM_TRAY_ICON in src/tray.rs.
const WM_TRAY_ICON: u32 = WM_USER + 101;
const POLL_INTERVAL: Duration = Duration::from_millis(100);
const WINDOW_TIMEOUT: Duration = Duration::from_secs(8);
const QUIET_START_OBSERVATION: Duration = Duration::from_millis(1500);

struct ChildProcess(Child);

impl ChildProcess {
    fn launch(args: &[&str]) -> Self {
        let child = Command::new(env!("CARGO_BIN_EXE_venu"))
            .args(args)
            .spawn()
            .expect("Venu should start");
        Self(child)
    }

    fn id(&self) -> u32 {
        self.0.id()
    }

    fn assert_running(&mut self) {
        assert!(
            self.0.try_wait().expect("query child process").is_none(),
            "Venu exited before the smoke check completed"
        );
    }

    fn stop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

impl Drop for ChildProcess {
    fn drop(&mut self) {
        self.stop();
    }
}

struct WindowSearch {
    process_id: u32,
    title: String,
    found: Option<HWND>,
}

unsafe extern "system" fn find_window_for_process(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let search = &mut *(lparam.0 as *mut WindowSearch);
    let mut process_id = 0;
    GetWindowThreadProcessId(hwnd, Some(&mut process_id));
    if process_id != search.process_id {
        return BOOL(1);
    }

    let mut title = [0u16; 256];
    let length = GetWindowTextW(hwnd, &mut title);
    if length > 0 && String::from_utf16_lossy(&title[..length as usize]) == search.title {
        search.found = Some(hwnd);
        return BOOL(0);
    }
    BOOL(1)
}

fn find_process_window(process_id: u32, title: &str) -> Option<HWND> {
    let mut search = WindowSearch {
        process_id,
        title: title.to_owned(),
        found: None,
    };
    unsafe {
        let _ = EnumWindows(
            Some(find_window_for_process),
            LPARAM(&mut search as *mut WindowSearch as isize),
        );
    }
    search.found
}

fn wait_for_window(process_id: u32, title: &str) -> HWND {
    let deadline = Instant::now() + WINDOW_TIMEOUT;
    loop {
        if let Some(hwnd) = find_process_window(process_id, title) {
            if unsafe { IsWindowVisible(hwnd).as_bool() } {
                return hwnd;
            }
        }
        assert!(Instant::now() < deadline, "timed out waiting for {title}");
        thread::sleep(POLL_INTERVAL);
    }
}

fn wait_for_no_window(process_id: u32, title: &str) {
    let deadline = Instant::now() + WINDOW_TIMEOUT;
    loop {
        if find_process_window(process_id, title).is_none() {
            return;
        }
        assert!(Instant::now() < deadline, "unexpected {title} window");
        thread::sleep(POLL_INTERVAL);
    }
}

fn assert_window_stays_absent(process_id: u32, title: &str, duration: Duration) {
    let deadline = Instant::now() + duration;
    loop {
        assert!(
            find_process_window(process_id, title).is_none(),
            "unexpected {title} window"
        );
        if Instant::now() >= deadline {
            return;
        }
        thread::sleep(POLL_INTERVAL);
    }
}

fn wait_for_hidden_window(process_id: u32, title: &str) {
    let deadline = Instant::now() + WINDOW_TIMEOUT;
    loop {
        let hidden = find_process_window(process_id, title)
            .is_none_or(|hwnd| unsafe { !IsWindowVisible(hwnd).as_bool() });
        if hidden {
            return;
        }
        assert!(Instant::now() < deadline, "{title} did not hide");
        thread::sleep(POLL_INTERVAL);
    }
}

fn close_settings(process_id: u32) {
    let hwnd = wait_for_window(process_id, SETTINGS_TITLE);
    unsafe {
        PostMessageW(hwnd, WM_CLOSE, WPARAM(0), LPARAM(0)).expect("post close request");
    }
    wait_for_hidden_window(process_id, SETTINGS_TITLE);
}

#[test]
fn tray_default_startup_and_settings_restore_work_at_runtime() {
    let mut app = ChildProcess::launch(&[]);
    let app_id = app.id();

    // Hosted Windows runners may not expose Explorer's notification area, so
    // assert the user-visible contract: ordinary launch keeps running without
    // creating eframe's Settings window. Creating a tray HWND is exercised
    // below when the runner makes it observable.
    assert_window_stays_absent(app_id, SETTINGS_TITLE, QUIET_START_OBSERVATION);
    app.assert_running();

    // A sign-in style launch is quiet when another copy is already running.
    let status = Command::new(env!("CARGO_BIN_EXE_venu"))
        .arg("--startup")
        .status()
        .expect("launch --startup request");
    assert!(status.success());
    wait_for_no_window(app_id, SETTINGS_TITLE);

    // When the test session exposes the hidden tray HWND, cover the same
    // second-instance path used by the explicit flag and the tray callback.
    if let Some(tray) = find_process_window(app_id, TRAY_TITLE) {
        let status = Command::new(env!("CARGO_BIN_EXE_venu"))
            .arg("--settings")
            .status()
            .expect("launch --settings request");
        assert!(status.success());
        wait_for_window(app_id, SETTINGS_TITLE);
        app.assert_running();

        close_settings(app_id);
        unsafe {
            PostMessageW(tray, WM_TRAY_ICON, WPARAM(0), LPARAM(WM_LBUTTONUP as isize))
                .expect("post tray click");
        }
        wait_for_window(app_id, SETTINGS_TITLE);
        app.assert_running();
    } else {
        eprintln!("Hosted Windows session did not expose a tray HWND; tray callback smoke skipped");
    }

    app.stop();

    // Also check the explicit flag when it is the first launch.
    let mut direct_settings = ChildProcess::launch(&["--settings"]);
    wait_for_window(direct_settings.id(), SETTINGS_TITLE);
    direct_settings.assert_running();
}
