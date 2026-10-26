//! Runtime state for the notch: what it is showing, how open it is, and
//! whether the user is currently typing into it.
//!
//! Nothing here touches Win32 or Direct2D — it is pure state so the open /
//! collapse / carousel behaviour can be reasoned about on its own.

use std::time::{Duration, Instant};

use crate::config::{AppConfig, BorderEffect, BorderEffectConfig, SlideKind};
use crate::notch::anim::{smoothstep, Spring};
use crate::notch::layout::{self, PodKind, Slot};

/// Placeholder shown when the focus line has been cleared entirely.
const EMPTY_FOCUS: &str = "What are you working on?";

/// How long a confirmation badge stays up.
const FLASH_SECONDS: f32 = 1.6;

/// What the inline text field is editing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EditTarget {
    /// The Status slide's focus line.
    #[default]
    Focus,
    /// A new item for the Status slide's short list.
    NewItem,
}

/// A short confirmation shown over the notch: the action itself is invisible
/// (clicks go elsewhere, hover simply stops), so the notch says what changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Badge {
    #[default]
    ClickThrough,
    /// Hover was paused (`true`) or resumed.
    HoverPaused(bool),
    Copied,
    AddedToShelf,
}

/// Delay before the first border effect after launch, or after one is picked
/// in settings, so the choice is seen straight away rather than an interval
/// later.
const EFFECT_FIRST_RUN: f32 = 0.6;

pub struct NotchState {
    /// 0 = collapsed pill, 1 = fully open panel.
    pub expand: Spring,
    /// 0 = normal collapsed/expanded, 1 = dynamic alert pill toast.
    pub toast_progress: Spring,
    /// Continuous carousel position; settles on `active`.
    pub carousel: Spring,
    /// The slide the notch rests on.
    pub active: usize,

    pub hovered: bool,
    /// When the cursor left. Collapsing waits out a grace period so brushing
    /// the edge of the panel does not slam it shut.
    left_at: Option<Instant>,

    /// Set by the pin toggle. Holds the panel open regardless of hover, the
    /// same as `editing` does — pinning is just a user-requested version of
    /// "the cursor is still here".
    pub pinned: bool,

    /// Opened by a click, the shortcut or the mouse trigger: stays open until
    /// a click elsewhere, the same gesture again, or a menu choice closes it.
    pub sticky: bool,
    /// Hover may not open the notch before this ("Pause hover" in the menu).
    hover_paused_until: Option<Instant>,
    /// Seconds the cursor has rested on the closed pill. Hover opens the
    /// notch once this passes the configured delay.
    pub dwell: f32,
    /// After the notch is closed on purpose, hover may not reopen it until
    /// the cursor has left; otherwise closing it from under the cursor would
    /// bounce straight back open.
    pub hover_blocked: bool,
    /// The small grow of the closed pill under the cursor: "this opens".
    pub peek: Spring,
    /// Whether the cursor is on the closed pill and the pill should show it.
    pub peeking: bool,
    /// What the expansion was last asked to be, open or shut.
    pub open_target: bool,
    /// The wheel moved the closed pill to another face. The resting-face
    /// rule leaves it there until the panel next opens.
    pub face_override: bool,

    pub editing: bool,
    pub edit_target: EditTarget,
    pub edit_buffer: String,
    /// Music wave bar heights, 0..1, fed by the window's audio meter.
    pub wave: [f32; layout::WAVE_BARS],
    pub caret_on: bool,
    caret_clock: f32,

    /// Marquee scroll position, in pixels.
    pub marquee_offset: f32,

    /// Free-running clock for ambient motion (the accent pulse).
    pub elapsed: f32,

    /// Currently inspected notification in Notification Center expanded slide.
    pub selected_notification_id: Option<u64>,

    /// Set when something the user changed needs writing back to disk.
    pub dirty: bool,

    /// Seconds left on the click-through confirmation badge. The chord is
    /// invisible by nature — the clicks go somewhere else — so the notch says
    /// out loud which mode it just entered, then gets out of the way.
    pub badge_time: f32,
    /// What the badge says while `badge_time` runs.
    pub badge: Badge,

    /// Seconds into the running border effect; `None` between runs, which is
    /// almost always. Part of the frame key, so the notch only repaints for
    /// an effect while one is actually playing.
    pub effect_time: Option<f32>,
    /// Seconds until the next run.
    effect_wait: f32,
    /// What the schedule was last armed for, so picking another effect or
    /// pressing Preview starts a run immediately.
    effect_armed: (BorderEffect, u32, bool),

