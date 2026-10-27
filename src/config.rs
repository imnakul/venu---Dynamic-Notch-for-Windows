use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct EdgeSelection {
    pub top: bool,
    pub right: bool,
    pub bottom: bool,
    pub left: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct PaddingConfig {
    pub top: u32,
    pub right: u32,
    pub bottom: u32,
    pub left: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FontConfig {
    pub family: String,
    pub size: f32,
    pub bold: bool,
    pub italic: bool,
}

impl Default for FontConfig {
    fn default() -> Self {
        Self {
            family: "Geist".to_string(),
            size: 20.0,
            bold: true,
            italic: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ColorConfig {
    pub text_color: [f32; 4], // RGBA 0.0 - 1.0
    pub bg_color: [f32; 4],   // RGBA 0.0 - 1.0
}

impl Default for ColorConfig {
    fn default() -> Self {
        Self {
            text_color: [1.0, 0.9, 0.2, 1.0],   // Vibrant Gold / Yellow
            bg_color: [0.08, 0.08, 0.12, 0.85], // Sleek Dark Semi-Transparent
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnimConfig {
    pub speed: f32, // pixels per second
    pub reverse: bool,
}

impl Default for AnimConfig {
    fn default() -> Self {
        Self {
            speed: 120.0,
            reverse: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Notch
// ---------------------------------------------------------------------------

/// Horizontal anchor for the notch on its monitor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NotchAlign {
    Left,
    Center,
    Right,
}

fn default_theme() -> NotchTheme {
    NotchTheme::Dark
}

fn default_true() -> bool {
    true
}

/// How the notch is finished.
///
/// This is a surface treatment, not a full re-skin: the accent stays yours and
/// the layout never changes. Only the panel and the type tones move.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NotchTheme {
    /// Near-black obsidian. The default, and the only one that truly vanishes
    /// into a laptop bezel.
    Dark,
    /// Bone-white panel with dark type, for light desktops.
    Light,
    /// Translucent frosted glass, with the screen behind it blurred through the panel.
    Frosted,
    /// Pure clear see-through glass without backdrop blur.
    Transparent,
    /// Deep soft-diffusion blur with smooth ambient background colors.
    Blurred,
    /// Windows Fluent Acrylic style with rich ambient tint and balanced diffusion.
    Acrylic,
}

impl NotchTheme {
    pub const ALL: [NotchTheme; 6] = [
        NotchTheme::Dark,
        NotchTheme::Light,
        NotchTheme::Frosted,
        NotchTheme::Transparent,
        NotchTheme::Blurred,
        NotchTheme::Acrylic,
    ];

    pub fn label(self) -> &'static str {
        match self {
            NotchTheme::Dark => "Dark",
            NotchTheme::Light => "Light",
            NotchTheme::Frosted => "Frosted",
            NotchTheme::Transparent => "Transparent",
            NotchTheme::Blurred => "Blurred",
            NotchTheme::Acrylic => "Acrylic",
        }
    }

    /// The panel colour this theme wants. Applied when the user switches
    /// themes; they are free to tint it afterwards.
    pub fn default_surface(self) -> [f32; 4] {
        match self {
            NotchTheme::Dark => [0.031, 0.031, 0.043, 0.97],
            NotchTheme::Light => [0.965, 0.965, 0.976, 0.97],
            // Low alpha on purpose: the blurred capture behind it supplies
            // most of the body, and this is only the tint on top.
            NotchTheme::Frosted => [0.07, 0.07, 0.09, 0.55],
            NotchTheme::Transparent => [0.04, 0.04, 0.06, 0.28],
            NotchTheme::Blurred => [0.06, 0.06, 0.08, 0.60],
            NotchTheme::Acrylic => [0.09, 0.09, 0.13, 0.70],
        }
    }
}

/// How the settings window itself looks and sits. Nothing to do with the
/// overlays — this is only the chrome around the controls.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SettingsWindowConfig {
    /// How frosted the glass is, 0 to 1. Low lets the blurred desktop show
    /// through; high is close to a solid panel. Windows sets the blur itself.
    pub glass: f32,
    /// Keep the window above others, like a floating palette.
    pub pinned: bool,
    /// The one saturated colour in the window: switches, tabs, the selected
    /// page, slider fills.
    pub accent: [f32; 4],
}

impl SettingsWindowConfig {
    /// Below this, light text over a bright desktop stops being legible.
    pub const MIN_GLASS: f32 = 0.15;
}

impl Default for SettingsWindowConfig {
    fn default() -> Self {
        Self {
            glass: 0.6,
            pinned: true,
            accent: [0.463, 0.580, 0.980, 1.0],
        }
    }
}

/// The faces the notch can show. The order of [`NotchConfig::slides`] is the
/// carousel order the scroll wheel walks through.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SlideKind {
    /// Split view: what today is about (left) and today's short list (right).
    Status,
    /// Oversized clock plus the date.
    Clock,
    /// The classic scrolling marquee, now living inside the notch.
    Marquee,
    /// A picture you want to be reminded of, with an optional caption.
    Wallpaper,
    /// Whatever is currently playing in the system's media session, if
    /// anything: title, artist, art, and transport controls.
    Media,
    /// Notification Center: alerts, task updates, and messages from allowed apps.
    Notifications,
    /// Claude Code usage: context window, session cost, and rate limits.
    Usage,
    /// Live Windows system resource usage and power state.
    Stats,
    /// A Pomodoro focus timer with breaks.
    Timer,
    /// What is on your calendar next, from an iCal feed.
    Calendar,
    /// One-click switches: microphone, sound, dark mode, keep awake.
    Controls,
    /// Pinned files, folders, apps and scripts, plus the newest files of one
    /// folder. Venu stores paths only, never copies.
    Shelf,
    /// Recently copied text, one click to copy it again. Opt-in, memory only.
    Clipboard,
}

impl SlideKind {
    pub const ALL: [SlideKind; 13] = [
        SlideKind::Status,
        SlideKind::Clock,
        SlideKind::Marquee,
        SlideKind::Wallpaper,
        SlideKind::Media,
        SlideKind::Notifications,
        SlideKind::Usage,
        SlideKind::Stats,
        SlideKind::Timer,
        SlideKind::Calendar,
        SlideKind::Controls,
        SlideKind::Shelf,
        SlideKind::Clipboard,
    ];

    /// The deck a fresh install starts with: everything except clipboard
    /// history, which is opt-in and would only say "turned off" until then.
    pub fn default_deck() -> Vec<SlideKind> {
        Self::ALL
            .into_iter()
            .filter(|kind| *kind != SlideKind::Clipboard)
            .collect()
    }

    pub fn label(self) -> &'static str {
        match self {
            SlideKind::Status => "Status",
            SlideKind::Clock => "Clock",
            SlideKind::Marquee => "Moving text",
            SlideKind::Wallpaper => "Wallpaper",
            SlideKind::Media => "Now Playing",
            SlideKind::Notifications => "Notifications",
            SlideKind::Usage => "Claude Usage",
            SlideKind::Stats => "Stats",
            SlideKind::Timer => "Focus Timer",
            SlideKind::Calendar => "Calendar",
            SlideKind::Controls => "Quick Toggles",
            SlideKind::Shelf => "Shelf",
            SlideKind::Clipboard => "Clipboard",
        }
    }
}

/// Default face the notch shows when collapsed and resting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum CollapsedMode {
    /// Stays on whichever slide was last viewed in the carousel.
    #[default]
    LastActive,
    /// Resets to Status slide.
    Status,
    /// Resets to Clock / Time slide.
    Clock,
    /// Resets to Moving Text marquee.
    Marquee,
    /// Resets to Wallpaper picture slide.
    Wallpaper,
    /// Resets to Now Playing media slide.
    Media,
    /// Resets to Notifications unread indicator.
    Notifications,
    /// Resets to Claude Code usage slide.
    Usage,
    /// Resets to system stats slide.
    Stats,
    /// Resets to the focus timer.
    Timer,
    /// Resets to the calendar slide.
    Calendar,
    /// Smart / Auto mode: shows active alerts if unread, or Now Playing if music is on, otherwise Clock.
    Auto,
}

impl CollapsedMode {
    pub const ALL: [CollapsedMode; 12] = [
        CollapsedMode::LastActive,
        CollapsedMode::Status,
        CollapsedMode::Clock,
        CollapsedMode::Marquee,
        CollapsedMode::Wallpaper,
        CollapsedMode::Media,
        CollapsedMode::Notifications,
        CollapsedMode::Usage,
        CollapsedMode::Stats,
        CollapsedMode::Timer,
        CollapsedMode::Calendar,
        CollapsedMode::Auto,
    ];

    pub fn label(self) -> &'static str {
        match self {
            CollapsedMode::LastActive => "Last Active",
            CollapsedMode::Status => "Status",
            CollapsedMode::Clock => "Clock",
            CollapsedMode::Marquee => "Moving text",
            CollapsedMode::Wallpaper => "Wallpaper",
            CollapsedMode::Media => "Now Playing",
            CollapsedMode::Notifications => "Notifications",
            CollapsedMode::Usage => "Claude Usage",
            CollapsedMode::Stats => "Stats",
            CollapsedMode::Timer => "Focus Timer",
            CollapsedMode::Calendar => "Calendar",
            CollapsedMode::Auto => "Dynamic / Auto",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum NotificationGlowStyle {
    #[default]
    CountdownDrain,
    RgbBorderMoving,
    WavyRgbMoving,
    NeonGlow,
}

impl NotificationGlowStyle {
    pub const ALL: [Self; 4] = [
        Self::CountdownDrain,
        Self::RgbBorderMoving,
        Self::WavyRgbMoving,
        Self::NeonGlow,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::CountdownDrain => "Countdown Drain (3-Sided Border)",
            Self::RgbBorderMoving => "Moving RGB Perimeter",
            Self::WavyRgbMoving => "Wavy Moving RGB Wave",
            Self::NeonGlow => "Neon Ambient Bloom",
        }
    }
}

/// A short animation the notch plays on its own every so often, not tied to
/// any notification. Everything but the glass shine runs along the rim.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum BorderEffect {
    #[default]
    Off,
    Rainbow,
    GlassShine,
    SweepRight,
    SweepLeft,
    SweepBounce,
    Breathe,
    Aurora,
    Comet,
    Neon,
    Heartbeat,
    Fire,
}

impl BorderEffect {
    pub const ALL: [Self; 12] = [
        Self::Off,
        Self::Rainbow,
        Self::GlassShine,
        Self::SweepRight,
        Self::SweepLeft,
        Self::SweepBounce,
        Self::Breathe,
        Self::Aurora,
        Self::Comet,
        Self::Neon,
        Self::Heartbeat,
        Self::Fire,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Rainbow => "Rainbow border",
            Self::GlassShine => "Glass reflection",
            Self::SweepRight => "Colour sweep, left to right",
            Self::SweepLeft => "Colour sweep, right to left",
            Self::SweepBounce => "Colour sweep, there and back",
            Self::Breathe => "Breathing glow",
            Self::Aurora => "Aurora",
            Self::Comet => "Comet",
            Self::Neon => "Neon sign",
            Self::Heartbeat => "Heartbeat",
            Self::Fire => "Fire",
        }
    }

    /// Whether the effect is drawn in the configured colour. Rainbow brings
    /// its own colours and the glass shine is always white.
    pub fn uses_color(self) -> bool {
        matches!(
            self,
            Self::SweepRight
                | Self::SweepLeft
                | Self::SweepBounce
                | Self::Breathe
                | Self::Comet
                | Self::Neon
                | Self::Heartbeat
        )
    }
}

/// Which state of the notch a border effect plays in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum EffectScope {
    #[default]
    Always,
    /// Only on the resting pill; it fades out as the notch opens.
    CollapsedOnly,
    /// Only on the open panel.
    ExpandedOnly,
}

impl EffectScope {
    pub const ALL: [Self; 3] = [Self::Always, Self::CollapsedOnly, Self::ExpandedOnly];

    pub fn label(self) -> &'static str {
        match self {
            Self::Always => "Always",
            Self::CollapsedOnly => "Pill only",
            Self::ExpandedOnly => "Open panel only",
        }
    }

    /// How much of the effect shows at `expand` (0 shut, 1 open).
    pub fn visibility(self, expand: f32) -> f32 {
        let open = expand.clamp(0.0, 1.0);
        match self {
            Self::Always => 1.0,
            Self::CollapsedOnly => 1.0 - open,
            Self::ExpandedOnly => open,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BorderEffectConfig {
    pub effect: BorderEffect,
    /// Seconds from the end of one run to the start of the next.
    pub interval_secs: u32,
    /// How long one run lasts.
    pub duration_secs: f32,
    pub color: [f32; 4],
    /// Loop the effect without a break instead of playing it on the
    /// interval. Drawn at a capped frame rate, so it stays cheap.
    pub continuous: bool,
    /// Whether the effect plays on the resting pill, the open panel, or both.
    pub scope: EffectScope,
    /// Bumped by the settings window to play the effect once, right now.
    /// Never saved: it only has to differ from the last value the notch saw.
    #[serde(skip)]
    pub preview: u32,
}

impl BorderEffectConfig {
    pub const MIN_INTERVAL_SECS: u32 = 5;
    pub const MAX_INTERVAL_SECS: u32 = 600;
    /// Frames a second a looping effect is drawn at. Half the overlay's full
    /// rate: the motion is slow enough that the difference does not show,
    /// and it halves what a never-ending effect costs.
    pub const CONTINUOUS_FPS: f32 = 30.0;
}

impl Default for BorderEffectConfig {
    fn default() -> Self {
        Self {
            effect: BorderEffect::Off,
            interval_secs: 30,
            duration_secs: 1.8,
            color: [0.22, 0.74, 0.97, 1.0], // #38BDF8, the logo's cyan
            continuous: false,
            scope: EffectScope::Always,
            preview: 0,
        }
    }
}

/// The focus timer's lengths, in minutes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TimerConfig {
    pub focus_mins: u32,
    pub short_break_mins: u32,
    pub long_break_mins: u32,
    /// Focus rounds before a long break instead of a short one.
    pub rounds: u32,
    /// Start the break as soon as a focus round ends.
    pub auto_start_breaks: bool,
}

impl Default for TimerConfig {
    fn default() -> Self {
        Self {
            focus_mins: 25,
            short_break_mins: 5,
            long_break_mins: 15,
            rounds: 4,
            auto_start_breaks: true,
        }
    }
}

/// Where the calendar slide reads events from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CalendarConfig {
    /// An iCal (`.ics`) address — the "secret address" Google Calendar or
    /// Outlook offer for sharing — or a path to an `.ics` file.
    pub source: String,
    /// How often the feed is fetched again.
    pub refresh_mins: u32,
}

impl Default for CalendarConfig {
    fn default() -> Self {
        Self {
            source: String::new(),
            refresh_mins: 15,
        }
    }
}

/// Toasts for the charger being plugged in or out, and for a low battery.
/// Silent on a desktop: with no battery there is nothing to report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BatteryAlertConfig {
    pub enabled: bool,
    /// Warn once when the charge falls to this, and again at half of it.
    pub low_pct: u8,
}

impl Default for BatteryAlertConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            low_pct: 20,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NotificationConfig {
    pub enabled: bool,
    /// Opt-in Windows toast history. This works only in an identity-enabled
    /// package after the user grants Windows notification access.
    pub windows_listener_enabled: bool,
    /// Allowed apps whose notifications will trigger the Dynamic Notch alert toast.
    pub allowed_apps: Vec<String>,
    /// How long the dynamic alert capsule dwells on screen before settling back (in seconds).
    pub toast_duration_secs: f32,
    /// Local HTTP webhook server port for receiving notifications from AI tools & scripts.
    pub webhook_port: u16,
    pub sound_enabled: bool,
    /// Glowing border style when notification alert appears.
    pub glow_style: NotificationGlowStyle,
    /// Custom RGBA colors mapped per application name.
    pub app_colors: std::collections::HashMap<String, [f32; 4]>,
}

impl Default for NotificationConfig {
    fn default() -> Self {
        let mut app_colors = std::collections::HashMap::new();
        app_colors.insert("Antigravity".to_string(), [0.0, 0.94, 1.0, 1.0]); // #00F0FF Electric Cyan
        app_colors.insert("Codex".to_string(), [0.06, 0.72, 0.51, 1.0]); // #10B981 Emerald
        app_colors.insert("Claude".to_string(), [0.98, 0.45, 0.09, 1.0]); // #EA580C Terracotta
        app_colors.insert("Cursor".to_string(), [0.39, 0.40, 0.95, 1.0]); // #6366F1 Indigo
        app_colors.insert("Terminal".to_string(), [0.66, 0.33, 0.97, 1.0]); // #A855F7 Purple
        app_colors.insert("VS Code".to_string(), [0.0, 0.47, 0.83, 1.0]); // #0078D4 VS Blue
        app_colors.insert("Slack".to_string(), [0.88, 0.12, 0.35, 1.0]); // #E01E5A Berry
        app_colors.insert("Discord".to_string(), [0.35, 0.40, 0.95, 1.0]); // #5865F2 Blurple

        Self {
            enabled: true,
            windows_listener_enabled: false,
            allowed_apps: vec![
                "Antigravity".to_string(),
                "Codex".to_string(),
                "Claude".to_string(),
                "Cursor".to_string(),
                "Terminal".to_string(),
                "VS Code".to_string(),
            ],
            toast_duration_secs: 4.5,
            webhook_port: 18923,
            sound_enabled: true,
            glow_style: NotificationGlowStyle::CountdownDrain,
            app_colors,
        }
    }
}

impl NotificationConfig {
    pub fn get_app_color(&self, app: &str) -> [f32; 4] {
        if let Some(c) = self.app_colors.get(app) {
            return *c;
        }
        for (k, v) in &self.app_colors {
            if k.eq_ignore_ascii_case(app) {
                return *v;
            }
        }
        match app.to_lowercase().as_str() {
            "antigravity" => [0.0, 0.94, 1.0, 1.0],
            "codex" => [0.06, 0.72, 0.51, 1.0],
            "claude" => [0.98, 0.45, 0.09, 1.0],
            "cursor" => [0.39, 0.40, 0.95, 1.0],
            "terminal" => [0.66, 0.33, 0.97, 1.0],
            "vs code" | "vscode" => [0.0, 0.47, 0.83, 1.0],
            "slack" => [0.88, 0.12, 0.35, 1.0],
            "discord" => [0.35, 0.40, 0.95, 1.0],
            _ => [0.22, 0.74, 0.97, 1.0],
        }
    }
}

/// One line on today's short list. Deliberately not a to-do system: this is
/// only meant to hold the handful of things today is actually about.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatusItem {
    pub text: String,
    pub done: bool,
}

