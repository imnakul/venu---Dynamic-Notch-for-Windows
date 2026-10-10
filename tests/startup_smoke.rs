#![cfg(target_os = "windows")]

use std::fs::{self, File};
use std::io::Read;
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use windows::Win32::Foundation::{BOOL, HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible,
    PostMessageW, WM_CLOSE, WM_LBUTTONUP, WM_USER,
};

const TRAY_TITLE: &str = "VenuTray";
const SETTINGS_TITLE: &str = "Venu - Settings";
// Must stay in sync with WM_TRAY_ICON in src/tray.rs.
const WM_TRAY_ICON: u32 = WM_USER + 101;
const POLL_INTERVAL: Duration = Duration::from_millis(100);
const WINDOW_TIMEOUT: Duration = Duration::from_secs(8);
const QUIET_START_OBSERVATION: Duration = Duration::from_millis(1500);

struct ChildProcess {
    child: Child,
    diagnostic_dir: PathBuf,
    remove_diagnostics: bool,
}

impl ChildProcess {
    fn launch(args: &[&str], label: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let temp_root = std::env::var_os("RUNNER_TEMP")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let diagnostic_dir = temp_root.join(format!(
            "venu-startup-smoke-{}-{label}-{nonce}",
            std::process::id()
        ));
        let local_app_data = diagnostic_dir.join("LocalAppData");
        let roaming_app_data = diagnostic_dir.join("AppData");
        fs::create_dir_all(&local_app_data).expect("create isolated LocalAppData");
        fs::create_dir_all(&roaming_app_data).expect("create isolated AppData");

        let stdout = File::create(diagnostic_dir.join("stdout.log")).expect("create stdout log");
        let stderr = File::create(diagnostic_dir.join("stderr.log")).expect("create stderr log");
        let child = Command::new(env!("CARGO_BIN_EXE_venu"))
            .args(args)
            .env("APPDATA", roaming_app_data)
            .env("LOCALAPPDATA", local_app_data)
            .env("VENU_STARTUP_SMOKE", "1")
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .spawn()
            .expect("Venu should start");
        Self {
            child,
            diagnostic_dir,
            remove_diagnostics: false,
        }
    }

    fn id(&self) -> u32 {
        self.child.id()
    }

    fn assert_running(&mut self, phase: &str) {
        if self
            .child
            .try_wait()
            .expect("query child process")
            .is_some()
        {
            panic!("Venu exited during {phase}.\n{}", self.diagnostics(phase));
        }
    }

    fn diagnostics(&mut self, phase: &str) -> String {
        let status = match self.child.try_wait() {
            Ok(Some(status)) => format!("exited with {status}"),
            Ok(None) => "still running".to_owned(),
            Err(error) => format!("process status unavailable: {error}"),
        };
        let windows = process_windows(self.id())
            .into_iter()
            .map(|window| {
                format!(
                    "title={:?} class={:?} visible={}",
                    window.title, window.class_name, window.visible
                )
            })
            .collect::<Vec<_>>()
            .join("\n  ");
        let startup_log = read_diagnostic_file(
            &self
                .diagnostic_dir
                .join("LocalAppData")
                .join("Venu")
                .join("startup.log"),
        );
        let stdout = read_diagnostic_file(&self.diagnostic_dir.join("stdout.log"));
        let stderr = read_diagnostic_file(&self.diagnostic_dir.join("stderr.log"));
        let secondary_stdout =
            read_diagnostic_file(&self.diagnostic_dir.join("secondary-settings.stdout.log"));
        let secondary_stderr =
            read_diagnostic_file(&self.diagnostic_dir.join("secondary-settings.stderr.log"));
        format!(
            "phase={phase}\npid={} process={status}\nwindow enumeration:\n  {}\nstartup.log:\n{}\nstdout.log:\n{}\nstderr.log:\n{}\nsecondary-settings stdout:\n{}\nsecondary-settings stderr:\n{}\ndiagnostics at {}",
            self.id(),
            if windows.is_empty() { "(no top-level windows)" } else { &windows },
            startup_log,
            stdout,
            stderr,
            secondary_stdout,
            secondary_stderr,
            self.diagnostic_dir.display()
        )
    }