    /// Split pill side segments, `[left, right]`. Empty and free unless the
    /// split is switched on.
    pub split: [Slot<SlideKind>; 2],
    /// Satellite pods, `[right, left]`. Fed by the window, which owns the
    /// readers that know what is live.
    pub pods: [Slot<PodKind>; 2],
    /// Width the moving-text segment wants, measured from its text by the
    /// window. Zero until measured, which falls back to the default width.
    pub text_segment_width: f32,
    /// Set when the panel opened onto a segment or pod's slide instead of the
    /// one the pill rests on: `(resting slide, slide opened onto)`. Collapsing
    /// puts the resting one back, unless the user scrolled elsewhere meanwhile.
    home: Option<(usize, usize)>,
}

impl NotchState {
    pub fn new(cfg: &AppConfig) -> Self {
        let active = cfg.notch.clamped_active();
        Self {
            // Stiff enough to feel immediate, damped just under critical so it
            // arrives with the faintest settle rather than a dead stop.
            expand: Spring::new(0.0, 200.0, 26.0),
            toast_progress: Spring::new(0.0, 220.0, 28.0),
            carousel: Spring::new(active as f32, 230.0, 30.0),
            active,
            hovered: false,
            left_at: None,
            pinned: false,
            sticky: false,
            hover_paused_until: None,
            dwell: 0.0,
            hover_blocked: false,
            // Quick and soft: a hint, not an event.
            peek: Spring::new(0.0, 320.0, 34.0),
            peeking: false,
            open_target: false,
            face_override: false,
            editing: false,
            edit_target: EditTarget::Focus,
            edit_buffer: String::new(),
            wave: [0.22; layout::WAVE_BARS],
            caret_on: true,
            caret_clock: 0.0,
            marquee_offset: 0.0,
            elapsed: 0.0,
            selected_notification_id: None,
            dirty: false,
            badge_time: 0.0,
            badge: Badge::ClickThrough,
            effect_time: None,
            effect_wait: EFFECT_FIRST_RUN,
            effect_armed: (
                cfg.notch.border_effect.effect,
                cfg.notch.border_effect.preview,
                cfg.notch.border_effect.continuous,
            ),
            // Same feel as the carousel: quick, settling without a wobble.
            split: [Slot::new(230.0, 30.0), Slot::new(230.0, 30.0)],
            // A touch springier, so a pod visibly buds out of the notch.
            pods: [Slot::new(260.0, 24.0), Slot::new(260.0, 24.0)],
            text_segment_width: 0.0,
            home: None,
        }
    }

    /// Current widths of the split pill's `[left, right]` segments, animated.
    pub fn split_widths(&self) -> (f32, f32) {
        let width = |slot: &Slot<SlideKind>| {
            slot.shown
                .map(|kind| self.segment_width(kind) * slot.amount())
                .unwrap_or(0.0)
        };
        (width(&self.split[0]), width(&self.split[1]))
    }

    /// Full width of a side segment showing `kind`: moving text as measured,
    /// everything else by its fixed size.
    pub fn segment_width(&self, kind: SlideKind) -> f32 {
        if kind == SlideKind::Marquee && self.text_segment_width > 0.0 {
            self.text_segment_width
        } else {
            layout::segment_width(kind)
        }
    }

    /// Whether a side segment is showing `kind` at all.
    pub fn split_shows(&self, kind: SlideKind) -> bool {
        self.split.iter().any(|slot| slot.visible() == Some(kind))
    }

    /// Seat and animate the pods. `wanted` is in priority order; pods keep
    /// the side they already have.
    pub fn step_pods(&mut self, wanted: [Option<PodKind>; 2], dt: f32) {
        let seated = [self.pods[0].shown, self.pods[1].shown];
        let sides = layout::assign_sides(seated, wanted);
        for (slot, want) in self.pods.iter_mut().zip(sides) {
            slot.step(want, dt);
        }
    }

    /// Open onto the first of `kinds` that is in the carousel, remembering
    /// where the pill was resting. Returns whether it switched slides.
    pub fn open_onto(&mut self, kinds: &[SlideKind], cfg: &AppConfig) -> bool {
        if self.home.is_some() {
            return false;
        }
        let slides = cfg.notch.effective_slides();
        let Some(idx) = kinds
            .iter()
            .find_map(|kind| slides.iter().position(|s| s == kind))
        else {
            return false;
        };
        if idx == self.active {
            return false;
        }
        self.home = Some((self.active, idx));
        self.active = idx;
        // Jump, rather than fly the carousel past everything in between.
        self.carousel.set(idx as f32);
        true
    }