impl StatusItem {
    pub fn new(text: &str) -> Self {
        Self {
            text: text.to_string(),
            done: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatusConfig {
    /// Small tracked label above the focus line, e.g. "TODAY".
    pub heading: String,
    /// The one thing being worked on right now. Editable inline from the notch.
    pub focus: String,
    /// Everything else today is about. Capped in the UI, not here.
    pub items: Vec<StatusItem>,
}

impl Default for StatusConfig {
    fn default() -> Self {
        Self {
            heading: "TODAY".to_string(),
            focus: "Set what you are working on".to_string(),
            items: vec![
                StatusItem::new("Ship the notch overlay"),
                StatusItem::new("Review pull requests"),
                StatusItem::new("Write the release notes"),
            ],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WallpaperConfig {
    /// Absolute path to a PNG/JPG/BMP/GIF. Empty means "show the placeholder".
    pub path: String,
    pub caption: String,

    /// Which point of the image to keep in frame when it is cropped to the
    /// panel, as a 0..1 fraction of the image. 0.5/0.5 is dead centre.
    #[serde(default = "half")]
    pub focus_x: f32,
    #[serde(default = "half")]
    pub focus_y: f32,

    /// Extra magnification on top of the cover fit. 1.0 shows as much of the
    /// image as the panel shape allows.
    #[serde(default = "one")]
    pub zoom: f32,

    /// Panel size while the wallpaper slide is showing. Zero means "use the
    /// notch's normal expanded size"; a picture usually wants more room than a
    /// line of text does, so it gets to ask for its own.
    #[serde(default)]
    pub panel_width: u32,
    #[serde(default)]
    pub panel_height: u32,
}

fn half() -> f32 {
    0.5
}

fn one() -> f32 {
    1.0
}

impl Default for WallpaperConfig {
    fn default() -> Self {
        Self {
            path: String::new(),
            caption: String::new(),
            focus_x: 0.5,
            focus_y: 0.5,
            zoom: 1.0,
            panel_width: 0,
            panel_height: 0,
        }
    }
}

impl WallpaperConfig {
    pub fn has_image(&self) -> bool {
        !self.path.trim().is_empty()
    }

    /// Clamp the framing controls into ranges the painter can rely on.
    pub fn sanitised(&self) -> (f32, f32, f32) {
        (
            self.focus_x.clamp(0.0, 1.0),
            self.focus_y.clamp(0.0, 1.0),
            self.zoom.clamp(1.0, 4.0),
        )
    }
}

/// Per-slide overrides for the moving-text slide.
///
/// A scrolling line wants a different shape than a clock does — more width,
/// less height, and its own type size. Every field is an override rather than
/// a value: `0` means "whatever the notch itself is set to", so the slide only
/// diverges where the user deliberately made it diverge, and the notch's own
/// settings keep working as the single place to change everything at once.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MarqueeConfig {
    /// Whether the line scrolls. Off, it simply sits there: the same message,
    /// held still. Motion is what makes a marquee readable at a glance from
    /// across the room, and also what makes it impossible to ignore while you
    /// are trying to work — so which one you want depends on the message.
    pub scroll: bool,
    pub collapsed_width: u32,
    pub collapsed_height: u32,
    pub panel_width: u32,
    pub panel_height: u32,
    /// Type size of the line in the open panel.
    pub font_size: f32,
    /// Type size of the line in the collapsed pill.
    pub pill_font_size: f32,
}

impl Default for MarqueeConfig {
    fn default() -> Self {
        Self {
            // Scrolling is the whole point of the slide; a static line is the
            // deliberate choice, not the starting state.
            scroll: true,
            collapsed_width: 0,
            collapsed_height: 0,
            panel_width: 0,
            panel_height: 0,
            font_size: 0.0,
            pill_font_size: 0.0,
        }
    }
}

impl MarqueeConfig {
    /// Type size for the open panel, or `fallback` when the user has not
    /// asked for one. Clamped because the value round-trips through JSON and
    /// a hand-edited zero-or-huge size would otherwise reach DirectWrite.
    pub fn panel_font(&self, fallback: f32) -> f32 {
        if self.font_size > 0.0 {
            self.font_size.clamp(8.0, 120.0)
        } else {
            fallback
        }
    }

    /// Type size for the collapsed pill, or `fallback`.
    pub fn pill_font(&self, fallback: f32) -> f32 {
        if self.pill_font_size > 0.0 {
            self.pill_font_size.clamp(6.0, 64.0)
        } else {
            fallback
        }
    }
}

/// A starting point for [`ShapeConfig`]. Picking one fills in the numbers;
/// touching any slider afterwards turns it into [`ShapePreset::Custom`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ShapePreset {
    /// The original Venu silhouette.
    #[default]
    Classic,
    /// Fully round ends with small shoulders — the phone-island look.
    Pill,
    /// Gentle corners and wide, melting shoulders.
    Soft,
    /// Tight corners and barely-there shoulders, for a crisp, technical look.
    Sharp,
    /// Hand-tuned values.
    Custom,
}

impl ShapePreset {
    /// The presets offered as one-click starting points (not `Custom`).
    pub const PICKS: [ShapePreset; 4] = [
        ShapePreset::Classic,
        ShapePreset::Pill,
        ShapePreset::Soft,
        ShapePreset::Sharp,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ShapePreset::Classic => "Classic",
            ShapePreset::Pill => "Pill",
            ShapePreset::Soft => "Soft",
            ShapePreset::Sharp => "Sharp",
            ShapePreset::Custom => "Custom",
        }
    }

    /// `(pill corners, panel corners, pill shoulders, panel shoulders)`, or
    /// `None` for `Custom`, which keeps whatever is set.
    pub fn geometry(self) -> Option<(f32, f32, f32, f32)> {
        match self {
            ShapePreset::Classic => Some((20.0, 26.0, 16.0, 24.0)),
            ShapePreset::Pill => Some((ShapeConfig::MAX_RADIUS, 34.0, 9.0, 14.0)),
            ShapePreset::Soft => Some((14.0, 20.0, 22.0, 32.0)),
            ShapePreset::Sharp => Some((6.0, 10.0, 4.0, 6.0)),
            ShapePreset::Custom => None,
        }
    }
}

/// How the notch's silhouette and finish are drawn.
///
/// Every number here feeds the same outline the painter fills, the window
/// hit-tests and the border effects trace (`notch/geom.rs`), so changing it
/// changes all three together. The defaults are the original constants, so a
/// config written before this existed looks exactly as it did.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ShapeConfig {
    pub preset: ShapePreset,
    /// Bottom-corner radius of the resting pill, in pixels. Never more than
    /// half the pill's height, so the largest value is a fully round end.
    pub corner_radius: f32,
    /// Bottom-corner radius of the open panel.
    pub corner_radius_open: f32,
    /// How far the concave shoulders reach along the bezel while the pill is
    /// fused to it. Ignored when the notch floats (Position > Down above 0).
    pub shoulder: f32,
    /// Shoulder reach when the panel is open.
    pub shoulder_open: f32,
    /// Strength of the hairline outline, 0 (none) to 2 (twice the theme's).
    pub outline: f32,
    /// Strength of the glassy highlight along the top lip, 0 to 2.
    pub sheen: f32,
    /// Strength of the drop shadow, 0 to 2.
    pub shadow: f32,
}

impl Default for ShapeConfig {
    fn default() -> Self {
        let mut shape = Self {
            preset: ShapePreset::Classic,
            corner_radius: 0.0,
            corner_radius_open: 0.0,
            shoulder: 0.0,
            shoulder_open: 0.0,
            outline: 1.0,
            sheen: 1.0,
            shadow: 1.0,
        };
        shape.apply_preset(ShapePreset::Classic);
        shape
    }
}

impl ShapeConfig {
    pub const MAX_RADIUS: f32 = 60.0;
    pub const MAX_SHOULDER: f32 = 48.0;
    pub const MAX_STRENGTH: f32 = 2.0;
    /// Smallest shoulder kept while fused. Zero would make the outline treat
    /// the top edge as a free edge and stroke a line along the screen's own
    /// edge; a single pixel reads as square and keeps the top fused.
    pub const MIN_FUSED_SHOULDER: f32 = 1.0;

    /// Load a preset's geometry. Strengths are a separate concern (the finish,
    /// not the silhouette) and are left alone.
    pub fn apply_preset(&mut self, preset: ShapePreset) {
        self.preset = preset;
        if let Some((pill, panel, shoulder, shoulder_open)) = preset.geometry() {
            self.corner_radius = pill;
            self.corner_radius_open = panel;
            self.shoulder = shoulder;
            self.shoulder_open = shoulder_open;
        }
    }

    /// Bottom-corner radius at expansion `t` (0 = pill, 1 = panel) for a pill
    /// `pill_height` tall.
    pub fn radius_at(&self, pill_height: f32, t: f32) -> f32 {
        let pill = clean(self.corner_radius, Self::MAX_RADIUS).min(pill_height.max(0.0) * 0.5);
        let panel = clean(self.corner_radius_open, Self::MAX_RADIUS);
        pill + (panel - pill) * t.clamp(0.0, 1.0)
    }

    /// Shoulder reach at expansion `t` while fused to the bezel.
    pub fn shoulder_at(&self, t: f32) -> f32 {
        let pill = clean(self.shoulder, Self::MAX_SHOULDER);
        let panel = clean(self.shoulder_open, Self::MAX_SHOULDER);
        (pill + (panel - pill) * t.clamp(0.0, 1.0)).max(Self::MIN_FUSED_SHOULDER)
    }

    pub fn outline_strength(&self) -> f32 {
        clean(self.outline, Self::MAX_STRENGTH)
    }

    pub fn sheen_strength(&self) -> f32 {
        clean(self.sheen, Self::MAX_STRENGTH)
    }

    pub fn shadow_strength(&self) -> f32 {
        clean(self.shadow, Self::MAX_STRENGTH)
    }
}

/// Clamp a hand-editable number into `0..=max`, treating NaN as 0, so a
/// mistyped `config.json` can never feed the geometry something absurd.
fn clean(value: f32, max: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, max)
    } else {
        0.0
    }
}

