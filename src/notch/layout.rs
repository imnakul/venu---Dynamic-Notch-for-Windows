//! Layout maths for the split pill and the satellite pods.
//!
//! Pure on purpose — no Win32, no Direct2D — so what goes where, and when,
//! can be unit-tested on its own. The window turns these numbers into shapes,
//! the painter draws them, and the settings preview draws the same numbers,
//! so the three never disagree.
//!
//! Both features live inside the notch's existing layered window. The window
//! is already sized to the open panel, which is several times wider than the
//! resting pill, so pods and segments fit in space that is allocated anyway:
//! no extra windows, surfaces or blits, and nothing is painted for them while
//! they are hidden.

use crate::config::{PodsConfig, SlideKind, SplitConfig};
use crate::notch::anim::Spring;

/// Breathing room between the pill and a pod, on top of both shoulders.
pub const POD_GAP: f32 = 6.0;

/// Padding every face keeps from a segment edge, so the gap either side of a
/// divider is always the same. Matches the pill's own gutter.
pub const SEGMENT_PADDING: f32 = 14.0;

/// Moving text sizes its segment to the text; these bound it. Past the
/// widest, a scrolling line scrolls and a static one ends in an ellipsis.
pub const MIN_TEXT_SEGMENT: f32 = 96.0;
pub const MAX_TEXT_SEGMENT: f32 = 420.0;

/// Sub-pixel slack, so text measured to fit exactly is never trimmed to an
/// ellipsis by rounding.
const TEXT_SLACK: f32 = 4.0;

/// Segment width for moving text that measures `text_width` in the pill's
/// font: the text plus padding either side, within the bounds above.
pub fn text_segment_width(text_width: f32) -> f32 {
    (text_width.max(0.0) + SEGMENT_PADDING * 2.0 + TEXT_SLACK)
        .clamp(MIN_TEXT_SEGMENT, MAX_TEXT_SEGMENT)
}

/// Width a face needs as a side segment of the split pill, padding included.
///
/// Sized to what each compact face actually draws, so a segment never cuts
/// its own text off: a clock needs a time and AM/PM, Stats three columns.
/// Moving text is the exception: its real width is measured from the text
/// (see [`text_segment_width`]); this is only its fallback.
pub fn segment_width(kind: SlideKind) -> f32 {
    match kind {
        SlideKind::Clock => 96.0,
        SlideKind::Wallpaper => 96.0,
        SlideKind::Usage => 140.0,
        SlideKind::Timer => 140.0,
        SlideKind::Controls => 140.0,
        SlideKind::Status => 170.0,
        SlideKind::Marquee => 170.0,
        SlideKind::Notifications => 170.0,
        SlideKind::Media => 180.0,
        SlideKind::Calendar => 180.0,
        SlideKind::Stats => 210.0,
    }
}

/// The widest a split pill can add on top of its middle face, for sizing the
/// window frame once rather than per frame.
pub fn max_split_extra(split: &SplitConfig) -> f32 {
    if !split.enabled {
        return 0.0;
    }
    [split.left, split.right]
        .into_iter()
        .flatten()
        .map(|kind| match kind {
            // Measured at runtime, so reserve the most it can take.
            SlideKind::Marquee => MAX_TEXT_SEGMENT,
            other => segment_width(other),
        })
        .sum()
}

/// What the split pill wants either side of `center` right now, as
/// `[left, right]`. Nothing when the split is off, and never a repeat of the
/// face already in the middle or on the other side.
pub fn split_targets(split: &SplitConfig, center: Option<SlideKind>) -> [Option<SlideKind>; 2] {
    if !split.enabled {
        return [None, None];
    }
    let left = split.left.filter(|k| Some(*k) != center);
    let right = split
        .right
        .filter(|k| Some(*k) != center && Some(*k) != left);
    [left, right]
}

/// Which part of the resting pill a point falls in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PillPart {
    Left,
    Center,
    Right,
}

