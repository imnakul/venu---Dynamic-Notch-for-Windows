use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

static NEXT_NOTIF_ID: AtomicU64 = AtomicU64::new(1);

/// Maximum number of historical notifications to keep in memory.
const MAX_NOTIFICATIONS: usize = 50;
/// Windows history stays deliberately small; the notch shows the latest four.
const MAX_NATIVE_NOTIFICATIONS: usize = 4;
/// Keep dismissal tombstones longer than the bounded OS snapshot so visible
/// rows cannot reappear after a local clear/dismiss.
const MAX_DISMISSED_NATIVE_KEYS: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NotificationLevel {
    Info,
    Success,
    Warning,
    Action,
}

impl Default for NotificationLevel {
    fn default() -> Self {
        Self::Info
    }
}

impl NotificationLevel {
    #[allow(dead_code)]
    pub fn badge_color(self) -> [f32; 4] {
        match self {
            Self::Info => [0.22, 0.74, 0.97, 1.0],    // Cyan
            Self::Success => [0.13, 0.85, 0.53, 1.0], // Emerald
            Self::Warning => [1.00, 0.65, 0.00, 1.0], // Amber
            Self::Action => [0.66, 0.33, 0.97, 1.0],  // Violet
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Notification {
    pub id: u64,
    pub app: String,
    pub title: String,
    pub body: String,
    pub level: NotificationLevel,
    pub timestamp_secs: u64,
    pub time_str: String,
    pub read: bool,
    /// Stable source key for an OS toast. Webhook entries have no source key.
    #[serde(default)]
    pub source_key: Option<String>,
}

/// A bounded, already-sanitized snapshot row from Windows Notification Center.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeNotification {
    pub source_key: String,
    pub app: String,
    pub title: String,
    pub body: String,
    pub timestamp_secs: u64,
    pub time_str: String,
}

#[derive(Debug, Clone)]
pub struct ToastAlert {
    pub notification: Notification,
    pub duration: f32,
    pub remaining: f32,
}

#[derive(Debug, Default)]
pub struct NotificationCenter {
    pub items: Vec<Notification>,
    pub active_toast: Option<ToastAlert>,
    /// Bumped by every mutation. The notch reads it once a tick to decide
    /// whether the frame on screen is still the right one, which is cheaper
    /// and more exact than diffing the list itself.
    revision: u64,
    /// Local dismissals suppress unchanged OS snapshots without deleting the
    /// notification from Windows. It is capped to keep the session bounded.
    dismissed_native_keys: VecDeque<String>,
}

static NOTIFICATION_STORE: parking_lot::RwLock<Option<Arc<RwLock<NotificationCenter>>>> =
    parking_lot::RwLock::new(None);

pub fn global_store() -> Arc<RwLock<NotificationCenter>> {
    let read = NOTIFICATION_STORE.read();
    if let Some(store) = read.as_ref() {
        return Arc::clone(store);
    }
    drop(read);
    let mut write = NOTIFICATION_STORE.write();
    if let Some(store) = write.as_ref() {
        return Arc::clone(store);
    }
    let store = Arc::new(RwLock::new(NotificationCenter::default()));
    *write = Some(Arc::clone(&store));
    store
}

fn current_time_formatted() -> (u64, String) {
    let now = SystemTime::now();
    let epoch = now.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();

    // Local time formatting using Windows API GetLocalTime
    let local = unsafe {
        let st = windows::Win32::System::SystemInformation::GetLocalTime();
        let hour = if st.wHour == 0 {
            12
        } else if st.wHour > 12 {
            st.wHour - 12
        } else {
            st.wHour
        };
        let ampm = if st.wHour >= 12 { "PM" } else { "AM" };
        format!("{:02}:{:02} {}", hour, st.wMinute, ampm)
    };

    (epoch, local)
}

impl NotificationCenter {
    /// Monotonic counter over every change to the centre. Equal revisions
    /// mean the notch would draw the same notifications it drew last time.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn unread_count(&self) -> usize {
        self.items.iter().filter(|n| !n.read).count()
    }

    /// Push a new notification. Returns `true` if it passed the allowed app whitelist
    /// and generated an active toast alert.
    pub fn push(
        &mut self,
        app: &str,
        title: &str,
        body: &str,
        level: NotificationLevel,
        duration: f32,
        allowed_apps: &[String],
    ) -> bool {
        let (timestamp_secs, time_str) = current_time_formatted();
        let id = NEXT_NOTIF_ID.fetch_add(1, Ordering::SeqCst);

        let notif = Notification {
            id,
            app: app.trim().to_string(),
            title: title.trim().to_string(),
            body: body.trim().to_string(),
            level,
            timestamp_secs,
            time_str,
            read: false,
            source_key: None,
        };

        // Insert at head of history
        self.revision += 1;
        self.items.insert(0, notif.clone());
        if self.items.len() > MAX_NOTIFICATIONS {
            self.items.truncate(MAX_NOTIFICATIONS);
        }

        // Check if allowed
        let allowed =
            allowed_apps.is_empty() || allowed_apps.iter().any(|a| a.eq_ignore_ascii_case(app));

        if allowed {
            let dur = if duration > 0.5 { duration } else { 4.5 };
            self.active_toast = Some(ToastAlert {
                notification: notif,
                duration: dur,
                remaining: dur,
            });
            true
        } else {
            false
        }
    }

    pub fn tick(&mut self, dt: f32) {
        if let Some(toast) = self.active_toast.as_mut() {
            toast.remaining -= dt;
            if toast.remaining <= 0.0 {
                self.active_toast = None;
                self.revision += 1;
            }
        }
    }

    pub fn dismiss_toast(&mut self) {
        self.revision += 1;
        if let Some(toast) = self.active_toast.take() {
            if let Some(item) = self
                .items
                .iter_mut()
                .find(|i| i.id == toast.notification.id)
            {
                item.read = true;
            }
        }
    }

    pub fn mark_read(&mut self, id: u64) {
        if let Some(item) = self.items.iter_mut().find(|item| item.id == id) {
            if !item.read {
                item.read = true;
                self.revision = self.revision.wrapping_add(1);
            }
        }
    }

    pub fn clear_all(&mut self) {
        let native_keys = self
            .items
            .iter()
            .filter_map(|item| item.source_key.clone())
            .collect::<Vec<_>>();
        for key in native_keys {
            self.remember_native_dismissal(&key);
        }
        self.revision += 1;
        self.items.clear();
        self.active_toast = None;
    }

    #[allow(dead_code)]
    pub fn mark_all_read(&mut self) {
        self.revision += 1;
        for item in &mut self.items {
            item.read = true;
        }
    }

    #[allow(dead_code)]
    pub fn dismiss_item(&mut self, id: u64) {
        self.revision += 1;
        let source_key = self
            .items
            .iter()
            .find(|item| item.id == id)
            .and_then(|item| item.source_key.clone());
        if let Some(key) = source_key {
            self.remember_native_dismissal(&key);
        }
        if let Some(toast) = &self.active_toast {
            if toast.notification.id == id {
                self.active_toast = None;
            }
        }
        self.items.retain(|i| i.id != id);
    }

    /// Merge the current Windows toast snapshot into the session history.
    /// Snapshot removals are ignored so dismissing a toast in Windows does not
    /// erase its local history row. Only genuinely new rows can create one
    /// active notch toast, so a burst of OS events stays visually quiet.
    pub fn sync_native_snapshot(
        &mut self,
        mut incoming: Vec<NativeNotification>,
        duration: f32,
        allow_toast: bool,
    ) {
        incoming.sort_by(|a, b| b.timestamp_secs.cmp(&a.timestamp_secs));
        incoming.truncate(MAX_NATIVE_NOTIFICATIONS);

        let mut newest_added = None;
        let mut changed = false;
        for native in incoming {
            if self
                .dismissed_native_keys
                .iter()
                .any(|key| key == &native.source_key)
            {
                continue;
            }

            if let Some(existing) = self
                .items
                .iter_mut()
                .find(|item| item.source_key.as_deref() == Some(native.source_key.as_str()))
            {
                if existing.app != native.app
                    || existing.title != native.title
                    || existing.body != native.body
                    || existing.timestamp_secs != native.timestamp_secs
                    || existing.time_str != native.time_str
                {
                    existing.app = native.app;
                    existing.title = native.title;
                    existing.body = native.body;
                    existing.timestamp_secs = native.timestamp_secs;
                    existing.time_str = native.time_str;
                    changed = true;
                }
                continue;
            }

            let item = Notification {
                id: NEXT_NOTIF_ID.fetch_add(1, Ordering::SeqCst),
                app: native.app,
                title: native.title,
                body: native.body,
                level: NotificationLevel::Info,
                timestamp_secs: native.timestamp_secs,
                time_str: native.time_str,
                read: false,
                source_key: Some(native.source_key),
            };
            if newest_added
                .as_ref()
                .is_none_or(|latest: &Notification| item.timestamp_secs > latest.timestamp_secs)
            {
                newest_added = Some(item.clone());
            }
            self.items.push(item);
            changed = true;
        }

        if !changed {
            return;
        }

        self.items
            .sort_by(|a, b| b.timestamp_secs.cmp(&a.timestamp_secs));
        let mut native_kept = 0;
        self.items.retain(|item| {
            if item.source_key.is_some() {
                native_kept += 1;
                native_kept <= MAX_NATIVE_NOTIFICATIONS
            } else {
                true
            }
        });
        self.items.truncate(MAX_NOTIFICATIONS);
        self.revision = self.revision.wrapping_add(1);

        if allow_toast {
            if let Some(notification) = newest_added {
                let dur = if duration > 0.5 { duration } else { 4.5 };
                self.active_toast = Some(ToastAlert {
                    notification,
                    duration: dur,
                    remaining: dur,
                });
            }
        }
    }

    fn remember_native_dismissal(&mut self, key: &str) {
        if self.dismissed_native_keys.iter().any(|seen| seen == key) {
            return;
        }
        self.dismissed_native_keys.push_back(key.to_owned());
        while self.dismissed_native_keys.len() > MAX_DISMISSED_NATIVE_KEYS {
            self.dismissed_native_keys.pop_front();
        }
    }
}

#[cfg(test)]
mod native_history_tests {
    use super::{NativeNotification, NotificationCenter};

