use std::fs::{self, OpenOptions};
use std::io::Write;
use std::panic::PanicHookInfo;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_LOG_BYTES: u64 = 64 * 1024;
static LOG_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

pub fn install_panic_hook() {
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        record_panic(info);
        previous_hook(info);
    }));
}

pub fn record_eframe_error(error: &eframe::Error) {
    let kind = match error {
        eframe::Error::AppCreation(_) => "app_creation",
        eframe::Error::Winit(_) => "window_creation",
        eframe::Error::WinitEventLoop(_) => "event_loop",
        _ => "graphics_or_runtime",
    };
    write_record(&format!("eframe_error kind={kind}"));
}

fn record_panic(info: &PanicHookInfo<'_>) {
    let site = info.location().map_or_else(
        || "unknown".to_owned(),
        |location| {
            let file = Path::new(location.file())
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("unknown");
            format!("{file}:{}:{}", location.line(), location.column())
        },
    );
    let payload_kind = if info.payload().is::<&str>() || info.payload().is::<String>() {
        "string"
    } else {
        "other"
    };

    // Panic payloads can contain user supplied text. Keep the useful source
    // location and payload category while leaving the payload itself out.
    write_record(&format!("panic site={site} payload={payload_kind}"));
}

fn write_record(event: &str) {
    let Some(local_data) = dirs::data_local_dir() else {
        return;
    };
    let path = local_data.join("Venu").join("startup.log");
    let lock = LOG_LOCK.get_or_init(|| Mutex::new(()));
    let _guard = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let record = format!("{timestamp} {event}\n");

    let Some(parent) = path.parent() else {
        return;
    };
    if fs::create_dir_all(parent).is_err() {
        return;
    }

    let current_size = fs::metadata(&path)
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    if current_size.saturating_add(record.len() as u64) > MAX_LOG_BYTES {
        if let Ok(mut file) = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&path)
        {
            let _ = writeln!(file, "{timestamp} previous startup records truncated");
            let _ = file.write_all(record.as_bytes());
        }
    } else if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&path) {
        let _ = file.write_all(record.as_bytes());
    }
}