/// Classify `x` against a pill spanning `left..right` whose side segments are
/// `left_w` and `right_w` wide.
pub fn pill_part(x: f32, left: f32, right: f32, left_w: f32, right_w: f32) -> PillPart {
    if left_w > 0.5 && x < left + left_w {
        PillPart::Left
    } else if right_w > 0.5 && x > right - right_w {
        PillPart::Right
    } else {
        PillPart::Center
    }
}

/// One place that shows one thing at a time and animates between them.
///
/// The occupant shrinks away completely before the next one grows in, so two
/// faces never overlap mid-swap, and a slot that is empty and wants to stay
/// empty costs nothing.
#[derive(Debug, Clone, Copy)]
pub struct Slot<K> {
    /// What is drawn in the slot, possibly on its way out.
    pub shown: Option<K>,
    /// How far the occupant is in, 0..1 (springy, may overshoot slightly).
    pub presence: Spring,
}

impl<K: Copy + PartialEq> Slot<K> {
    pub fn new(stiffness: f32, damping: f32) -> Self {
        Self {
            shown: None,
            presence: Spring::new(0.0, stiffness, damping),
        }
    }

    /// Move one step towards showing `want`.
    pub fn step(&mut self, want: Option<K>, dt: f32) {
        if self.shown != want {
            if self.shown.is_none() || self.presence.value <= 0.02 {
                // Empty (or emptied): the newcomer starts from nothing.
                self.shown = want;
                self.presence.set(0.0);
            } else {
                // Clear the old occupant out first.
                self.presence.step(0.0, dt);
                return;
            }
        }
        let target = if self.shown.is_some() { 1.0 } else { 0.0 };
        self.presence.step(target, dt);
    }

    /// How much of the occupant is visible, 0..1, for widths and fades.
    pub fn amount(&self) -> f32 {
        if self.shown.is_some() {
            self.presence.value.clamp(0.0, 1.0)
        } else {
            0.0
        }
    }

    /// The occupant, if it is visible at all.
    pub fn visible(&self) -> Option<K> {
        self.shown.filter(|_| self.amount() > 0.004)
    }
}

// ---------------------------------------------------------------------------
// Pods
// ---------------------------------------------------------------------------

/// A live activity that can earn a pod. Declared in priority order: when more
/// is happening than there is room for, the earlier kinds win.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PodKind {
    /// A coding agent is working (reported over the `/activity` webhook).
    Agent,
    /// The focus timer is running.
    Timer,
    /// A calendar event is on now or starting soon.
    Calendar,
    /// Something is playing.
    Media,
}

impl PodKind {
    pub const ALL: [PodKind; 4] = [
        PodKind::Agent,
        PodKind::Timer,
        PodKind::Calendar,
        PodKind::Media,
    ];

    /// Resting width of the pod, in px. Height always matches the pill.
    pub fn width(self) -> f32 {
        match self {
            PodKind::Agent => 78.0,
            PodKind::Timer => 82.0,
            PodKind::Calendar => 70.0,
            PodKind::Media => 52.0,
        }
    }

    /// Slides that hovering the pod opens onto, best first. The first one
    /// that is in the carousel wins; with none, the notch just opens.
    pub fn opens(self) -> &'static [SlideKind] {
        match self {
            PodKind::Agent => &[SlideKind::Usage, SlideKind::Notifications],
            PodKind::Timer => &[SlideKind::Timer],
            PodKind::Calendar => &[SlideKind::Calendar],
            PodKind::Media => &[SlideKind::Media],
        }
    }

    /// The face that already says what this pod would say, if it is on the
    /// pill. An agent has no face of its own.
    fn face(self) -> Option<SlideKind> {
        match self {
            PodKind::Agent => None,
            PodKind::Timer => Some(SlideKind::Timer),
            PodKind::Calendar => Some(SlideKind::Calendar),
            PodKind::Media => Some(SlideKind::Media),
        }
    }

    fn enabled(self, cfg: &PodsConfig) -> bool {
        match self {
            PodKind::Agent => cfg.agents,
            PodKind::Timer => cfg.timer,
            PodKind::Calendar => cfg.calendar,
            PodKind::Media => cfg.media,
        }
    }
}