    fn native(key: &str, timestamp: u64, title: &str) -> NativeNotification {
        NativeNotification {
            source_key: key.to_owned(),
            app: "Messages".to_owned(),
            title: title.to_owned(),
            body: "Preview".to_owned(),
            timestamp_secs: timestamp,
            time_str: "10:30 AM".to_owned(),
        }
    }

    #[test]
    fn native_snapshot_seeds_silently_and_keeps_only_four_latest() {
        let mut center = NotificationCenter::default();
        center.sync_native_snapshot(
            (1..=5)
                .map(|n| native(&format!("app:{n}"), n, &format!("item {n}")))
                .collect(),
            4.5,
            false,
        );

        assert_eq!(center.items.len(), 4);
        assert_eq!(center.items[0].title, "item 5");
        assert_eq!(center.items[3].title, "item 2");
        assert!(center.active_toast.is_none());
    }

    #[test]
    fn native_notifications_are_not_filtered_by_webhook_app_rules() {
        let mut center = NotificationCenter::default();
        center.sync_native_snapshot(
            vec![native("unlisted-app:1", 1, "from any app")],
            4.5,
            false,
        );

        assert_eq!(center.items.len(), 1);
        assert_eq!(center.items[0].app, "Messages");
    }

    #[test]
    fn repeated_snapshot_updates_in_place_and_new_event_toasts_once() {
        let mut center = NotificationCenter::default();
        center.sync_native_snapshot(vec![native("app:7", 7, "old")], 4.5, false);
        let original_id = center.items[0].id;

        center.sync_native_snapshot(vec![native("app:7", 8, "updated")], 4.5, true);
        assert_eq!(center.items.len(), 1);
        assert_eq!(center.items[0].id, original_id);
        assert_eq!(center.items[0].title, "updated");
        assert!(
            center.active_toast.is_none(),
            "updates should not create a duplicate toast"
        );

        center.sync_native_snapshot(vec![native("app:8", 9, "new")], 4.5, true);
        assert_eq!(center.items[0].title, "new");
        assert_eq!(
            center.active_toast.as_ref().unwrap().notification.title,
            "new"
        );
    }