/// The resting pill split into up to three live segments: whatever the notch
/// is showing in the middle, plus a fixed face on either side.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SplitConfig {
    pub enabled: bool,
    /// Face shown left of the main one, if any.
    pub left: Option<SlideKind>,
    /// Face shown right of the main one, if any.
    pub right: Option<SlideKind>,
    /// Hovering a side segment opens the panel straight onto that slide
    /// (when it is in the carousel), instead of onto the main one.
    pub open_hovered: bool,
}

impl Default for SplitConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            left: Some(SlideKind::Clock),
            right: Some(SlideKind::Stats),
            open_hovered: true,
        }
    }
}

/// Satellite pods: small live-activity pills that bud off either side of the
/// notch while something is happening, and merge back into it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PodsConfig {
    pub enabled: bool,
    /// How many pods may show at once, 1 or 2.
    pub max: u32,
    /// A coding agent reported work in progress over the `/activity` webhook.
    pub agents: bool,
    /// The focus timer is running.
    pub timer: bool,
    /// A calendar event is on now or starting soon.
    pub calendar: bool,
    /// Something is playing.
    pub media: bool,
    /// How soon an upcoming event earns a pod, in minutes.
    pub calendar_lead_mins: u32,
}

impl Default for PodsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max: 2,
            agents: true,
            timer: true,
            calendar: true,
            media: true,
            calendar_lead_mins: 10,
        }
    }
}

