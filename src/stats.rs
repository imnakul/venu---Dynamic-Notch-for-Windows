//! Cached, whole-system telemetry for the Stats page and notch slide.
//!
//! Win32 and PDH reads run on one demand-driven worker. Visible views renew a
//! short lease; painting only clones the latest snapshot and never waits for a
//! performance counter.

use parking_lot::{Condvar, Mutex};
use std::ffi::CStr;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use windows::core::PCSTR;
use windows::Win32::Foundation::FILETIME;
use windows::Win32::System::Performance::{
    PdhAddEnglishCounterA, PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterArrayA,
    PdhOpenQueryA, PDH_FMT_COUNTERVALUE_ITEM_A, PDH_FMT_DOUBLE, PDH_MORE_DATA,
};
use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows::Win32::System::Threading::GetSystemTimes;

pub(crate) use crate::stats_math::format_bytes;
use crate::stats_math::{
    busiest_gpu_engine, cpu_utilization, normalize_power, power_hint, power_value, CpuTimes,
};

const SAMPLE_INTERVAL: Duration = Duration::from_secs(1);
const DEMAND_LEASE: Duration = Duration::from_secs(2);
const GPU_COUNTER: &[u8] = b"\\GPU Engine(*)\\Utilization Percentage\0";

#[derive(Debug, Clone, Default)]
pub struct StatsSnapshot {
    pub revision: u64,
    /// Whole-system processor utilization between consecutive valid samples.
    pub cpu_pct: Option<f32>,
    /// Whole-system physical RAM in use.
    pub ram_pct: Option<f32>,
    pub ram_used: u64,
    pub ram_total: u64,
    /// Busiest physical GPU engine, after summing its per-process instances.
    pub gpu_pct: Option<f32>,
    pub ac_online: Option<bool>,
    pub battery_present: Option<bool>,
    pub battery_pct: Option<u8>,
    pub charging: bool,
}

impl StatsSnapshot {
    pub fn power_value(&self) -> String {
        power_value(
            self.battery_pct,
            self.charging,
            self.ac_online,
            self.battery_present,
        )
    }

    pub fn power_hint(&self) -> &'static str {
        power_hint(
            self.battery_present,
            self.battery_pct,
            self.ac_online,
            self.charging,
        )
    }
}

#[derive(Default)]
struct SharedState {
    snapshot: StatsSnapshot,
    demand_until: Option<Instant>,
}

struct StatsService {
    shared: Arc<(Mutex<SharedState>, Condvar)>,
}

impl StatsService {
    fn new() -> Self {
        let shared = Arc::new((Mutex::new(SharedState::default()), Condvar::new()));
        let worker_state = Arc::clone(&shared);
        if let Err(error) = std::thread::Builder::new()
            .name("venu-stats".to_string())
            .spawn(move || sample_worker(worker_state))
        {
            eprintln!("[stats] could not start sampling worker: {error}");
        }
        Self { shared }
    }

    fn request_sampling(&self) {
        let (state_lock, wake) = &*self.shared;
        let now = Instant::now();
        let mut state = state_lock.lock();
        let was_active = state.demand_until.is_some_and(|until| until > now);
        if !was_active {
            let revision = state.snapshot.revision.wrapping_add(1);
            state.snapshot = StatsSnapshot {
                revision,
                ..StatsSnapshot::default()
            };
        }
        state.demand_until = Some(now + DEMAND_LEASE);
        wake.notify_one();
    }

    fn snapshot(&self) -> StatsSnapshot {
        self.shared.0.lock().snapshot.clone()
    }
}

static SERVICE: OnceLock<StatsService> = OnceLock::new();

fn service() -> &'static StatsService {
    SERVICE.get_or_init(StatsService::new)
}

/// Renew the sampling lease while a Stats view contributes visible pixels.
/// The worker stops sampling and releases its PDH query after the lease expires.
pub fn request_sampling() {
    service().request_sampling();
}

/// Return the latest cached values. This does no Win32 or PDH work.
pub fn snapshot() -> StatsSnapshot {
    SERVICE
        .get()
        .map_or_else(StatsSnapshot::default, StatsService::snapshot)
}

fn sample_worker(shared: Arc<(Mutex<SharedState>, Condvar)>) {
    let mut sampler = Sampler::default();

    loop {
        let (state_lock, wake) = &*shared;
        let mut state = state_lock.lock();

        while state
            .demand_until
            .is_none_or(|demand_until| demand_until <= Instant::now())
        {
            state.demand_until = None;
            let revision = state.snapshot.revision;
            drop(state);
            // Dropping the sampler closes PDH and removes the CPU baseline so
            // the first CPU sample after inactivity is a warm-up value.
            sampler = Sampler::with_revision(revision);
            state = state_lock.lock();
            if state
                .demand_until
                .is_some_and(|demand_until| demand_until > Instant::now())
            {
                break;
            }
            wake.wait(&mut state);
        }

        let now = Instant::now();
        let next_sample = sampler
            .last_sample
            .map(|last| last + SAMPLE_INTERVAL)
            .unwrap_or(now);
        if next_sample <= now {
            drop(state);
            sampler.sample_once();
            state_lock.lock().snapshot = sampler.snapshot.clone();
            continue;
        }

        let demand_until = state.demand_until.unwrap_or(now);
        let until_sample = next_sample.saturating_duration_since(now);
        let until_demand_expires = demand_until.saturating_duration_since(now);
        wake.wait_for(&mut state, until_sample.min(until_demand_expires));
    }
}

struct GpuQuery {
    // windows 0.58 exposes PDH handles as isize, not named handle wrappers.
    query: isize,
    counter: isize,
}