    #[test]
    fn operating_system_removal_keeps_local_history_and_dismissal_suppresses_resurrection() {
        let mut center = NotificationCenter::default();
        center.sync_native_snapshot(vec![native("app:1", 1, "kept")], 4.5, false);
        let id = center.items[0].id;

        center.sync_native_snapshot(Vec::new(), 4.5, false);
        assert_eq!(center.items[0].id, id);

        center.dismiss_item(id);
        center.sync_native_snapshot(vec![native("app:1", 1, "kept")], 4.5, false);
        assert!(center.items.is_empty());
    }

    #[test]
    fn clear_all_suppresses_all_current_native_rows_but_not_future_rows() {
        let mut center = NotificationCenter::default();
        center.sync_native_snapshot(
            vec![native("app:1", 1, "one"), native("app:2", 2, "two")],
            4.5,
            false,
        );
        center.clear_all();
        center.sync_native_snapshot(
            vec![native("app:1", 1, "one"), native("app:2", 2, "two")],
            4.5,
            false,
        );
        assert!(center.items.is_empty());

        center.sync_native_snapshot(vec![native("app:3", 3, "future")], 4.5, false);
        assert_eq!(center.items[0].title, "future");
    }
}

/// JSON payload received over the local HTTP Webhook endpoint.
#[derive(Debug, Deserialize)]
pub struct WebhookPayload {
    pub app: Option<String>,
    pub title: Option<String>,
    pub body: Option<String>,
    pub message: Option<String>,
    pub level: Option<NotificationLevel>,
    pub duration: Option<f32>,
}

/// Latest known Claude Code usage snapshot, reported by the `statusLine` hook.
#[derive(Debug, Clone, Default)]
pub struct UsageSnapshot {
    pub context_used_pct: Option<f32>,
    pub cost_usd: Option<f64>,
    pub rate_5h_pct: Option<f32>,
    pub rate_5h_resets_at: Option<String>,
    pub rate_7d_pct: Option<f32>,
    pub rate_7d_resets_at: Option<String>,
    /// When this snapshot was last updated, so a stale slide can say so.
    pub updated_at_secs: u64,
}

/// JSON payload received over the local `/usage` Webhook endpoint.
#[derive(Debug, Deserialize)]
pub struct UsagePayload {
    pub context_used_pct: Option<f32>,
    pub cost_usd: Option<f64>,
    pub rate_5h_pct: Option<f32>,
    pub rate_5h_resets_at: Option<String>,
    pub rate_7d_pct: Option<f32>,
    pub rate_7d_resets_at: Option<String>,
}

static USAGE_STORE: parking_lot::RwLock<Option<Arc<RwLock<UsageSnapshot>>>> =
    parking_lot::RwLock::new(None);

pub fn usage_store() -> Arc<RwLock<UsageSnapshot>> {
    let read = USAGE_STORE.read();
    if let Some(store) = read.as_ref() {
        return Arc::clone(store);
    }
    drop(read);
    let mut write = USAGE_STORE.write();
    if let Some(store) = write.as_ref() {
        return Arc::clone(store);
    }
    let store = Arc::new(RwLock::new(UsageSnapshot::default()));
    *write = Some(Arc::clone(&store));
    store
}

fn handle_webhook_client(
    mut stream: TcpStream,
    store: Arc<RwLock<NotificationCenter>>,
    allowed_apps: Arc<parking_lot::RwLock<Vec<String>>>,
) {
    let mut buffer = [0u8; 8192];
    let n = match stream.read(&mut buffer) {
        Ok(n) if n > 0 => n,
        _ => return,
    };

    let req_str = String::from_utf8_lossy(&buffer[..n]);
    let mut lines = req_str.lines();
    let first_line = lines.next().unwrap_or("");
    let path = first_line.split_whitespace().nth(1).unwrap_or("/");

    // Support CORS Preflight
    if first_line.starts_with("OPTIONS") {
        let resp = "HTTP/1.1 204 No Content\r\n\
                    Access-Control-Allow-Origin: *\r\n\
                    Access-Control-Allow-Methods: POST, GET, OPTIONS\r\n\
                    Access-Control-Allow-Headers: Content-Type\r\n\
                    Content-Length: 0\r\n\r\n";
        let _ = stream.write_all(resp.as_bytes());
        return;
    }

    if first_line.starts_with("GET") {
        let body = r#"{"status":"running","service":"Venu Dynamic Notch"}"#;
        let resp = format!(
            "HTTP/1.1 200 OK\r\n\
             Access-Control-Allow-Origin: *\r\n\
             Content-Type: application/json\r\n\
             Content-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );
        let _ = stream.write_all(resp.as_bytes());
        return;
    }

    if first_line.starts_with("POST") {
        // Find double newline marking the start of HTTP body
        if let Some(pos) = req_str.find("\r\n\r\n").or_else(|| req_str.find("\n\n")) {
            let header_offset = if req_str.contains("\r\n\r\n") { 4 } else { 2 };
            let body_str = &req_str[pos + header_offset..];

            if path == "/usage" {
                if let Ok(payload) = serde_json::from_str::<UsagePayload>(body_str.trim()) {
                    let epoch = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs();
                    let store = usage_store();
                    *store.write() = UsageSnapshot {
                        context_used_pct: payload.context_used_pct,
                        cost_usd: payload.cost_usd,
                        rate_5h_pct: payload.rate_5h_pct,
                        rate_5h_resets_at: payload.rate_5h_resets_at,
                        rate_7d_pct: payload.rate_7d_pct,
                        rate_7d_resets_at: payload.rate_7d_resets_at,
                        updated_at_secs: epoch,
                    };

                    let resp_body = r#"{"status":"ok","delivered":true}"#;
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\n\
                         Access-Control-Allow-Origin: *\r\n\
                         Content-Type: application/json\r\n\
                         Content-Length: {}\r\n\r\n{}",
                        resp_body.len(),
                        resp_body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                    return;
                }
            } else if let Ok(payload) = serde_json::from_str::<WebhookPayload>(body_str.trim()) {
                let app = payload.app.unwrap_or_else(|| "System".to_string());
                let title = payload.title.unwrap_or_else(|| "Alert".to_string());
                let body = payload
                    .body
                    .or(payload.message)
                    .unwrap_or_else(|| "".to_string());
                let level = payload.level.unwrap_or_default();
                let duration = payload.duration.unwrap_or(4.5);

                let allowed = allowed_apps.read().clone();
                store
                    .write()
                    .push(&app, &title, &body, level, duration, &allowed);

                let resp_body = r#"{"status":"ok","delivered":true}"#;
                let resp = format!(
                    "HTTP/1.1 200 OK\r\n\
                     Access-Control-Allow-Origin: *\r\n\
                     Content-Type: application/json\r\n\
                     Content-Length: {}\r\n\r\n{}",
                    resp_body.len(),
                    resp_body
                );
                let _ = stream.write_all(resp.as_bytes());
                return;
            }
        }
    }

    let err_body = r#"{"status":"error","message":"Invalid request"}"#;
    let resp = format!(
        "HTTP/1.1 400 Bad Request\r\n\
         Access-Control-Allow-Origin: *\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\n\r\n{}",
        err_body.len(),
        err_body
    );
    let _ = stream.write_all(resp.as_bytes());
}

static SERVER_ALLOWED_APPS: parking_lot::RwLock<Option<Arc<parking_lot::RwLock<Vec<String>>>>> =
    parking_lot::RwLock::new(None);

pub fn update_server_allowed_apps(apps: Vec<String>) {
    let read = SERVER_ALLOWED_APPS.read();
    if let Some(arc) = read.as_ref() {
        *arc.write() = apps;
    }
}

/// Spawn the zero-overhead local Webhook TCP server on a background thread.
pub fn start_webhook_server(port: u16, initial_allowed: Vec<String>) {
    let allowed_arc = Arc::new(parking_lot::RwLock::new(initial_allowed));
    *SERVER_ALLOWED_APPS.write() = Some(Arc::clone(&allowed_arc));

    let addr = format!("127.0.0.1:{}", port);
    let listener = match TcpListener::bind(&addr) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("[notch-webhook] could not bind to {}: {:?}", addr, e);
            return;
        }
    };

    let store = global_store();
    std::thread::Builder::new()
        .name("notch-webhook".into())
        .spawn(move || {
            for stream in listener.incoming() {
                if let Ok(stream) = stream {
                    let store_clone = Arc::clone(&store);
                    let allowed_clone = Arc::clone(&allowed_arc);
                    handle_webhook_client(stream, store_clone, allowed_clone);
                }
            }
        })
        .ok();
}