impl PodsConfig {
    pub const MAX_PODS: u32 = 2;

    /// The configured limit, clamped to what the notch has room for.
    pub fn limit(&self) -> usize {
        self.max.clamp(1, Self::MAX_PODS) as usize
    }
}

/// What opens the notch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum OpenWith {
    /// A click opens it and it stays open until you click elsewhere.
    Click,
    /// Resting the cursor on it opens it; leaving closes it.
    Hover,
    /// Either. A click keeps it open, a hover only while you stay.
    #[default]
    Both,
}

impl OpenWith {
    pub fn hovers(self) -> bool {
        matches!(self, OpenWith::Hover | OpenWith::Both)
    }

    pub fn clicks(self) -> bool {
        matches!(self, OpenWith::Click | OpenWith::Both)
    }
}

/// Where the scroll wheel walks through the pages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ScrollPages {
    Off,
    /// Only while the panel is open.
    OpenOnly,
    /// Over the closed pill too, which changes the face it rests on.
    #[default]
    Always,
    /// Only over the closed pill.
    ClosedOnly,
}

impl ScrollPages {
    /// From the two switches Settings shows: on the closed pill, on the open
    /// panel.
    pub fn from_parts(closed: bool, open: bool) -> Self {
        match (closed, open) {
            (true, true) => ScrollPages::Always,
            (false, true) => ScrollPages::OpenOnly,
            (true, false) => ScrollPages::ClosedOnly,
            (false, false) => ScrollPages::Off,
        }
    }

    pub fn applies(self, open: bool) -> bool {
        match self {
            ScrollPages::Off => false,
            ScrollPages::OpenOnly => open,
            ScrollPages::ClosedOnly => !open,
            ScrollPages::Always => true,
        }
    }
}

/// A mouse gesture over the notch that opens or closes it. It is read by the
/// low-level hook, so it works even while clicks pass through the notch, and
/// it is only taken while the cursor is over the notch: everywhere else the
/// button does what it always did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum MouseTrigger {
    #[default]
    Off,
    MiddleClick,
    BackButton,
    ForwardButton,
    CtrlClick,
    AltClick,
    ShiftClick,
}

impl MouseTrigger {
    pub const ALL: [MouseTrigger; 7] = [
        MouseTrigger::Off,
        MouseTrigger::MiddleClick,
        MouseTrigger::BackButton,
        MouseTrigger::ForwardButton,
        MouseTrigger::CtrlClick,
        MouseTrigger::AltClick,
        MouseTrigger::ShiftClick,
    ];

    pub fn label(self) -> &'static str {
        match self {
            MouseTrigger::Off => "None",
            MouseTrigger::MiddleClick => "Middle click",
            MouseTrigger::BackButton => "Mouse back button",
            MouseTrigger::ForwardButton => "Mouse forward button",
            MouseTrigger::CtrlClick => "Ctrl + click",
            MouseTrigger::AltClick => "Alt + click",
            MouseTrigger::ShiftClick => "Shift + click",
        }
    }
}

/// A global keyboard shortcut. `key` is a key name as egui spells it ("N",
/// "Space", "F5", "Up"), which [`crate::notch::keys`] maps to a virtual key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hotkey {
    #[serde(default)]
    pub ctrl: bool,
    #[serde(default)]
    pub alt: bool,
    #[serde(default)]
    pub shift: bool,
    #[serde(default)]
    pub win: bool,
    pub key: String,
}

impl Hotkey {
    pub fn ctrl_alt(key: &str) -> Self {
        Self {
            ctrl: true,
            alt: true,
            shift: false,
            win: false,
            key: key.to_owned(),
        }
    }

    /// At least one modifier: a bare letter as a global shortcut would
    /// swallow that letter in every app.
    pub fn has_modifier(&self) -> bool {
        self.ctrl || self.alt || self.shift || self.win
    }

    /// "Ctrl + Alt + N".
    pub fn label(&self) -> String {
        let mut parts: Vec<&str> = Vec::with_capacity(5);
        if self.ctrl {
            parts.push("Ctrl");
        }
        if self.alt {
            parts.push("Alt");
        }
        if self.shift {
            parts.push("Shift");
        }
        if self.win {
            parts.push("Win");
        }
        parts.push(&self.key);
        parts.join(" + ")
    }
}

/// How the notch is opened, closed and driven from the mouse and keyboard.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct InputConfig {
    pub open_with: OpenWith,
    /// How long the cursor has to rest on the pill before hover opens it.
    /// Moving across it on the way to something else never counts.
    pub hover_delay_ms: u32,
    /// Let hover open the notch while a full-screen app, game or
    /// presentation is in front. Off, it only opens by click or shortcut.
    pub hover_in_fullscreen: bool,
    /// `None` until the user picks one; see [`NotchConfig::scroll_pages`].
    pub scroll: Option<ScrollPages>,
    pub mouse_trigger: MouseTrigger,
    /// Ways the open panel closes. At least one is always on (see
    /// [`InputConfig::clean`]), or it could never close.
    pub close_on_esc: bool,
    pub close_on_outside_click: bool,
    /// Closes by itself once the cursor has left it, after the collapse delay.
    pub close_on_leave: bool,
    /// Right-click opens the notch's menu on the closed pill and on the open
    /// panel. Read by the mouse hook, so it works with click-through on.
    pub menu_on_pill: bool,
    pub menu_on_panel: bool,
    /// Open or close the notch.
    pub hotkey_toggle: Option<Hotkey>,
    /// Turn click-through on or off.
    pub hotkey_click_through: Option<Hotkey>,
    /// Stop hover from opening the notch, or let it again.
    pub hotkey_pause_hover: Option<Hotkey>,
}

impl Default for InputConfig {
    fn default() -> Self {
        Self {
            open_with: OpenWith::Both,
            hover_delay_ms: 200,
            hover_in_fullscreen: false,
            scroll: None,
            mouse_trigger: MouseTrigger::Off,
            close_on_esc: true,
            close_on_outside_click: true,
            close_on_leave: true,
            menu_on_pill: true,
            menu_on_panel: true,
            hotkey_toggle: Some(Hotkey::ctrl_alt("N")),
            hotkey_click_through: Some(Hotkey::ctrl_alt("C")),
            hotkey_pause_hover: None,
        }
    }
}

impl InputConfig {
    pub const MAX_HOVER_DELAY_MS: u32 = 1500;

    /// Never leave the notch with no way to close: with every close option
    /// off, a click outside comes back on.
    pub fn clean(&mut self) {
        if !self.close_on_esc && !self.close_on_outside_click && !self.close_on_leave {
            self.close_on_outside_click = true;
        }
    }

    /// Put the opening, closing, menu and wheel behaviour back to the
    /// defaults, keeping the user's shortcuts. The way out of a setup that
    /// left the notch impossible to use.
    pub fn reset_behaviour(&mut self) {
        let shortcuts = (
            self.hotkey_toggle.take(),
            self.hotkey_click_through.take(),
            self.hotkey_pause_hover.take(),
        );
        *self = Self::default();
        self.hotkey_toggle = shortcuts.0;
        self.hotkey_click_through = shortcuts.1;
        self.hotkey_pause_hover = shortcuts.2;
    }

