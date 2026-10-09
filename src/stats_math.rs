//! Pure system-metric normalization and aggregation helpers.

use std::collections::HashMap;

pub(crate) type CpuTimes = (u64, u64, u64); // idle, kernel, user

pub(crate) fn cpu_utilization(previous: Option<CpuTimes>, current: CpuTimes) -> Option<f32> {
    let previous = previous?;
    let idle_delta = current.0.checked_sub(previous.0)?;
    let kernel_delta = current.1.checked_sub(previous.1)?;
    let user_delta = current.2.checked_sub(previous.2)?;
    let total = kernel_delta.checked_add(user_delta)?;
    if total == 0 || idle_delta > total {
        return None;
    }
    Some((100.0 * (total - idle_delta) as f64 / total as f64).clamp(0.0, 100.0) as f32)
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct GpuEngineKey {
    luid_low: String,
    luid_high: String,
    physical_adapter: u32,
    engine: u32,
    engine_type: String,
}

fn normalize_luid_part(part: &str) -> Option<String> {
    let digits = part
        .strip_prefix("0x")
        .or_else(|| part.strip_prefix("0X"))?;
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    Some(format!("0x{}", digits.to_ascii_lowercase()))
}

/// Parse a per-process GPU Engine instance into its physical engine identity.
/// PID is validated but deliberately excluded from the grouping key.
fn parse_gpu_engine_identity(name: &str) -> Option<GpuEngineKey> {
    let tokens: Vec<_> = name.split('_').collect();
    if tokens.len() < 11 || tokens.first().copied()? != "pid" {
        return None;
    }
    tokens.get(1)?.parse::<u32>().ok()?;

    let luid_at = tokens.iter().position(|part| *part == "luid")?;
    let luid_low = normalize_luid_part(tokens.get(luid_at + 1)?)?;
    let luid_high = normalize_luid_part(tokens.get(luid_at + 2)?)?;
    let phys_at = tokens.iter().position(|part| *part == "phys")?;
    let physical_adapter = tokens.get(phys_at + 1)?.parse().ok()?;
    let engine_at = tokens.iter().position(|part| *part == "eng")?;
    let engine = tokens.get(engine_at + 1)?.parse().ok()?;
    let type_at = tokens.iter().position(|part| *part == "engtype")?;
    let mut engine_type = tokens[type_at + 1..].join("_").to_ascii_lowercase();
    if let Some(suffix_at) = engine_type.rfind('#') {
        if engine_type[suffix_at + 1..].parse::<u32>().is_ok() {
            engine_type.truncate(suffix_at);
        }
    }
    if engine_type.is_empty() {
        return None;
    }

    Some(GpuEngineKey {
        luid_low,
        luid_high,
        physical_adapter,
        engine,
        engine_type,
    })
}

pub(crate) fn busiest_gpu_engine<'a>(
    samples: impl IntoIterator<Item = (&'a str, f64)>,
) -> Option<f32> {
    let mut totals: HashMap<GpuEngineKey, f64> = HashMap::new();
    for (name, value) in samples {
        if !value.is_finite() || value < 0.0 {
            continue;
        }
        let Some(key) = parse_gpu_engine_identity(name) else {
            continue;
        };
        *totals.entry(key).or_default() += value;
    }
    totals
        .values()
        .copied()
        .filter(|value| value.is_finite())
        .max_by(f64::total_cmp)
        .map(|value| value.clamp(0.0, 100.0) as f32)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PowerReading {
    pub ac_online: Option<bool>,
    pub battery_present: Option<bool>,
    pub battery_pct: Option<u8>,
    pub charging: bool,
}

pub(crate) fn normalize_power(
    battery_flag: u8,
    battery_pct: u8,
    ac_line_status: u8,
) -> PowerReading {
    let battery_present = match battery_flag {
        128 => Some(false), // Windows: no system battery.
        255 => None,        // Windows: status unknown.
        _ => Some(true),
    };
    let ac_online = match ac_line_status {
        0 => Some(false),
        1 => Some(true),
        _ => None,
    };
    let battery_pct = (battery_present == Some(true) && battery_pct <= 100).then_some(battery_pct);
    let charging = battery_present == Some(true) && battery_flag & 0x08 != 0;

    PowerReading {
        ac_online,
        battery_present,
        battery_pct,
        charging,
    }
}

pub(crate) fn power_value(
    battery_pct: Option<u8>,
    charging: bool,
    ac_online: Option<bool>,
    battery_present: Option<bool>,
) -> String {
    match (battery_pct, charging, ac_online, battery_present) {
        (Some(pct), true, _, _) => format!("{pct}% · CHARGING"),
        (Some(pct), false, Some(true), _) => format!("{pct}% · AC POWER"),
        (Some(pct), false, Some(false), _) => format!("{pct}% · BATTERY"),
        (Some(pct), false, None, _) => format!("{pct}%"),
        (None, _, Some(true), _) => "AC POWER".to_string(),
        (None, _, Some(false), Some(true)) => "BATTERY".to_string(),
        _ => "—".to_string(),
    }
}

pub(crate) fn power_hint(
    battery_present: Option<bool>,
    battery_pct: Option<u8>,
    ac_online: Option<bool>,
    charging: bool,
) -> &'static str {
    match (battery_present, battery_pct, ac_online, charging) {
        (_, Some(_), _, true) => "Charging",
        (_, Some(_), Some(true), false) => "Plugged in",
        (_, Some(_), Some(false), false) => "On battery",
        (Some(false), _, Some(true), _) => "Desktop AC power",
        (Some(false), _, _, _) => "No battery detected",
        (Some(true), None, Some(true), _) => "AC power · charge unavailable",
        (Some(true), None, Some(false), _) => "Battery · charge unavailable",
        (_, _, Some(true), _) => "AC power",
        _ => "Unavailable",
    }
}

pub(crate) fn format_bytes(bytes: u64) -> String {
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
    const MIB: f64 = 1024.0 * 1024.0;
    if bytes as f64 >= GIB {
        format!("{:.1} GB", bytes as f64 / GIB)
    } else {
        format!("{:.0} MB", bytes as f64 / MIB)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_uses_consecutive_global_times_and_keeps_warmup_unavailable() {
        assert_eq!(cpu_utilization(None, (10, 20, 30)), None);
        assert_eq!(
            cpu_utilization(Some((10, 20, 30)), (20, 40, 30)),
            Some(50.0)
        );
        assert_eq!(cpu_utilization(Some((10, 20, 30)), (30, 30, 40)), Some(0.0));
        assert_eq!(cpu_utilization(Some((10, 20, 30)), (11, 20, 30)), None);
        assert_eq!(cpu_utilization(Some((10, 20, 30)), (31, 30, 40)), None);
    }

    #[test]
    fn gpu_sums_process_instances_for_the_same_physical_engine() {
        let value = busiest_gpu_engine([
            (
                "pid_100_luid_0x00000000_0x00000001_phys_0_eng_0_engtype_3D",
                20.0,
            ),
            (
                "pid_200_luid_0x00000000_0x00000001_phys_0_eng_0_engtype_3D",
                30.0,
            ),
        ]);
        assert_eq!(value, Some(50.0));
    }

    #[test]
    fn gpu_keeps_engines_and_adapters_separate() {
        let value = busiest_gpu_engine([
            (
                "pid_100_luid_0x00000000_0x00000001_phys_0_eng_0_engtype_3D",
                20.0,
            ),
            (
                "pid_200_luid_0x00000000_0x00000001_phys_0_eng_1_engtype_3D",
                30.0,
            ),
            (
                "pid_300_luid_0x00000000_0x00000002_phys_0_eng_0_engtype_3D",
                25.0,
            ),
            (
                "pid_400_luid_0x00000000_0x00000001_phys_1_eng_0_engtype_3D",
                35.0,
            ),
            (
                "pid_500_luid_0x00000000_0x00000001_phys_0_eng_2_engtype_Compute_0",
                18.0,
            ),
            (
                "pid_600_luid_0x00000000_0x00000001_phys_0_eng_2_engtype_Compute_1",
                19.0,
            ),
        ]);
        assert_eq!(value, Some(35.0));
    }

    #[test]
    fn gpu_ignores_pdh_duplicate_suffixes_but_keeps_full_engine_type() {
        let value = busiest_gpu_engine([
            (
                "pid_100_luid_0x00000000_0x00000001_phys_0_eng_0_engtype_3D",
                20.0,
            ),
            (
                "pid_200_luid_0x00000000_0x00000001_phys_0_eng_0_engtype_3D#1",
                25.0,
            ),
            (
                "pid_300_luid_0x00000000_0x00000001_phys_0_eng_0_engtype_Copy",
                40.0,
            ),
        ]);
        assert_eq!(value, Some(45.0));
    }

    #[test]
    fn gpu_skips_invalid_instances_and_caps_aggregated_load() {
        let value = busiest_gpu_engine([
            ("not-a-pdh-instance", 95.0),
            ("pid_100_luid_x_0x00000001_phys_0_eng_0_engtype_3D", 95.0),
            (
                "pid_100_luid_0x00000000_0x00000001_phys_0_eng_0_engtype_3D",
                f64::NAN,
            ),
            (
                "pid_200_luid_0x00000000_0x00000001_phys_0_eng_0_engtype_3D",
                -1.0,
            ),
            (
                "pid_300_luid_0x00000000_0x00000001_phys_0_eng_0_engtype_3D",
                80.0,
            ),
            (
                "pid_400_luid_0x00000000_0x00000001_phys_0_eng_0_engtype_3D",
                60.0,
            ),
        ]);
        assert_eq!(value, Some(100.0));
        assert_eq!(busiest_gpu_engine([("bad", f64::INFINITY)]), None);
    }

    #[test]
    fn power_flags_distinguish_no_battery_from_unknown() {
        let desktop = normalize_power(128, 255, 1);
        assert_eq!(desktop.battery_present, Some(false));
        assert_eq!(desktop.battery_pct, None);
        assert!(!desktop.charging);

        let unknown = normalize_power(255, 80, 1);
        assert_eq!(unknown.battery_present, None);
        assert_eq!(unknown.battery_pct, None);
        assert!(!unknown.charging);

        let battery = normalize_power(0x08, 76, 1);
        assert_eq!(battery.battery_pct, Some(76));
        assert!(battery.charging);
        assert_eq!(normalize_power(0, 101, 0).battery_pct, None);
    }

    #[test]
    fn desktop_power_never_invents_a_battery_percentage() {
        assert_eq!(
            power_value(None, false, Some(true), Some(false)),
            "AC POWER"
        );
        assert_eq!(
            power_hint(Some(false), None, Some(true), false),
            "Desktop AC power"
        );
        assert_eq!(power_value(None, true, Some(true), None), "AC POWER");
    }

    #[test]
    fn byte_format_is_compact() {
        assert_eq!(format_bytes(8 * 1024 * 1024 * 1024), "8.0 GB");
        assert_eq!(format_bytes(512 * 1024 * 1024), "512 MB");
    }
}
