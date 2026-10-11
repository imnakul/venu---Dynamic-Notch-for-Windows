//! Opt-in bridge to Windows' supported toast notification listener.
//!
//! Reading the Windows notification center requires package identity and user
//! consent. Permission is requested only from the explicit Settings action on
//! the UI thread. Collection and snapshot parsing happen on one bounded worker.

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::sync::OnceLock;
use std::thread;

use parking_lot::RwLock;
use windows::core::PWSTR;
use windows::Foundation::{AsyncOperationCompletedHandler, AsyncStatus, TypedEventHandler};
use windows::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, FILETIME, SYSTEMTIME};
use windows::Win32::Storage::FileSystem::FileTimeToLocalFileTime;
use windows::Win32::Storage::Packaging::Appx::GetCurrentPackageFullName;
use windows::Win32::System::Time::FileTimeToSystemTime;
use windows::Win32::System::WinRT::{
    RoInitialize, RoUninitialize, RO_INIT_MULTITHREADED, RO_INIT_SINGLETHREADED,
};
use windows::UI::Notifications::Management::{
    UserNotificationListener, UserNotificationListenerAccessStatus,
};
use windows::UI::Notifications::{
    KnownNotificationBindings, NotificationKinds, UserNotification,
    UserNotificationChangedEventArgs,
};

use crate::config::AppConfig;
use crate::notch::notify::{self, NativeNotification};

const DISABLED: u8 = 0;
const NEEDS_IDENTITY: u8 = 1;
const NEEDS_CONSENT: u8 = 2;
const REQUESTING: u8 = 3;
const CONNECTED: u8 = 4;
const DENIED: u8 = 5;
const ERROR: u8 = 6;

#[derive(Clone, Copy)]
enum AccessState {
    Allowed,
    Denied,
    NeedsConsent,
    Error,
}

#[derive(Clone, Copy)]
enum Command {
    Wake,
}

static STATUS: AtomicU8 = AtomicU8::new(DISABLED);
static WORKER: OnceLock<SyncSender<Command>> = OnceLock::new();
static REQUESTED_ENABLED: AtomicBool = AtomicBool::new(false);
static TOAST_DURATION_BITS: AtomicU32 = AtomicU32::new(4.5f32.to_bits());
static UI_WINRT_INITIALIZED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

fn set_status(status: u8) {
    let previous = STATUS.swap(status, Ordering::SeqCst);
    if previous != status {
        if let Some(ctx) = crate::gui::get_egui_context() {
            ctx.request_repaint();
        }
    }
}

fn permission_status(enabled: bool, has_identity: bool, access: AccessState) -> u8 {
    if !enabled {
        DISABLED
    } else if !has_identity {
        NEEDS_IDENTITY
    } else {
        match access {
            AccessState::Allowed => CONNECTED,
            AccessState::Denied => DENIED,
            AccessState::NeedsConsent => NEEDS_CONSENT,
            AccessState::Error => ERROR,
        }
    }
}

pub fn status_label() -> &'static str {
    match STATUS.load(Ordering::SeqCst) {
        NEEDS_IDENTITY => "Package identity required",
        NEEDS_CONSENT => "Windows permission needed",
        REQUESTING => "Waiting for Windows permission",
        CONNECTED => "Connected · all Windows apps",
        DENIED => "Permission denied in Windows Settings",
        ERROR => "Could not connect",
        _ => "Off",
    }
}

pub fn is_connected() -> bool {
    STATUS.load(Ordering::SeqCst) == CONNECTED
}

pub fn set_toast_duration(duration_secs: f32) {
    TOAST_DURATION_BITS.store(duration_secs.to_bits(), Ordering::Relaxed);
}

/// Whether this process has the sparse/full package identity required by the
/// Windows listener API. The ordinary portable and Inno builds do not.
fn has_package_identity() -> bool {
    let mut length = 0u32;
    let status = unsafe { GetCurrentPackageFullName(&mut length, PWSTR(std::ptr::null_mut())) };
    status.0 == 0 || status == ERROR_INSUFFICIENT_BUFFER
}

