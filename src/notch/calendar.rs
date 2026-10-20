//! The Calendar slide's events, read from an iCal feed.
//!
//! Windows only lets packaged apps read the system calendar, so Venu reads
//! the same thing every calendar service can publish instead: an `.ics`
//! address (Google Calendar's "secret address in iCal format", Outlook's
//! "publish calendar" ICS link) or a local `.ics` file.
//!
//! Fetching happens on its own thread, every few minutes, and never on the
//! overlay thread. The notch only reads the parsed list, which is a lock and
//! a clone of a handful of events.
//!
//! Times are kept as minutes since 1970-01-01 in *local* time. Events in UTC
//! are moved to local with the current offset; events with a named time zone
//! are taken as local, which is right for the usual case of a calendar kept
//! in the zone its owner lives in.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

use parking_lot::{Condvar, Mutex, RwLock};
use windows::Win32::System::SystemInformation::{GetLocalTime, GetSystemTime};

/// How far ahead recurring events are expanded.
const HORIZON_DAYS: i64 = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    /// Local minutes since the epoch.
    pub start: i64,
    pub end: i64,
    pub all_day: bool,
    pub title: String,
    pub location: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Status {
    /// No source configured.
    #[default]
    Unset,
    Loading,
    Ready,
    Failed(String),
}

#[derive(Default)]
struct Shared {
    events: Vec<Event>,
    status: Status,
}

static SHARED: RwLock<Shared> = RwLock::new(Shared {
    events: Vec::new(),
    status: Status::Unset,
});
static REVISION: AtomicU64 = AtomicU64::new(0);

/// The source the fetch thread should read, and a nudge to read it now.
struct Wanted {
    source: String,
    refresh: Duration,
    changed: bool,
}

fn wanted() -> &'static (Mutex<Wanted>, Condvar) {
    static WANTED: OnceLock<(Mutex<Wanted>, Condvar)> = OnceLock::new();
    WANTED.get_or_init(|| {
        std::thread::Builder::new()
            .name("venu-calendar".into())
            .spawn(fetch_loop)
            .ok();
        (
            Mutex::new(Wanted {
                source: String::new(),
                refresh: Duration::from_secs(15 * 60),
                changed: false,
            }),
            Condvar::new(),
        )
    })
}

/// Tell the fetcher what to read. Called every tick; does nothing unless the
/// source or interval changed. The thread is only started the first time a
/// source is set, so a user who never uses the calendar never pays for it.
pub fn configure(source: &str, refresh_mins: u32) {
    static LAST: Mutex<Option<(String, u32)>> = Mutex::new(None);
    let mut last = LAST.lock();
    if last.as_ref().map(|(s, r)| (s.as_str(), *r)) == Some((source, refresh_mins)) {
        return;
    }
    *last = Some((source.to_owned(), refresh_mins));
    if source.trim().is_empty() && SHARED.read().status == Status::Unset {
        return;
    }
    let (lock, cv) = wanted();
    let mut w = lock.lock();
    w.source = source.trim().to_owned();
    w.refresh = Duration::from_secs(refresh_mins.clamp(5, 24 * 60) as u64 * 60);
    w.changed = true;
    cv.notify_one();
}

/// Ask for a fetch now, as from a "Refresh" button.
pub fn refresh_now() {
    let (lock, cv) = wanted();
    lock.lock().changed = true;
    cv.notify_one();
}

pub fn revision() -> u64 {
    REVISION.load(Ordering::Relaxed)
}

pub fn status() -> Status {
    SHARED.read().status.clone()
}

/// Events that have not ended yet, soonest first, at most `n`.
pub fn upcoming(now: i64, n: usize) -> Vec<Event> {
    SHARED
        .read()
        .events
        .iter()
        .filter(|e| e.end > now)
        .take(n)
        .cloned()
        .collect()
}

fn publish(events: Option<Vec<Event>>, status: Status) {
    let mut shared = SHARED.write();
    if let Some(events) = events {
        shared.events = events;
    }
    shared.status = status;
    REVISION.fetch_add(1, Ordering::Relaxed);
}

fn fetch_loop() {
    // WinRT's HTTP client wants a COM apartment on this thread.
    unsafe {
        let _ = windows::Win32::System::Com::CoInitializeEx(
            None,
            windows::Win32::System::Com::COINIT_MULTITHREADED,
        );
    }
    let (lock, cv) = wanted();
    loop {
        let (source, refresh) = {
            let mut w = lock.lock();
            w.changed = false;
            (w.source.clone(), w.refresh)
        };

        if source.is_empty() {
            publish(Some(Vec::new()), Status::Unset);
        } else {
            if SHARED.read().events.is_empty() {
                publish(None, Status::Loading);
            }
            match read_source(&source) {
                Ok(text) => {
                    let events = parse(&text, now_local());
                    publish(Some(events), Status::Ready);
                }
                // Keep showing the last good events through a blip.
                Err(e) => publish(None, Status::Failed(e)),
            }
        }

        let mut w = lock.lock();
        if !w.changed {
            cv.wait_for(&mut w, refresh);
        }
    }
}

