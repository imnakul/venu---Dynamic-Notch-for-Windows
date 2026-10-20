//! Charging and low-battery alerts.
//!
//! Polls the power state every few seconds — one cheap syscall — and turns
//! the moments worth knowing about into a notch toast: the charger going in
//! or out, and the charge falling to the warning level and to half of it.
//! A PC without a battery never says anything.

use std::time::{Duration, Instant};

use parking_lot::Mutex;
use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};

use crate::config::BatteryAlertConfig;
use crate::notch::notify::{self, NotificationLevel};

const POLL: Duration = Duration::from_secs(4);

/// `BatteryFlag` bit meaning "no system battery".
const NO_BATTERY: u8 = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Reading {
    on_ac: bool,
    pct: u8,
}

struct Watch {
    last_poll: Option<Instant>,
    last: Option<Reading>,
    /// The lowest warning level already announced on this discharge, so each
    /// one is said once.
    warned_at: Option<u8>,
}

static WATCH: Mutex<Watch> = Mutex::new(Watch {
    last_poll: None,
    last: None,
    warned_at: None,
});

fn read() -> Option<Reading> {
    let mut status = SYSTEM_POWER_STATUS::default();
    unsafe { GetSystemPowerStatus(&mut status).ok()? };
    if status.BatteryFlag & NO_BATTERY != 0 || status.BatteryLifePercent > 100 {
        return None;
    }
    Some(Reading {
        on_ac: status.ACLineStatus == 1,
        pct: status.BatteryLifePercent,
    })
}

/// What to say about the change from `before` to `now`, if anything.
fn alert(
    cfg: &BatteryAlertConfig,
    before: Option<Reading>,
    now: Reading,
    warned_at: &mut Option<u8>,
) -> Option<(String, String, NotificationLevel)> {
    if now.on_ac {
        *warned_at = None;
    }
    if let Some(before) = before {
        if before.on_ac != now.on_ac {
            return Some(if now.on_ac {
                (
                    "Charging".to_owned(),
                    format!("Plugged in at {}%.", now.pct),
                    NotificationLevel::Success,
                )
            } else {
                (
                    "On battery".to_owned(),
                    format!("Unplugged at {}%.", now.pct),
                    NotificationLevel::Info,
                )
            });
        }
    }
    if now.on_ac || cfg.low_pct == 0 {
        return None;
    }
    let levels = [cfg.low_pct, (cfg.low_pct / 2).max(1)];
    let level = levels.into_iter().filter(|l| now.pct <= *l).min()?;
    if warned_at.is_some_and(|w| w <= level) {
        return None;
    }
    *warned_at = Some(level);
    Some((
        "Battery low".to_owned(),
        format!("{}% left. Time to plug in.", now.pct),
        NotificationLevel::Warning,
    ))
}

/// Called every overlay tick; does real work once every few seconds.
pub fn tick(cfg: &BatteryAlertConfig, toast_secs: f32) {
    if !cfg.enabled {
        return;
    }
    let mut watch = WATCH.lock();
    if watch.last_poll.is_some_and(|t| t.elapsed() < POLL) {
        return;
    }
    watch.last_poll = Some(Instant::now());
    let Some(now) = read() else {
        return;
    };
    let before = watch.last.replace(now);
    let mut warned = watch.warned_at;
    let said = alert(cfg, before, now, &mut warned);
    watch.warned_at = warned;
    drop(watch);

    if let Some((title, body, level)) = said {
        notify::global_store()
            .write()
            .push("Battery", &title, &body, level, toast_secs, &[]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(on_ac: bool, pct: u8) -> Reading {
        Reading { on_ac, pct }
    }

    #[test]
    fn plugging_in_and_out_is_announced() {
        let cfg = BatteryAlertConfig::default();
        let mut w = None;
        assert!(alert(&cfg, Some(r(false, 50)), r(true, 50), &mut w).is_some());
        assert!(alert(&cfg, Some(r(true, 50)), r(false, 50), &mut w).is_some());
        assert!(alert(&cfg, Some(r(false, 50)), r(false, 49), &mut w).is_none());
    }

    #[test]
    fn low_battery_warns_once_per_level() {
        let cfg = BatteryAlertConfig::default();
        let mut w = None;
        assert!(alert(&cfg, Some(r(false, 21)), r(false, 20), &mut w).is_some());
        assert!(alert(&cfg, Some(r(false, 20)), r(false, 19), &mut w).is_none());
        assert!(alert(&cfg, Some(r(false, 11)), r(false, 10), &mut w).is_some());
        assert!(alert(&cfg, Some(r(false, 10)), r(false, 9), &mut w).is_none());
    }
}