fn worker_sender() -> Option<&'static SyncSender<Command>> {
    if let Some(sender) = WORKER.get() {
        return Some(sender);
    }

    let (sender, receiver) = sync_channel(1);
    if WORKER.set(sender.clone()).is_ok() {
        let worker_sender = sender.clone();
        if thread::Builder::new()
            .name("Venu Windows notifications".to_owned())
            .spawn(move || worker_loop(receiver, worker_sender))
            .is_err()
        {
            set_status(ERROR);
            return None;
        }
    }
    WORKER.get()
}

/// Start or stop the listener according to the saved opt-in preference.
/// Disabled installs never create the worker thread.
pub fn set_enabled(enabled: bool) {
    REQUESTED_ENABLED.store(enabled, Ordering::SeqCst);
    if enabled {
        if let Some(sender) = worker_sender() {
            if let Err(TrySendError::Disconnected(_)) = sender.try_send(Command::Wake) {
                set_status(ERROR);
            }
        }
    } else {
        if let Some(sender) = WORKER.get() {
            // If the bounded queue is full, a pending wake will observe the
            // requested state and detach the listener. Never block the UI on
            // a WinRT snapshot already being collected by the worker.
            let _ = sender.try_send(Command::Wake);
        }
        set_status(DISABLED);
    }
}

/// Run only after the user clicked Connect in Settings. This is the only call
/// site for RequestAccessAsync and it is executed after releasing AppConfig's
/// write lock on eframe's UI thread.
pub fn request_access_on_ui_thread(config: std::sync::Arc<RwLock<AppConfig>>) {
    if !has_package_identity() {
        set_status(NEEDS_IDENTITY);
        return;
    }

    if !UI_WINRT_INITIALIZED.load(Ordering::SeqCst) {
        let initialized = unsafe { RoInitialize(RO_INIT_SINGLETHREADED) };
        if initialized.is_err() {
            set_status(ERROR);
            return;
        }
        UI_WINRT_INITIALIZED.store(true, Ordering::SeqCst);
    }

    let listener = match UserNotificationListener::Current() {
        Ok(listener) => listener,
        Err(_) => {
            set_status(ERROR);
            return;
        }
    };

    match listener.GetAccessStatus() {
        Ok(UserNotificationListenerAccessStatus::Allowed) => {
            persist_opt_in(&config, true);
            set_status(CONNECTED);
            set_enabled(true);
        }
        Ok(UserNotificationListenerAccessStatus::Denied) => set_status(DENIED),
        Ok(_) => {
            set_status(REQUESTING);
            let operation = match listener.RequestAccessAsync() {
                Ok(operation) => operation,
                Err(_) => {
                    set_status(ERROR);
                    return;
                }
            };
            let config = std::sync::Arc::clone(&config);
            let completed =
                AsyncOperationCompletedHandler::<UserNotificationListenerAccessStatus>::new(
                    move |operation, status| {
                        if status != AsyncStatus::Completed {
                            set_status(ERROR);
                            return Ok(());
                        }
                        let Some(operation) = operation else {
                            set_status(ERROR);
                            return Ok(());
                        };
                        let access = match operation.GetResults() {
                            Ok(access) => access,
                            Err(_) => {
                                set_status(ERROR);
                                return Ok(());
                            }
                        };
                        if access == UserNotificationListenerAccessStatus::Allowed {
                            persist_opt_in(&config, true);
                            set_status(CONNECTED);
                            set_enabled(true);
                        } else {
                            persist_opt_in(&config, false);
                            set_status(DENIED);
                        }
                        Ok(())
                    },
                );
            if operation.SetCompleted(&completed).is_err() {
                set_status(ERROR);
            }
        }
        Err(_) => set_status(ERROR),
    }
}

fn persist_opt_in(config: &std::sync::Arc<RwLock<AppConfig>>, enabled: bool) {
    let mut config = config.write();
    if config.notch.notifications.windows_listener_enabled != enabled {
        config.notch.notifications.windows_listener_enabled = enabled;
        config.save();
    }
}