    /// Back to the resting slide after the panel closed, if it was opened
    /// onto a segment or pod and the user did not move on from there.
    pub fn restore_home(&mut self) {
        if let Some((resting, opened)) = self.home.take() {
            if self.active == opened {
                self.active = resting;
                self.carousel.set(resting as f32);
            }
        }
    }

    /// Run the border effect on its interval. Between runs this is a
    /// subtraction a tick; nothing is painted.
    fn tick_border_effect(&mut self, cfg: &AppConfig, dt: f32) {
        let fx = &cfg.notch.border_effect;
        let armed = (fx.effect, fx.preview, fx.continuous);
        if armed != self.effect_armed {
            self.effect_armed = armed;
            self.effect_time = None;
            self.effect_wait = EFFECT_FIRST_RUN;
        }
        if fx.effect == BorderEffect::Off {
            self.effect_time = None;
            return;
        }

        if let Some(time) = self.effect_time {
            let time = time + dt;
            if fx.continuous {
                // Wrap rather than stop; every effect starts and ends in the
                // same place, so the seam does not show.
                self.effect_time = Some(time % fx.duration_secs.max(0.1));
            } else if time >= fx.duration_secs {
                self.effect_time = None;
                self.effect_wait =
                    fx.interval_secs.max(BorderEffectConfig::MIN_INTERVAL_SECS) as f32;
            } else {
                self.effect_time = Some(time);
            }
            return;
        }

        self.effect_wait -= dt;
        // A notification glow owns the rim while it is up; wait it out.
        let toast_up = self.toast_progress.value > 0.01;
        if self.effect_wait <= 0.0 && !toast_up {
            self.effect_time = Some(0.0);
        }
    }

    /// Progress of the running border effect, 0 to 1.
    pub fn effect_progress(&self, cfg: &AppConfig) -> Option<f32> {
        let duration = cfg.notch.border_effect.duration_secs.max(0.1);
        self.effect_time.map(|t| (t / duration).clamp(0.0, 1.0))
    }

    /// Show the click-through badge for long enough to read and no longer.
    pub fn flash_click_through(&mut self) {
        self.flash(Badge::ClickThrough);
    }

    pub fn flash(&mut self, badge: Badge) {
        self.badge = badge;
        self.badge_time = FLASH_SECONDS;
    }

    /// Stop holding the panel open for a click; it closes once the cursor is
    /// away, like any hover.
    pub fn release_sticky(&mut self) {
        self.sticky = false;
        if !self.hovered {
            self.left_at = Some(Instant::now());
        }
    }

    pub fn toggle_pinned(&mut self) {
        self.pinned = !self.pinned;
        // Unpinning while the cursor has already wandered off should start
        // the collapse grace period now, not carry on looking "hovered".
        if !self.pinned && !self.hovered {
            self.left_at = Some(Instant::now());
        }
    }

    /// Reset active slide to the user's default_collapsed setting upon collapse.
    pub fn apply_default_collapsed(
        &mut self,
        cfg: &AppConfig,
        media_active: bool,
        unread_notifs: bool,
    ) {
        use crate::config::CollapsedMode;
        let slides = cfg.notch.effective_slides();
        let target_kind = match cfg.notch.default_collapsed {
            CollapsedMode::LastActive => return,
            CollapsedMode::Status => SlideKind::Status,
            CollapsedMode::Clock => SlideKind::Clock,
            CollapsedMode::Marquee => SlideKind::Marquee,
            CollapsedMode::Wallpaper => SlideKind::Wallpaper,
            CollapsedMode::Media => SlideKind::Media,
            CollapsedMode::Notifications => SlideKind::Notifications,
            CollapsedMode::Usage => SlideKind::Usage,
            CollapsedMode::Stats => SlideKind::Stats,
            CollapsedMode::Timer => SlideKind::Timer,
            CollapsedMode::Calendar => SlideKind::Calendar,
            CollapsedMode::Auto => {
                if unread_notifs && slides.contains(&SlideKind::Notifications) {
                    SlideKind::Notifications
                } else if media_active && slides.contains(&SlideKind::Media) {
                    SlideKind::Media
                } else if slides.contains(&SlideKind::Clock) {
                    SlideKind::Clock
                } else {
                    SlideKind::Status
                }
            }
        };

        if let Some(idx) = slides.iter().position(|s| *s == target_kind) {
            if self.active != idx {
                self.active = idx;
            }
        }
    }

