//! The focus timer behind the Timer slide: Pomodoro rounds of focus and
//! break.
//!
//! The state is process-wide rather than part of the notch's per-frame state,
//! so the settings window and the notch see the same timer, and it keeps
//! counting whichever slide is on screen. It is a deadline, not a counter:
//! nothing ticks while it runs, and the notch only repaints for it once a
//! second, and only while a Timer slide is showing.

use std::time::{Duration, Instant};

use parking_lot::Mutex;

use crate::config::TimerConfig;
use crate::notch::notify::{self, NotificationLevel};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Focus,
    ShortBreak,
    LongBreak,
}

impl Phase {
    pub fn label(self) -> &'static str {
        match self {
            Phase::Focus => "FOCUS",
            Phase::ShortBreak => "SHORT BREAK",
            Phase::LongBreak => "LONG BREAK",
        }
    }

    fn length(self, cfg: &TimerConfig) -> Duration {
        let mins = match self {
            Phase::Focus => cfg.focus_mins,
            Phase::ShortBreak => cfg.short_break_mins,
            Phase::LongBreak => cfg.long_break_mins,
        };
        Duration::from_secs(mins.clamp(1, 240) as u64 * 60)
    }
}

/// What the slide draws.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimerView {
    pub phase: Phase,
    pub running: bool,
    /// Whole seconds left, rounded up so "0:00" only shows at the very end.
    pub remaining_secs: u32,
    pub total_secs: u32,
    /// 1-based focus round within the set.
    pub round: u32,
    pub rounds: u32,
}

impl TimerView {
    /// "24:59", or "1:05:00" past an hour.
    pub fn clock(&self) -> String {
        let s = self.remaining_secs;
        if s >= 3600 {
            format!("{}:{:02}:{:02}", s / 3600, (s % 3600) / 60, s % 60)
        } else {
            format!("{}:{:02}", s / 60, s % 60)
        }
    }

    /// How much of the phase is done, 0 to 1.
    pub fn progress(&self) -> f32 {
        if self.total_secs == 0 {
            0.0
        } else {
            1.0 - self.remaining_secs as f32 / self.total_secs as f32
        }
    }

    /// Everything that changes what the slide shows, packed for a frame key.
    pub fn key(&self) -> u64 {
        self.remaining_secs as u64
            | (self.running as u64) << 32
            | (self.phase as u64) << 33
            | (self.round as u64) << 36
    }
}

struct Timer {
    phase: Phase,
    round: u32,
    /// When the running phase ends. `None` while paused or not started.
    ends_at: Option<Instant>,
    /// Time left while paused; `None` means the whole phase.
    left: Option<Duration>,
}

static TIMER: Mutex<Timer> = Mutex::new(Timer {
    phase: Phase::Focus,
    round: 1,
    ends_at: None,
    left: None,
});

impl Timer {
    fn remaining(&self, cfg: &TimerConfig, now: Instant) -> Duration {
        match self.ends_at {
            Some(end) => end.saturating_duration_since(now),
            None => self.left.unwrap_or_else(|| self.phase.length(cfg)),
        }
    }

    fn start(&mut self, cfg: &TimerConfig, now: Instant) {
        self.ends_at = Some(now + self.remaining(cfg, now));
        self.left = None;
    }

    /// Move to the phase after this one. Returns the phase just finished.
    fn advance(&mut self, cfg: &TimerConfig) -> Phase {
        let done = self.phase;
        let rounds = cfg.rounds.max(1);
        self.phase = match done {
            Phase::Focus if self.round >= rounds => Phase::LongBreak,
            Phase::Focus => Phase::ShortBreak,
            Phase::ShortBreak => {
                self.round += 1;
                Phase::Focus
            }
            Phase::LongBreak => {
                self.round = 1;
                Phase::Focus
            }
        };
        self.ends_at = None;
        self.left = None;
        done
    }
}

pub fn view(cfg: &TimerConfig) -> TimerView {
    let t = TIMER.lock();
    let total = t.phase.length(cfg);
    let remaining = t.remaining(cfg, Instant::now());
    TimerView {
        phase: t.phase,
        running: t.ends_at.is_some(),
        remaining_secs: remaining.as_secs_f32().ceil() as u32,
        total_secs: total.as_secs() as u32,
        round: t.round,
        rounds: cfg.rounds.max(1),
    }
}

/// Start, or pause where it is.
pub fn toggle(cfg: &TimerConfig) {
    let mut t = TIMER.lock();
    let now = Instant::now();
    if t.ends_at.is_some() {
        t.left = Some(t.remaining(cfg, now));
        t.ends_at = None;
    } else {
        t.start(cfg, now);
    }
}

/// Back to the start of a fresh set.
pub fn reset() {
    let mut t = TIMER.lock();
    *t = Timer {
        phase: Phase::Focus,
        round: 1,
        ends_at: None,
        left: None,
    };
}

/// End this phase now and move to the next, paused.
pub fn skip(cfg: &TimerConfig) {
    TIMER.lock().advance(cfg);
}

/// Called every overlay tick. Rolls the phase over when it runs out and says
/// so with a toast. Cheap: one lock and a comparison.
pub fn tick(cfg: &TimerConfig, toast_secs: f32) {
    let mut t = TIMER.lock();
    let Some(end) = t.ends_at else {
        return;
    };
    if Instant::now() < end {
        return;
    }
    let done = t.advance(cfg);
    let (title, body) = match done {
        Phase::Focus => {
            if cfg.auto_start_breaks {
                t.start(cfg, Instant::now());
            }
            let next = if t.phase == Phase::LongBreak {
                "long break"
            } else {
                "short break"
            };
            ("Focus round done", format!("Nice work. Time for a {next}."))
        }
        Phase::ShortBreak | Phase::LongBreak => {
            ("Break's over", "Ready for the next focus round?".to_owned())
        }
    };
    drop(t);
    // An empty allow-list lets Venu's own alerts through whatever the user's
    // app filter says.
    notify::global_store().write().push(
        "Focus Timer",
        title,
        &body,
        NotificationLevel::Success,
        toast_secs,
        &[],
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounds_lead_to_a_long_break_and_back() {
        let cfg = TimerConfig {
            rounds: 2,
            ..Default::default()
        };
        let mut t = Timer {
            phase: Phase::Focus,
            round: 1,
            ends_at: None,
            left: None,
        };
        t.advance(&cfg);
        assert_eq!(t.phase, Phase::ShortBreak);
        t.advance(&cfg);
        assert_eq!((t.phase, t.round), (Phase::Focus, 2));
        t.advance(&cfg);
        assert_eq!(t.phase, Phase::LongBreak);
        t.advance(&cfg);
        assert_eq!((t.phase, t.round), (Phase::Focus, 1));
    }

    #[test]
    fn pausing_keeps_the_time_left() {
        let cfg = TimerConfig::default();
        let now = Instant::now();
        let mut t = Timer {
            phase: Phase::Focus,
            round: 1,
            ends_at: Some(now + Duration::from_secs(90)),
            left: None,
        };
        let left = t.remaining(&cfg, now);
        t.left = Some(left);
        t.ends_at = None;
        assert_eq!(t.remaining(&cfg, now + Duration::from_secs(500)), left);
    }
}