    /// The shortcuts in a fixed order, which is also their id order when
    /// they are registered with Windows.
    pub fn hotkeys(&self) -> [Option<&Hotkey>; 3] {
        [
            self.hotkey_toggle.as_ref(),
            self.hotkey_click_through.as_ref(),
            self.hotkey_pause_hover.as_ref(),
        ]
    }
}

/// The Now Playing panel.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MediaConfig {
    /// Let the equaliser bars follow the sound that is actually playing.
    /// Off, they stand still. On costs a light repaint while music plays.
    pub live_wave: bool,
    /// Panel size for this slide; 0 means the centred default.
    pub panel_width: u32,
    pub panel_height: u32,
}

impl Default for MediaConfig {
    fn default() -> Self {
        Self {
            live_wave: true,
            panel_width: 0,
            panel_height: 0,
        }
    }
}

impl MediaConfig {
    /// The centred player's own size, used while the size fields are 0.
    pub const AUTO_WIDTH: f32 = 470.0;
    pub const AUTO_HEIGHT: f32 = 300.0;
}

/// Files, folders, apps and scripts pinned to the notch. Only the paths are
/// stored; nothing is copied.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ShelfConfig {
    pub items: Vec<String>,
    /// A folder whose newest files are shown beside the pins, such as
    /// Screenshots. Empty for none.
    pub folder: String,
    /// How many of its newest files to show.
    pub folder_count: u32,
}

impl Default for ShelfConfig {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            folder: String::new(),
            folder_count: 6,
        }
    }
}

impl ShelfConfig {
    pub const MAX_ITEMS: usize = 24;
    pub const MAX_FOLDER_COUNT: u32 = 8;

    /// Pin a path, unless it is already pinned or the shelf is full.
    /// Returns whether anything changed.
    pub fn pin(&mut self, path: &str) -> bool {
        let path = path.trim();
        if path.is_empty()
            || self.items.len() >= Self::MAX_ITEMS
            || self.items.iter().any(|p| p.eq_ignore_ascii_case(path))
        {
            return false;
        }
        self.items.push(path.to_owned());
        true
    }
}

/// Clipboard history. Off by default: what people copy includes passwords.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClipboardConfig {
    pub enabled: bool,
    pub max_items: u32,
}

impl Default for ClipboardConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max_items: 15,
        }
    }
}

impl ClipboardConfig {
    pub const MAX: u32 = 30;
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NotchConfig {
    pub enabled: bool,
    pub monitor_index: usize,
    pub align: NotchAlign,
    /// Nudge from the anchor, in pixels. Positive is right.
    pub offset_x: i32,
    /// Distance from the top of the monitor. 0 keeps the notch fused to the
    /// bezel and enables the concave shoulders; anything else detaches it.
    pub offset_y: i32,
    pub collapsed_width: u32,
    pub collapsed_height: u32,
    pub expanded_width: u32,
    pub expanded_height: u32,
    pub slides: Vec<SlideKind>,
    /// The slide the notch rests on when collapsed. Persisted so the notch
    /// comes back showing whatever you last left it on.
    pub active_slide: usize,
    #[serde(default)]
    pub default_collapsed: CollapsedMode,
    #[serde(default)]
    pub notifications: NotificationConfig,
    #[serde(default)]
    pub border_effect: BorderEffectConfig,
    #[serde(default)]
    pub timer: TimerConfig,
    #[serde(default)]
    pub calendar: CalendarConfig,
    #[serde(default)]
    pub battery: BatteryAlertConfig,
    pub accent: [f32; 4],
    pub surface: [f32; 4],
    #[serde(default = "default_theme")]
    pub theme: NotchTheme,
    pub font_family: String,
    pub clock_24h: bool,
    /// Grace period after the cursor leaves before collapsing, so brushing
    /// past the edge of the panel does not slam it shut.
    pub collapse_delay_ms: u32,
    pub scroll_to_switch: bool,
    pub always_on_top: bool,
    /// When on, the notch stops taking mouse clicks: they land on whatever is
    /// underneath instead. It still opens on hover and still answers the
    /// wheel, so the deck stays usable — only clicking through to the window
    /// below changes.
    ///
    /// Toggled by pressing the left and right mouse buttons together with the
    /// cursor over the notch, because once click-through is on there is no
    /// button left to press to turn it back off.
    #[serde(default)]
    pub click_through: bool,
    /// Silhouette and finish: corner roundness, shoulders, outline, shadow.
    #[serde(default)]
    pub shape: ShapeConfig,
    /// Extra live segments either side of the resting pill.
    #[serde(default)]
    pub split: SplitConfig,
    /// Live-activity pods beside the notch.
    #[serde(default)]
    pub pods: PodsConfig,
    /// How the notch opens and closes, and what the mouse and keyboard do.
    #[serde(default)]
    pub input: InputConfig,
    /// The Now Playing panel.
    #[serde(default)]
    pub media: MediaConfig,
    /// Pinned paths and the folder peek.
    #[serde(default)]
    pub shelf: ShelfConfig,
    /// Clipboard history.
    #[serde(default)]
    pub clipboard: ClipboardConfig,
    /// Which one-time deck upgrades have run, so a page added in a release
    /// shows up for existing users once and stays removed if they remove it.
    #[serde(default)]
    pub deck_revision: u32,
    /// Every feature in the order the user arranged them, including the ones
    /// switched off, so turning one back on returns it to its place.
    /// [`NotchConfig::slides`] stays the deck itself (the ones switched on);
    /// this only remembers where the others sit. Empty in configs from
    /// before it existed, which [`NotchConfig::deck_order`] fills in.
    #[serde(default)]
    pub slide_order: Vec<SlideKind>,
}

impl Default for NotchConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            monitor_index: 0,
            align: NotchAlign::Center,
            offset_x: 0,
            offset_y: 0,
            collapsed_width: 188,
            collapsed_height: 34,
            expanded_width: 760,
            expanded_height: 208,
            slides: SlideKind::default_deck(),
            active_slide: 0,
            default_collapsed: CollapsedMode::LastActive,
            notifications: NotificationConfig::default(),
            border_effect: BorderEffectConfig::default(),
            timer: TimerConfig::default(),
            calendar: CalendarConfig::default(),
            battery: BatteryAlertConfig::default(),
            accent: [1.0, 0.604, 0.235, 1.0],     // ember
            surface: [0.031, 0.031, 0.043, 0.97], // obsidian
            theme: NotchTheme::Dark,
            font_family: "Geist".to_string(),
            clock_24h: false,
            collapse_delay_ms: 220,
            scroll_to_switch: true,
            always_on_top: true,
            click_through: false,
            shape: ShapeConfig::default(),
            split: SplitConfig::default(),
            pods: PodsConfig::default(),
            input: InputConfig::default(),
            media: MediaConfig::default(),
            shelf: ShelfConfig::default(),
            clipboard: ClipboardConfig::default(),
            deck_revision: NotchConfig::DECK_REVISION,
            slide_order: Vec::new(),
        }
    }
}

impl NotchConfig {
    /// Bump with each page that should appear once for existing users.
    pub const DECK_REVISION: u32 = 1;

    /// Add pages introduced since this config was last saved. Returns whether
    /// anything changed.
    pub fn upgrade_deck(&mut self) -> bool {
        if self.deck_revision >= Self::DECK_REVISION {
            return false;
        }
        // Revision 1: the Shelf.
        if self.deck_revision < 1 && !self.slides.contains(&SlideKind::Shelf) {
            self.slides.push(SlideKind::Shelf);
        }
        self.deck_revision = Self::DECK_REVISION;
        true
    }

    /// Every feature, in the user's order, on or off.
    ///
    /// Starts from the remembered [`NotchConfig::slide_order`], adds any kind
    /// it is missing (older configs, features added in a release), then fills
    /// the places held by switched-on features from [`NotchConfig::slides`]
    /// in its order. The deck therefore always wins for the features that are
    /// on — anything that edits it directly, like the Stats switch, still
    /// reads right here — and switched-off features keep their places.
    pub fn deck_order(&self) -> Vec<SlideKind> {
        let mut order: Vec<SlideKind> = Vec::with_capacity(SlideKind::ALL.len());
        for kind in self.slide_order.iter().chain(self.slides.iter()) {
            if !order.contains(kind) {
                order.push(*kind);
            }
        }
        for kind in SlideKind::ALL {
            if !order.contains(&kind) {
                order.push(kind);
            }
        }
        let mut on = self.slides.iter().copied();
        for slot in order.iter_mut() {
            if self.slides.contains(slot) {
                if let Some(kind) = on.next() {
                    *slot = kind;
                }
            }
        }
        order
    }

    /// Store a new arrangement: `order` holds every feature in place, and
    /// the deck becomes those for which `is_on` holds, in that order. The
    /// notch keeps resting on the same face if it is still on. Refuses an
    /// arrangement with nothing on, so the notch always has a face; returns
    /// whether it was applied.
    pub fn set_deck(&mut self, order: Vec<SlideKind>, is_on: impl Fn(SlideKind) -> bool) -> bool {
        let slides: Vec<SlideKind> = order.iter().copied().filter(|k| is_on(*k)).collect();
        if slides.is_empty() {
            return false;
        }
        let resting = self.effective_slides().get(self.clamped_active()).copied();
        if !slides.contains(&SlideKind::Stats) && self.default_collapsed == CollapsedMode::Stats {
            self.default_collapsed = CollapsedMode::LastActive;
        }
        self.active_slide = resting
            .and_then(|kind| slides.iter().position(|k| *k == kind))
            .unwrap_or(0);
        self.slides = slides;
        self.slide_order = order;
        true
    }