/// Which activities are live right now. Gathered by the window, which owns
/// the timer, media and calendar readers.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PodCandidates {
    pub agent: bool,
    pub timer: bool,
    pub calendar: bool,
    pub media: bool,
}

impl PodCandidates {
    fn has(&self, kind: PodKind) -> bool {
        match kind {
            PodKind::Agent => self.agent,
            PodKind::Timer => self.timer,
            PodKind::Calendar => self.calendar,
            PodKind::Media => self.media,
        }
    }
}

/// The pods that should show, highest priority first, at most `cfg.limit()`.
///
/// An activity whose own face is already on the pill — in the middle or in a
/// split segment — gets no pod: it would only say the same thing twice.
pub fn wanted_pods(
    cfg: &PodsConfig,
    live: PodCandidates,
    on_pill: &[Option<SlideKind>],
) -> [Option<PodKind>; 2] {
    let mut out = [None; 2];
    if !cfg.enabled {
        return out;
    }
    let limit = cfg.limit().min(out.len());
    let mut n = 0;
    for kind in PodKind::ALL {
        if n == limit {
            break;
        }
        let duplicate = kind
            .face()
            .is_some_and(|face| on_pill.contains(&Some(face)));
        if kind.enabled(cfg) && live.has(kind) && !duplicate {
            out[n] = Some(kind);
            n += 1;
        }
    }
    out
}

/// Seat `wanted` pods in the two pod slots, `[right, left]`.
///
/// A pod that is already seated keeps its side, so pods never hop across the
/// notch when another one arrives or leaves. Newcomers take the free side,
/// right first — reading order: the notch, then what is new.
pub fn assign_sides(
    seated: [Option<PodKind>; 2],
    wanted: [Option<PodKind>; 2],
) -> [Option<PodKind>; 2] {
    let mut out = [None; 2];
    for (i, kind) in seated.into_iter().enumerate() {
        if kind.is_some() && wanted.contains(&kind) {
            out[i] = kind;
        }
    }
    for kind in wanted.into_iter().flatten() {
        if out.contains(&Some(kind)) {
            continue;
        }
        if let Some(free) = out.iter_mut().find(|slot| slot.is_none()) {
            *free = Some(kind);
        }
    }
    out
}

/// Shoulder reach for a pod beside a slab whose own shoulders reach `main`.
/// A little smaller, so the pod reads as the notch's satellite, not its twin.
pub fn pod_shoulder(main: f32) -> f32 {
    main * 0.75
}

/// Horizontal span of a pod beside a slab spanning `main_left..main_right`.
///
/// At `amount` 0 the pod is tucked under the slab's edge; at 1 it sits out
/// beside it with clear space between the two sets of shoulders. Index 0 is
/// the right-hand pod, 1 the left, matching [`assign_sides`].
pub fn pod_span(
    index: usize,
    amount: f32,
    main_left: f32,
    main_right: f32,
    main_shoulder: f32,
    pod_width: f32,
    pod_shoulder: f32,
) -> (f32, f32) {
    let amount = amount.clamp(0.0, 1.0);
    let travel = POD_GAP + main_shoulder + pod_shoulder;
    if index == 0 {
        let tucked = main_right - pod_width;
        let out = main_right + travel;
        let left = tucked + (out - tucked) * amount;
        (left, left + pod_width)
    } else {
        let tucked = main_left + pod_width;
        let out = main_left - travel;
        let right = tucked + (out - tucked) * amount;
        (right - pod_width, right)
    }
}