    fn stop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    fn clean_after_success(&mut self) {
        self.remove_diagnostics = true;
    }

    fn run_secondary(&mut self, args: &[&str], label: &str) -> ExitStatus {
        let stdout = File::create(self.diagnostic_dir.join(format!("{label}.stdout.log")))
            .expect("create secondary stdout log");
        let stderr = File::create(self.diagnostic_dir.join(format!("{label}.stderr.log")))
            .expect("create secondary stderr log");
        let mut child = Command::new(env!("CARGO_BIN_EXE_venu"))
            .args(args)
            .env("APPDATA", self.diagnostic_dir.join("AppData"))
            .env("LOCALAPPDATA", self.diagnostic_dir.join("LocalAppData"))
            .env("VENU_STARTUP_SMOKE", "1")
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .spawn()
            .expect("launch secondary Venu request");
        let deadline = Instant::now() + WINDOW_TIMEOUT;
        loop {
            if let Some(status) = child.try_wait().expect("query secondary process") {
                return status;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let status = child.wait().ok();
                panic!(
                    "secondary Venu request {args:?} did not exit within {WINDOW_TIMEOUT:?} (status {status:?}).\n{}",
                    self.diagnostics(label)
                );
            }
            thread::sleep(POLL_INTERVAL);
        }
    }
}

impl Drop for ChildProcess {
    fn drop(&mut self) {
        self.stop();
        if self.remove_diagnostics {
            let _ = fs::remove_dir_all(&self.diagnostic_dir);
        }
    }
}

fn read_diagnostic_file(path: &std::path::Path) -> String {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(_) => return "(not written)".to_owned(),
    };
    let mut text = String::new();
    let _ = file.read_to_string(&mut text);
    const MAX: usize = 8 * 1024;
    if text.len() > MAX {
        text = text[text.len() - MAX..].to_owned();
    }
    if text.trim().is_empty() {
        "(empty)".to_owned()
    } else {
        text
    }
}

struct WindowInfo {
    hwnd: HWND,
    title: String,
    class_name: String,
    visible: bool,
}

unsafe extern "system" fn collect_process_window(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let (process_id, windows) = &mut *(lparam.0 as *mut (u32, Vec<WindowInfo>));
    let mut owner = 0;
    GetWindowThreadProcessId(hwnd, Some(&mut owner));
    if owner != *process_id {
        return BOOL(1);
    }

    let mut title = [0u16; 512];
    let title_len = GetWindowTextW(hwnd, &mut title).max(0) as usize;
    let mut class_name = [0u16; 256];
    let class_len = GetClassNameW(hwnd, &mut class_name).max(0) as usize;
    windows.push(WindowInfo {
        hwnd,
        title: String::from_utf16_lossy(&title[..title_len]),
        class_name: String::from_utf16_lossy(&class_name[..class_len]),
        visible: IsWindowVisible(hwnd).as_bool(),
    });
    BOOL(1)
}

fn process_windows(process_id: u32) -> Vec<WindowInfo> {
    let mut search = (process_id, Vec::new());
    unsafe {
        let _ = EnumWindows(
            Some(collect_process_window),
            LPARAM(&mut search as *mut (u32, Vec<WindowInfo>) as isize),
        );
    }
    search.1
}

fn find_process_window(process_id: u32, title: &str) -> Option<HWND> {
    process_windows(process_id)
        .into_iter()
        .find(|window| window.title == title)
        .map(|window| window.hwnd)
}

fn wait_for_window(child: &mut ChildProcess, title: &str, phase: &str) -> HWND {
    let deadline = Instant::now() + WINDOW_TIMEOUT;
    loop {
        child.assert_running(phase);
        if let Some(hwnd) = find_process_window(child.id(), title) {
            if unsafe { IsWindowVisible(hwnd).as_bool() } {
                return hwnd;
            }
        }
        if Instant::now() >= deadline {
            panic!(
                "timed out waiting for visible window {title:?}.\n{}",
                child.diagnostics(phase)
            );
        }
        thread::sleep(POLL_INTERVAL);
    }
}