fn worker_loop(receiver: Receiver<Command>, sender: SyncSender<Command>) {
    if unsafe { RoInitialize(RO_INIT_MULTITHREADED) }.is_err() {
        set_status(ERROR);
        return;
    }

    let mut listener: Option<UserNotificationListener> = None;
    let mut registration = None;
    let mut seeded = false;

    while receiver.recv().is_ok() {
        if !REQUESTED_ENABLED.load(Ordering::SeqCst) {
            detach_listener(&mut listener, &mut registration);
            seeded = false;
            set_status(DISABLED);
            continue;
        }

        if listener.is_none() {
            if !has_package_identity() {
                set_status(NEEDS_IDENTITY);
                continue;
            }
            let current = match UserNotificationListener::Current() {
                Ok(listener) => listener,
                Err(_) => {
                    set_status(ERROR);
                    continue;
                }
            };
            let access = match current.GetAccessStatus() {
                Ok(UserNotificationListenerAccessStatus::Allowed) => AccessState::Allowed,
                Ok(UserNotificationListenerAccessStatus::Denied) => AccessState::Denied,
                Ok(_) => AccessState::NeedsConsent,
                Err(_) => AccessState::Error,
            };
            match permission_status(true, true, access) {
                CONNECTED => {}
                DENIED => {
                    set_status(DENIED);
                    continue;
                }
                NEEDS_CONSENT => {
                    set_status(NEEDS_CONSENT);
                    continue;
                }
                _ => {
                    set_status(ERROR);
                    continue;
                }
            }

            let event_sender = sender.clone();
            let handler = TypedEventHandler::<
                UserNotificationListener,
                UserNotificationChangedEventArgs,
            >::new(move |_, _| {
                // The bounded channel naturally coalesces an event
                // burst while a worker snapshot is being collected.
                let _ = event_sender.try_send(Command::Wake);
                Ok(())
            });
            let token = match current.NotificationChanged(&handler) {
                Ok(token) => token,
                Err(_) => {
                    set_status(ERROR);
                    continue;
                }
            };
            listener = Some(current);
            registration = Some(token);
            set_status(CONNECTED);

            if let Some(current) = listener.as_ref() {
                seeded = refresh_snapshot(current, false);
            }
            continue;
        }

        let access = listener
            .as_ref()
            .and_then(|current| current.GetAccessStatus().ok());
        match access {
            Some(UserNotificationListenerAccessStatus::Allowed) => {
                if let Some(current) = listener.as_ref() {
                    if refresh_snapshot(current, seeded) {
                        seeded = true;
                    }
                }
            }
            Some(UserNotificationListenerAccessStatus::Denied) => {
                detach_listener(&mut listener, &mut registration);
                seeded = false;
                REQUESTED_ENABLED.store(false, Ordering::SeqCst);
                set_status(DENIED);
            }
            Some(_) => {
                detach_listener(&mut listener, &mut registration);
                seeded = false;
                set_status(NEEDS_CONSENT);
            }
            None if listener.is_some() => {
                detach_listener(&mut listener, &mut registration);
                seeded = false;
                set_status(ERROR);
            }
            None => {}
        }
    }

    if let (Some(current), Some(token)) = (listener, registration) {
        let _ = current.RemoveNotificationChanged(token);
    }
    unsafe { RoUninitialize() };
}

fn detach_listener(
    listener: &mut Option<UserNotificationListener>,
    registration: &mut Option<windows::Foundation::EventRegistrationToken>,
) {
    if let (Some(current), Some(token)) = (listener.take(), registration.take()) {
        let _ = current.RemoveNotificationChanged(token);
    }
}