    /// Slow breathing applied to the accent dot, 0.75..1.0.
    pub fn pulse(&self) -> f32 {
        0.875 + 0.125 * (self.elapsed * 1.7).sin()
    }

    pub fn is_open(&self) -> bool {
        self.expand.value > 0.5
    }

    /// The focus string to render: the live edit buffer while typing, the
    /// saved value otherwise.
    pub fn display_focus(&self, cfg: &AppConfig) -> String {
        let raw = if self.editing && self.edit_target == EditTarget::Focus {
            self.edit_buffer.as_str()
        } else {
            cfg.status.focus.as_str()
        };

        if raw.trim().is_empty() && !self.editing {
            EMPTY_FOCUS.to_string()
        } else {
            raw.to_string()
        }
    }

    /// Feed this frame's hover result and return the expansion target.
    ///
    /// `hover_hold` is whether hover wants the panel open: the cursor is on
    /// it and either it is already open or it rested long enough to open it.
    /// A click-held, pinned or edited panel stays open regardless.
    pub fn set_hovered(&mut self, hover_hold: bool, collapse_delay: Duration) -> f32 {
        self.hovered = hover_hold;

        if hover_hold {
            self.left_at = None;
        } else if self.left_at.is_none() {
            self.left_at = Some(Instant::now());
        }

        // Typing keeps the panel open even if the cursor wanders off, or the
        // field would vanish mid-sentence. Pinning does the same, on purpose.
        let target = if hover_hold || self.editing || self.pinned || self.sticky {
            1.0
        } else {
            match self.left_at {
                Some(at) if at.elapsed() >= collapse_delay && self.open_target => 0.0,
                Some(_) if self.open_target => 1.0,
                _ => 0.0,
            }
        };
        if target > 0.5 && !self.open_target {
            // Opening hands the resting face back to the default rule.
            self.face_override = false;
        }
        self.open_target = target > 0.5;
        target
    }

    /// Open and hold open: a click, the shortcut or the mouse trigger.
    pub fn open_sticky(&mut self) {
        self.sticky = true;
        self.hover_blocked = false;
    }

    /// Close now, whatever opened it, and keep hover from reopening it until
    /// the cursor has left.
    pub fn close_now(&mut self) {
        self.sticky = false;
        self.pinned = false;
        self.open_target = false;
        self.hovered = false;
        self.left_at = None;
        self.dwell = 0.0;
        self.hover_blocked = true;
        if self.editing {
            self.cancel_edit();
        }
    }

    /// The shortcut and the mouse trigger: open if shut, close if open.
    pub fn toggle_open(&mut self) {
        if self.open_target {
            self.close_now();
        } else {
            self.open_sticky();
        }
    }

    /// Stop hover from opening the notch for `duration`, or resume it.
    pub fn pause_hover(&mut self, duration: Option<Duration>) {
        self.hover_paused_until = duration.map(|d| Instant::now() + d);
    }

    pub fn hover_paused(&self) -> bool {
        self.hover_paused_until
            .is_some_and(|until| Instant::now() < until)
    }