/// How far a pod can reach out from either side of the slab, for sizing the
/// window frame once. Zero when pods are off.
pub fn max_pod_reach(cfg: &PodsConfig, shoulder: f32) -> f32 {
    if !cfg.enabled {
        return 0.0;
    }
    let widest = PodKind::ALL
        .into_iter()
        .map(PodKind::width)
        .fold(0.0, f32::max);
    // Pod body, both sets of shoulders and the gap, plus the pod's own
    // shoulder beyond its far edge.
    widest + POD_GAP + shoulder * 3.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split(left: Option<SlideKind>, right: Option<SlideKind>) -> SplitConfig {
        SplitConfig {
            enabled: true,
            left,
            right,
            open_hovered: true,
        }
    }

    fn settle<K: Copy + PartialEq>(slot: &mut Slot<K>, want: Option<K>) {
        for _ in 0..240 {
            slot.step(want, 1.0 / 60.0);
        }
    }

    #[test]
    fn split_off_wants_nothing() {
        let cfg = SplitConfig {
            enabled: false,
            ..split(Some(SlideKind::Clock), Some(SlideKind::Stats))
        };
        assert_eq!(split_targets(&cfg, Some(SlideKind::Media)), [None, None]);
        assert_eq!(max_split_extra(&cfg), 0.0);
    }

    #[test]
    fn split_never_repeats_the_middle_or_itself() {
        let cfg = split(Some(SlideKind::Clock), Some(SlideKind::Clock));
        assert_eq!(
            split_targets(&cfg, Some(SlideKind::Media)),
            [Some(SlideKind::Clock), None]
        );
        assert_eq!(split_targets(&cfg, Some(SlideKind::Clock)), [None, None]);
    }

    #[test]
    fn text_segments_fit_their_text_within_bounds() {
        assert_eq!(
            text_segment_width(200.0),
            200.0 + SEGMENT_PADDING * 2.0 + TEXT_SLACK
        );
        assert_eq!(text_segment_width(10.0), MIN_TEXT_SEGMENT);
        assert_eq!(text_segment_width(2000.0), MAX_TEXT_SEGMENT);
        // The frame reserves the widest a text segment can get.
        let cfg = split(Some(SlideKind::Marquee), Some(SlideKind::Clock));
        assert_eq!(
            max_split_extra(&cfg),
            MAX_TEXT_SEGMENT + segment_width(SlideKind::Clock)
        );
    }

    #[test]
    fn pill_parts_follow_segment_widths() {
        assert_eq!(pill_part(105.0, 100.0, 500.0, 90.0, 80.0), PillPart::Left);
        assert_eq!(pill_part(300.0, 100.0, 500.0, 90.0, 80.0), PillPart::Center);
        assert_eq!(pill_part(495.0, 100.0, 500.0, 90.0, 80.0), PillPart::Right);
        // No segments: everything is the middle.
        assert_eq!(pill_part(101.0, 100.0, 500.0, 0.0, 0.0), PillPart::Center);
    }

    #[test]
    fn slot_grows_in_and_out() {
        let mut slot = Slot::new(230.0, 30.0);
        settle(&mut slot, Some(1));
        assert_eq!(slot.shown, Some(1));
        assert!((slot.amount() - 1.0).abs() < 1e-3);
        settle(&mut slot, None);
        assert_eq!(slot.shown, None);
        assert_eq!(slot.amount(), 0.0);
    }

    #[test]
    fn slot_clears_the_old_occupant_before_the_new_one_grows() {
        let mut slot = Slot::new(230.0, 30.0);
        settle(&mut slot, Some(1));
        let mut saw_two = false;
        for _ in 0..240 {
            slot.step(Some(2), 1.0 / 60.0);
            if slot.shown == Some(1) {
                assert!(!saw_two, "old occupant came back");
            } else {
                assert_eq!(slot.shown, Some(2));
                saw_two = true;
            }
        }
        assert!(saw_two);
        assert!((slot.amount() - 1.0).abs() < 1e-3);
    }

    #[test]
    fn idle_slot_stays_idle() {
        let mut slot: Slot<u8> = Slot::new(230.0, 30.0);
        slot.step(None, 1.0 / 60.0);
        assert_eq!(slot.shown, None);
        assert_eq!(slot.presence.value, 0.0);
        assert_eq!(slot.visible(), None);
    }

    fn pods(max: u32) -> PodsConfig {
        PodsConfig {
            enabled: true,
            max,
            ..PodsConfig::default()
        }
    }

    fn everything() -> PodCandidates {
        PodCandidates {
            agent: true,
            timer: true,
            calendar: true,
            media: true,
        }
    }

    #[test]
    fn pods_off_wants_nothing() {
        let cfg = PodsConfig::default();
        assert!(!cfg.enabled, "pods are opt-in");
        assert_eq!(wanted_pods(&cfg, everything(), &[]), [None, None]);
        assert_eq!(max_pod_reach(&cfg, 16.0), 0.0);
    }

    #[test]
    fn pods_follow_priority_and_limit() {
        assert_eq!(
            wanted_pods(&pods(2), everything(), &[]),
            [Some(PodKind::Agent), Some(PodKind::Timer)]
        );
        assert_eq!(
            wanted_pods(&pods(1), everything(), &[]),
            [Some(PodKind::Agent), None]
        );
        // A hand-edited limit is clamped to what fits.
        assert_eq!(pods(9).limit(), 2);
        assert_eq!(pods(0).limit(), 1);
    }

    #[test]
    fn a_face_on_the_pill_suppresses_its_pod() {
        let live = PodCandidates {
            timer: true,
            media: true,
            ..PodCandidates::default()
        };
        assert_eq!(
            wanted_pods(&pods(2), live, &[Some(SlideKind::Media), None, None]),
            [Some(PodKind::Timer), None]
        );
    }

    #[test]
    fn disabled_sources_are_skipped() {
        let cfg = PodsConfig {
            agents: false,
            timer: false,
            ..pods(2)
        };
        assert_eq!(
            wanted_pods(&cfg, everything(), &[]),
            [Some(PodKind::Calendar), Some(PodKind::Media)]
        );
    }

    #[test]
    fn seated_pods_keep_their_side() {
        // Timer sits on the left; an agent arriving takes the free right.
        let seated = [None, Some(PodKind::Timer)];
        let wanted = [Some(PodKind::Agent), Some(PodKind::Timer)];
        assert_eq!(
            assign_sides(seated, wanted),
            [Some(PodKind::Agent), Some(PodKind::Timer)]
        );
        // The agent leaves: the timer stays put on the left.
        assert_eq!(
            assign_sides(
                [Some(PodKind::Agent), Some(PodKind::Timer)],
                [Some(PodKind::Timer), None]
            ),
            [None, Some(PodKind::Timer)]
        );
        // Newcomers go right first.
        assert_eq!(
            assign_sides([None, None], [Some(PodKind::Media), None]),
            [Some(PodKind::Media), None]
        );
    }

    #[test]
    fn pods_tuck_under_and_clear_the_slab() {
        let (l, r) = pod_span(0, 0.0, 100.0, 300.0, 16.0, 80.0, 12.0);
        assert_eq!(
            (l, r),
            (220.0, 300.0),
            "tucked right pod hides under the slab"
        );
        let (l, r) = pod_span(0, 1.0, 100.0, 300.0, 16.0, 80.0, 12.0);
        assert_eq!(l, 300.0 + POD_GAP + 16.0 + 12.0);
        assert_eq!(r - l, 80.0);
        let (l, r) = pod_span(1, 1.0, 100.0, 300.0, 16.0, 80.0, 12.0);
        assert_eq!(r, 100.0 - POD_GAP - 16.0 - 12.0);
        assert_eq!(r - l, 80.0);
    }
}
