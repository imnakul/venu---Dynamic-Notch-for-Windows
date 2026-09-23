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
            family: "Segoe UI".to_string(),
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

/// How the settings window is painted.
///
/// `System` follows whatever Windows reports for apps, so a machine set to
/// switch at sunset takes the window with it. The other two pin it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum UiTheme {
    #[default]
    System,
    Light,
    Dark,
}

impl UiTheme {
    pub const ALL: [UiTheme; 3] = [UiTheme::System, UiTheme::Light, UiTheme::Dark];

    pub fn label(self) -> &'static str {
        match self {
            UiTheme::System => "System",
            UiTheme::Light => "Light",
            UiTheme::Dark => "Dark",
        }
    }
}

/// The faces the notch can show. The order of [`NotchConfig::slides`] is the
/// carousel order the scroll wheel walks through.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
}

impl SlideKind {
    pub const ALL: [SlideKind; 7] = [
        SlideKind::Status,
        SlideKind::Clock,
        SlideKind::Marquee,
        SlideKind::Wallpaper,
        SlideKind::Media,
        SlideKind::Notifications,
        SlideKind::Usage,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SlideKind::Status => "Status",
            SlideKind::Clock => "Clock",
            SlideKind::Marquee => "Moving text",
            SlideKind::Wallpaper => "Wallpaper",
            SlideKind::Media => "Now Playing",
            SlideKind::Notifications => "Notifications",
            SlideKind::Usage => "Claude Usage",
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
    /// Smart / Auto mode: shows active alerts if unread, or Now Playing if music is on, otherwise Clock.
    Auto,
}

impl CollapsedMode {
    pub const ALL: [CollapsedMode; 9] = [
        CollapsedMode::LastActive,
        CollapsedMode::Status,
        CollapsedMode::Clock,
        CollapsedMode::Marquee,
        CollapsedMode::Wallpaper,
        CollapsedMode::Media,
        CollapsedMode::Notifications,
        CollapsedMode::Usage,
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NotificationConfig {
    pub enabled: bool,
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
            slides: SlideKind::ALL.to_vec(),
            active_slide: 0,
            default_collapsed: CollapsedMode::LastActive,
            notifications: NotificationConfig::default(),
            accent: [1.0, 0.604, 0.235, 1.0],     // ember
            surface: [0.031, 0.031, 0.043, 0.97], // obsidian
            theme: NotchTheme::Dark,
            font_family: "Plus Jakarta Sans".to_string(),
            clock_24h: false,
            collapse_delay_ms: 220,
            scroll_to_switch: true,
            always_on_top: true,
            click_through: false,
        }
    }
}

impl NotchConfig {
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

    pub fn clamped_active(&self) -> usize {
        let len = self.effective_slides().len();
        self.active_slide.min(len.saturating_sub(1))
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
                family: "Segoe UI".to_string(),
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
    /// How the settings window itself is painted. Nothing to do with the
    /// overlays — this is only the chrome around the controls.
    #[serde(default)]
    pub ui_theme: UiTheme,
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
            ui_theme: UiTheme::default(),
            launch_on_startup: false,
        }
    }
}

impl AppConfig {
    pub fn config_path() -> PathBuf {
        if let Some(mut path) = dirs::config_dir() {
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
                if let Ok(config) = serde_json::from_str::<AppConfig>(&content) {
                    return config;
                }
            }
        }

        // Backward compatibility fallback: check legacy movingtext path if venu doesn't exist yet
        if let Some(mut legacy_path) = dirs::config_dir() {
            legacy_path.push("movingtext");
            legacy_path.push("config.json");
            if legacy_path.exists() {
                if let Ok(content) = fs::read_to_string(&legacy_path) {
                    if let Ok(config) = serde_json::from_str::<AppConfig>(&content) {
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