    pub fn tick(&mut self, cfg: &AppConfig, target_expand: f32, dt: f32) {
        self.elapsed += dt;
        self.badge_time = (self.badge_time - dt).max(0.0);
        self.tick_border_effect(cfg, dt);

        self.expand.step(target_expand, dt);
        let peek_target = if self.peeking { 1.0 } else { 0.0 };
        self.peek.step(peek_target, dt);
        self.carousel.step(self.active as f32, dt);

        // Side segments follow whatever the pill is resting on: never a
        // repeat of the middle face, and nothing at all with the split off.
        let slides = cfg.notch.effective_slides();
        let [left, right] =
            layout::split_targets(&cfg.notch.split, slides.get(self.active).copied());
        self.split[0].step(left, dt);
        self.split[1].step(right, dt);

        // The marquee only advances when it can be seen.
        let expand = self.expand.value.clamp(0.0, 1.0);
        let collapsed_alpha = 1.0 - smoothstep(0.02, 0.30, expand);
        let expanded_alpha = smoothstep(0.42, 0.96, expand);
        // Match the painter's visibility thresholds. Merely having a
        // marquee somewhere in the deck must not animate a hidden slide.
        let marquee_visible = (collapsed_alpha > 0.004
            && (slides
                .get(self.active)
                .map(|s| *s == SlideKind::Marquee)
                .unwrap_or(false)
                || self.split_shows(SlideKind::Marquee)))
            || (expanded_alpha > 0.004
                && slides.iter().enumerate().any(|(i, slide)| {
                    *slide == SlideKind::Marquee
                        && expanded_alpha
                            * (1.0 - (i as f32 - self.carousel.value).abs())
                                .clamp(0.0, 1.0)
                                .powf(1.4)
                            > 0.006
                }));

        if marquee_visible && cfg.marquee.scroll {
            let speed = if cfg.animation.reverse {
                -cfg.animation.speed
            } else {
                cfg.animation.speed
            };
            self.marquee_offset += speed * dt;
            // Keep the accumulator small; the painter re-wraps it anyway.
            if self.marquee_offset.abs() > 1.0e6 {
                self.marquee_offset = 0.0;
            }
        }

        if self.editing {
            self.caret_clock += dt;
            if self.caret_clock >= 0.53 {
                self.caret_clock = 0.0;
                self.caret_on = !self.caret_on;
            }
        } else {
            self.caret_on = true;
            self.caret_clock = 0.0;
        }
    }

    /// Advance the carousel by `steps` slides, clamped to the ends.
    ///
    /// Deliberately clamped rather than wrapping: hitting a wall tells you
    /// where you are in the deck, whereas wrapping makes a four-slide carousel
    /// feel like an infinite scroll.
    pub fn step_slide(&mut self, steps: i32, cfg: &AppConfig) {
        let len = cfg.notch.effective_slides().len();
        if len == 0 {
            return;
        }
        let next = (self.active as i32 + steps).clamp(0, len as i32 - 1) as usize;
        if next != self.active {
            self.active = next;
            self.dirty = true;
            // Changing slides while typing would strand the edit.
            self.cancel_edit();
        }
    }

    pub fn clamp_active(&mut self, cfg: &AppConfig) {
        let len = cfg.notch.effective_slides().len();
        if len == 0 {
            return;
        }
        if self.active >= len {
            self.active = len - 1;
            self.carousel.set(self.active as f32);
        }
    }

    pub fn begin_edit(&mut self, cfg: &AppConfig) {
        if self.editing {
            return;
        }
        self.editing = true;
        self.edit_target = EditTarget::Focus;
        self.edit_buffer = cfg.status.focus.clone();
        self.caret_on = true;
        self.caret_clock = 0.0;
    }

    /// Start typing a new item for the short list.
    pub fn begin_add(&mut self) {
        if self.editing {
            return;
        }
        self.editing = true;
        self.edit_target = EditTarget::NewItem;
        self.edit_buffer.clear();
        self.caret_on = true;
        self.caret_clock = 0.0;
    }

    /// Whether the short list's add row is being typed into.
    pub fn adding_item(&self) -> bool {
        self.editing && self.edit_target == EditTarget::NewItem
    }

    pub fn cancel_edit(&mut self) {
        self.editing = false;
        self.edit_buffer.clear();
    }

    /// Commit the buffer into `cfg`. Returns true when something changed.
    pub fn commit_edit(&mut self, cfg: &mut AppConfig) -> bool {
        if !self.editing {
            return false;
        }
        let next = self.edit_buffer.trim().to_string();
        self.editing = false;
        self.edit_buffer.clear();

        if self.edit_target == EditTarget::NewItem {
            if next.is_empty() || cfg.status.items.len() >= layout::STATUS_MAX_ITEMS {
                return false;
            }
            cfg.status.items.push(crate::config::StatusItem::new(&next));
            return true;
        }

        if next != cfg.status.focus {
            cfg.status.focus = next;
            self.dirty = true;
            true
        } else {
            false
        }
    }

    /// Handle one typed character. Control characters are filtered out here so
    /// the caller only deals with intent.
    pub fn type_char(&mut self, ch: char) {
        if !self.editing {
            return;
        }
        // Reasonable ceiling: the focus line is one thought, not a paragraph.
        if self.edit_buffer.chars().count() >= 160 {
            return;
        }
        if !ch.is_control() {
            self.edit_buffer.push(ch);
            self.caret_on = true;
            self.caret_clock = 0.0;
        }
    }