    /// Switch a feature on in its remembered place in the order.
    pub fn show_feature(&mut self, kind: SlideKind) {
        let on = self.slides.clone();
        self.set_deck(self.deck_order(), |k| k == kind || on.contains(&k));
    }

    /// Slides, guaranteed non-empty, so the carousel always has something to
    /// land on even if every slide was unchecked in settings.
    /// The deck as it is actually shown.
    ///
    /// Borrowed rather than cloned: this is read several times per frame — by
    /// the placement maths, the carousel blend and the painter — and handing
    /// back a fresh `Vec` each time put a heap allocation on every one of
    /// those calls, sixty times a second, for a list that almost never
    /// changes.
    pub fn effective_slides(&self) -> &[SlideKind] {
        const FALLBACK: [SlideKind; 1] = [SlideKind::Clock];
        if self.slides.is_empty() {
            &FALLBACK
        } else {
            &self.slides
        }
    }

    /// Where the scroll wheel changes pages. Configs from before the
    /// three-way setting only had an on/off switch, which keeps its meaning.
    pub fn scroll_pages(&self) -> ScrollPages {
        self.input.scroll.unwrap_or(if self.scroll_to_switch {
            ScrollPages::Always
        } else {
            ScrollPages::Off
        })
    }

    pub fn clamped_active(&self) -> usize {
        let len = self.effective_slides().len();
        self.active_slide.min(len.saturating_sub(1))
    }

    /// Add or remove only the Stats face, preserving the current face and the
    /// user's order for every other slide.
    pub fn set_stats_enabled(&mut self, enabled: bool) {
        let previous_kind = self.effective_slides().get(self.clamped_active()).copied();
        let previous_index = self.clamped_active();

        if enabled {
            if !self.slides.contains(&SlideKind::Stats) {
                self.slides.push(SlideKind::Stats);
            }
        } else {
            self.slides.retain(|slide| *slide != SlideKind::Stats);
            if self.default_collapsed == CollapsedMode::Stats {
                self.default_collapsed = CollapsedMode::LastActive;
            }
        }

        self.active_slide = previous_kind
            .filter(|kind| self.slides.contains(kind))
            .and_then(|kind| self.slides.iter().position(|slide| *slide == kind))
            .unwrap_or_else(|| {
                if self.slides.is_empty() {
                    0
                } else {
                    previous_index.min(self.slides.len() - 1)
                }
            });
    }

    /// Make Stats the resting face without disturbing the carousel position.
    pub fn set_stats_default(&mut self, enabled: bool) {
        if enabled {
            if !self.slides.contains(&SlideKind::Stats) {
                self.slides.push(SlideKind::Stats);
            }
            self.default_collapsed = CollapsedMode::Stats;
        } else if self.default_collapsed == CollapsedMode::Stats {
            self.default_collapsed = CollapsedMode::LastActive;
        }
    }
}

// ---------------------------------------------------------------------------
// FlashScreen
// ---------------------------------------------------------------------------

/// What the periodic flash puts on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum FlashContent {
    #[default]
    Text,
    Image,
    /// The image and the text share one flash, side by side or stacked.
    Both,
}

impl FlashContent {
    pub const ALL: [FlashContent; 3] =
        [FlashContent::Text, FlashContent::Image, FlashContent::Both];

    pub fn label(self) -> &'static str {
        match self {
            FlashContent::Text => "Text",
            FlashContent::Image => "Image",
            FlashContent::Both => "Text + Image",
        }
    }
}

/// How the text sits against the image when the flash carries both.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum FlashLayout {
    #[default]
    TextTop,
    TextBottom,
    TextLeft,
    TextRight,
}

impl FlashLayout {
    pub const ALL: [FlashLayout; 4] = [
        FlashLayout::TextTop,
        FlashLayout::TextBottom,
        FlashLayout::TextLeft,
        FlashLayout::TextRight,
    ];

    pub fn label(self) -> &'static str {
        match self {
            FlashLayout::TextTop => "Text on top",
            FlashLayout::TextBottom => "Text below",
            FlashLayout::TextLeft => "Text on left",
            FlashLayout::TextRight => "Text on right",
        }
    }
}

/// What sits behind the content block. Drawn only behind the message itself,
/// never across the whole screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum FlashBackground {
    #[default]
    Transparent,
    /// Blurred desktop behind the block, plus a light wash.
    Frosted,
    /// Flat white tint.
    White,
    /// Flat dark tint.
    Dark,
    /// Blurred desktop behind the block, no tint.
    Blur,
}

impl FlashBackground {
    pub const ALL: [FlashBackground; 5] = [
        FlashBackground::Transparent,
        FlashBackground::Frosted,
        FlashBackground::White,
        FlashBackground::Dark,
        FlashBackground::Blur,
    ];

    pub fn label(self) -> &'static str {
        match self {
            FlashBackground::Transparent => "Transparent",
            FlashBackground::Frosted => "Frosted",
            FlashBackground::White => "White overlay",
            FlashBackground::Dark => "Dark overlay",
            FlashBackground::Blur => "Blur",
        }
    }

    /// Frosted and Blur sample the screen behind the window, which needs the
    /// window kept out of its own capture.
    pub fn samples_desktop(self) -> bool {
        matches!(self, FlashBackground::Frosted | FlashBackground::Blur)
    }
}

/// How each flash arrives and leaves. One choice covers both halves: a slide
/// that enters from one edge exits through the other, and the zoom that
/// reveals from the centre folds back into it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum FlashAnim {
    /// Reveals with a zoom in from the centre, leaves with a fade + zoom out.
    #[default]
    ZoomCenter,
    /// Sweeps in from the left edge, settles in the centre, exits to the right.
    SlideLeftToRight,
    /// Sweeps in from the right edge, settles in the centre, exits to the left.
    SlideRightToLeft,
    /// Plain opacity fade in and out, no movement.
    Fade,
}

impl FlashAnim {
    pub const ALL: [FlashAnim; 4] = [
        FlashAnim::ZoomCenter,
        FlashAnim::SlideLeftToRight,
        FlashAnim::SlideRightToLeft,
        FlashAnim::Fade,
    ];

    pub fn label(self) -> &'static str {
        match self {
            FlashAnim::ZoomCenter => "Zoom reveal from center",
            FlashAnim::SlideLeftToRight => "Slide: left to right",
            FlashAnim::SlideRightToLeft => "Slide: right to left",
            FlashAnim::Fade => "Fade: soft in, soft out",
        }
    }
}

/// The FlashScreen: every so often, for a few seconds, a line of text, an
/// image, or both appears in the middle of the screen — then leaves on its
/// own. Everything that can hold more than one entry rotates: each flash uses
/// the next text, the next image, and the next colour in turn.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FlashConfig {
    pub enabled: bool,
    /// Seconds between the end of one flash and the start of the next.
    pub interval_secs: f32,
    /// How long the flash stays at full presence, before its exit begins.
    pub duration_secs: f32,
    pub content: FlashContent,
    /// Only meaningful when the content is Both.
    pub layout: FlashLayout,
    /// The messages, rotated one per flash.
    pub texts: Vec<String>,
    /// Image paths, rotated one per flash.
    pub images: Vec<String>,
    pub font: FontConfig,
    /// Text colours, rotated one per flash — or swept as a gradient.
    pub text_colors: Vec<[f32; 4]>,
    /// Paint the text with a gradient of every colour instead of one solid
    /// colour per flash.
    pub gradient_text: bool,
    /// A glare sweeping left to right across the text, once per flash.
    pub shine: bool,
    /// How long that one pass takes, in seconds.
    pub shine_speed_secs: f32,
    /// Image height as a fraction of the screen height.
    pub image_scale: f32,
    /// What is drawn behind the content block.
    pub bg_kind: FlashBackground,
    /// 0 (invisible) to 1 (full effect) — the strength of the background.
    pub bg_strength: f32,
    /// How far the background extends beyond the content, in pixels, on
    /// every side.
    pub bg_padding: f32,
    pub anim: FlashAnim,
    /// Length of the entry/exit animation. Short is snappy, long is smooth.
    pub anim_speed_secs: f32,
    pub monitor_index: usize,
    pub click_through: bool,
    pub always_on_top: bool,
}

impl Default for FlashConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            interval_secs: 300.0,
            duration_secs: 5.0,
            content: FlashContent::Text,
            layout: FlashLayout::TextTop,
            texts: vec!["⚡ Stay focused!".to_string()],
            images: Vec::new(),
            font: FontConfig {
                family: "Geist".to_string(),
                size: 96.0,
                bold: true,
                italic: false,
            },
            text_colors: vec![[1.0, 1.0, 1.0, 1.0]],
            gradient_text: false,
            shine: true,
            shine_speed_secs: 1.2,
            image_scale: 0.5,
            bg_kind: FlashBackground::Transparent,
            bg_strength: 0.6,
            bg_padding: 48.0,
            anim: FlashAnim::ZoomCenter,
            anim_speed_secs: 0.6,
            monitor_index: 0,
            click_through: true,
            always_on_top: true,
        }
    }
}

/// What a flash shows when the message list holds only blank lines — a flash
/// of nothing would look like the feature is broken.
pub const DEFAULT_FLASH_TEXT: &str = "⚡ Stay focused!";