fn read_source(source: &str) -> Result<String, String> {
    let lower = source.to_ascii_lowercase();
    if lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("webcal://")
    {
        let url = if lower.starts_with("webcal://") {
            format!("https://{}", &source["webcal://".len()..])
        } else {
            source.to_owned()
        };
        fetch(&url).map_err(|e| format!("Could not download the calendar ({})", e.message()))
    } else {
        std::fs::read_to_string(source).map_err(|e| format!("Could not open the file ({e})"))
    }
}

fn fetch(url: &str) -> windows::core::Result<String> {
    use windows::core::HSTRING;
    use windows::Foundation::Uri;
    use windows::Web::Http::HttpClient;

    let client = HttpClient::new()?;
    let uri = Uri::CreateUri(&HSTRING::from(url))?;
    let text = client.GetStringAsync(&uri)?.get()?;
    Ok(text.to_string_lossy())
}

// -- time ------------------------------------------------------------------

/// Days from 1970-01-01 to the given civil date (Howard Hinnant's algorithm).
pub fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The civil date of a day count from 1970-01-01.
pub fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// 0 = Sunday.
pub fn weekday(days: i64) -> i64 {
    (days + 4).rem_euclid(7)
}

fn minutes(st: &windows::Win32::Foundation::SYSTEMTIME) -> i64 {
    days_from_civil(st.wYear as i64, st.wMonth as i64, st.wDay as i64) * 1440
        + st.wHour as i64 * 60
        + st.wMinute as i64
}

/// Local minutes since the epoch, now.
pub fn now_local() -> i64 {
    minutes(&unsafe { GetLocalTime() })
}

fn utc_offset() -> i64 {
    let local = minutes(&unsafe { GetLocalTime() });
    let utc = minutes(&unsafe { GetSystemTime() });
    // Round to the quarter hour to absorb the minute ticking over between
    // the two reads.
    ((local - utc) as f64 / 15.0).round() as i64 * 15
}

// -- parsing ---------------------------------------------------------------

/// A date-time value: local minutes, and whether it was a bare date.
fn parse_time(value: &str, params: &str, offset: i64) -> Option<(i64, bool)> {
    let v = value.trim();
    let num = |s: &str| s.parse::<i64>().ok();
    let date = |s: &str| -> Option<i64> {
        Some(days_from_civil(
            num(s.get(0..4)?)?,
            num(s.get(4..6)?)?,
            num(s.get(6..8)?)?,
        ))
    };
    if v.len() == 8 || params.contains("VALUE=DATE") && !v.contains('T') {
        return Some((date(v)? * 1440, true));
    }
    let day = date(v)?;
    let t = v.get(9..)?;
    let mins = day * 1440 + num(t.get(0..2)?)? * 60 + num(t.get(2..4)?)?;
    Some((
        if v.ends_with('Z') {
            mins + offset
        } else {
            mins
        },
        false,
    ))
}

/// `PT1H30M`, `P1D`, `-PT15M` and so on, in minutes.
fn parse_duration(v: &str) -> Option<i64> {
    let (sign, rest) = match v.strip_prefix('-') {
        Some(r) => (-1, r),
        None => (1, v.strip_prefix('+').unwrap_or(v)),
    };
    let rest = rest.strip_prefix('P')?;
    let (mut total, mut n, mut in_time) = (0i64, 0i64, false);
    for c in rest.chars() {
        match c {
            '0'..='9' => n = n * 10 + c.to_digit(10)? as i64,
            'T' => in_time = true,
            'W' => (total, n) = (total + n * 7 * 1440, 0),
            'D' => (total, n) = (total + n * 1440, 0),
            'H' if in_time => (total, n) = (total + n * 60, 0),
            'M' if in_time => (total, n) = (total + n, 0),
            'S' if in_time => n = 0,
            _ => return None,
        }
    }
    Some(sign * total)
}