fn wait_for_window_handle(child: &mut ChildProcess, title: &str, phase: &str) -> HWND {
    let deadline = Instant::now() + WINDOW_TIMEOUT;
    loop {
        child.assert_running(phase);
        if let Some(hwnd) = find_process_window(child.id(), title) {
            return hwnd;
        }
        if Instant::now() >= deadline {
            panic!(
                "timed out waiting for HWND titled {title:?}.\n{}",
                child.diagnostics(phase)
            );
        }
        thread::sleep(POLL_INTERVAL);
    }
}

fn wait_for_no_window(child: &mut ChildProcess, title: &str, phase: &str) {
    let deadline = Instant::now() + WINDOW_TIMEOUT;
    loop {
        child.assert_running(phase);
        if find_process_window(child.id(), title).is_none() {
            return;
        }
        if Instant::now() >= deadline {
            panic!("unexpected {title:?} window.\n{}", child.diagnostics(phase));
        }
        thread::sleep(POLL_INTERVAL);
    }
}

fn assert_window_stays_absent(
    child: &mut ChildProcess,
    title: &str,
    duration: Duration,
    phase: &str,
) {
    let deadline = Instant::now() + duration;
    loop {
        child.assert_running(phase);
        if find_process_window(child.id(), title).is_some() {
            panic!("unexpected {title:?} window.\n{}", child.diagnostics(phase));
        }
        if Instant::now() >= deadline {
            return;
        }
        thread::sleep(POLL_INTERVAL);
    }
}

fn wait_for_hidden_window(child: &mut ChildProcess, title: &str, phase: &str) {
    let deadline = Instant::now() + WINDOW_TIMEOUT;
    loop {
        child.assert_running(phase);
        let hidden = find_process_window(child.id(), title)
            .is_none_or(|hwnd| unsafe { !IsWindowVisible(hwnd).as_bool() });
        if hidden {
            return;
        }
        if Instant::now() >= deadline {
            panic!("{title:?} did not hide.\n{}", child.diagnostics(phase));
        }
        thread::sleep(POLL_INTERVAL);
    }
}

fn close_settings(child: &mut ChildProcess, phase: &str) {
    let hwnd = wait_for_window(child, SETTINGS_TITLE, phase);
    unsafe {
        PostMessageW(hwnd, WM_CLOSE, WPARAM(0), LPARAM(0)).expect("post close request");
    }
    wait_for_hidden_window(child, SETTINGS_TITLE, "Settings close-to-tray");
}

#[test]
fn tray_default_startup_and_settings_restore_work_at_runtime() {
    let mut app = ChildProcess::launch(&[], "tray");
    // The tray owner HWND is intentionally hidden; Shell_NotifyIcon renders
    // its icon separately in Explorer's notification area.
    let tray = wait_for_window_handle(&mut app, TRAY_TITLE, "ordinary launch creates tray HWND");
    assert_window_stays_absent(
        &mut app,
        SETTINGS_TITLE,
        QUIET_START_OBSERVATION,
        "ordinary launch stays tray-only",
    );

    // A sign-in style launch stays quiet when another copy is already running.
    let status = app.run_secondary(&["--startup"], "secondary-startup");
    assert!(status.success());
    wait_for_no_window(
        &mut app,
        SETTINGS_TITLE,
        "secondary --startup remains quiet",
    );

    // The explicit request must be routed to the already running tray process.
    let status = app.run_secondary(&["--settings"], "secondary-settings");
    assert!(status.success());
    wait_for_window(
        &mut app,
        SETTINGS_TITLE,
        "second-instance --settings opens primary window",
    );
    app.assert_running("primary Settings window opened");

    close_settings(&mut app, "Settings window is visible before close");
    unsafe {
        PostMessageW(tray, WM_TRAY_ICON, WPARAM(0), LPARAM(WM_LBUTTONUP as isize))
            .expect("post tray click");
    }
    wait_for_window(&mut app, SETTINGS_TITLE, "tray callback reopens Settings");
    app.assert_running("tray callback reopened Settings");
    app.stop();

    // With no existing process, --settings must create a visible window itself.
    let mut direct_settings = ChildProcess::launch(&["--settings"], "settings");
    wait_for_window(
        &mut direct_settings,
        SETTINGS_TITLE,
        "first-instance --settings opens Settings",
    );
    direct_settings.assert_running("first-instance Settings window opened");
    direct_settings.stop();

    app.clean_after_success();
    direct_settings.clean_after_success();
}