fn refresh_snapshot(listener: &UserNotificationListener, allow_toast: bool) -> bool {
    let notifications = match listener
        .GetNotificationsAsync(NotificationKinds::Toast)
        .and_then(|operation| operation.get())
    {
        Ok(notifications) => notifications,
        Err(_) => {
            set_status(ERROR);
            return false;
        }
    };

    // The collection does not promise an ordering. Scan only creation times,
    // keep the newest 64 indices in a bounded heap, then parse text for those.
    let count = notifications.Size().unwrap_or(0);
    let newest = newest_notification_indices(
        (0..count).filter_map(|index| {
            let notification = notifications.GetAt(index).ok()?;
            let created = notification.CreationTime().ok()?.UniversalTime;
            Some((index, created))
        }),
        64,
    );
    let mut snapshot = Vec::with_capacity(newest.len());
    for index in newest {
        if let Ok(notification) = notifications.GetAt(index) {
            if let Some(item) = parse_notification(&notification) {
                snapshot.push(item);
            }
        }
    }

    let duration = f32::from_bits(TOAST_DURATION_BITS.load(Ordering::Relaxed));
    let store = notify::global_store();
    store
        .write()
        .sync_native_snapshot(snapshot, duration, allow_toast);
    set_status(CONNECTED);
    true
}

fn newest_notification_indices(
    created_times: impl IntoIterator<Item = (u32, i64)>,
    limit: usize,
) -> Vec<u32> {
    let mut newest = BinaryHeap::<Reverse<(i64, u32)>>::with_capacity(limit);
    for (index, created) in created_times {
        if newest.len() < limit {
            newest.push(Reverse((created, index)));
        } else if newest
            .peek()
            .is_some_and(|Reverse((oldest, _))| created > *oldest)
        {
            newest.pop();
            newest.push(Reverse((created, index)));
        }
    }

    let mut newest = newest.into_vec();
    newest.sort_by(
        |Reverse((created_a, index_a)), Reverse((created_b, index_b))| {
            created_b.cmp(created_a).then_with(|| index_b.cmp(index_a))
        },
    );
    newest
        .into_iter()
        .map(|Reverse((_, index))| index)
        .collect()
}