impl FlashConfig {
    /// Intervals round-trip through JSON, so a hand-edited zero or negative
    /// must not reach the frame loop.
    pub fn safe_interval(&self) -> f32 {
        self.interval_secs.max(5.0)
    }

    pub fn safe_duration(&self) -> f32 {
        self.duration_secs.clamp(0.5, 600.0)
    }

    pub fn safe_anim_secs(&self) -> f32 {
        self.anim_speed_secs.clamp(0.1, 3.0)
    }

    pub fn safe_shine_secs(&self) -> f32 {
        self.shine_speed_secs.clamp(0.2, 8.0)
    }

    /// The message this turn shows. `turn` is the flash counter, so the list
    /// walks forward one entry per flash.
    pub fn text_for_turn(&self, turn: u32) -> &str {
        if self.texts.is_empty() {
            return DEFAULT_FLASH_TEXT;
        }
        let text = &self.texts[turn as usize % self.texts.len()];
        if text.trim().is_empty() {
            DEFAULT_FLASH_TEXT
        } else {
            text
        }
    }

    /// The image this turn shows. Empty when there is nothing to show.
    pub fn image_for_turn(&self, turn: u32) -> &str {
        if self.images.is_empty() {
            ""
        } else {
            self.images[turn as usize % self.images.len()].trim()
        }
    }

    /// The colour this turn's text uses, rotating through the list.
    pub fn color_for_turn(&self, turn: u32) -> [f32; 4] {
        if self.text_colors.is_empty() {
            [1.0, 1.0, 1.0, 1.0]
        } else {
            self.text_colors[turn as usize % self.text_colors.len()]
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppConfig {
    pub text: String,
    /// Master switch for the edge marquee — the strips of scrolling text
    /// around the screen border, which are a separate overlay from the notch.
    /// Off, every strip is torn down regardless of which edges are ticked, so
    /// the edge selection is preserved for when it comes back.
    #[serde(default = "default_true")]
    pub overlay_enabled: bool,
    #[serde(default = "default_phrase_spacing")]
    pub phrase_spacing: u32,
    pub edges: EdgeSelection,
    pub padding: PaddingConfig,
    pub thickness: u32,
    pub font: FontConfig,
    pub colors: ColorConfig,
    pub animation: AnimConfig,
    pub click_through: bool,
    pub always_on_top: bool,
    pub monitor_index: usize,
    #[serde(default)]
    pub notch: NotchConfig,
    #[serde(default)]
    pub status: StatusConfig,
    #[serde(default)]
    pub wallpaper: WallpaperConfig,
    #[serde(default)]
    pub marquee: MarqueeConfig,
    #[serde(default)]
    pub flash: FlashConfig,
    /// The settings window's glass, and whether it floats above others.
    #[serde(default)]
    pub settings_window: SettingsWindowConfig,
    /// Whether Venu registers itself to start when Windows signs in. The
    /// registry side of this lives in `crate::autostart`; the entry is
    /// refreshed against this flag on every launch.
    #[serde(default)]
    pub launch_on_startup: bool,
}

fn default_phrase_spacing() -> u32 {
    6
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            text: "⭐ REMINDER: Stay focused & stay hydrated! • Venu Dynamic Notch ⭐".to_string(),
            overlay_enabled: true,
            phrase_spacing: 6,
            edges: EdgeSelection::default(),
            padding: PaddingConfig::default(),
            thickness: 36,
            font: FontConfig::default(),
            colors: ColorConfig::default(),
            animation: AnimConfig::default(),
            click_through: true,
            always_on_top: true,
            monitor_index: 0,
            notch: NotchConfig::default(),
            status: StatusConfig::default(),
            wallpaper: WallpaperConfig::default(),
            marquee: MarqueeConfig::default(),
            flash: FlashConfig::default(),
            settings_window: SettingsWindowConfig::default(),
            launch_on_startup: false,
        }
    }
}

impl AppConfig {
    /// Upgrade only the old bundled/default family names. Other user-selected
    /// families stay intact, and the rest of the JSON config is untouched.
    fn migrate_font_defaults(&mut self) -> bool {
        let mut changed = false;
        if is_legacy_default_font(&self.font.family, "Segoe UI") {
            self.font.family = "Geist".to_string();
            changed = true;
        }
        if is_legacy_default_font(&self.notch.font_family, "Plus Jakarta Sans") {
            self.notch.font_family = "Geist".to_string();
            changed = true;
        }
        if is_legacy_default_font(&self.flash.font.family, "Segoe UI") {
            self.flash.font.family = "Geist".to_string();
            changed = true;
        }
        changed
    }

    pub fn config_path() -> PathBuf {
        if let Some(mut path) = app_config_directory() {
            path.push("venu");
            let _ = fs::create_dir_all(&path);
            path.push("config.json");
            path
        } else {
            PathBuf::from("config.json")
        }
    }

    pub fn load() -> Self {
        let path = Self::config_path();
        if path.exists() {
            if let Ok(content) = fs::read_to_string(&path) {
                if let Ok(mut config) = serde_json::from_str::<AppConfig>(&content) {
                    let fonts = config.migrate_font_defaults();
                    let deck = config.notch.upgrade_deck();
                    if fonts || deck {
                        config.save();
                    }
                    return config;
                }
            }
        }

        // Backward compatibility fallback: check legacy movingtext path if venu doesn't exist yet
        if let Some(mut legacy_path) = app_config_directory() {
            legacy_path.push("movingtext");
            legacy_path.push("config.json");
            if legacy_path.exists() {
                if let Ok(content) = fs::read_to_string(&legacy_path) {
                    if let Ok(mut config) = serde_json::from_str::<AppConfig>(&content) {
                        let _ = config.migrate_font_defaults();
                        let _ = config.notch.upgrade_deck();
                        config.save();
                        return config;
                    }
                }
            }
        }

        let default_cfg = Self::default();
        default_cfg.save();
        default_cfg
    }

    pub fn save(&self) {
        let path = Self::config_path();
        if let Ok(content) = serde_json::to_string_pretty(self) {
            let _ = fs::write(path, content);
        }
    }
}

fn is_legacy_default_font(family: &str, previous_default: &str) -> bool {
    let family = family.trim();
    family.is_empty() || family.eq_ignore_ascii_case(previous_default)
}

fn app_config_directory() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    let appdata_override = std::env::var_os("APPDATA").map(PathBuf::from);
    #[cfg(not(target_os = "windows"))]
    let appdata_override = None;

    app_config_directory_from(appdata_override, dirs::config_dir())
}