impl GpuQuery {
    fn new() -> Option<Self> {
        unsafe {
            let mut query = 0isize;
            if PdhOpenQueryA(PCSTR::null(), 0, &mut query) != 0 {
                return None;
            }

            let mut counter = 0isize;
            let status = PdhAddEnglishCounterA(query, PCSTR(GPU_COUNTER.as_ptr()), 0, &mut counter);
            if status != 0 {
                let _ = PdhCloseQuery(query);
                return None;
            }

            // Percentage counters need two samples. The first value is read on
            // the next one-second worker tick, not during painting.
            let _ = PdhCollectQueryData(query);
            Some(Self { query, counter })
        }
    }

    fn sample(&mut self) -> Option<f32> {
        unsafe {
            if PdhCollectQueryData(self.query) != 0 {
                return None;
            }

            let mut bytes = 0u32;
            let mut count = 0u32;
            let first = PdhGetFormattedCounterArrayA(
                self.counter,
                PDH_FMT_DOUBLE,
                &mut bytes,
                &mut count,
                None,
            );
            if first != PDH_MORE_DATA || bytes == 0 || count == 0 {
                return None;
            }

            // PDH writes the item array plus its strings into one caller-owned
            // byte buffer. usize gives it enough alignment for the item array.
            let words =
                (bytes as usize + std::mem::size_of::<usize>() - 1) / std::mem::size_of::<usize>();
            let mut buffer = vec![0usize; words];
            let items = buffer.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_A;
            let status = PdhGetFormattedCounterArrayA(
                self.counter,
                PDH_FMT_DOUBLE,
                &mut bytes,
                &mut count,
                Some(items),
            );
            if status != 0 {
                return None;
            }

            let slice = std::slice::from_raw_parts(items, count as usize);
            let instances: Vec<(String, f64)> = slice
                .iter()
                .filter_map(|item| {
                    // 0 = valid data, 1 = new data. Ignore stale/error entries.
                    if item.FmtValue.CStatus > 1 || item.szName.is_null() {
                        return None;
                    }
                    let name = CStr::from_ptr(item.szName.0 as *const i8)
                        .to_string_lossy()
                        .into_owned();
                    let value = item.FmtValue.Anonymous.doubleValue;
                    Some((name, value))
                })
                .collect();
            busiest_gpu_engine(
                instances
                    .iter()
                    .map(|(name, value)| (name.as_str(), *value)),
            )
        }
    }
}

impl Drop for GpuQuery {
    fn drop(&mut self) {
        unsafe {
            let _ = PdhCloseQuery(self.query);
        }
    }
}

struct Sampler {
    last_sample: Option<Instant>,
    previous_cpu: Option<CpuTimes>,
    gpu: Option<GpuQuery>,
    gpu_attempted: bool,
    snapshot: StatsSnapshot,
}

impl Default for Sampler {
    fn default() -> Self {
        Self::with_revision(0)
    }
}

impl Sampler {
    fn with_revision(revision: u64) -> Self {
        Self {
            last_sample: None,
            previous_cpu: None,
            gpu: None,
            gpu_attempted: false,
            snapshot: StatsSnapshot {
                revision,
                ..StatsSnapshot::default()
            },
        }
    }

    fn sample_once(&mut self) {
        self.last_sample = Some(Instant::now());
        self.snapshot.revision = self.snapshot.revision.wrapping_add(1);
        self.sample_cpu();
        self.sample_memory();
        self.sample_power();

        if !self.gpu_attempted {
            self.gpu_attempted = true;
            self.gpu = GpuQuery::new();
        } else {
            self.snapshot.gpu_pct = self.gpu.as_mut().and_then(GpuQuery::sample);
        }
    }

    fn sample_cpu(&mut self) {
        let mut idle = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        if unsafe { GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user)) }.is_err() {
            self.previous_cpu = None;
            self.snapshot.cpu_pct = None;
            return;
        }

        let current = (
            filetime_ticks(idle),
            filetime_ticks(kernel),
            filetime_ticks(user),
        );
        self.snapshot.cpu_pct = cpu_utilization(self.previous_cpu, current);
        self.previous_cpu = Some(current);
    }

    fn sample_memory(&mut self) {
        let mut memory = MEMORYSTATUSEX {
            dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
            ..Default::default()
        };
        if unsafe { GlobalMemoryStatusEx(&mut memory) }.is_ok() {
            self.snapshot.ram_total = memory.ullTotalPhys;
            self.snapshot.ram_used = memory.ullTotalPhys.saturating_sub(memory.ullAvailPhys);
            self.snapshot.ram_pct = Some(memory.dwMemoryLoad.min(100) as f32);
        } else {
            self.snapshot.ram_pct = None;
            self.snapshot.ram_used = 0;
            self.snapshot.ram_total = 0;
        }
    }

    fn sample_power(&mut self) {
        let mut power = SYSTEM_POWER_STATUS::default();
        if unsafe { GetSystemPowerStatus(&mut power) }.is_err() {
            self.snapshot.ac_online = None;
            self.snapshot.battery_present = None;
            self.snapshot.battery_pct = None;
            self.snapshot.charging = false;
            return;
        }

        let reading = normalize_power(
            power.BatteryFlag,
            power.BatteryLifePercent,
            power.ACLineStatus,
        );
        self.snapshot.ac_online = reading.ac_online;
        self.snapshot.battery_present = reading.battery_present;
        self.snapshot.battery_pct = reading.battery_pct;
        self.snapshot.charging = reading.charging;
    }
}

fn filetime_ticks(value: FILETIME) -> u64 {
    ((value.dwHighDateTime as u64) << 32) | value.dwLowDateTime as u64
}