fn unescape(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    let mut chars = v.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') | Some('N') => out.push(' '),
                Some(other) => out.push(other),
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[derive(Default)]
struct RawEvent {
    uid: String,
    start: Option<(i64, bool)>,
    end: Option<(i64, bool)>,
    duration: Option<i64>,
    title: String,
    location: String,
    rrule: Option<String>,
    exdates: Vec<i64>,
    recurrence_id: Option<i64>,
    cancelled: bool,
}

/// Every occurrence of the feed's events between a day ago and
/// [`HORIZON_DAYS`] ahead of `now`, sorted by start.
pub fn parse(text: &str, now: i64) -> Vec<Event> {
    let offset = utc_offset();

    // Unfold: a line starting with a space or tab continues the one before.
    let mut lines: Vec<String> = Vec::new();
    for raw in text.lines() {
        if let Some(rest) = raw.strip_prefix(' ').or_else(|| raw.strip_prefix('\t')) {
            if let Some(last) = lines.last_mut() {
                last.push_str(rest);
                continue;
            }
        }
        lines.push(raw.to_owned());
    }

    let mut raws: Vec<RawEvent> = Vec::new();
    let mut current: Option<RawEvent> = None;
    for line in &lines {
        if line == "BEGIN:VEVENT" {
            current = Some(RawEvent::default());
            continue;
        }
        if line == "END:VEVENT" {
            if let Some(ev) = current.take() {
                raws.push(ev);
            }
            continue;
        }
        let Some(ev) = current.as_mut() else {
            continue;
        };
        let Some((head, value)) = line.split_once(':') else {
            continue;
        };
        let (name, params) = head.split_once(';').unwrap_or((head, ""));
        match name.to_ascii_uppercase().as_str() {
            "UID" => ev.uid = value.to_owned(),
            "DTSTART" => ev.start = parse_time(value, params, offset),
            "DTEND" => ev.end = parse_time(value, params, offset),
            "DURATION" => ev.duration = parse_duration(value),
            "SUMMARY" => ev.title = unescape(value),
            "LOCATION" => ev.location = unescape(value),
            "RRULE" => ev.rrule = Some(value.to_owned()),
            "EXDATE" => ev.exdates.extend(
                value
                    .split(',')
                    .filter_map(|v| parse_time(v, params, offset).map(|(m, _)| m)),
            ),
            "RECURRENCE-ID" => ev.recurrence_id = parse_time(value, params, offset).map(|t| t.0),
            "STATUS" => ev.cancelled = value.eq_ignore_ascii_case("CANCELLED"),
            _ => {}
        }
    }

    // A moved or cancelled single occurrence of a series arrives as its own
    // VEVENT with a RECURRENCE-ID; the series must skip that slot.
    let mut overridden: Vec<(String, i64)> = Vec::new();
    for ev in &raws {
        if let Some(rid) = ev.recurrence_id {
            overridden.push((ev.uid.clone(), rid));
        }
    }

    let window = (now - 1440, now + HORIZON_DAYS * 1440);
    let mut out = Vec::new();
    for ev in raws {
        let Some((start, all_day)) = ev.start else {
            continue;
        };
        let length = match (ev.end, ev.duration) {
            (Some((end, _)), _) if end > start => end - start,
            (_, Some(d)) if d > 0 => d,
            _ if all_day => 1440,
            _ => 0,
        };
        let make = |at: i64| Event {
            start: at,
            end: at + length,
            all_day,
            title: if ev.title.is_empty() {
                "(No title)".to_owned()
            } else {
                ev.title.clone()
            },
            location: ev.location.clone(),
        };

        let rule = ev.rrule.as_deref().filter(|_| ev.recurrence_id.is_none());
        let starts = match rule {
            Some(rule) => expand(rule, start, window.1, offset),
            None => vec![start],
        };
        for at in starts {
            if ev.cancelled || at + length.max(1) <= window.0 || at > window.1 {
                continue;
            }
            if ev.exdates.contains(&at)
                || rule.is_some() && overridden.iter().any(|(u, t)| *u == ev.uid && *t == at)
            {
                continue;
            }
            out.push(make(at));
        }
    }
    out.sort_by_key(|e| (e.start, e.end));
    out
}

/// Occurrence starts of an `RRULE` from `start` up to `until`. Covers the
/// rules calendars actually emit for meetings: DAILY, WEEKLY (with BYDAY),
/// MONTHLY on the same date, YEARLY; with INTERVAL, COUNT and UNTIL.
fn expand(rule: &str, start: i64, until: i64, offset: i64) -> Vec<i64> {
    let mut freq = "";
    let mut interval = 1i64;
    let mut count: Option<usize> = None;
    let mut end = until;
    let mut by_day: Vec<i64> = Vec::new();
    for part in rule.split(';') {
        let Some((k, v)) = part.split_once('=') else {
            continue;
        };
        match k {
            "FREQ" => freq = v,
            "INTERVAL" => interval = v.parse().unwrap_or(1).max(1),
            "COUNT" => count = v.parse().ok(),
            "UNTIL" => {
                if let Some((t, all_day)) = parse_time(v, "", offset) {
                    end = end.min(if all_day { t + 1439 } else { t });
                }
            }
            "BYDAY" => {
                by_day = v
                    .split(',')
                    .filter_map(|d| {
                        let d = d.trim_start_matches(|c: char| c == '-' || c.is_ascii_digit());
                        ["SU", "MO", "TU", "WE", "TH", "FR", "SA"]
                            .iter()
                            .position(|n| *n == d)
                            .map(|i| i as i64)
                    })
                    .collect()
            }
            _ => {}
        }
    }

    let day0 = start.div_euclid(1440);
    let time = start.rem_euclid(1440);
    let mut out = Vec::new();
    let max = count.unwrap_or(usize::MAX);
    let push = |at: i64, out: &mut Vec<i64>| -> bool {
        if at > end || out.len() >= max {
            return false;
        }
        out.push(at);
        true
    };

    match freq {
        "DAILY" => {
            let mut d = day0;
            while push(d * 1440 + time, &mut out) {
                d += interval;
            }
        }
        "WEEKLY" => {
            let days = if by_day.is_empty() {
                vec![weekday(day0)]
            } else {
                by_day
            };
            // Walk week by week from the Sunday of the first week.
            let mut week = day0 - weekday(day0);
            'weeks: loop {
                for wd in 0..7 {
                    let d = week + wd;
                    if d < day0 || !days.contains(&wd) {
                        continue;
                    }
                    if !push(d * 1440 + time, &mut out) {
                        break 'weeks;
                    }
                }
                week += 7 * interval;
                if week * 1440 > end {
                    break;
                }
            }
        }
        "MONTHLY" | "YEARLY" => {
            let (y0, m0, d0) = civil_from_days(day0);
            let step = if freq == "MONTHLY" {
                interval
            } else {
                12 * interval
            };
            let mut k = 0;
            loop {
                let months = m0 - 1 + k * step;
                let (y, m) = (y0 + months.div_euclid(12), months.rem_euclid(12) + 1);
                let d = days_from_civil(y, m, d0);
                // Skip months without that date (the 31st, February 30th).
                if civil_from_days(d).2 == d0 && !push(d * 1440 + time, &mut out) {
                    break;
                }
                if d * 1440 > end {
                    break;
                }
                k += 1;
            }
        }
        _ => out.push(start),
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_round_trip() {
        for d in [-1000, 0, 19_000, 20_744] {
            let (y, m, dd) = civil_from_days(d);
            assert_eq!(days_from_civil(y, m, dd), d);
        }
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(weekday(0), 4); // a Thursday
    }

    #[test]
    fn durations() {
        assert_eq!(parse_duration("PT1H30M"), Some(90));
        assert_eq!(parse_duration("P1D"), Some(1440));
        assert_eq!(parse_duration("-PT15M"), Some(-15));
    }

    #[test]
    fn weekly_meeting_expands_on_its_days() {
        let monday = days_from_civil(2026, 10, 19);
        let start = monday * 1440 + 9 * 60;
        let ics = format!(
            "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:a\r\nDTSTART:{}\r\nDURATION:PT30M\r\n\
             RRULE:FREQ=WEEKLY;BYDAY=MO,WE\r\nSUMMARY:Stand\\, up\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
            "20261019T090000"
        );
        let events = parse(&ics, start);
        let days: Vec<i64> = events.iter().map(|e| e.start / 1440 - monday).collect();
        assert_eq!(days, vec![0, 2, 7]);
        assert_eq!(events[0].title, "Stand, up");
        assert_eq!(events[0].end - events[0].start, 30);
        assert_eq!(events[0].start, start);
    }

    #[test]
    fn overridden_and_cancelled_occurrences_are_skipped() {
        let ics = "BEGIN:VEVENT\nUID:b\nDTSTART:20261019T100000\nDTEND:20261019T110000\n\
                   RRULE:FREQ=DAILY;COUNT=3\nSUMMARY:Daily\nEND:VEVENT\n\
                   BEGIN:VEVENT\nUID:b\nRECURRENCE-ID:20261020T100000\n\
                   DTSTART:20261020T150000\nDTEND:20261020T160000\nSUMMARY:Moved\nEND:VEVENT\n";
        let now = days_from_civil(2026, 10, 19) * 1440;
        let titles: Vec<(i64, String)> = parse(ics, now)
            .into_iter()
            .map(|e| (e.start % 1440 / 60, e.title))
            .collect();
        assert_eq!(
            titles,
            vec![
                (10, "Daily".to_owned()),
                (15, "Moved".to_owned()),
                (10, "Daily".to_owned())
            ]
        );
    }
}