fn app_config_directory_from(
    appdata_override: Option<PathBuf>,
    known_folder: Option<PathBuf>,
) -> Option<PathBuf> {
    appdata_override
        .filter(|path| !path.as_os_str().is_empty())
        .or(known_folder)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appdata_override_is_used_before_the_known_folder() {
        let override_path = PathBuf::from("C:/test/AppData/Roaming");
        let known_folder = PathBuf::from("C:/user/AppData/Roaming");
        assert_eq!(
            app_config_directory_from(Some(override_path.clone()), Some(known_folder.clone())),
            Some(override_path)
        );
        assert_eq!(
            app_config_directory_from(Some(PathBuf::new()), Some(known_folder.clone())),
            Some(known_folder)
        );
    }

    #[test]
    fn notch_config_written_before_shape_split_and_pods_still_loads() {
        // Strip the new keys out of a current config, as an older Venu
        // would have written it, and read it back.
        let mut value = serde_json::to_value(AppConfig::default()).unwrap();
        let notch = value["notch"].as_object_mut().unwrap();
        for key in ["shape", "split", "pods"] {
            assert!(notch.remove(key).is_some(), "{key} is serialised");
        }
        let old: AppConfig = serde_json::from_value(value).unwrap();
        assert_eq!(old.notch.shape, ShapeConfig::default());
        assert_eq!(old.notch.split, SplitConfig::default());
        assert_eq!(old.notch.pods, PodsConfig::default());
        assert!(
            !old.notch.split.enabled && !old.notch.pods.enabled,
            "both are opt-in"
        );
    }

    #[test]
    fn default_shape_reproduces_the_original_silhouette() {
        // What `window.rs` drew before shapes were configurable.
        let legacy_radius = |h: f32, t: f32| {
            let pill = (h * 0.5).min(20.0);
            pill + (26.0 - pill) * t
        };
        let legacy_flare = |t: f32| 16.0 + (24.0 - 16.0) * t;
        let shape = ShapeConfig::default();
        for h in [18.0, 34.0, 40.0, 60.0, 120.0] {
            for t in [0.0, 0.25, 0.5, 1.0] {
                assert!(
                    (shape.radius_at(h, t) - legacy_radius(h, t)).abs() < 1e-4,
                    "h {h} t {t}"
                );
            }
        }
        for t in [0.0, 0.5, 1.0] {
            assert!((shape.shoulder_at(t) - legacy_flare(t)).abs() < 1e-4);
        }
        assert_eq!(
            (
                shape.outline_strength(),
                shape.sheen_strength(),
                shape.shadow_strength()
            ),
            (1.0, 1.0, 1.0)
        );
    }

    #[test]
    fn shape_numbers_are_clamped_and_presets_apply() {
        let mut shape = ShapeConfig {
            corner_radius: f32::NAN,
            corner_radius_open: 900.0,
            shoulder: -4.0,
            outline: f32::INFINITY,
            ..ShapeConfig::default()
        };
        assert_eq!(shape.radius_at(34.0, 0.0), 0.0);
        assert_eq!(shape.radius_at(34.0, 1.0), ShapeConfig::MAX_RADIUS);
        // Fused shoulders never collapse to zero (see MIN_FUSED_SHOULDER).
        assert_eq!(shape.shoulder_at(0.0), ShapeConfig::MIN_FUSED_SHOULDER);
        assert_eq!(shape.outline_strength(), 0.0);

        shape.apply_preset(ShapePreset::Pill);
        // A pill preset is fully round whatever the pill's height.
        assert_eq!(shape.radius_at(34.0, 0.0), 17.0);
        assert_eq!(shape.radius_at(80.0, 0.0), 40.0);
        shape.apply_preset(ShapePreset::Custom);
        assert_eq!(shape.preset, ShapePreset::Custom);
        assert_eq!(shape.radius_at(80.0, 0.0), 40.0, "Custom keeps the numbers");
    }

    #[test]
    fn new_font_defaults_use_geist() {
        let config = AppConfig::default();
        assert_eq!(config.font.family, "Geist");
        assert_eq!(config.notch.font_family, "Geist");
        assert_eq!(config.flash.font.family, "Geist");
    }

    #[test]
    fn font_migration_changes_only_legacy_defaults_and_round_trips() {
        let mut config = AppConfig::default();
        config.font.family = "Segoe UI".to_string();
        config.notch.font_family = "Plus Jakarta Sans".to_string();
        config.flash.font.family = "  ".to_string();

        assert!(config.migrate_font_defaults());
        assert_eq!(config.font.family, "Geist");
        assert_eq!(config.notch.font_family, "Geist");
        assert_eq!(config.flash.font.family, "Geist");

        let encoded = serde_json::to_string(&config).unwrap();
        let decoded: AppConfig = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, config);
        assert!(!config.clone().migrate_font_defaults());

        config.flash.font.family = "Segoe UI".to_string();
        assert!(config.migrate_font_defaults());
        assert_eq!(config.flash.font.family, "Geist");

        config.font.family = "Plus Jakarta Sans".to_string();
        config.notch.font_family = "Segoe UI".to_string();
        config.flash.font.family = "Arial".to_string();
        assert!(!config.migrate_font_defaults());
        assert_eq!(config.font.family, "Plus Jakarta Sans");
        assert_eq!(config.notch.font_family, "Segoe UI");
        assert_eq!(config.flash.font.family, "Arial");
    }

    #[test]
    fn old_notification_config_defaults_windows_listener_to_off() {
        let old_config = r#"{"enabled":true,"allowed_apps":["Codex"],"webhook_port":18923}"#;
        let decoded: NotificationConfig = serde_json::from_str(old_config).unwrap();
        assert!(!decoded.windows_listener_enabled);
        assert!(decoded.enabled);
        assert_eq!(decoded.allowed_apps, ["Codex"]);
    }

    #[test]
    fn stats_controls_preserve_other_slides_and_the_current_face() {
        let mut notch = NotchConfig::default();
        notch.slides = vec![SlideKind::Clock, SlideKind::Media, SlideKind::Usage];
        notch.active_slide = 1;
        notch.set_stats_enabled(true);
        assert_eq!(
            notch.slides,
            [
                SlideKind::Clock,
                SlideKind::Media,
                SlideKind::Usage,
                SlideKind::Stats
            ]
        );
        assert_eq!(notch.clamped_active(), 1);

        notch.set_stats_default(true);
        assert_eq!(notch.default_collapsed, CollapsedMode::Stats);
        assert_eq!(notch.clamped_active(), 1);
        notch.set_stats_enabled(false);
        assert_eq!(
            notch.slides,
            [SlideKind::Clock, SlideKind::Media, SlideKind::Usage]
        );
        assert_eq!(notch.default_collapsed, CollapsedMode::LastActive);
        assert_eq!(notch.clamped_active(), 1);
    }

    #[test]
    fn configs_from_before_the_controls_load_with_safe_defaults() {
        // A notch block as v0.5.2 wrote it: no input, media, shelf,
        // clipboard or deck revision, and the old scroll switch off.
        let mut notch = NotchConfig::default();
        notch.slides = vec![SlideKind::Clock, SlideKind::Media];
        notch.scroll_to_switch = false;
        let mut value = serde_json::to_value(&notch).unwrap();
        let object = value.as_object_mut().unwrap();
        for key in ["input", "media", "shelf", "clipboard", "deck_revision"] {
            object.remove(key);
        }
        let mut old: NotchConfig = serde_json::from_value(value).unwrap();

        assert_eq!(old.input, InputConfig::default());
        assert_eq!(old.input.open_with, OpenWith::Both);
        assert!(!old.clipboard.enabled, "clipboard history must stay opt-in");
        // The old switch keeps its meaning until a new choice is made.
        assert_eq!(old.scroll_pages(), ScrollPages::Off);
        old.input.scroll = Some(ScrollPages::OpenOnly);
        assert_eq!(old.scroll_pages(), ScrollPages::OpenOnly);

        // The Shelf appears once, and stays gone if removed afterwards.
        assert!(old.upgrade_deck());
        assert_eq!(old.slides.last(), Some(&SlideKind::Shelf));
        old.slides.retain(|s| *s != SlideKind::Shelf);
        assert!(!old.upgrade_deck());
        assert!(!old.slides.contains(&SlideKind::Shelf));
    }

    #[test]
    fn a_fresh_deck_has_the_shelf_but_not_the_clipboard() {
        let notch = NotchConfig::default();
        assert!(notch.slides.contains(&SlideKind::Shelf));
        assert!(!notch.slides.contains(&SlideKind::Clipboard));
        assert_eq!(notch.deck_revision, NotchConfig::DECK_REVISION);
    }

    #[test]
    fn the_shelf_pins_each_path_once_and_has_a_ceiling() {
        let mut shelf = ShelfConfig::default();
        assert!(shelf.pin(r"C:\Tools\build.bat"));
        assert!(!shelf.pin(r"c:\tools\BUILD.bat"), "same path, other case");
        assert!(!shelf.pin("   "));
        for i in 0..ShelfConfig::MAX_ITEMS {
            shelf.pin(&format!(r"C:\file{i}.txt"));
        }
        assert_eq!(shelf.items.len(), ShelfConfig::MAX_ITEMS);
    }

    #[test]
    fn the_notch_always_keeps_a_way_to_close() {
        let mut input = InputConfig::default();
        input.close_on_esc = false;
        input.close_on_outside_click = false;
        input.close_on_leave = false;
        input.clean();
        assert!(input.close_on_outside_click);
    }

    #[test]
    fn wheel_switches_map_to_where_scrolling_works() {
        assert!(ScrollPages::from_parts(true, false).applies(false));
        assert!(!ScrollPages::from_parts(true, false).applies(true));
        assert!(ScrollPages::from_parts(false, true).applies(true));
        assert!(!ScrollPages::from_parts(false, false).applies(true));
        assert_eq!(ScrollPages::from_parts(true, true), ScrollPages::Always);
    }

    #[test]
    fn reset_controls_restores_behaviour_but_keeps_shortcuts() {
        let mut input = InputConfig {
            open_with: OpenWith::Hover,
            close_on_leave: false,
            menu_on_pill: false,
            mouse_trigger: MouseTrigger::CtrlClick,
            hotkey_toggle: Some(Hotkey::ctrl_alt("J")),
            ..InputConfig::default()
        };
        input.reset_behaviour();
        assert_eq!(input.open_with, OpenWith::Both);
        assert!(input.close_on_leave && input.menu_on_pill);
        assert_eq!(input.mouse_trigger, MouseTrigger::Off);
        assert_eq!(input.hotkey_toggle, Some(Hotkey::ctrl_alt("J")));
    }

    #[test]
    fn deck_order_lists_every_feature_and_follows_the_deck() {
        let mut notch = NotchConfig {
            slides: vec![SlideKind::Media, SlideKind::Clock],
            ..NotchConfig::default()
        };
        let order = notch.deck_order();
        assert_eq!(order.len(), SlideKind::ALL.len());
        assert_eq!(&order[..2], &[SlideKind::Media, SlideKind::Clock]);

        // A hidden feature keeps its remembered place between two that are on.
        notch.slide_order = vec![SlideKind::Clock, SlideKind::Timer, SlideKind::Media];
        notch.slides = vec![SlideKind::Clock, SlideKind::Media];
        assert_eq!(
            &notch.deck_order()[..3],
            &[SlideKind::Clock, SlideKind::Timer, SlideKind::Media]
        );

        // The deck's own order wins for features that are on.
        notch.slides = vec![SlideKind::Media, SlideKind::Clock];
        assert_eq!(
            &notch.deck_order()[..3],
            &[SlideKind::Media, SlideKind::Timer, SlideKind::Clock]
        );
    }

    #[test]
    fn set_deck_keeps_the_resting_face_and_never_empties() {
        let mut notch = NotchConfig {
            slides: vec![SlideKind::Clock, SlideKind::Media],
            active_slide: 1,
            ..NotchConfig::default()
        };
        let order = vec![SlideKind::Media, SlideKind::Timer, SlideKind::Clock];
        assert!(notch.set_deck(order.clone(), |k| k != SlideKind::Timer));
        assert_eq!(notch.slides, vec![SlideKind::Media, SlideKind::Clock]);
        assert_eq!(notch.active_slide, 0, "still resting on Now Playing");
        assert!(!notch.set_deck(order, |_| false));
        assert_eq!(notch.slides.len(), 2, "an empty deck is refused");
    }
}