    pub fn backspace(&mut self) {
        if self.editing {
            self.edit_buffer.pop();
            self.caret_on = true;
            self.caret_clock = 0.0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resting(cfg: &AppConfig, slide: SlideKind, expanded: bool) -> NotchState {
        let mut state = NotchState::new(cfg);
        state.active = cfg
            .notch
            .effective_slides()
            .iter()
            .position(|s| *s == slide)
            .unwrap();
        state.carousel.set(state.active as f32);
        state.expand.set(if expanded { 1.0 } else { 0.0 });
        state
    }

    #[test]
    fn hidden_marquee_does_not_invalidate_a_resting_panel() {
        let cfg = AppConfig::default();
        for slide in cfg.notch.effective_slides().iter().copied() {
            if slide == SlideKind::Marquee {
                continue;
            }
            for expanded in [false, true] {
                let mut state = resting(&cfg, slide, expanded);
                // Multiple idle ticks must leave this part of FrameKey stable.
                for _ in 0..120 {
                    state.tick(&cfg, if expanded { 1.0 } else { 0.0 }, 0.032);
                }
                assert_eq!(state.marquee_offset, 0.0, "{slide:?}, expanded={expanded}");
            }
        }
    }

    #[test]
    fn static_marquee_does_not_animate() {
        let mut cfg = AppConfig::default();
        cfg.marquee.scroll = false;
        for expanded in [false, true] {
            let mut state = resting(&cfg, SlideKind::Marquee, expanded);
            state.tick(&cfg, if expanded { 1.0 } else { 0.0 }, 0.032);
            assert_eq!(state.marquee_offset, 0.0);
        }
    }

    #[test]
    fn visible_marquee_keeps_scrolling_in_both_sizes() {
        let cfg = AppConfig::default();
        for expanded in [false, true] {
            let mut state = resting(&cfg, SlideKind::Marquee, expanded);
            state.tick(&cfg, if expanded { 1.0 } else { 0.0 }, 0.032);
            assert!(state.marquee_offset > 0.0);
        }
    }

    #[test]
    fn marquee_scrolls_while_entering_the_carousel() {
        let cfg = AppConfig::default();
        let mut state = resting(&cfg, SlideKind::Marquee, true);
        state.carousel.set(state.active as f32 - 0.5);
        state.tick(&cfg, 1.0, 0.016);
        assert!(state.marquee_offset > 0.0);
    }

    #[test]
    fn reverse_scroll_is_preserved() {
        let mut cfg = AppConfig::default();
        cfg.animation.reverse = true;
        let mut state = resting(&cfg, SlideKind::Marquee, false);
        state.tick(&cfg, 0.0, 0.032);
        assert!(state.marquee_offset < 0.0);
    }

    /// The whole per-frame tick, held collapsed, for `seconds`.
    fn tick_for(state: &mut NotchState, cfg: &AppConfig, seconds: f32) {
        let mut left = seconds;
        while left > 0.0 {
            let dt = left.min(1.0 / 60.0);
            state.tick(cfg, 0.0, dt);
            left -= dt;
        }
    }

    fn split_cfg(left: SlideKind, right: SlideKind) -> AppConfig {
        let mut cfg = AppConfig::default();
        cfg.notch.split.enabled = true;
        cfg.notch.split.left = Some(left);
        cfg.notch.split.right = Some(right);
        cfg
    }

    #[test]
    fn split_segments_grow_in_and_skip_the_middle_face() {
        let cfg = split_cfg(SlideKind::Clock, SlideKind::Stats);
        let mut state = resting(&cfg, SlideKind::Media, false);
        tick_for(&mut state, &cfg, 1.5);
        let (l, r) = state.split_widths();
        assert!((l - layout::segment_width(SlideKind::Clock)).abs() < 0.5);
        assert!((r - layout::segment_width(SlideKind::Stats)).abs() < 0.5);

        // Resting on the Clock: the left segment would repeat it, so it goes.
        let clock = cfg
            .notch
            .effective_slides()
            .iter()
            .position(|s| *s == SlideKind::Clock);
        state.active = clock.unwrap();
        tick_for(&mut state, &cfg, 1.5);
        assert_eq!(state.split_widths().0, 0.0);
        assert!(!state.split_shows(SlideKind::Clock));
    }

    #[test]
    fn measured_text_sets_the_moving_text_segment() {
        let cfg = split_cfg(SlideKind::Marquee, SlideKind::Stats);
        let mut state = resting(&cfg, SlideKind::Wallpaper, false);
        state.text_segment_width = layout::text_segment_width(250.0);
        tick_for(&mut state, &cfg, 1.5);
        let (left, right) = state.split_widths();
        assert!((left - layout::text_segment_width(250.0)).abs() < 0.5);
        assert!((right - layout::segment_width(SlideKind::Stats)).abs() < 0.5);
    }

    #[test]
    fn split_off_costs_nothing() {
        let cfg = AppConfig::default();
        let mut state = resting(&cfg, SlideKind::Clock, false);
        tick_for(&mut state, &cfg, 1.0);
        assert_eq!(state.split_widths(), (0.0, 0.0));
        assert!(state.split.iter().all(|s| s.shown.is_none()));
    }

    #[test]
    fn marquee_in_a_side_segment_scrolls() {
        let cfg = split_cfg(SlideKind::Marquee, SlideKind::Stats);
        let mut state = resting(&cfg, SlideKind::Clock, false);
        tick_for(&mut state, &cfg, 1.0);
        let before = state.marquee_offset;
        tick_for(&mut state, &cfg, 0.5);
        assert_ne!(state.marquee_offset, before);
    }

    #[test]
    fn opening_onto_a_segment_restores_the_resting_face() {
        let cfg = split_cfg(SlideKind::Clock, SlideKind::Stats);
        let mut state = resting(&cfg, SlideKind::Media, false);
        let media = state.active;
        assert!(state.open_onto(&[SlideKind::Stats], &cfg));
        assert_ne!(state.active, media);
        assert!(!state.dirty, "a peek is not a choice to persist");
        state.restore_home();
        assert_eq!(state.active, media);

        // Scrolling away while open is a real choice: it sticks.
        assert!(state.open_onto(&[SlideKind::Stats], &cfg));
        state.step_slide(-1, &cfg);
        let chosen = state.active;
        state.restore_home();
        assert_eq!(state.active, chosen);
    }

    #[test]
    fn opening_onto_a_slide_outside_the_deck_does_nothing() {
        let mut cfg = split_cfg(SlideKind::Clock, SlideKind::Stats);
        cfg.notch.slides.retain(|s| *s != SlideKind::Stats);
        let mut state = resting(&cfg, SlideKind::Media, false);
        let before = state.active;
        assert!(!state.open_onto(&[SlideKind::Stats], &cfg));
        assert_eq!(state.active, before);
    }

    #[test]
    fn pods_keep_their_side_and_leave_cleanly() {
        let cfg = AppConfig::default();
        let mut state = NotchState::new(&cfg);
        let step = |state: &mut NotchState, wanted, secs: f32| {
            for _ in 0..(secs * 60.0) as usize {
                state.step_pods(wanted, 1.0 / 60.0);
            }
        };
        step(&mut state, [Some(PodKind::Timer), None], 1.0);
        assert_eq!(state.pods[0].shown, Some(PodKind::Timer));
        step(
            &mut state,
            [Some(PodKind::Agent), Some(PodKind::Timer)],
            1.0,
        );
        assert_eq!(
            state.pods[0].shown,
            Some(PodKind::Timer),
            "timer stays right"
        );
        assert_eq!(state.pods[1].shown, Some(PodKind::Agent));
        step(&mut state, [None, None], 1.0);
        assert!(state
            .pods
            .iter()
            .all(|p| p.shown.is_none() && p.amount() == 0.0));
    }

    fn effect_cfg(effect: BorderEffect) -> AppConfig {
        let mut cfg = AppConfig::default();
        cfg.notch.border_effect.effect = effect;
        cfg.notch.border_effect.interval_secs = 30;
        cfg.notch.border_effect.duration_secs = 1.0;
        cfg
    }

    /// Advance the effect clock alone, in idle-poll sized steps.
    fn run_for(state: &mut NotchState, cfg: &AppConfig, seconds: f32) {
        let mut left = seconds;
        while left > 0.0 {
            let dt = left.min(0.032);
            state.tick_border_effect(cfg, dt);
            left -= dt;
        }
    }

    #[test]
    fn border_effect_off_never_runs() {
        let cfg = effect_cfg(BorderEffect::Off);
        let mut state = NotchState::new(&cfg);
        run_for(&mut state, &cfg, 120.0);
        assert!(state.effect_time.is_none());
    }

    #[test]
    fn border_effect_runs_once_then_waits_its_interval() {
        let cfg = effect_cfg(BorderEffect::Rainbow);
        let mut state = NotchState::new(&cfg);

        run_for(&mut state, &cfg, EFFECT_FIRST_RUN + 0.05);
        assert!(state.effect_time.is_some(), "first run starts promptly");

        run_for(&mut state, &cfg, 1.0);
        assert!(state.effect_time.is_none(), "and stops after its duration");

        run_for(&mut state, &cfg, 29.0);
        assert!(state.effect_time.is_none(), "idle for the interval");
        run_for(&mut state, &cfg, 1.1);
        assert!(state.effect_time.is_some(), "then plays again");
    }

    #[test]
    fn preview_restarts_the_schedule() {
        let mut cfg = effect_cfg(BorderEffect::SweepRight);
        let mut state = NotchState::new(&cfg);
        run_for(&mut state, &cfg, EFFECT_FIRST_RUN + 1.2);
        assert!(state.effect_time.is_none());

        cfg.notch.border_effect.preview += 1;
        run_for(&mut state, &cfg, EFFECT_FIRST_RUN + 0.05);
        assert!(state.effect_time.is_some());
    }

    #[test]
    fn continuous_effect_loops_without_a_break() {
        let mut cfg = effect_cfg(BorderEffect::Rainbow);
        cfg.notch.border_effect.continuous = true;
        let mut state = NotchState::new(&cfg);
        run_for(&mut state, &cfg, EFFECT_FIRST_RUN + 0.05);
        for _ in 0..20 {
            run_for(&mut state, &cfg, 0.37);
            let p = state.effect_progress(&cfg).expect("never stops");
            assert!((0.0..1.0).contains(&p));
        }
    }

    #[test]
    fn border_effect_waits_for_a_notification_glow() {
        let cfg = effect_cfg(BorderEffect::Breathe);
        let mut state = NotchState::new(&cfg);
        state.toast_progress.set(1.0);
        run_for(&mut state, &cfg, 5.0);
        assert!(state.effect_time.is_none());

        state.toast_progress.set(0.0);
        run_for(&mut state, &cfg, 0.05);
        assert!(state.effect_time.is_some());
    }

    #[test]
    fn a_click_keeps_the_panel_open_until_it_is_released() {
        let cfg = AppConfig::default();
        let mut state = NotchState::new(&cfg);
        let grace = Duration::from_millis(0);

        state.open_sticky();
        assert_eq!(state.set_hovered(false, grace), 1.0);
        assert_eq!(
            state.set_hovered(false, grace),
            1.0,
            "still open with the cursor away"
        );

        state.release_sticky();
        assert_eq!(state.set_hovered(false, grace), 0.0);
    }

    #[test]
    fn closing_on_purpose_keeps_hover_from_reopening_it() {
        let cfg = AppConfig::default();
        let mut state = NotchState::new(&cfg);
        let grace = Duration::from_millis(220);

        state.toggle_open();
        assert_eq!(state.set_hovered(true, grace), 1.0);
        state.toggle_open();
        assert!(state.hover_blocked);
        // The window turns hover off while blocked, so it asks for shut.
        assert_eq!(
            state.set_hovered(false, grace),
            0.0,
            "closes at once, no grace period"
        );
        assert!(!state.sticky && !state.pinned);
    }

    #[test]
    fn hover_pause_lasts_until_resumed() {
        let cfg = AppConfig::default();
        let mut state = NotchState::new(&cfg);
        assert!(!state.hover_paused());
        state.pause_hover(Some(Duration::from_secs(60)));
        assert!(state.hover_paused());
        state.pause_hover(None);
        assert!(!state.hover_paused());
    }

    #[test]
    fn quick_add_appends_to_the_list_and_leaves_the_focus_alone() {
        let mut cfg = AppConfig::default();
        let focus = cfg.status.focus.clone();
        let before = cfg.status.items.len();
        let mut state = NotchState::new(&cfg);

        state.begin_add();
        assert!(state.adding_item());
        for ch in "Call the bank".chars() {
            state.type_char(ch);
        }
        assert_eq!(
            state.display_focus(&cfg),
            focus,
            "the focus line does not echo the new item"
        );
        assert!(state.commit_edit(&mut cfg));
        assert_eq!(cfg.status.items.len(), before + 1);
        assert_eq!(cfg.status.items.last().unwrap().text, "Call the bank");
        assert_eq!(cfg.status.focus, focus);

        // An empty entry adds nothing.
        state.begin_add();
        assert!(!state.commit_edit(&mut cfg));
        assert_eq!(cfg.status.items.len(), before + 1);
    }
}