fn parse_notification(notification: &UserNotification) -> Option<NativeNotification> {
    let id = notification.Id().ok()?;
    let created = notification.CreationTime().ok()?.UniversalTime;
    let app_info = notification.AppInfo().ok();
    let app_identity = app_info
        .as_ref()
        .and_then(|app| app.AppUserModelId().ok())
        .map(|value| value.to_string())
        .unwrap_or_default();
    let app = app_info
        .as_ref()
        .and_then(|app| app.DisplayInfo().ok())
        .and_then(|display| display.DisplayName().ok())
        .map(|value| value.to_string())
        .filter(|value| !value.trim().is_empty())
        .or_else(|| (!app_identity.is_empty()).then(|| app_identity.clone()))
        .unwrap_or_else(|| "Windows app".to_owned());

    let mut text = Vec::new();
    if let (Ok(binding_name), Ok(toast)) = (
        KnownNotificationBindings::ToastGeneric(),
        notification.Notification(),
    ) {
        if let Ok(visual) = toast.Visual() {
            if let Ok(binding) = visual.GetBinding(&binding_name) {
                if let Ok(elements) = binding.GetTextElements() {
                    for index in 0..elements.Size().unwrap_or(0).min(8) {
                        if let Ok(element) = elements.GetAt(index) {
                            if let Ok(value) = element.Text() {
                                let value = bounded_text(&value.to_string(), 512);
                                if !value.trim().is_empty() {
                                    text.push(value);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    let (title, body) = compose_toast_text(&text);
    let timestamp_secs = (created / 10_000_000 - 11_644_473_600).max(0) as u64;
    let time_str = format_local_time(created);
    let source_key = notification_source_key(&app_identity, &app, id, created);

    Some(NativeNotification {
        source_key,
        app: bounded_text(&app, 80),
        title,
        body: bounded_text(&body, 1024),
        timestamp_secs,
        time_str,
    })
}

fn bounded_text(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let mut text = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        text.push('…');
    }
    text
}

fn compose_toast_text(parts: &[String]) -> (String, String) {
    let mut nonempty = parts.iter().filter(|part| !part.trim().is_empty());
    let title = nonempty
        .next()
        .map(|part| bounded_text(part, 512))
        .unwrap_or_else(|| "Notification".to_owned());
    let body = nonempty
        .map(|part| bounded_text(part, 512))
        .collect::<Vec<_>>()
        .join(" · ");
    (title, bounded_text(&body, 1024))
}

fn notification_source_key(identity: &str, app_name: &str, id: u32, created: i64) -> String {
    let source_app = if identity.trim().is_empty() {
        bounded_text(app_name, 160)
    } else {
        bounded_text(identity, 160)
    };
    format!("{source_app}:{id}:{created}")
}

fn format_local_time(universal_time: i64) -> String {
    let ticks =
        (universal_time as i128 + 116_444_736_000_000_000i128).clamp(0, u64::MAX as i128) as u64;
    let utc = FILETIME {
        dwLowDateTime: ticks as u32,
        dwHighDateTime: (ticks >> 32) as u32,
    };
    let mut local = FILETIME::default();
    let mut system = SYSTEMTIME::default();
    let converted = unsafe {
        FileTimeToLocalFileTime(&utc, &mut local).is_ok()
            && FileTimeToSystemTime(&local, &mut system).is_ok()
    };
    if !converted {
        return "--:--".to_owned();
    }
    let hour = match system.wHour {
        0 => 12,
        hour if hour > 12 => hour - 12,
        hour => hour,
    };
    let period = if system.wHour >= 12 { "PM" } else { "AM" };
    format!("{hour:02}:{:02} {period}", system.wMinute)
}

#[cfg(test)]
mod tests {
    use super::{
        bounded_text, compose_toast_text, newest_notification_indices, notification_source_key,
        permission_status, AccessState, CONNECTED, DENIED, DISABLED, ERROR, NEEDS_CONSENT,
        NEEDS_IDENTITY,
    };

    #[test]
    fn notification_text_is_bounded_by_characters() {
        assert_eq!(bounded_text("abcd", 3), "abc…");
        assert_eq!(bounded_text("Hi 👋", 4), "Hi 👋");
    }

    #[test]
    fn empty_and_optional_toast_fields_have_a_compact_fallback() {
        assert_eq!(compose_toast_text(&[]), ("Notification".into(), "".into()));
        assert_eq!(
            compose_toast_text(&[" ".into(), "Body only".into()]),
            ("Body only".into(), "".into())
        );
    }

    #[test]
    fn long_windows_text_is_limited_before_entering_the_history() {
        let long = "x".repeat(2_000);
        let (title, body) =
            compose_toast_text(&[long.clone(), long.clone(), long.clone(), long.clone()]);
        assert_eq!(title.chars().count(), 513);
        assert_eq!(body.chars().count(), 1_025);
        assert!(title.ends_with('…'));
        assert!(body.ends_with('…'));
    }

    #[test]
    fn source_keys_use_identity_or_a_display_name_fallback() {
        assert_eq!(
            notification_source_key("AUMID", "Messages", 4, 100),
            "AUMID:4:100"
        );
        assert_eq!(
            notification_source_key("", "Calendar", 4, 100),
            "Calendar:4:100"
        );
    }

    #[test]
    fn listener_permission_lifecycle_has_explicit_states() {
        assert_eq!(
            permission_status(false, true, AccessState::Allowed),
            DISABLED
        );
        assert_eq!(
            permission_status(true, false, AccessState::Allowed),
            NEEDS_IDENTITY
        );
        assert_eq!(
            permission_status(true, true, AccessState::NeedsConsent),
            NEEDS_CONSENT
        );
        assert_eq!(permission_status(true, true, AccessState::Denied), DENIED);
        assert_eq!(
            permission_status(true, true, AccessState::Allowed),
            CONNECTED
        );
        assert_eq!(permission_status(true, true, AccessState::Error), ERROR);
    }

    #[test]
    fn snapshot_selection_is_timestamp_ordered_and_memory_bounded() {
        let newest =
            newest_notification_indices([(0, 80), (1, 10), (2, 70), (3, 20), (4, 90), (5, 60)], 3);
        assert_eq!(newest, vec![4, 0, 2]);
        assert!(newest_notification_indices([(0, 1), (1, 2)], 0).is_empty());
    }
}
