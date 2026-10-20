use eframe::egui::{
    self, Align, Align2, Color32, Frame, Layout, Margin, Rect, RichText, Rounding, Sense, Stroke,
    Vec2,
};
use parking_lot::RwLock;
use std::sync::Arc;

use crate::config::{
    AppConfig, BorderEffect, BorderEffectConfig, FlashAnim, FlashBackground, FlashContent,
    FlashLayout, NotchAlign, NotchTheme, SlideKind, StatusItem,
};

mod color_picker;
mod filedlg;
mod font_setup;
mod glass;
mod theme;
mod wallpaper;

static SETTINGS_HWND: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);
static EGUI_CTX: parking_lot::RwLock<Option<egui::Context>> = parking_lot::RwLock::new(None);

pub fn get_egui_context() -> Option<egui::Context> {
    EGUI_CTX.read().clone()
}

unsafe extern "system" fn enum_windows_callback(
    hwnd: windows::Win32::Foundation::HWND,
    lparam: windows::Win32::Foundation::LPARAM,
) -> windows::Win32::Foundation::BOOL {
    let mut pid = 0u32;
    windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId(hwnd, Some(&mut pid));
    if pid == std::process::id() {
        let mut buffer = [0u16; 512];
        let len = windows::Win32::UI::WindowsAndMessaging::GetWindowTextW(hwnd, &mut buffer);
        if len > 0 {
            let text = String::from_utf16_lossy(&buffer[..len as usize]);
            if (text.contains("Venu") || text.contains("MovingText")) && !text.contains("Tray") {
                let out_ptr = lparam.0 as *mut isize;
                *out_ptr = hwnd.0 as isize;
                return windows::Win32::Foundation::BOOL(0);
            }
        }
    }
    windows::Win32::Foundation::BOOL(1)
}

pub unsafe fn find_settings_hwnd() -> Option<windows::Win32::Foundation::HWND> {
    let raw = SETTINGS_HWND.load(std::sync::atomic::Ordering::SeqCst);
    if raw != 0 {
        let hwnd = windows::Win32::Foundation::HWND(raw as *mut _);
        if windows::Win32::UI::WindowsAndMessaging::IsWindow(hwnd).as_bool() {
            return Some(hwnd);
        }
    }

    let title = windows::core::HSTRING::from("Venu - Settings");
    if let Ok(hwnd) = windows::Win32::UI::WindowsAndMessaging::FindWindowW(
        None,
        windows::core::PCWSTR(title.as_ptr()),
    ) {
        if !hwnd.is_invalid() && hwnd.0 != std::ptr::null_mut() {
            let mut pid = 0u32;
            windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId(hwnd, Some(&mut pid));
            if pid == std::process::id() {
                SETTINGS_HWND.store(hwnd.0 as isize, std::sync::atomic::Ordering::SeqCst);
                return Some(hwnd);
            }
        }
    }

    let mut found_hwnd_raw: isize = 0;
    let _ = windows::Win32::UI::WindowsAndMessaging::EnumWindows(
        Some(enum_windows_callback),
        windows::Win32::Foundation::LPARAM(&mut found_hwnd_raw as *mut isize as isize),
    );
    if found_hwnd_raw != 0 {
        let hwnd = windows::Win32::Foundation::HWND(found_hwnd_raw as *mut _);
        SETTINGS_HWND.store(found_hwnd_raw, std::sync::atomic::Ordering::SeqCst);
        return Some(hwnd);
    }

    None
}

pub fn setup_custom_fonts(ctx: &egui::Context) {
    font_setup::setup_custom_fonts(ctx);
}

/// The settings window's fixed size, in points. Big enough that the longest
/// tab needs only a short scroll, small enough to float beside the notch
/// without covering what is being tuned.
pub const WINDOW_SIZE: [f32; 2] = [940.0, 640.0];

const SIDEBAR_WIDTH: f32 = 224.0;

/// The label column of a [`SettingsApp::slider_line`]. Fixed, so every slider
/// on a page starts at the same x whatever its label says.
const SLIDER_LABEL_WIDTH: f32 = 72.0;

/// Room left beside a slider for its value readout.
const SLIDER_VALUE_WIDTH: f32 = 84.0;

/// Width of a slider's value box. Fixed, so a page's sliders end on the same
/// edge as its dropdowns and colour buttons.
const SLIDER_VALUE_BOX: f32 = 76.0;

/// Accents offered as one-click swatches. Anything else is one click further,
/// in the full picker beside them.
const ACCENT_PRESETS: [[f32; 4]; 8] = [
    [0.463, 0.580, 0.980, 1.0], // periwinkle, the default
    [0.220, 0.741, 0.973, 1.0], // cyan, the logo's
    [0.204, 0.827, 0.600, 1.0], // mint
    [0.639, 0.902, 0.208, 1.0], // lime
    [0.984, 0.749, 0.141, 1.0], // amber
    [0.976, 0.451, 0.086, 1.0], // orange
    [0.957, 0.247, 0.369, 1.0], // rose
    [0.659, 0.333, 0.969, 1.0], // violet
];

/// On/off switches in place of egui's checkboxes: a pill track that fills
/// with the accent, and a round knob that slides across. Named so a call site
/// reads like the checkbox it replaced.
trait ToggleExt {
    fn toggle(&mut self, on: &mut bool, label: impl Into<egui::WidgetText>) -> egui::Response;
}

impl ToggleExt for egui::Ui {
    fn toggle(&mut self, on: &mut bool, label: impl Into<egui::WidgetText>) -> egui::Response {
        let label: egui::WidgetText = label.into();
        let track = Vec2::new(32.0, 18.0);
        let galley = (!label.is_empty()).then(|| {
            label.into_galley(
                self,
                Some(egui::TextWrapMode::Extend),
                f32::INFINITY,
                egui::TextStyle::Body,
            )
        });
        let text_width = galley.as_ref().map_or(0.0, |g| g.size().x + 10.0);
        let size = Vec2::new(track.x + text_width, track.y.max(20.0));
        let (rect, mut response) = self.allocate_exact_size(size, Sense::click());
        if response.clicked() {
            *on = !*on;
            response.mark_changed();
        }
        response.widget_info(|| {
            egui::WidgetInfo::selected(egui::WidgetType::Checkbox, self.is_enabled(), *on, "")
        });

        let t = self.ctx().animate_bool_with_time(response.id, *on, 0.16);
        let hover = response.hovered() && self.is_enabled();
        let fade = if self.is_enabled() { 1.0 } else { 0.4 };
        let track_rect = Rect::from_min_size(
            egui::pos2(rect.left(), rect.center().y - track.y * 0.5),
            track,
        );
        let off = if hover {
            Color32::from_white_alpha(46)
        } else {
            Color32::from_white_alpha(32)
        };
        self.painter().rect_filled(
            track_rect,
            Rounding::same(track.y * 0.5),
            lerp_color(off, theme::accent(), t).gamma_multiply(fade),
        );
        let radius = track.y * 0.5 - 2.5;
        let x = egui::lerp(
            (track_rect.left() + radius + 2.5)..=(track_rect.right() - radius - 2.5),
            t,
        );
        self.painter().circle_filled(
            egui::pos2(x, track_rect.center().y),
            radius,
            Color32::WHITE.gamma_multiply(fade),
        );

        if let Some(galley) = galley {
            let pos = egui::pos2(
                track_rect.right() + 10.0,
                rect.center().y - galley.size().y * 0.5,
            );
            self.painter()
                .galley(pos, galley, theme::text_primary().gamma_multiply(fade));
        }
        response
    }
}

/// Rounding of the window's own corners, which Windows 11 applies; the hairline
/// frame is drawn to match.
const WINDOW_ROUNDING: f32 = 8.0;

/// The parts of the app, from the user's point of view. Every page belongs to
/// one, and the rail shows them as labelled groups.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Group {
    Notch,
    Screen,
    App,
}

impl Group {
    fn label(self) -> &'static str {
        match self {
            Group::Notch => "NOTCH",
            Group::Screen => "ON SCREEN",
            Group::App => "APP",
        }
    }
}

/// The glyph drawn beside a page in the rail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Icon {
    Notch,
    Palette,
    Layers,
    Bell,
    Monitor,
    Bolt,
    Gear,
}

/// One tab of a page: a short set of controls with a single purpose.
struct Tab {
    title: &'static str,
    /// What someone might type to find this tab, beyond its title: the names
    /// of the controls on it, and what people tend to call them.
    keywords: &'static str,
    draw: fn(&mut egui::Ui, &mut PageCtx<'_>),
}

/// One entry in the rail.
///
/// This is the scalability story: a new feature is one entry in [`PAGES`], or
/// one more tab on a page, and one function. Nothing else — not the rail, the
/// tab bar, search or the transitions — has to learn about it.
struct Page {
    group: Group,
    title: &'static str,
    /// One line under the heading saying what this page is for. Worth writing
    /// properly: it is the only chance to explain a page before the user has
    /// to guess from its name.
    blurb: &'static str,
    icon: Icon,
    tabs: &'static [Tab],
}

/// Everything a page is allowed to touch. Passed as one struct so adding a
/// shared resource later does not mean editing every page signature.
struct PageCtx<'a> {
    cfg: &'a mut AppConfig,
    /// Set by any page that edited the config, so the shell knows to save.
    changed: &'a mut bool,
    temp_text: &'a mut String,
    preview: &'a mut wallpaper::PreviewCache,
}

/// The rail, top to bottom. Order here is order on screen.
const PAGES: &[Page] = &[
    Page {
        group: Group::Notch,
        title: "General",
        blurb: "A pill fused to the top of your screen. Hover it and it springs open; scroll \
                inside to move between slides.",
        icon: Icon::Notch,
        tabs: &[
            Tab {
                title: "Behaviour",
                keywords: "show notch enable on off keep above always on top click through \
                           pass scroll wheel switch slides default collapsed face rest",
                draw: SettingsApp::tab_notch_behaviour,
            },
            Tab {
                title: "Size",
                keywords: "width height collapsed expanded pill panel dimensions",
                draw: SettingsApp::tab_notch_size,
            },
            Tab {
                title: "Position",
                keywords: "monitor display screen align left centre center right offset \
                           placement",
                draw: SettingsApp::tab_notch_position,
            },
        ],
    },
    Page {
        group: Group::Notch,
        title: "Appearance",
        blurb: "How the notch looks: its finish, colours and type, and the border effects it \
                plays on its own.",
        icon: Icon::Palette,
        tabs: &[
            Tab {
                title: "Style",
                keywords: "theme surface dark light frosted transparent blurred acrylic glass \
                           accent panel colour color font typeface 24-hour clock collapse delay",
                draw: SettingsApp::tab_notch_style,
            },
            Tab {
                title: "Border effects",
                keywords: "rainbow rgb glass reflection shine sweep breathe glow animation \
                           interval continuous loop duration preview",
                draw: SettingsApp::tab_notch_effects,
            },
        ],
    },
    Page {
        group: Group::Notch,
        title: "Slides",
        blurb: "The faces the notch carries, the order the scroll wheel walks through them, \
                and what each one shows.",
        icon: Icon::Layers,
        tabs: &[
            Tab {
                title: "Carousel",
                keywords: "slides order add remove reorder deck faces media clock usage",
                draw: SettingsApp::tab_slides_carousel,
            },
            Tab {
                title: "Status",
                keywords: "focus heading one line task list on deck todo checklist",
                draw: SettingsApp::tab_slides_status,
            },
            Tab {
                title: "Wallpaper",
                keywords: "image picture photo caption crop",
                draw: SettingsApp::tab_slides_wallpaper,
            },
            Tab {
                title: "Moving text",
                keywords: "marquee scrolling text size override",
                draw: SettingsApp::tab_slides_marquee,
            },
            Tab {
                title: "Stats",
                keywords: "cpu ram memory gpu power battery system usage monitor",
                draw: SettingsApp::page_app_stats,
            },
        ],
    },
    Page {
        group: Group::Notch,
        title: "Notifications",
        blurb: "Alerts from the apps you allow open the notch into a short-lived toast.",
        icon: Icon::Bell,
        tabs: &[
            Tab {
                title: "General",
                keywords: "enable dynamic notifications alerts toast",
                draw: SettingsApp::tab_notif_general,
            },
            Tab {
                title: "Apps",
                keywords: "allowed apps whitelist antigravity codex claude cursor terminal \
                           vs code slack discord program colours colors badge",
                draw: SettingsApp::tab_notif_apps,
            },
            Tab {
                title: "Alerts",
                keywords: "toast duration alert border animation glow drain",
                draw: SettingsApp::tab_notif_alerts,
            },
            Tab {
                title: "Integration",
                keywords: "webhook server port http post test alert clear",
                draw: SettingsApp::tab_notif_integration,
            },
        ],
    },
    Page {
        group: Group::Screen,
        title: "Edge Marquee",
        blurb: "Strips of scrolling text pinned to the edges of the screen. Separate from the \
                notch, and switched on and off separately.",
        icon: Icon::Monitor,
        tabs: &[
            Tab {
                title: "General",
                keywords: "show edge marquee enable edges top right bottom left strip \
                           thickness clearance margins padding",
                draw: SettingsApp::tab_edge_general,
            },
            Tab {
                title: "Message",
                keywords: "text message presets spacing repeat gap",
                draw: SettingsApp::tab_edge_message,
            },
            Tab {
                title: "Style",
                keywords: "font family size bold italic text colour color background",
                draw: SettingsApp::tab_edge_style,
            },
            Tab {
                title: "Motion",
                keywords: "scroll speed reverse direction always on top click through",
                draw: SettingsApp::tab_edge_motion,
            },
        ],
    },
    Page {
        group: Group::Screen,
        title: "Flash Screen",
        blurb: "Every so often, a line of text or an image appears in the middle of the \
                screen for a few seconds, then leaves on its own.",
        icon: Icon::Bolt,
        tabs: &[
            Tab {
                title: "General",
                keywords: "enable flashscreen reminders schedule interval every duration \
                           monitor always on top click through",
                draw: SettingsApp::tab_flash_general,
            },
            Tab {
                title: "Content",
                keywords: "text image quote what appears message",
                draw: SettingsApp::tab_flash_content,
            },
            Tab {
                title: "Style",
                keywords: "font type size bold italic colour color shine sweep background \
                           backdrop dim",
                draw: SettingsApp::tab_flash_style,
            },
            Tab {
                title: "Animation",
                keywords: "animation enter exit fade zoom slide speed",
                draw: SettingsApp::tab_flash_animation,
            },
        ],
    },
    Page {
        group: Group::App,
        title: "Preferences",
        blurb: "The settings window itself, starting with Windows, and where your settings \
                are kept.",
        icon: Icon::Gear,
        tabs: &[
            Tab {
                title: "Window",
                keywords: "glass blur transparency strength frosted opacity pin always on top \
                           floating",
                draw: SettingsApp::tab_prefs_window,
            },
            Tab {
                title: "Startup",
                keywords: "launch on startup sign in autostart boot",
                draw: SettingsApp::tab_prefs_startup,
            },
            Tab {
                title: "Storage",
                keywords: "settings file path config json copy",
                draw: SettingsApp::tab_prefs_storage,
            },
        ],
    },
];

/// Every tab whose title, page or keywords contain all the words of `query`,
/// as `(page, tab)` indices, in rail order.
fn search(query: &str) -> Vec<(usize, usize)> {
    let terms: Vec<String> = query.split_whitespace().map(|t| t.to_lowercase()).collect();
    if terms.is_empty() {
        return Vec::new();
    }
    let mut hits = Vec::new();
    for (p, page) in PAGES.iter().enumerate() {
        for (t, tab) in page.tabs.iter().enumerate() {
            let hay = format!("{} {} {}", page.title, tab.title, tab.keywords).to_lowercase();
            if terms.iter().all(|term| hay.contains(term.as_str())) {
                hits.push((p, t));
            }
        }
    }
    hits
}

pub struct SettingsApp {
    config: Arc<RwLock<AppConfig>>,
    temp_text: String,
    /// Index into [`PAGES`].
    active: usize,
    /// The open tab of every page, so coming back to a page lands on the tab
    /// it was left on.
    tabs: Vec<usize>,
    /// Runs 0 to 1 while content slides in. Held at 1 the rest of the time,
    /// so a settled window does no animation work at all.
    slide: f32,
    /// Where incoming content travels from, as a unit direction. A new tab
    /// comes in sideways, from the side it sits on in the tab bar; a new page
    /// rises from below. Direction is what makes the movement a small map
    /// rather than decoration.
    slide_from: Vec2,
    /// The palette in force last frame. The style is only rebuilt when this
    /// changes, which is while the glass slider moves and never otherwise.
    palette: Option<theme::Palette>,
    /// Decoded wallpaper, kept between frames so the preview does not re-read
    /// the file sixty times a second.
    preview: wallpaper::PreviewCache,
    /// Whether the window is on screen. Venu is a tray app: most of the time
    /// this is false, and a hidden window has nothing worth redrawing.
    on_screen: bool,
    /// The search box. Non-empty, the rail lists matching tabs instead.
    query: String,
    /// `None` until the window exists and the blur has been asked for; then
    /// whether Windows agreed to it.
    blurred: Option<bool>,
    logo: Option<egui::TextureHandle>,
}

/// A strong ease-out — the same shape as `cubic-bezier(0.23, 1, 0.32, 1)`.
///
/// Almost all of the distance is covered in the first third of the duration,
/// so the movement reads as a response to the click rather than as a wait.
fn ease_out(t: f32) -> f32 {
    if t >= 1.0 {
        1.0
    } else {
        1.0 - 2f32.powf(-10.0 * t)
    }
}

/// Seconds as a person would say them: "45 s", "5 min", "2 h 30 min".
///
/// The flash interval runs from 5 seconds to 4 hours, and a raw seconds read
/// out ("7200 s") makes the slider's low end unusable.
fn flash_interval_text(v: f64) -> String {
    let s = v.round() as u64;
    if s < 60 {
        format!("{s} s")
    } else if s < 3600 {
        if s % 60 == 0 {
            format!("{} min", s / 60)
        } else {
            format!("{:.1} min", s as f64 / 60.0)
        }
    } else {
        let (h, m) = (s / 3600, (s % 3600) / 60);
        if m == 0 {
            format!("{h} h")
        } else {
            format!("{h} h {m} min")
        }
    }
}

/// The logo for the rail, decoded once at a size that stays crisp at 200%.
fn load_logo(ctx: &egui::Context) -> Option<egui::TextureHandle> {
    let image = image::load_from_memory(include_bytes!("../../assets/icon.png")).ok()?;
    let image = image
        .resize_exact(64, 64, image::imageops::FilterType::Lanczos3)
        .to_rgba8();
    let pixels = egui::ColorImage::from_rgba_unmultiplied([64, 64], image.as_raw());
    Some(ctx.load_texture("venu_logo", pixels, egui::TextureOptions::LINEAR))
}

impl SettingsApp {
    /// `on_screen` is false when Venu was started into the tray, which is what
    /// the sign-in entry does.
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        config: Arc<RwLock<AppConfig>>,
        on_screen: bool,
    ) -> Self {
        *EGUI_CTX.write() = Some(cc.egui_ctx.clone());
        setup_custom_fonts(&cc.egui_ctx);

        let mut style = (*cc.egui_ctx.style()).clone();
        style.spacing.item_spacing = Vec2::new(8.0, 8.0);
        style.spacing.button_padding = Vec2::new(12.0, 6.0);
        style.spacing.slider_width = 240.0;
        style.spacing.combo_width = 180.0;
        // Numbers read in Geist Mono, so a column of values lines up digit for
        // digit and a value that changes does not jitter its neighbours.
        style.drag_value_text_style = egui::TextStyle::Monospace;
        // Nothing here is text to copy, and selectable labels would swallow
        // the drags that move the window by its title bar.
        style.interaction.selectable_labels = false;
        cc.egui_ctx.set_style(style);

        let current_text = config.read().text.clone();
        Self {
            config,
            temp_text: current_text,
            active: 0,
            tabs: vec![0; PAGES.len()],
            slide: 1.0,
            slide_from: Vec2::ZERO,
            palette: None,
            preview: wallpaper::PreviewCache::new(),
            on_screen,
            query: String::new(),
            blurred: None,
            logo: load_logo(&cc.egui_ctx),
        }
    }

    fn section_title(ui: &mut egui::Ui, title: &str) {
        ui.label(
            RichText::new(title)
                .font(egui::FontId::monospace(10.5))
                .color(theme::text_tertiary())
                .strong(),
        );
        ui.add_space(10.0);
    }

    fn divider(ui: &mut egui::Ui) {
        ui.add_space(6.0);
        let width = ui.available_width();
        let (line, _) = ui.allocate_exact_size(Vec2::new(width, 1.0), Sense::hover());
        ui.painter().hline(
            line.x_range(),
            line.center().y,
            Stroke::new(1.0, theme::divider()),
        );
        ui.add_space(16.0);
    }

    fn row_stacked(
        ui: &mut egui::Ui,
        label: &str,
        help: Option<&str>,
        add_contents: impl FnOnce(&mut egui::Ui),
    ) {
        ui.label(RichText::new(label).size(13.0).color(theme::text_primary()));
        if let Some(h) = help {
            ui.add_space(2.0);
            ui.label(RichText::new(h).size(11.0).color(theme::text_secondary()));
        }
        ui.add_space(8.0);
        ui.scope(|ui| {
            // A slider under a heading runs the full width of the column, so
            // the sliders on a page share both their edges.
            ui.spacing_mut().interact_size.x = SLIDER_VALUE_BOX;
            ui.spacing_mut().slider_width = (ui.available_width() - SLIDER_VALUE_WIDTH).max(160.0);
            add_contents(ui);
        });
        ui.add_space(18.0);
    }

    /// A round swatch for the accent chooser, ringed when it is the one in use.
    fn accent_dot(ui: &mut egui::Ui, rgba: [f32; 4], picked: bool) -> egui::Response {
        let (rect, response) = ui.allocate_exact_size(Vec2::splat(26.0), Sense::click());
        let color = Color32::from_rgb(
            (rgba[0] * 255.0).round() as u8,
            (rgba[1] * 255.0).round() as u8,
            (rgba[2] * 255.0).round() as u8,
        );
        let ring = ui
            .ctx()
            .animate_bool_with_time(response.id.with("ring"), picked, 0.16);
        let hover =
            ui.ctx()
                .animate_bool_with_time(response.id.with("hover"), response.hovered(), 0.1);
        let center = rect.center();
        // The swatch shrinks inside its ring when picked, so the ring reads as
        // a setting rather than an outline.
        ui.painter()
            .circle_filled(center, 10.0 - 2.5 * ring + hover * 1.0, color);
        if ring > 0.01 {
            ui.painter().circle_stroke(
                center,
                12.0,
                Stroke::new(1.6, Color32::WHITE.gamma_multiply(ring)),
            );
        }
        response.on_hover_cursor(egui::CursorIcon::PointingHand)
    }

    /// One labelled slider on its own line: a short label in a fixed column,
    /// then the slider filling the rest. Returns whether the value changed.
    fn slider_line(ui: &mut egui::Ui, label: &str, slider: egui::Slider<'_>) -> bool {
        ui.horizontal(|ui| {
            let (rect, _) =
                ui.allocate_exact_size(Vec2::new(SLIDER_LABEL_WIDTH, 20.0), Sense::hover());
            ui.painter().text(
                rect.left_center(),
                Align2::LEFT_CENTER,
                label,
                egui::FontId::proportional(12.0),
                theme::text_secondary(),
            );
            ui.spacing_mut().interact_size.x = SLIDER_VALUE_BOX;
            ui.spacing_mut().slider_width = (ui.available_width() - SLIDER_VALUE_WIDTH).max(120.0);
            ui.add(slider).changed()
        })
        .inner
    }

    fn row_inline(ui: &mut egui::Ui, label: &str, add_contents: impl FnOnce(&mut egui::Ui)) {
        ui.horizontal(|ui| {
            ui.label(RichText::new(label).size(13.0).color(theme::text_primary()));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                add_contents(ui);
            });
        });
        ui.add_space(12.0);
    }

    // -- chrome ----------------------------------------------------------

    /// Logo and name at the top of the rail. Also a handle: the window has no
    /// system title bar, so this and the page header are what move it.
    fn draw_brand(ui: &mut egui::Ui, logo: Option<&egui::TextureHandle>) {
        let (rect, response) = ui.allocate_exact_size(
            Vec2::new(ui.available_width(), 36.0),
            Sense::click_and_drag(),
        );
        if response.drag_started() {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }

        let mark = Rect::from_min_size(
            rect.left_center() - Vec2::new(-4.0, 13.0),
            Vec2::splat(26.0),
        );
        if let Some(logo) = logo {
            ui.painter().image(
                logo.id(),
                mark,
                Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        }
        let text_x = mark.right() + 10.0;
        ui.painter().text(
            egui::pos2(text_x, rect.center().y - 7.0),
            Align2::LEFT_CENTER,
            "Venu",
            egui::FontId::proportional(14.5),
            theme::text_primary(),
        );
        ui.painter().text(
            egui::pos2(text_x, rect.center().y + 9.0),
            Align2::LEFT_CENTER,
            "Settings",
            egui::FontId::proportional(11.0),
            theme::text_tertiary(),
        );
    }

    /// The search box. Typing filters the rail down to the tabs that match.
    fn draw_search(ui: &mut egui::Ui, query: &mut String, id: egui::Id) {
        let height = 32.0;
        let (rect, _) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::hover());
        let focused = ui.memory(|m| m.has_focus(id));
        let ring = ui
            .ctx()
            .animate_bool_with_time(id.with("ring"), focused, 0.14);

        ui.painter().rect(
            rect,
            Rounding::same(8.0),
            Color32::from_black_alpha(45),
            Stroke::new(
                1.0,
                lerp_color(
                    Color32::from_white_alpha(12),
                    theme::accent().gamma_multiply(0.8),
                    ring,
                ),
            ),
        );
        paint_glyph(
            ui.painter(),
            Glyph::Search,
            rect.left_center() + Vec2::new(14.0, 0.0),
            theme::text_tertiary(),
        );

        let field = Rect::from_min_max(
            rect.left_top() + Vec2::new(28.0, 0.0),
            rect.right_bottom() - Vec2::new(if query.is_empty() { 8.0 } else { 28.0 }, 0.0),
        );
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(field)
                .layout(Layout::left_to_right(Align::Center)),
        );
        child.add(
            egui::TextEdit::singleline(query)
                .id(id)
                .hint_text(RichText::new("Search settings").color(theme::text_tertiary()))
                .frame(false)
                .desired_width(field.width())
                .font(egui::FontId::proportional(12.5))
                .margin(Margin::symmetric(2.0, 0.0)),
        );

        if !query.is_empty() {
            let clear = Rect::from_center_size(
                rect.right_center() - Vec2::new(15.0, 0.0),
                Vec2::splat(20.0),
            );
            let response = ui.interact(clear, id.with("clear"), Sense::click());
            let tone = if response.hovered() {
                theme::text_primary()
            } else {
                theme::text_tertiary()
            };
            paint_glyph(ui.painter(), Glyph::Close, clear.center(), tone);
            if response.clicked() {
                query.clear();
            }
        }
    }

    /// One row of the rail: icon, label, hover wash and press feedback.
    /// Returns the row's rect and whether it was clicked. The selection pill
    /// is not drawn here; it glides between rows and is painted by the caller.
    fn rail_row(
        ui: &mut egui::Ui,
        icon: Option<Icon>,
        label: &str,
        detail: Option<&str>,
        selected: bool,
    ) -> (Rect, bool) {
        let width = ui.available_width();
        let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 32.0), Sense::click());

        if !selected {
            // A wash that fades rather than snaps. Nobody will notice it;
            // they would notice its absence as the rail feeling brittle.
            let hover = ui.ctx().animate_bool_with_time(
                response.id.with("hover"),
                response.hovered(),
                0.11,
            );
            if hover > 0.001 {
                ui.painter().rect_filled(
                    rect,
                    Rounding::same(8.0),
                    theme::surface().gamma_multiply(hover),
                );
            }
        }

        // Press feedback. A row that does not move under the finger feels
        // like it did not hear the click.
        let sink = if response.is_pointer_button_down_on() {
            1.0
        } else {
            0.0
        };
        let tone = if selected {
            theme::text_primary()
        } else {
            theme::text_secondary()
        };

        let mut x = rect.left() + 12.0 + sink;
        if let Some(icon) = icon {
            let glyph_tone = if selected { theme::accent() } else { tone };
            paint_icon(
                ui.painter(),
                icon,
                egui::pos2(x + 2.0, rect.center().y),
                glyph_tone,
            );
            x += 20.0;
        }
        let galley =
            ui.painter()
                .layout_no_wrap(label.to_owned(), egui::FontId::proportional(13.0), tone);
        let label_width = galley.size().x;
        ui.painter().galley(
            egui::pos2(x, rect.center().y - galley.size().y * 0.5),
            galley,
            tone,
        );
        if let Some(detail) = detail {
            ui.painter().text(
                egui::pos2(x + label_width + 8.0, rect.center().y),
                Align2::LEFT_CENTER,
                detail,
                egui::FontId::proportional(11.0),
                theme::text_tertiary(),
            );
        }

        ui.add_space(1.0);
        (rect, response.clicked())
    }

    /// Paint the rail's selection pill at `target`, gliding from wherever it
    /// was. Only the vertical position moves: rows are all one size.
    fn paint_rail_pill(ui: &egui::Ui, pill: egui::layers::ShapeIdx, target: Rect, key: &str) {
        let y = ui
            .ctx()
            .animate_value_with_time(egui::Id::new(key), target.top(), 0.2);
        let moved = Rect::from_min_size(egui::pos2(target.left(), y), target.size());
        ui.painter().set(
            pill,
            egui::Shape::Vec(vec![
                egui::Shape::rect_filled(moved, Rounding::same(8.0), theme::accent_wash()),
                egui::Shape::rect_filled(
                    Rect::from_min_size(
                        moved.left_top() + Vec2::new(0.0, 8.0),
                        Vec2::new(2.5, moved.height() - 16.0),
                    ),
                    Rounding::same(2.0),
                    theme::accent(),
                ),
            ]),
        );
    }

    /// The navigation rail: pages under their group headings.
    fn draw_nav(ui: &mut egui::Ui, active: usize) -> Option<usize> {
        let pill = ui.painter().add(egui::Shape::Noop);
        let mut target = None;
        let mut picked = None;

        let mut group: Option<Group> = None;
        for (index, page) in PAGES.iter().enumerate() {
            if group != Some(page.group) {
                ui.add_space(if group.is_some() { 14.0 } else { 0.0 });
                ui.horizontal(|ui| {
                    ui.add_space(12.0);
                    ui.label(
                        RichText::new(page.group.label())
                            .font(egui::FontId::monospace(9.5))
                            .color(theme::text_tertiary())
                            .strong(),
                    );
                });
                ui.add_space(4.0);
                group = Some(page.group);
            }

            let selected = active == index;
            let (rect, clicked) = Self::rail_row(ui, Some(page.icon), page.title, None, selected);
            if selected {
                target = Some(rect);
            }
            if clicked {
                picked = Some(index);
            }
        }

        if let Some(target) = target {
            Self::paint_rail_pill(ui, pill, target, "nav_pill");
        }
        picked
    }

    /// The rail while searching: one row per matching tab.
    fn draw_results(ui: &mut egui::Ui, query: &str) -> Option<(usize, usize)> {
        let hits = search(query);
        ui.horizontal(|ui| {
            ui.add_space(12.0);
            ui.label(
                RichText::new(match hits.len() {
                    0 => "NO RESULTS".to_owned(),
                    1 => "1 RESULT".to_owned(),
                    n => format!("{n} RESULTS"),
                })
                .size(9.5)
                .color(theme::text_tertiary())
                .strong(),
            );
        });
        ui.add_space(4.0);

        if hits.is_empty() {
            ui.horizontal_wrapped(|ui| {
                ui.add_space(12.0);
                ui.label(
                    RichText::new("Try a setting's name, like \"blur\", \"font\" or \"monitor\".")
                        .size(11.0)
                        .color(theme::text_secondary()),
                );
            });
            return None;
        }

        let mut picked = None;
        for (p, t) in hits {
            let page = &PAGES[p];
            let (_, clicked) = Self::rail_row(
                ui,
                Some(page.icon),
                page.tabs[t].title,
                Some(page.title),
                false,
            );
            if clicked {
                picked = Some((p, t));
            }
        }
        picked
    }

    /// The tab bar: a segmented control whose highlight glides to the picked
    /// tab, changing width on the way. Returns the tab clicked, if any.
    fn draw_tabs(ui: &mut egui::Ui, page: usize, tabs: &[Tab], active: usize) -> Option<usize> {
        let mut picked = None;
        Frame::none()
            .fill(Color32::from_black_alpha(45))
            .stroke(Stroke::new(1.0, Color32::from_white_alpha(10)))
            .rounding(Rounding::same(10.0))
            .inner_margin(Margin::same(3.0))
            .show(ui, |ui| {
                let pill = ui.painter().add(egui::Shape::Noop);
                let mut target = None;
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    for (index, tab) in tabs.iter().enumerate() {
                        let selected = index == active;
                        let galley = ui.painter().layout_no_wrap(
                            tab.title.to_owned(),
                            egui::FontId::proportional(12.5),
                            theme::text_primary(),
                        );
                        let size = Vec2::new(galley.size().x + 28.0, 28.0);
                        let (rect, response) = ui.allocate_exact_size(size, Sense::click());
                        if selected {
                            target = Some(rect);
                        } else {
                            let hover = ui.ctx().animate_bool_with_time(
                                response.id.with("hover"),
                                response.hovered(),
                                0.11,
                            );
                            if hover > 0.001 {
                                ui.painter().rect_filled(
                                    rect,
                                    Rounding::same(7.0),
                                    theme::surface().gamma_multiply(hover),
                                );
                            }
                        }
                        let tone = ui.ctx().animate_bool_with_time(
                            response.id.with("lit"),
                            selected,
                            0.16,
                        );
                        let color = lerp_color(theme::text_secondary(), theme::on_accent(), tone);
                        ui.painter().text(
                            rect.center(),
                            Align2::CENTER_CENTER,
                            tab.title,
                            egui::FontId::proportional(12.5),
                            color,
                        );
                        if response.clicked() && !selected {
                            picked = Some(index);
                        }
                    }
                });

                if let Some(target) = target {
                    // Position and width both travel, so the pill stretches
                    // from one label to the next rather than jumping. Keyed
                    // by page, so arriving on a page does not slide in from
                    // where another page's pill was.
                    let id = egui::Id::new(("tab_pill", page));
                    let x = ui
                        .ctx()
                        .animate_value_with_time(id.with("x"), target.left(), 0.22);
                    let w = ui
                        .ctx()
                        .animate_value_with_time(id.with("w"), target.width(), 0.22);
                    let moved = Rect::from_min_size(
                        egui::pos2(x, target.top()),
                        Vec2::new(w, target.height()),
                    );
                    ui.painter().set(
                        pill,
                        egui::Shape::rect_filled(moved, Rounding::same(7.0), theme::accent()),
                    );
                }
            });
        picked
    }

    /// The page heading, which doubles as the window's title bar: it moves
    /// the window and carries the pin, minimise and close buttons.
    fn draw_title_bar(ui: &mut egui::Ui, page: &Page, pinned: &mut bool) -> TitleAction {
        let (bar, _) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), 46.0), Sense::hover());
        let drag = ui.interact(bar, egui::Id::new("title_drag"), Sense::click_and_drag());
        if drag.drag_started() {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }

        ui.painter().text(
            bar.left_center() + Vec2::new(0.0, 4.0),
            Align2::LEFT_CENTER,
            page.title,
            egui::FontId::proportional(21.0),
            theme::text_primary(),
        );

        let mut action = TitleAction::None;
        let controls = [
            (Glyph::Close, "Hide to tray"),
            (Glyph::Minimize, "Minimise"),
            (
                Glyph::Pin,
                if *pinned {
                    "Floating above other windows — click to unpin"
                } else {
                    "Keep this window above others"
                },
            ),
        ];
        let mut right = bar.right() + 6.0;
        for (glyph, tip) in controls {
            let rect = Rect::from_min_size(
                egui::pos2(right - 34.0, bar.center().y - 14.0),
                Vec2::new(34.0, 28.0),
            );
            right -= 36.0;
            let response = ui
                .interact(rect, egui::Id::new(("title_btn", glyph)), Sense::click())
                .on_hover_text(tip);
            let hover =
                ui.ctx()
                    .animate_bool_with_time(response.id.with("hover"), response.hovered(), 0.1);
            let wash = if glyph == Glyph::Close {
                Color32::from_rgb(196, 43, 28)
            } else {
                theme::surface_active()
            };
            if hover > 0.001 {
                ui.painter()
                    .rect_filled(rect, Rounding::same(7.0), wash.gamma_multiply(hover));
            }
            let lit = glyph == Glyph::Pin && *pinned;
            let tone = if lit {
                theme::accent()
            } else if hover > 0.5 {
                theme::text_primary()
            } else {
                theme::text_secondary()
            };
            paint_glyph(ui.painter(), glyph, rect.center(), tone);

            if response.clicked() {
                action = match glyph {
                    Glyph::Close => TitleAction::Hide,
                    Glyph::Minimize => TitleAction::Minimize,
                    _ => {
                        *pinned = !*pinned;
                        TitleAction::Pin
                    }
                };
            }
        }
        action
    }

    // -- tabs --------------------------------------------------------------
    //
    // Each one is a thin arrangement of the sections below it. Keeping the
    // sections separate from the tabs is what lets a section move to another
    // tab without touching its body.

    fn tab_notch_behaviour(ui: &mut egui::Ui, cx: &mut PageCtx<'_>) {
        Self::section_title(ui, "BEHAVIOUR");
        Self::sec_notch_basics(ui, cx.cfg, cx.changed);
    }

    fn tab_notch_size(ui: &mut egui::Ui, cx: &mut PageCtx<'_>) {
        Self::sec_notch_size(ui, cx.cfg, cx.changed);
    }

    fn tab_notch_position(ui: &mut egui::Ui, cx: &mut PageCtx<'_>) {
        Self::sec_notch_place(ui, cx.cfg, cx.changed);
    }

    fn tab_notch_style(ui: &mut egui::Ui, cx: &mut PageCtx<'_>) {
        Self::sec_notch_look(ui, cx.cfg, cx.changed);
    }

    fn tab_notch_effects(ui: &mut egui::Ui, cx: &mut PageCtx<'_>) {
        Self::sec_notch_effects(ui, cx.cfg, cx.changed);
    }

    fn tab_slides_carousel(ui: &mut egui::Ui, cx: &mut PageCtx<'_>) {
        Self::section_title(ui, "IN THE CAROUSEL");
        Self::sec_notch_deck(ui, cx.cfg, cx.changed);
    }

    fn tab_slides_status(ui: &mut egui::Ui, cx: &mut PageCtx<'_>) {
        Self::sec_status_now(ui, cx.cfg, cx.changed);
        Self::sec_status_deck(ui, cx.cfg, cx.changed);
    }

    fn tab_slides_wallpaper(ui: &mut egui::Ui, cx: &mut PageCtx<'_>) {
        Self::sec_wallpaper(ui, cx.preview, cx.cfg, cx.changed);
    }

    fn tab_slides_marquee(ui: &mut egui::Ui, cx: &mut PageCtx<'_>) {
        Self::sec_notch_marquee(ui, cx.cfg, cx.changed);
    }

    fn tab_notif_general(ui: &mut egui::Ui, cx: &mut PageCtx<'_>) {
        Self::sec_notif_general(ui, cx.cfg, cx.changed);
    }

    fn tab_notif_apps(ui: &mut egui::Ui, cx: &mut PageCtx<'_>) {
        Self::sec_notif_apps(ui, cx.cfg, cx.changed);
        Self::divider(ui);
        Self::sec_notif_colors(ui, cx.cfg, cx.changed);
    }

    fn tab_notif_alerts(ui: &mut egui::Ui, cx: &mut PageCtx<'_>) {
        Self::sec_notif_alerts(ui, cx.cfg, cx.changed);
    }

    fn tab_notif_integration(ui: &mut egui::Ui, cx: &mut PageCtx<'_>) {
        Self::sec_notif_integration(ui, cx.cfg, cx.changed);
    }

    fn tab_edge_general(ui: &mut egui::Ui, cx: &mut PageCtx<'_>) {
        Self::sec_edge_master(ui, cx.cfg, cx.changed);
        Self::divider(ui);
        Self::sec_edge_edges(ui, cx.cfg, cx.changed);
        Self::divider(ui);
        Self::sec_edge_margins(ui, cx.cfg, cx.changed);
    }

    fn tab_edge_message(ui: &mut egui::Ui, cx: &mut PageCtx<'_>) {
        Self::sec_marquee_message(ui, cx.temp_text, cx.cfg, cx.changed);
    }

    fn tab_edge_style(ui: &mut egui::Ui, cx: &mut PageCtx<'_>) {
        Self::sec_edge_type(ui, cx.cfg, cx.changed);
        Self::divider(ui);
        Self::sec_edge_color(ui, cx.cfg, cx.changed);
    }

    fn tab_edge_motion(ui: &mut egui::Ui, cx: &mut PageCtx<'_>) {
        Self::sec_edge_motion(ui, cx.cfg, cx.changed);
        Self::divider(ui);
        Self::sec_edge_window(ui, cx.cfg, cx.changed);
    }

    fn tab_flash_general(ui: &mut egui::Ui, cx: &mut PageCtx<'_>) {
        Self::sec_flash_master(ui, cx.cfg, cx.changed);
        Self::divider(ui);
        Self::sec_flash_schedule(ui, cx.cfg, cx.changed);
        Self::divider(ui);
        Self::sec_flash_behavior(ui, cx.cfg, cx.changed);
    }

    fn tab_flash_content(ui: &mut egui::Ui, cx: &mut PageCtx<'_>) {
        Self::sec_flash_content(ui, cx.cfg, cx.changed);
    }

    fn tab_flash_style(ui: &mut egui::Ui, cx: &mut PageCtx<'_>) {
        // An image brings its own look; there is no type to set.
        if cx.cfg.flash.content != FlashContent::Image {
            Self::sec_flash_type(ui, cx.cfg, cx.changed);
            Self::divider(ui);
        }
        Self::sec_flash_backdrop(ui, cx.cfg, cx.changed);
    }

    fn tab_flash_animation(ui: &mut egui::Ui, cx: &mut PageCtx<'_>) {
        Self::sec_flash_anim(ui, cx.cfg, cx.changed);
    }

    fn page_app_stats(ui: &mut egui::Ui, cx: &mut PageCtx<'_>) {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_secs(1));

        let stats = crate::stats::snapshot();
        let metrics = [
            ("CPU", stats.cpu_pct, "All processors"),
            ("RAM", stats.ram_pct, "Physical memory"),
            ("GPU", stats.gpu_pct, "Busiest engine across the PC"),
        ];

        Frame::none()
            .fill(theme::surface())
            .stroke(Stroke::new(1.0_f32, theme::divider()))
            .rounding(Rounding::same(14.0))
            .inner_margin(Margin::same(20.0))
            .show(ui, |ui| {
                ui.set_min_height(206.0);
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new("YOUR PC")
                                .size(10.0)
                                .strong()
                                .color(theme::text_tertiary()),
                        );
                        ui.label(
                            RichText::new("Whole-system overview")
                                .size(15.0)
                                .strong()
                                .color(theme::text_primary()),
                        );
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(
                            RichText::new("UPDATES EVERY SECOND")
                                .size(9.0)
                                .strong()
                                .color(theme::text_tertiary()),
                        );
                        ui.add_space(7.0);
                        let (dot_rect, _) =
                            ui.allocate_exact_size(Vec2::splat(7.0), Sense::hover());
                        ui.painter()
                            .circle_filled(dot_rect.center(), 3.5, theme::accent());
                    });
                });

                ui.add_space(16.0);
                ui.columns(3, |columns| {
                    for (column, (label, value, hint)) in columns.iter_mut().zip(metrics) {
                        column.label(
                            RichText::new(label)
                                .size(10.0)
                                .strong()
                                .color(theme::text_tertiary()),
                        );
                        let sample = value;
                        let rendered_value = sample
                            .map(|pct| format!("{pct:.0}%"))
                            .unwrap_or_else(|| "—".to_string());
                        column.label(
                            RichText::new(rendered_value)
                                .font(egui::FontId::new(29.0, egui::FontFamily::Monospace))
                                .strong()
                                .color(theme::text_primary()),
                        );
                        column.add_space(2.0);
                        column.label(
                            RichText::new(hint)
                                .size(10.5)
                                .color(theme::text_secondary()),
                        );
                        if let Some(pct) = sample {
                            column.add_space(8.0);
                            column.add(
                                egui::ProgressBar::new(pct / 100.0)
                                    .desired_width(column.available_width()),
                            );
                        } else {
                            column.add_space(11.0);
                            column.label(
                                RichText::new("Waiting for sample")
                                    .size(9.5)
                                    .color(theme::text_tertiary()),
                            );
                        }
                    }
                });

                ui.add_space(13.0);
                ui.separator();
                ui.add_space(5.0);
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        RichText::new("MEMORY")
                            .size(9.0)
                            .strong()
                            .color(theme::text_tertiary()),
                    );
                    let memory = if stats.ram_total > 0 {
                        format!(
                            "{} of {}",
                            crate::stats::format_bytes(stats.ram_used),
                            crate::stats::format_bytes(stats.ram_total)
                        )
                    } else {
                        "—".to_string()
                    };
                    ui.label(
                        RichText::new(memory)
                            .font(egui::FontId::new(11.0, egui::FontFamily::Monospace))
                            .color(theme::text_primary()),
                    );
                    ui.add_space(18.0);
                    ui.label(
                        RichText::new("POWER")
                            .size(9.0)
                            .strong()
                            .color(theme::text_tertiary()),
                    );
                    ui.label(
                        RichText::new(stats.power_value())
                            .font(egui::FontId::new(11.0, egui::FontFamily::Monospace))
                            .color(theme::text_primary()),
                    );
                    ui.label(
                        RichText::new(format!("· {}", stats.power_hint()))
                            .size(10.0)
                            .color(theme::text_secondary()),
                    );
                });
            });

        ui.add_space(20.0);
        Self::section_title(ui, "SHOW IN THE NOTCH");
        let mut include_stats = cx.cfg.notch.slides.contains(&SlideKind::Stats);
        if ui
            .toggle(&mut include_stats, "Include Stats in the notch carousel")
            .changed()
        {
            cx.cfg.notch.set_stats_enabled(include_stats);
            *cx.changed = true;
        }
        let mut default_stats =
            cx.cfg.notch.default_collapsed == crate::config::CollapsedMode::Stats;
        if ui
            .toggle(&mut default_stats, "Rest on Stats when the notch closes")
            .changed()
        {
            cx.cfg.notch.set_stats_default(default_stats);
            *cx.changed = true;
        }
        ui.label(
            RichText::new(
                "CPU and memory cover the entire PC. GPU is the busiest physical engine after combining its process readings.",
            )
            .size(10.5)
            .color(theme::text_secondary()),
        );
    }

    fn tab_prefs_window(ui: &mut egui::Ui, cx: &mut PageCtx<'_>) {
        let win = &mut cx.cfg.settings_window;

        Self::section_title(ui, "ACCENT");
        Self::row_stacked(
            ui,
            "Accent colour",
            Some("Used for switches, tabs, sliders and the selected page."),
            |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    for preset in ACCENT_PRESETS {
                        if Self::accent_dot(ui, preset, win.accent == preset).clicked() {
                            win.accent = preset;
                            *cx.changed = true;
                        }
                    }
                    ui.add_space(6.0);
                    if color_picker::color_picker_button(ui, "Custom accent", &mut win.accent)
                        .changed()
                    {
                        win.accent[3] = 1.0;
                        *cx.changed = true;
                    }
                });
            },
        );

        Self::divider(ui);
        Self::section_title(ui, "GLASS");
        Self::row_stacked(
            ui,
            "Glass strength",
            Some(
                "How frosted this window is. Lower lets more of the blurred desktop show \
                 through; higher is closer to a solid panel. Windows does the blurring, so it \
                 costs nothing while the window sits still.",
            ),
            |ui| {
                let mut pct = (win.glass * 100.0).round();
                let floor = (crate::config::SettingsWindowConfig::MIN_GLASS * 100.0).round();
                if ui
                    .add(
                        egui::Slider::new(&mut pct, floor..=100.0)
                            .fixed_decimals(0)
                            .suffix("%"),
                    )
                    .changed()
                {
                    win.glass = pct / 100.0;
                    *cx.changed = true;
                }
            },
        );

        Self::divider(ui);
        Self::section_title(ui, "WINDOW");
        Self::row_inline(ui, "Float above other windows", |ui| {
            if ui
                .toggle(&mut win.pinned, "")
                .on_hover_text("Also on the pin button at the top of every page")
                .changed()
            {
                ui.ctx()
                    .send_viewport_cmd(egui::ViewportCommand::WindowLevel(if win.pinned {
                        egui::WindowLevel::AlwaysOnTop
                    } else {
                        egui::WindowLevel::Normal
                    }));
                *cx.changed = true;
            }
        });
        ui.label(
            RichText::new(
                "Closing the window hides it to the tray; Venu keeps running. Press Ctrl+F \
                 anywhere to search the settings.",
            )
            .size(11.0)
            .color(theme::text_secondary()),
        );
    }

    fn tab_prefs_startup(ui: &mut egui::Ui, cx: &mut PageCtx<'_>) {
        Self::section_title(ui, "STARTUP");
        Self::row_stacked(
            ui,
            "Launch on startup",
            Some(
                "Start Venu quietly in the tray when you sign in to Windows. It lives under \
                  your user's autostart key — no admin rights — and can also be paused from \
                  Task Manager's Startup list.",
            ),
            |ui| {
                let mut enabled = cx.cfg.launch_on_startup;
                let check = ui
                    .toggle(&mut enabled, "Start with Windows")
                    .on_hover_text("Registers venu.exe under the per-user Run key");
                if check.changed() {
                    let result = if enabled {
                        crate::autostart::enable()
                    } else {
                        crate::autostart::disable()
                    };
                    match result {
                        Ok(()) => {
                            cx.cfg.launch_on_startup = enabled;
                            *cx.changed = true;
                        }
                        Err(e) => eprintln!("[settings] autostart toggle failed: {e}"),
                    }
                }
                if enabled {
                    if let Some(cmd) = crate::autostart::registered_command() {
                        ui.label(RichText::new(cmd).size(10.5).color(theme::text_tertiary()));
                    }
                }
            },
        );
    }

    fn tab_prefs_storage(ui: &mut egui::Ui, _cx: &mut PageCtx<'_>) {
        Self::section_title(ui, "YOUR SETTINGS");

        ui.label(
            RichText::new("Everything in this window is saved here, as soon as you change it.")
                .size(11.0)
                .color(theme::text_secondary()),
        );
        ui.add_space(8.0);

        let path = AppConfig::config_path();
        let shown = path.display().to_string();
        let mut readonly = shown.as_str();
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut readonly)
                    .desired_width(ui.available_width() - 92.0)
                    .font(egui::FontId::proportional(11.0)),
            );
            if ui
                .button("Copy path")
                .on_hover_text("Copy the settings file path to the clipboard")
                .clicked()
            {
                ui.ctx().copy_text(shown.clone());
            }
        });
    }

    /// One cell of a segmented control: a button that stays lit when picked.
    ///
    /// Written by hand rather than reached for as a `SelectableLabel` so the
    /// picked cell can carry the accent rather than egui's selection blue.
    fn segment(ui: &mut egui::Ui, label: &str, picked: bool) -> egui::Response {
        let galley = ui.painter().layout_no_wrap(
            label.to_owned(),
            egui::FontId::proportional(12.0),
            theme::text_primary(),
        );
        let size = Vec2::new(galley.size().x + 26.0, 28.0);
        let (rect, response) = ui.allocate_exact_size(size, Sense::click());

        let lit = ui
            .ctx()
            .animate_bool_with_time(response.id.with("lit"), picked, 0.14);
        let hover = ui.ctx().animate_bool_with_time(
            response.id.with("hover"),
            response.hovered() && !picked,
            0.11,
        );

        ui.painter().rect_filled(
            rect,
            Rounding::same(8.0),
            theme::surface().gamma_multiply(1.0 - lit),
        );
        if hover > 0.001 {
            ui.painter().rect_filled(
                rect,
                Rounding::same(8.0),
                theme::surface_hover().gamma_multiply(hover),
            );
        }
        if lit > 0.001 {
            ui.painter().rect_filled(
                rect,
                Rounding::same(8.0),
                theme::accent().gamma_multiply(lit),
            );
        }

        let text = if lit > 0.5 {
            theme::on_accent()
        } else {
            theme::text_secondary()
        };
        ui.painter().text(
            rect.center(),
            Align2::CENTER_CENTER,
            label,
            egui::FontId::proportional(12.0),
            text,
        );

        response
    }

    fn draw_preview_bar(ui: &mut egui::Ui, cfg: &AppConfig) {
        ui.horizontal(|ui| {
            let (dot_rect, _) = ui.allocate_exact_size(Vec2::new(8.0, 8.0), Sense::hover());
            ui.painter()
                .circle_filled(dot_rect.center(), 3.0, theme::accent());

            ui.label(
                RichText::new("PREVIEW")
                    .size(10.0)
                    .color(theme::text_tertiary())
                    .strong(),
            );
            ui.add_space(14.0);

            let bg_color = Color32::from_rgba_unmultiplied(
                (cfg.colors.bg_color[0] * 255.0) as u8,
                (cfg.colors.bg_color[1] * 255.0) as u8,
                (cfg.colors.bg_color[2] * 255.0) as u8,
                (cfg.colors.bg_color[3] * 255.0) as u8,
            );
            let fg_color = Color32::from_rgba_unmultiplied(
                (cfg.colors.text_color[0] * 255.0) as u8,
                (cfg.colors.text_color[1] * 255.0) as u8,
                (cfg.colors.text_color[2] * 255.0) as u8,
                (cfg.colors.text_color[3] * 255.0) as u8,
            );

            // Painted rather than laid out: a label in a strip this short gets
            // wrapped or clipped to a sliver, and the preview is only useful
            // if the line reads as one line.
            let (rect, _) =
                ui.allocate_exact_size(Vec2::new(ui.available_width(), 30.0), Sense::hover());
            ui.painter()
                .rect_filled(rect, Rounding::same(6.0), bg_color);

            let spacing_str = " ".repeat(cfg.phrase_spacing as usize);
            let display_text = format!("{}{}{}", cfg.text, spacing_str, cfg.text);
            let mut text_rt = RichText::new(display_text)
                .size(cfg.font.size.clamp(12.0, 18.0))
                .color(fg_color);
            if cfg.font.italic {
                text_rt = text_rt.italics();
            }
            let galley = egui::WidgetText::from(text_rt).into_galley(
                ui,
                Some(egui::TextWrapMode::Extend),
                f32::INFINITY,
                egui::TextStyle::Body,
            );
            let pos = egui::pos2(rect.left() + 12.0, rect.center().y - galley.size().y * 0.5);
            ui.painter()
                .with_clip_rect(rect.shrink2(Vec2::new(8.0, 0.0)))
                .galley(pos, galley, fg_color);
        });
    }

    fn sec_marquee_message(
        ui: &mut egui::Ui,
        temp_text: &mut String,
        cfg: &mut AppConfig,
        changed: &mut bool,
    ) {
        Self::section_title(ui, "MARQUEE TEXT");

        Self::row_stacked(
            ui,
            "Message",
            Some("Unicode, Devanagari/Hindi, CJK and emoji are supported."),
            |ui| {
                if ui
                    .add(
                        egui::TextEdit::multiline(temp_text)
                            .desired_rows(4)
                            .desired_width(f32::INFINITY),
                    )
                    .changed()
                {
                    cfg.text = temp_text.clone();
                    *changed = true;
                }
            },
        );

        ui.label(
            RichText::new("Presets")
                .size(12.0)
                .color(theme::text_secondary()),
        );
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui.button("Hydration Reminder").clicked() {
                *temp_text =
                    "💧 REMINDER: Drink water & stay hydrated! • Take a 5-min break 🌿".to_string();
                cfg.text = temp_text.clone();
                *changed = true;
            }
            if ui.button("Focus Mode").clicked() {
                *temp_text = "🚀 STAY FOCUSED • Deep Work Active • Finish Tasks 🏆".to_string();
                cfg.text = temp_text.clone();
                *changed = true;
            }
            if ui.button("Hindi / CJK Sample").clicked() {
                *temp_text = "हरि प\u{941}र\u{941}ष जगद\u{94d}बन\u{94d}ध\u{941} महाउद\u{94d}धरण • 欢迎光临 • 双手合十 🌸"
                    .to_string();
                cfg.text = temp_text.clone();
                *changed = true;
            }
        });

        ui.add_space(18.0);
        Self::divider(ui);

        Self::section_title(ui, "SPACING");
        Self::row_stacked(
            ui,
            "Repetition Spacing",
            Some("Space between repeated copies of the phrase."),
            |ui| {
                // Padding *between* repeated copies, so it does nothing at
                // all while the line is held still.
                if ui
                    .add_enabled(
                        cfg.marquee.scroll,
                        egui::Slider::new(&mut cfg.phrase_spacing, 1..=50).text("spaces"),
                    )
                    .changed()
                {
                    *changed = true;
                }
            },
        );

        ui.add_space(18.0);
    }

    fn sec_notch_marquee(ui: &mut egui::Ui, cfg: &mut AppConfig, changed: &mut bool) {
        Self::row_stacked(
            ui,
            "Motion",
            Some("Off, the message is held still and centred instead of scrolling."),
            |ui| {
                ui.horizontal(|ui| {
                    if ui
                        .selectable_label(cfg.marquee.scroll, "Scrolling")
                        .clicked()
                        && !cfg.marquee.scroll
                    {
                        cfg.marquee.scroll = true;
                        *changed = true;
                    }
                    if ui
                        .selectable_label(!cfg.marquee.scroll, "Hold still")
                        .clicked()
                        && cfg.marquee.scroll
                    {
                        cfg.marquee.scroll = false;
                        *changed = true;
                    }
                });
            },
        );

        Self::row_stacked(
            ui,
            "Collapsed Pill",
            Some("The resting notch, when this is the slide it came to rest on."),
            |ui| {
                Self::size_pair(
                    ui,
                    &mut cfg.marquee.collapsed_width,
                    &mut cfg.marquee.collapsed_height,
                    900,
                    300,
                    changed,
                );
            },
        );

        Self::row_stacked(
            ui,
            "Open Panel",
            Some("A long line usually wants more width and less height than a clock does."),
            |ui| {
                Self::size_pair(
                    ui,
                    &mut cfg.marquee.panel_width,
                    &mut cfg.marquee.panel_height,
                    2200,
                    900,
                    changed,
                );
            },
        );

        Self::row_stacked(
            ui,
            "Type Size",
            Some("Zero follows the built-in size for each state."),
            |ui| {
                *changed |= Self::slider_line(
                    ui,
                    "Panel",
                    egui::Slider::new(&mut cfg.marquee.font_size, 0.0..=120.0)
                        .step_by(0.5)
                        .custom_formatter(|v, _| Self::size_text(v)),
                );
                *changed |= Self::slider_line(
                    ui,
                    "Pill",
                    egui::Slider::new(&mut cfg.marquee.pill_font_size, 0.0..=64.0)
                        .step_by(0.5)
                        .custom_formatter(|v, _| Self::size_text(v)),
                );
                ui.horizontal(|ui| {
                    ui.add_space(SLIDER_LABEL_WIDTH);
                    if ui.small_button("Reset all").clicked() {
                        cfg.marquee = Default::default();
                        *changed = true;
                    }
                });
            },
        );

        if !cfg.notch.slides.contains(&SlideKind::Marquee) {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("This slide is not in the deck.")
                        .size(11.0)
                        .color(theme::text_secondary()),
                );
                if ui.small_button("Add it").clicked() {
                    cfg.notch.slides.push(SlideKind::Marquee);
                    *changed = true;
                }
            });
        }
    }

    fn sec_edge_master(ui: &mut egui::Ui, cfg: &mut AppConfig, changed: &mut bool) {
        Self::section_title(ui, "MASTER SWITCH");
        if ui
            .toggle(&mut cfg.overlay_enabled, "Show the edge marquee")
            .changed()
        {
            *changed = true;
        }
    }

    fn sec_edge_edges(ui: &mut egui::Ui, cfg: &mut AppConfig, changed: &mut bool) {
        Self::section_title(ui, "ACTIVE EDGES");
        // Greyed out rather than left looking live but inert: it should be
        // obvious which control is in charge.
        ui.add_enabled_ui(cfg.overlay_enabled, |ui| {
            egui::Grid::new("edge_grid")
                .num_columns(2)
                .spacing([24.0, 10.0])
                .show(ui, |ui| {
                    if ui.toggle(&mut cfg.edges.top, "Top").changed() {
                        *changed = true;
                    }
                    if ui.toggle(&mut cfg.edges.right, "Right").changed() {
                        *changed = true;
                    }
                    ui.end_row();

                    if ui.toggle(&mut cfg.edges.bottom, "Bottom").changed() {
                        *changed = true;
                    }
                    if ui.toggle(&mut cfg.edges.left, "Left").changed() {
                        *changed = true;
                    }
                    ui.end_row();
                });
        });

        ui.add_space(16.0);
        Self::row_stacked(ui, "Strip Thickness", None, |ui| {
            if ui
                .add(egui::Slider::new(&mut cfg.thickness, 16..=100).text("px"))
                .changed()
            {
                *changed = true;
            }
        });
    }

    fn sec_edge_margins(ui: &mut egui::Ui, cfg: &mut AppConfig, changed: &mut bool) {
        Self::section_title(ui, "CLEARANCE MARGINS");
        ui.label(
            RichText::new("Padding to avoid overlapping window controls or the taskbar.")
                .size(11.0)
                .color(theme::text_secondary()),
        );
        ui.add_space(10.0);

        let pad = &mut cfg.padding;
        for (label, value) in [
            ("Top", &mut pad.top),
            ("Right", &mut pad.right),
            ("Bottom", &mut pad.bottom),
            ("Left", &mut pad.left),
        ] {
            *changed |=
                Self::slider_line(ui, label, egui::Slider::new(value, 0..=800).suffix(" px"));
        }
    }

    fn sec_edge_type(ui: &mut egui::Ui, cfg: &mut AppConfig, changed: &mut bool) {
        Self::section_title(ui, "TYPOGRAPHY");

        Self::row_inline(ui, "Font Family", |ui| {
            let fonts = [
                "Geist",
                "Geist Mono",
                "Segoe UI",
                "Plus Jakarta Sans",
                "Arial",
                "Consolas",
                "Impact",
                "Microsoft YaHei",
                "Yu Gothic",
            ];
            egui::ComboBox::from_id_salt("font_family_combo")
                .selected_text(cfg.font.family.as_str())
                .show_ui(ui, |ui| {
                    for f in fonts {
                        if ui
                            .selectable_value(&mut cfg.font.family, f.to_string(), f)
                            .clicked()
                        {
                            *changed = true;
                        }
                    }
                });
        });

        Self::row_stacked(ui, "Font Size", None, |ui| {
            if ui
                .add(egui::Slider::new(&mut cfg.font.size, 10.0..=70.0).text("pt"))
                .changed()
            {
                *changed = true;
            }
        });

        ui.horizontal(|ui| {
            if ui.toggle(&mut cfg.font.bold, "Bold").changed() {
                *changed = true;
            }
            ui.add_space(16.0);
            if ui.toggle(&mut cfg.font.italic, "Italic").changed() {
                *changed = true;
            }
        });

        ui.add_space(6.0);
    }

    fn sec_edge_color(ui: &mut egui::Ui, cfg: &mut AppConfig, changed: &mut bool) {
        Self::section_title(ui, "COLOR");

        Self::row_inline(ui, "Text Color", |ui| {
            if color_picker::color_picker_button(ui, "edge_text_color", &mut cfg.colors.text_color)
                .changed()
            {
                *changed = true;
            }
        });

        Self::row_inline(ui, "Background Color", |ui| {
            if color_picker::color_picker_button(ui, "edge_bg_color", &mut cfg.colors.bg_color)
                .changed()
            {
                *changed = true;
            }
        });
    }

    fn sec_edge_motion(ui: &mut egui::Ui, cfg: &mut AppConfig, changed: &mut bool) {
        Self::section_title(ui, "MOTION");

        Self::row_stacked(ui, "Scroll Speed", None, |ui| {
            if ui
                .add(egui::Slider::new(&mut cfg.animation.speed, 5.0..=500.0).text("px/s"))
                .changed()
            {
                *changed = true;
            }
        });

        Self::row_inline(ui, "Reverse Direction", |ui| {
            if ui.toggle(&mut cfg.animation.reverse, "").changed() {
                *changed = true;
            }
        });
    }

    fn sec_edge_window(ui: &mut egui::Ui, cfg: &mut AppConfig, changed: &mut bool) {
        Self::section_title(ui, "OVERLAY BEHAVIOR");

        Self::row_inline(ui, "Always On Top", |ui| {
            if ui
                .toggle(&mut cfg.always_on_top, "")
                .on_hover_text("Keep the text border above all open windows")
                .changed()
            {
                *changed = true;
            }
        });

        Self::row_inline(ui, "Click-Through Mode", |ui| {
            if ui
                .toggle(&mut cfg.click_through, "")
                .on_hover_text("Allow mouse clicks to pass through the text border")
                .changed()
            {
                *changed = true;
            }
        });
    }

    fn sec_flash_master(ui: &mut egui::Ui, cfg: &mut AppConfig, changed: &mut bool) {
        Self::section_title(ui, "MASTER SWITCH");
        if ui
            .toggle(&mut cfg.flash.enabled, "Show FlashScreen reminders")
            .changed()
        {
            *changed = true;
        }
        ui.add_space(4.0);
        ui.label(
            RichText::new(
                "Between flashes nothing is on screen and nothing is rendered. When one is \
                 due, the flash appears at the center of the monitor, stays for its duration, \
                 and leaves — no toast to dismiss, no sound, no click needed.",
            )
            .size(11.0)
            .color(theme::text_secondary()),
        );
        ui.add_space(10.0);
        if ui
            .button(RichText::new("⚡ Preview a flash now").strong())
            .on_hover_text("Plays one flash immediately — works even with the switch off")
            .clicked()
        {
            crate::flash::request_preview();
        }
    }

    fn sec_flash_schedule(ui: &mut egui::Ui, cfg: &mut AppConfig, changed: &mut bool) {
        Self::section_title(ui, "SCHEDULE");

        Self::row_stacked(
            ui,
            "Repeat every",
            Some("How long the screen stays quiet between flashes."),
            |ui| {
                ui.horizontal_wrapped(|ui| {
                    for (label, secs) in [
                        ("30 s", 30.0),
                        ("1 min", 60.0),
                        ("2 min", 120.0),
                        ("5 min", 300.0),
                        ("15 min", 900.0),
                        ("1 hour", 3600.0),
                    ] {
                        let picked = (cfg.flash.interval_secs - secs).abs() < 1.0;
                        if Self::segment(ui, label, picked).clicked() {
                            cfg.flash.interval_secs = secs;
                            *changed = true;
                        }
                    }
                });
                ui.add_space(8.0);
                // Any interval at all, on a slider beside the presets: the
                // presets are for the common cases, not a cage. Logarithmic
                // because the range runs from seconds to hours, and a linear
                // slider would spend its whole travel on the hours.
                let mut secs = cfg.flash.interval_secs.clamp(5.0, 14400.0);
                if ui
                    .add(
                        egui::Slider::new(&mut secs, 5.0..=14400.0)
                            .logarithmic(true)
                            .custom_formatter(|v, _| flash_interval_text(v)),
                    )
                    .changed()
                {
                    cfg.flash.interval_secs = secs;
                    *changed = true;
                }
            },
        );

        Self::row_stacked(
            ui,
            "Flash for",
            Some("How long each flash stays at full presence before it starts to leave."),
            |ui| {
                if ui
                    .add(
                        egui::Slider::new(&mut cfg.flash.duration_secs, 1.0..=60.0)
                            .suffix(" s")
                            .step_by(0.5),
                    )
                    .changed()
                {
                    *changed = true;
                }
            },
        );
    }

    fn sec_flash_content(ui: &mut egui::Ui, cfg: &mut AppConfig, changed: &mut bool) {
        Self::section_title(ui, "CONTENT");

        Self::row_stacked(ui, "What appears", None, |ui| {
            ui.horizontal_wrapped(|ui| {
                for option in FlashContent::ALL {
                    let picked = cfg.flash.content == option;
                    if Self::segment(ui, option.label(), picked).clicked() {
                        cfg.flash.content = option;
                        *changed = true;
                    }
                }
            });
        });

        if cfg.flash.content == FlashContent::Both {
            Self::row_stacked(
                ui,
                "Layout",
                Some("How the text sits against the image."),
                |ui| {
                    ui.horizontal_wrapped(|ui| {
                        for option in FlashLayout::ALL {
                            let picked = cfg.flash.layout == option;
                            if Self::segment(ui, option.label(), picked).clicked() {
                                cfg.flash.layout = option;
                                *changed = true;
                            }
                        }
                    });
                },
            );
        }

        let wants_text = cfg.flash.content != FlashContent::Image;
        let wants_image = cfg.flash.content != FlashContent::Text;

        if wants_text {
            Self::row_stacked(
                ui,
                "Messages",
                Some(
                    "With more than one, each flash shows the next in turn. Write or paste \
                      anything — Unicode, Devanagari/Hindi, CJK and emoji are supported.",
                ),
                |ui| {
                    let mut remove: Option<usize> = None;
                    for (i, text) in cfg.flash.texts.iter_mut().enumerate() {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(format!("{}.", i + 1))
                                    .size(12.0)
                                    .color(theme::text_tertiary()),
                            );
                            // Right-to-left so the button claims its width
                            // first and the field takes exactly what is left.
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui.button("Remove").clicked() {
                                    remove = Some(i);
                                }
                                if ui
                                    .add(
                                        egui::TextEdit::singleline(text)
                                            .desired_width(ui.available_width())
                                            .hint_text("What should flash across your screen?"),
                                    )
                                    .changed()
                                {
                                    *changed = true;
                                }
                            });
                        });
                        ui.add_space(4.0);
                    }
                    if let Some(i) = remove {
                        cfg.flash.texts.remove(i);
                        *changed = true;
                    }
                    if ui
                        .add_enabled(cfg.flash.texts.len() < 8, egui::Button::new("Add message"))
                        .clicked()
                    {
                        cfg.flash.texts.push(String::new());
                        *changed = true;
                    }
                },
            );
        }

        if wants_image {
            Self::row_stacked(
                ui,
                "Images",
                Some(
                    "Choose pictures from your PC. With more than one, each flash shows the \
                      next in turn. PNG, JPG, BMP, GIF or WebP.",
                ),
                |ui| {
                    let mut remove: Option<usize> = None;
                    for (i, path) in cfg.flash.images.iter_mut().enumerate() {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(format!("{}.", i + 1))
                                    .size(12.0)
                                    .color(theme::text_tertiary()),
                            );
                            // Buttons first, in right-to-left, so the path
                            // field gets exactly the width actually left.
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui.button("Remove").clicked() {
                                    remove = Some(i);
                                }
                                if ui.button("Browse...").clicked() {
                                    if let Some(picked) =
                                        filedlg::pick_image(path, "Choose a flash image")
                                    {
                                        *path = picked;
                                        *changed = true;
                                    }
                                }
                                if ui
                                    .add(
                                        egui::TextEdit::singleline(path)
                                            .desired_width(ui.available_width())
                                            .hint_text("C:\\Users\\...\\flash.jpg"),
                                    )
                                    .changed()
                                {
                                    *changed = true;
                                }
                            });
                        });
                        ui.add_space(4.0);
                    }
                    if let Some(i) = remove {
                        cfg.flash.images.remove(i);
                        *changed = true;
                    }
                    if ui
                        .add_enabled(cfg.flash.images.len() < 8, egui::Button::new("Add image"))
                        .clicked()
                    {
                        cfg.flash.images.push(String::new());
                        *changed = true;
                    }
                },
            );

            Self::row_stacked(
                ui,
                "Image size",
                Some("How much of the screen height the image takes up."),
                |ui| {
                    if ui
                        .add(
                            egui::Slider::new(&mut cfg.flash.image_scale, 0.1..=1.0)
                                .fixed_decimals(2)
                                .suffix("×"),
                        )
                        .changed()
                    {
                        *changed = true;
                    }
                },
            );
        }
    }

    fn sec_flash_type(ui: &mut egui::Ui, cfg: &mut AppConfig, changed: &mut bool) {
        Self::section_title(ui, "TYPE");

        Self::row_inline(ui, "Font Family", |ui| {
            let fonts = [
                "Geist",
                "Geist Mono",
                "Segoe UI",
                "Plus Jakarta Sans",
                "Arial",
                "Consolas",
                "Impact",
                "Microsoft YaHei",
                "Yu Gothic",
            ];
            egui::ComboBox::from_id_salt("flash_font_combo")
                .selected_text(cfg.flash.font.family.as_str())
                .show_ui(ui, |ui| {
                    for f in fonts {
                        if ui
                            .selectable_value(&mut cfg.flash.font.family, f.to_string(), f)
                            .clicked()
                        {
                            *changed = true;
                        }
                    }
                });
        });

        Self::row_stacked(
            ui,
            "Font Size",
            Some(
                "Flash type runs far larger than the marquees — it is meant to be read from \
                  across the room for a few seconds, not sat next to all day.",
            ),
            |ui| {
                if ui
                    .add(egui::Slider::new(&mut cfg.flash.font.size, 12.0..=400.0).text("pt"))
                    .changed()
                {
                    *changed = true;
                }
            },
        );

        ui.horizontal(|ui| {
            if ui.toggle(&mut cfg.flash.font.bold, "Bold").changed() {
                *changed = true;
            }
            ui.add_space(16.0);
            if ui.toggle(&mut cfg.flash.font.italic, "Italic").changed() {
                *changed = true;
            }
        });
        ui.add_space(10.0);

        Self::row_stacked(
            ui,
            "Text colors",
            Some(
                "One color is used as-is. With several, each flash takes the next in turn — \
                  or turn Gradient on to sweep all of them across the text at once.",
            ),
            |ui| {
                let mut remove: Option<usize> = None;
                // Read once, outside the loop: the rows borrow the list
                // mutably, and the Remove enablement only needs the count.
                let color_count = cfg.flash.text_colors.len();
                for (i, color) in cfg.flash.text_colors.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(format!("{}.", i + 1))
                                .size(12.0)
                                .color(theme::text_tertiary()),
                        );
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui
                                .add_enabled(color_count > 1, egui::Button::new("Remove"))
                                .clicked()
                            {
                                remove = Some(i);
                            }
                            if color_picker::color_picker_button(
                                ui,
                                &format!("flash_color_{i}"),
                                color,
                            )
                            .changed()
                            {
                                *changed = true;
                            }
                        });
                    });
                    ui.add_space(4.0);
                }
                if let Some(i) = remove {
                    cfg.flash.text_colors.remove(i);
                    *changed = true;
                }
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(
                            cfg.flash.text_colors.len() < 6,
                            egui::Button::new("Add color"),
                        )
                        .clicked()
                    {
                        // A sensible next pick rather than transparent black:
                        // warm the wheel a step from whatever is last.
                        let last = cfg.flash.text_colors.last().copied().unwrap_or([1.0; 4]);
                        let (h, s, v) = color_picker::rgb_to_hsv(last[0], last[1], last[2]);
                        let (r, g, b) = color_picker::hsv_to_rgb((h + 0.12) % 1.0, s, v);
                        cfg.flash.text_colors.push([r, g, b, last[3]]);
                        *changed = true;
                    }
                    ui.add_space(14.0);
                    if ui
                        .add_enabled(
                            cfg.flash.text_colors.len() >= 2,
                            egui::Checkbox::new(&mut cfg.flash.gradient_text, "Gradient sweep"),
                        )
                        .on_hover_text(
                            "Paints every color as one gradient across the text. Each flash \
                             starts the sweep on the next color, so it still changes every turn.",
                        )
                        .changed()
                    {
                        *changed = true;
                    }
                    ui.add_space(14.0);
                    if ui
                        .toggle(&mut cfg.flash.shine, "Shine sweep")
                        .on_hover_text(
                            "After the text has appeared, a single wave crosses it from the \
                             left end to the right, roughly at reading pace. The Shine speed \
                             control below sets how long that one pass takes.",
                        )
                        .changed()
                    {
                        *changed = true;
                    }
                });
            },
        );

        // Greyed out rather than left looking live but inert when the shine
        // itself is off.
        ui.add_enabled_ui(cfg.flash.shine, |ui| {
            Self::row_stacked(
                ui,
                "Shine speed",
                Some(
                    "How long the single wave takes to cross the text, once it has settled \
                      on screen. Around a second reads like natural reading pace.",
                ),
                |ui| {
                    if ui
                        .add(
                            egui::Slider::new(&mut cfg.flash.shine_speed_secs, 0.2..=5.0)
                                .suffix(" s")
                                .step_by(0.1),
                        )
                        .changed()
                    {
                        *changed = true;
                    }
                },
            );
        });
    }

    fn sec_flash_backdrop(ui: &mut egui::Ui, cfg: &mut AppConfig, changed: &mut bool) {
        Self::section_title(ui, "BACKGROUND");

        Self::row_stacked(
            ui,
            "Style",
            Some("Drawn behind the message itself — never the whole screen."),
            |ui| {
                ui.horizontal_wrapped(|ui| {
                    for option in FlashBackground::ALL {
                        let picked = cfg.flash.bg_kind == option;
                        if Self::segment(ui, option.label(), picked).clicked() {
                            cfg.flash.bg_kind = option;
                            *changed = true;
                        }
                    }
                });
            },
        );

        Self::row_stacked(
            ui,
            "Effectiveness",
            Some(
                "0% is invisible; 100% is the full effect. For Frosted and Blur this also \
                  scales how strong the blur is.",
            ),
            |ui| {
                let mut pct = (cfg.flash.bg_strength.clamp(0.0, 1.0) * 100.0).round() as i32;
                if ui
                    .add(
                        egui::Slider::new(&mut pct, 0..=100)
                            .custom_formatter(|v, _| format!("{}%", v.round() as i64)),
                    )
                    .changed()
                {
                    cfg.flash.bg_strength = pct as f32 / 100.0;
                    *changed = true;
                }
            },
        );

        Self::row_stacked(
            ui,
            "Padding",
            Some(
                "How far the background reaches beyond the message, on every side. Small hugs \
                  the text; large makes a broad plate of it.",
            ),
            |ui| {
                if ui
                    .add(egui::Slider::new(&mut cfg.flash.bg_padding, 0.0..=200.0).suffix(" px"))
                    .changed()
                {
                    *changed = true;
                }
            },
        );

        if cfg.flash.bg_kind.samples_desktop() {
            ui.label(
                RichText::new(
                    "While Frosted or Blur is on, the flash is kept out of screenshots and \
                     screen sharing — that is what stops the blur from sampling itself and \
                     smearing. Switch to any other style to be captured again.",
                )
                .size(11.0)
                .color(theme::text_secondary()),
            );
        }
    }

    fn sec_flash_anim(ui: &mut egui::Ui, cfg: &mut AppConfig, changed: &mut bool) {
        Self::section_title(ui, "ANIMATION");

        Self::row_stacked(
            ui,
            "Style",
            Some("How the flash arrives — and how it leaves. Each style covers both halves."),
            |ui| {
                egui::ComboBox::from_id_salt("flash_anim_combo")
                    .selected_text(cfg.flash.anim.label())
                    .show_ui(ui, |ui| {
                        for anim in FlashAnim::ALL {
                            if ui
                                .selectable_value(&mut cfg.flash.anim, anim, anim.label())
                                .clicked()
                            {
                                *changed = true;
                            }
                        }
                    });
            },
        );

        Self::row_stacked(
            ui,
            "Animation length",
            Some("Short is snappy, long is smooth. Applies to the entry and the exit alike."),
            |ui| {
                if ui
                    .add(
                        egui::Slider::new(&mut cfg.flash.anim_speed_secs, 0.1..=3.0)
                            .suffix(" s")
                            .step_by(0.05),
                    )
                    .changed()
                {
                    *changed = true;
                }
            },
        );
    }

    fn sec_flash_behavior(ui: &mut egui::Ui, cfg: &mut AppConfig, changed: &mut bool) {
        Self::section_title(ui, "ON SCREEN");

        Self::row_inline(ui, "Monitor", |ui| {
            let monitors = crate::notch::window::monitor_rects();
            let current = cfg
                .flash
                .monitor_index
                .min(monitors.len().saturating_sub(1));
            let label = monitors
                .get(current)
                .map(|m| {
                    format!(
                        "Display {} — {}x{}",
                        current + 1,
                        m.right - m.left,
                        m.bottom - m.top
                    )
                })
                .unwrap_or_else(|| "Display 1".to_string());

            egui::ComboBox::from_id_salt("flash_monitor_combo")
                .selected_text(label)
                .show_ui(ui, |ui| {
                    for (i, m) in monitors.iter().enumerate() {
                        let text = format!(
                            "Display {} — {}x{}",
                            i + 1,
                            m.right - m.left,
                            m.bottom - m.top
                        );
                        if ui
                            .selectable_value(&mut cfg.flash.monitor_index, i, text)
                            .clicked()
                        {
                            *changed = true;
                        }
                    }
                });
        });

        Self::row_inline(ui, "Always On Top", |ui| {
            if ui
                .toggle(&mut cfg.flash.always_on_top, "")
                .on_hover_text("Keep the flash above all open windows")
                .changed()
            {
                *changed = true;
            }
        });

        Self::row_inline(ui, "Click-Through Mode", |ui| {
            if ui
                .toggle(&mut cfg.flash.click_through, "")
                .on_hover_text("Let clicks pass through the flash to whatever is underneath")
                .changed()
            {
                *changed = true;
            }
        });
    }

    fn sec_notch_basics(ui: &mut egui::Ui, cfg: &mut AppConfig, changed: &mut bool) {
        if ui
            .toggle(&mut cfg.notch.enabled, "Show the notch")
            .changed()
        {
            *changed = true;
        }
        ui.add_space(6.0);
        if ui
            .toggle(&mut cfg.notch.always_on_top, "Keep above other windows")
            .changed()
        {
            *changed = true;
        }
        ui.add_space(6.0);
        if ui
            .toggle(&mut cfg.notch.click_through, "Let clicks pass through")
            .on_hover_text(
                "Clicks go to the window underneath instead of the notch. \
                 Hovering and the scroll wheel still work.",
            )
            .changed()
        {
            *changed = true;
        }
        ui.add_space(4.0);
        ui.label(
            RichText::new(
                "Press the left and right mouse buttons together with the cursor over the \
                 notch to switch this on or off without coming back here. It has to be a \
                 gesture: once clicks pass through, an ordinary click can no longer reach \
                 the notch to undo it.",
            )
            .size(11.0)
            .color(theme::text_secondary()),
        );
        ui.add_space(6.0);
        if ui
            .toggle(
                &mut cfg.notch.scroll_to_switch,
                "Scroll wheel changes slides",
            )
            .changed()
        {
            *changed = true;
        }

        ui.add_space(8.0);
        Self::row_inline(ui, "Default Collapsed Face", |ui| {
            let current = cfg.notch.default_collapsed;
            egui::ComboBox::from_id_salt("default_collapsed_face")
                .selected_text(current.label())
                .show_ui(ui, |ui| {
                    for mode in crate::config::CollapsedMode::ALL {
                        if ui
                            .selectable_value(&mut cfg.notch.default_collapsed, mode, mode.label())
                            .clicked()
                        {
                            *changed = true;
                        }
                    }
                });
        });
        ui.add_space(2.0);
        ui.label(
            RichText::new(
                "When you move your cursor away and the notch collapses, it returns to this face.",
            )
            .size(11.0)
            .color(theme::text_secondary()),
        );
    }

    fn sec_notif_general(ui: &mut egui::Ui, cfg: &mut AppConfig, changed: &mut bool) {
        Self::section_title(ui, "DYNAMIC NOTIFICATIONS");

        if ui
            .toggle(
                &mut cfg.notch.notifications.enabled,
                "Enable Dynamic Notch notifications",
            )
            .changed()
        {
            *changed = true;
        }

        ui.add_space(4.0);
        ui.label(
            RichText::new(
                "When a notification arrives from an allowed app, the notch springs open into a \
                 dynamic alert capsule, then smoothly settles back.",
            )
            .size(11.0)
            .color(theme::text_secondary()),
        );
    }

    fn sec_notif_apps(ui: &mut egui::Ui, cfg: &mut AppConfig, changed: &mut bool) {
        Self::section_title(ui, "ALLOWED APPS");

        ui.label(
            RichText::new("Only notifications from these apps will trigger dynamic alerts:")
                .size(12.0)
                .color(theme::text_secondary()),
        );
        ui.add_space(8.0);

        let presets = [
            "Antigravity",
            "Codex",
            "Claude",
            "Cursor",
            "Terminal",
            "VS Code",
            "Slack",
            "Discord",
        ];

        ui.horizontal_wrapped(|ui| {
            for preset in presets {
                let is_allowed = cfg
                    .notch
                    .notifications
                    .allowed_apps
                    .iter()
                    .any(|a| a.eq_ignore_ascii_case(preset));
                if ui.selectable_label(is_allowed, preset).clicked() {
                    if is_allowed {
                        cfg.notch
                            .notifications
                            .allowed_apps
                            .retain(|a| !a.eq_ignore_ascii_case(preset));
                    } else {
                        cfg.notch
                            .notifications
                            .allowed_apps
                            .push(preset.to_string());
                    }
                    crate::notch::notify::update_server_allowed_apps(
                        cfg.notch.notifications.allowed_apps.clone(),
                    );
                    *changed = true;
                }
            }
        });

        ui.add_space(10.0);
        ui.label(
            RichText::new(format!(
                "Active whitelist: {}",
                if cfg.notch.notifications.allowed_apps.is_empty() {
                    "All apps allowed (whitelist empty)".to_string()
                } else {
                    cfg.notch.notifications.allowed_apps.join(", ")
                }
            ))
            .size(11.0)
            .color(theme::text_tertiary()),
        );
    }

    fn sec_notif_alerts(ui: &mut egui::Ui, cfg: &mut AppConfig, changed: &mut bool) {
        Self::section_title(ui, "ALERT SETTINGS");

        Self::row_inline(ui, "Toast duration", |ui| {
            if ui
                .add(
                    egui::Slider::new(&mut cfg.notch.notifications.toast_duration_secs, 2.0..=10.0)
                        .suffix(" s")
                        .step_by(0.5),
                )
                .changed()
            {
                *changed = true;
            }
        });

        ui.add_space(8.0);
        Self::row_inline(ui, "Alert Border Animation", |ui| {
            let current = cfg.notch.notifications.glow_style;
            egui::ComboBox::from_id_salt("notif_glow_style")
                .selected_text(current.label())
                .show_ui(ui, |ui| {
                    for style in crate::config::NotificationGlowStyle::ALL {
                        if ui
                            .selectable_value(
                                &mut cfg.notch.notifications.glow_style,
                                style,
                                style.label(),
                            )
                            .clicked()
                        {
                            *changed = true;
                        }
                    }
                });
        });
        ui.add_space(2.0);
        ui.label(
            RichText::new(
                "When a notification arrives, a glowing border lights up around the 3 sides of the notch \
                 and dynamically drains down as the visual countdown timer.",
            )
            .size(11.0)
            .color(theme::text_secondary()),
        );
    }

    fn sec_notif_colors(ui: &mut egui::Ui, cfg: &mut AppConfig, changed: &mut bool) {
        Self::section_title(ui, "APP COLOURS");

        ui.label(
            RichText::new("Custom glowing badge & border colors for each application:")
                .size(12.0)
                .color(theme::text_secondary()),
        );
        ui.add_space(6.0);

        let app_list = [
            "Antigravity",
            "Codex",
            "Claude",
            "Cursor",
            "Terminal",
            "VS Code",
            "Slack",
            "Discord",
        ];
        for app in app_list {
            let mut current_color = cfg.notch.notifications.get_app_color(app);
            ui.horizontal(|ui| {
                ui.label(RichText::new(app).size(13.0).color(theme::text_primary()));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .color_edit_button_rgba_unmultiplied(&mut current_color)
                        .changed()
                    {
                        cfg.notch
                            .notifications
                            .app_colors
                            .insert(app.to_string(), current_color);
                        *changed = true;
                    }
                });
            });
            ui.add_space(2.0);
        }
    }

    fn sec_notif_integration(ui: &mut egui::Ui, cfg: &mut AppConfig, _changed: &mut bool) {
        Self::section_title(ui, "INTEGRATION & TEST");

        ui.horizontal(|ui| {
            ui.label(
                RichText::new("Local Webhook Server:")
                    .size(12.0)
                    .color(theme::text_primary()),
            );
            ui.label(
                RichText::new(format!(
                    "http://127.0.0.1:{}/notify",
                    cfg.notch.notifications.webhook_port
                ))
                .size(12.0)
                .color(theme::text_tertiary())
                .monospace(),
            );
        });
        ui.add_space(4.0);
        ui.label(
            RichText::new(
                "AI agents (Antigravity, Codex, Claude) and CLI scripts can send POST requests with JSON \
                 {\"app\":\"Antigravity\",\"title\":\"Task Finished\",\"body\":\"Ready to review\"} directly.",
            )
            .size(11.0)
            .color(theme::text_secondary()),
        );

        ui.add_space(12.0);
        ui.horizontal(|ui| {
            if ui
                .button(RichText::new("🚀 Send Antigravity Test Alert").strong())
                .clicked()
            {
                let notif_store = crate::notch::notify::global_store();
                let dur = cfg.notch.notifications.toast_duration_secs;
                let allowed = &cfg.notch.notifications.allowed_apps;
                notif_store.write().push(
                    "Antigravity",
                    "Task Completed",
                    "Dynamic Notch glowing drain border & Hugeicons are active with 0 errors.",
                    crate::notch::notify::NotificationLevel::Success,
                    dur,
                    allowed,
                );
            }

            ui.add_space(8.0);

            if ui.button("⚡ Send Claude Alert").clicked() {
                let notif_store = crate::notch::notify::global_store();
                let dur = cfg.notch.notifications.toast_duration_secs;
                let allowed = &cfg.notch.notifications.allowed_apps;
                notif_store.write().push(
                    "Claude",
                    "Detailed Card Inspection",
                    "Clicking this toast will expand the detailed view in Notification Center!",
                    crate::notch::notify::NotificationLevel::Info,
                    dur,
                    allowed,
                );
            }

            ui.add_space(8.0);

            if ui.button("Clear All").clicked() {
                let notif_store = crate::notch::notify::global_store();
                notif_store.write().clear_all();
            }
        });
    }

    fn sec_notch_deck(ui: &mut egui::Ui, cfg: &mut AppConfig, changed: &mut bool) {
        let mut move_up: Option<usize> = None;
        let mut move_down: Option<usize> = None;
        let mut remove: Option<usize> = None;
        let len = cfg.notch.slides.len();

        for (i, slide) in cfg.notch.slides.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("{}.", i + 1))
                        .size(12.0)
                        .color(theme::text_tertiary()),
                );
                ui.label(
                    RichText::new(slide.label())
                        .size(13.0)
                        .color(theme::text_primary()),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    // Removing the last slide would leave nothing to show.
                    if ui
                        .add_enabled(len > 1, egui::Button::new("Remove"))
                        .clicked()
                    {
                        remove = Some(i);
                    }
                    if ui
                        .add_enabled(i + 1 < len, egui::Button::new("▼"))
                        .clicked()
                    {
                        move_down = Some(i);
                    }
                    if ui.add_enabled(i > 0, egui::Button::new("▲")).clicked() {
                        move_up = Some(i);
                    }
                });
            });
            ui.add_space(4.0);
        }

        if let Some(i) = move_up {
            cfg.notch.slides.swap(i, i - 1);
            *changed = true;
        }
        if let Some(i) = move_down {
            cfg.notch.slides.swap(i, i + 1);
            *changed = true;
        }
        if let Some(i) = remove {
            if cfg.notch.slides.get(i) == Some(&SlideKind::Stats) {
                cfg.notch.set_stats_enabled(false);
            } else {
                cfg.notch.slides.remove(i);
            }
            *changed = true;
        }

        let missing: Vec<SlideKind> = SlideKind::ALL
            .iter()
            .copied()
            .filter(|k| !cfg.notch.slides.contains(k))
            .collect();

        if !missing.is_empty() {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Add")
                        .size(12.0)
                        .color(theme::text_secondary()),
                );
                for kind in missing {
                    if ui.button(kind.label()).clicked() {
                        cfg.notch.slides.push(kind);
                        *changed = true;
                    }
                }
            });
        }

        if cfg.notch.active_slide >= cfg.notch.slides.len().max(1) {
            cfg.notch.active_slide = 0;
        }
    }

    fn sec_notch_size(ui: &mut egui::Ui, cfg: &mut AppConfig, changed: &mut bool) {
        Self::section_title(ui, "SIZE");

        let n = &mut cfg.notch;
        Self::row_stacked(ui, "Collapsed", Some("The resting pill."), |ui| {
            *changed |= Self::slider_line(
                ui,
                "Width",
                egui::Slider::new(&mut n.collapsed_width, 60..=900).suffix(" px"),
            );
            *changed |= Self::slider_line(
                ui,
                "Height",
                egui::Slider::new(&mut n.collapsed_height, 18..=120).suffix(" px"),
            );
        });

        Self::row_stacked(ui, "Expanded", Some("The panel it opens into."), |ui| {
            *changed |= Self::slider_line(
                ui,
                "Width",
                egui::Slider::new(&mut n.expanded_width, 320..=2200).suffix(" px"),
            );
            *changed |= Self::slider_line(
                ui,
                "Height",
                egui::Slider::new(&mut n.expanded_height, 120..=900).suffix(" px"),
            );
        });
    }

    fn sec_notch_place(ui: &mut egui::Ui, cfg: &mut AppConfig, changed: &mut bool) {
        Self::section_title(ui, "PLACEMENT");

        Self::row_inline(ui, "Monitor", |ui| {
            let monitors = crate::notch::window::monitor_rects();
            let current = cfg
                .notch
                .monitor_index
                .min(monitors.len().saturating_sub(1));
            let label = monitors
                .get(current)
                .map(|m| {
                    format!(
                        "Display {} — {}x{}",
                        current + 1,
                        m.right - m.left,
                        m.bottom - m.top
                    )
                })
                .unwrap_or_else(|| "Display 1".to_string());

            egui::ComboBox::from_id_salt("notch_monitor_combo")
                .selected_text(label)
                .show_ui(ui, |ui| {
                    for (i, m) in monitors.iter().enumerate() {
                        let text = format!(
                            "Display {} — {}x{}",
                            i + 1,
                            m.right - m.left,
                            m.bottom - m.top
                        );
                        if ui
                            .selectable_value(&mut cfg.notch.monitor_index, i, text)
                            .clicked()
                        {
                            *changed = true;
                        }
                    }
                });
        });

        Self::row_inline(ui, "Align", |ui| {
            for (align, label) in [
                (NotchAlign::Right, "Right"),
                (NotchAlign::Center, "Center"),
                (NotchAlign::Left, "Left"),
            ] {
                if ui
                    .selectable_value(&mut cfg.notch.align, align, label)
                    .clicked()
                {
                    *changed = true;
                }
            }
        });

        Self::row_stacked(
            ui,
            "Offset",
            Some("Y of 0 fuses the notch to the bezel; anything higher lets it float."),
            |ui| {
                *changed |= Self::slider_line(
                    ui,
                    "Sideways",
                    egui::Slider::new(&mut cfg.notch.offset_x, -2000..=2000).suffix(" px"),
                );
                *changed |= Self::slider_line(
                    ui,
                    "Down",
                    egui::Slider::new(&mut cfg.notch.offset_y, 0..=1200).suffix(" px"),
                );
            },
        );
    }

    fn sec_notch_look(ui: &mut egui::Ui, cfg: &mut AppConfig, changed: &mut bool) {
        Self::section_title(ui, "LOOK & FEEL");

        Self::row_stacked(
            ui,
            "Theme & Surface Style",
            Some("Sets the notch panel finish — solid bezels or live glass treatments (Frosted, Transparent, Blurred, Acrylic)."),
            |ui| {
                ui.horizontal_wrapped(|ui| {
                    for theme in NotchTheme::ALL {
                        let selected = cfg.notch.theme == theme;
                        if ui.selectable_label(selected, theme.label()).clicked() && !selected {
                            cfg.notch.theme = theme;
                            // The panel colour is the one theme value the user
                            // can also set by hand, so switching theme resets
                            // it to that theme's own surface rather than
                            // leaving a dark slab wearing light type.
                            cfg.notch.surface = theme.default_surface();
                            *changed = true;
                        }
                    }
                });
            },
        );

        Self::color_row(ui, "Accent", &mut cfg.notch.accent, changed);
        Self::color_row(ui, "Panel", &mut cfg.notch.surface, changed);

        Self::row_inline(ui, "Font", |ui| {
            let fonts = [
                "Geist",
                "Geist Mono",
                "Plus Jakarta Sans",
                "Segoe UI",
                "Consolas",
                "Arial",
                "Microsoft YaHei",
            ];
            egui::ComboBox::from_id_salt("notch_font_combo")
                .selected_text(cfg.notch.font_family.as_str())
                .show_ui(ui, |ui| {
                    for f in fonts {
                        if ui
                            .selectable_value(&mut cfg.notch.font_family, f.to_string(), f)
                            .clicked()
                        {
                            *changed = true;
                        }
                    }
                });
        });

        if ui
            .toggle(&mut cfg.notch.clock_24h, "24-hour clock")
            .changed()
        {
            *changed = true;
        }
        ui.add_space(14.0);

        Self::row_stacked(
            ui,
            "Collapse Delay",
            Some("Grace period after the cursor leaves, so brushing past does not slam it shut."),
            |ui| {
                if ui
                    .add(egui::Slider::new(&mut cfg.notch.collapse_delay_ms, 0..=1500).text("ms"))
                    .changed()
                {
                    *changed = true;
                }
            },
        );
    }

    fn sec_notch_effects(ui: &mut egui::Ui, cfg: &mut AppConfig, changed: &mut bool) {
        Self::section_title(ui, "BORDER EFFECTS");

        let fx = &mut cfg.notch.border_effect;
        Self::row_inline(ui, "Effect", |ui| {
            egui::ComboBox::from_id_salt("notch_border_effect")
                .selected_text(fx.effect.label())
                .show_ui(ui, |ui| {
                    for effect in BorderEffect::ALL {
                        if ui
                            .selectable_value(&mut fx.effect, effect, effect.label())
                            .clicked()
                        {
                            *changed = true;
                        }
                    }
                });
            if fx.effect != BorderEffect::Off && ui.button("Preview").clicked() {
                fx.preview = fx.preview.wrapping_add(1);
                *changed = true;
            }
        });

        if fx.effect != BorderEffect::Off {
            Self::row_stacked(
                ui,
                "Plays",
                Some(if fx.continuous {
                    "Loops without a break, drawn at 30 frames a second to keep it light."
                } else {
                    "Once per interval. The notch does no drawing at all in between."
                }),
                |ui| {
                    ui.horizontal(|ui| {
                        for (continuous, label) in
                            [(false, "On an interval"), (true, "Continuously")]
                        {
                            if Self::segment(ui, label, fx.continuous == continuous).clicked()
                                && fx.continuous != continuous
                            {
                                fx.continuous = continuous;
                                *changed = true;
                            }
                        }
                    });
                },
            );
        }

        if fx.effect != BorderEffect::Off && !fx.continuous {
            Self::row_stacked(
                ui,
                "Play every",
                Some("Seconds from the end of one run to the start of the next."),
                |ui| {
                    if ui
                        .add(
                            egui::Slider::new(
                                &mut fx.interval_secs,
                                BorderEffectConfig::MIN_INTERVAL_SECS
                                    ..=BorderEffectConfig::MAX_INTERVAL_SECS,
                            )
                            .logarithmic(true)
                            .suffix(" s"),
                        )
                        .changed()
                    {
                        *changed = true;
                    }
                },
            );
        }

        if fx.effect != BorderEffect::Off {
            let (label, help) = if fx.continuous {
                ("Loop length", "Seconds per lap. Longer is calmer.")
            } else {
                ("Duration", "How long one run lasts.")
            };
            Self::row_stacked(ui, label, Some(help), |ui| {
                if ui
                    .add(
                        egui::Slider::new(&mut fx.duration_secs, 0.8..=8.0)
                            .step_by(0.1)
                            .suffix(" s"),
                    )
                    .changed()
                {
                    *changed = true;
                }
            });
            if fx.effect.uses_color() {
                Self::color_row(ui, "Effect colour", &mut fx.color, changed);
            }
        }
    }

    fn sec_status_now(ui: &mut egui::Ui, cfg: &mut AppConfig, changed: &mut bool) {
        Self::section_title(ui, "THE ONE LINE");
        ui.label(
            RichText::new(
                "One line for what you are actually doing. Not a notes app, not a backlog — \
                 click the line inside the open notch to change it without coming here.",
            )
            .size(11.0)
            .color(theme::text_secondary()),
        );
        ui.add_space(14.0);

        Self::row_stacked(ui, "Heading", Some("Small label above the line."), |ui| {
            if ui
                .add(
                    egui::TextEdit::singleline(&mut cfg.status.heading)
                        .desired_width(f32::INFINITY)
                        .hint_text("TODAY"),
                )
                .changed()
            {
                *changed = true;
            }
        });

        Self::row_stacked(ui, "Focus", None, |ui| {
            if ui
                .add(
                    egui::TextEdit::multiline(&mut cfg.status.focus)
                        .desired_width(f32::INFINITY)
                        .desired_rows(2)
                        .hint_text("What are you working on?"),
                )
                .changed()
            {
                *changed = true;
            }
        });
    }

    fn sec_status_deck(ui: &mut egui::Ui, cfg: &mut AppConfig, changed: &mut bool) {
        Self::section_title(ui, "ON DECK");
        ui.label(
            RichText::new("The short list beside it. Keep it to today.")
                .size(11.0)
                .color(theme::text_secondary()),
        );
        ui.add_space(10.0);

        let mut remove: Option<usize> = None;
        for (i, item) in cfg.status.items.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                if ui.toggle(&mut item.done, "").changed() {
                    *changed = true;
                }
                // Right-to-left so the button claims its own width first and
                // the field takes exactly what is left. Subtracting a guessed
                // button width instead is what pushed it off the panel.
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.button("Remove").clicked() {
                        remove = Some(i);
                    }
                    if ui
                        .add(
                            egui::TextEdit::singleline(&mut item.text)
                                .desired_width(ui.available_width())
                                .hint_text("Something else today"),
                        )
                        .changed()
                    {
                        *changed = true;
                    }
                });
            });
            ui.add_space(4.0);
        }

        if let Some(i) = remove {
            cfg.status.items.remove(i);
            *changed = true;
        }

        ui.add_space(6.0);
        // Past about six the list stops fitting the panel and starts spilling
        // into a "+N more" line, which is a worse read than a shorter list.
        if ui
            .add_enabled(cfg.status.items.len() < 8, egui::Button::new("Add item"))
            .clicked()
        {
            cfg.status.items.push(StatusItem::new(""));
            *changed = true;
        }
    }

    fn sec_wallpaper(
        ui: &mut egui::Ui,
        cache: &mut wallpaper::PreviewCache,
        cfg: &mut AppConfig,
        changed: &mut bool,
    ) {
        let panel_w = if cfg.wallpaper.panel_width > 0 {
            cfg.wallpaper.panel_width
        } else {
            cfg.notch.expanded_width
        } as f32;
        let panel_h = if cfg.wallpaper.panel_height > 0 {
            cfg.wallpaper.panel_height
        } else {
            cfg.notch.expanded_height
        } as f32;

        Self::row_stacked(
            ui,
            "Image",
            Some(
                "PNG, JPG, BMP, GIF or WebP. Drag the preview to choose which part the open \
                 notch shows.",
            ),
            |ui| {
                let wp = &mut cfg.wallpaper;
                let has_image = wp.has_image();
                ui.horizontal(|ui| {
                    let label = if has_image {
                        "Change image..."
                    } else {
                        "Browse for an image..."
                    };
                    let browse = egui::Button::new(RichText::new(label).color(theme::on_accent()))
                        .fill(theme::accent());
                    if ui.add(browse).clicked() {
                        if let Some(picked) = filedlg::pick_image(&wp.path, "Choose a wallpaper") {
                            wp.path = picked;
                            // A new picture invalidates the old framing.
                            wp.focus_x = 0.5;
                            wp.focus_y = 0.5;
                            wp.zoom = 1.0;
                            *changed = true;
                        }
                    }
                    if has_image {
                        if ui.button("Remove").clicked() {
                            wp.path.clear();
                            *changed = true;
                        }
                        let name = std::path::Path::new(&wp.path)
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_else(|| wp.path.clone());
                        ui.add_space(4.0);
                        ui.add(
                            egui::Label::new(
                                RichText::new(name)
                                    .font(egui::FontId::monospace(11.0))
                                    .color(theme::text_tertiary()),
                            )
                            .truncate(),
                        )
                        .on_hover_text(wp.path.as_str());
                    }
                });

                ui.add_space(12.0);
                if wallpaper::preview(
                    ui,
                    cache,
                    wp,
                    panel_w / panel_h.max(1.0),
                    "No image yet. Browse for one to see it framed here.",
                ) {
                    *changed = true;
                }

                if has_image {
                    ui.add_space(10.0);
                    *changed |= Self::slider_line(
                        ui,
                        "Zoom",
                        egui::Slider::new(&mut wp.zoom, 1.0..=4.0)
                            .fixed_decimals(2)
                            .suffix("x"),
                    );
                    ui.horizontal(|ui| {
                        ui.add_space(SLIDER_LABEL_WIDTH);
                        if ui.small_button("Reset framing").clicked() {
                            wp.focus_x = 0.5;
                            wp.focus_y = 0.5;
                            wp.zoom = 1.0;
                            *changed = true;
                        }
                    });
                }
            },
        );

        Self::row_stacked(
            ui,
            "Panel Size",
            Some("How big the notch opens on this slide. \"auto\" follows the panel default."),
            |ui| {
                Self::size_pair(
                    ui,
                    &mut cfg.wallpaper.panel_width,
                    &mut cfg.wallpaper.panel_height,
                    2200,
                    900,
                    changed,
                );
            },
        );

        Self::row_stacked(
            ui,
            "Caption",
            Some("Drawn over the photo, in the open panel and in the collapsed pill."),
            |ui| {
                if ui
                    .add(
                        egui::TextEdit::singleline(&mut cfg.wallpaper.caption)
                            .desired_width(f32::INFINITY)
                            .hint_text("Why this is on your screen"),
                    )
                    .changed()
                {
                    *changed = true;
                }
            },
        );

        if !cfg.notch.slides.contains(&SlideKind::Wallpaper) {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("This slide is not in the deck.")
                        .size(11.0)
                        .color(theme::text_secondary()),
                );
                if ui.small_button("Add it").clicked() {
                    cfg.notch.slides.push(SlideKind::Wallpaper);
                    *changed = true;
                }
            });
        }
    }

    /// A width/height pair of override fields, with the "auto" formatting and
    /// the inherit button they always come with.
    fn size_pair(
        ui: &mut egui::Ui,
        width: &mut u32,
        height: &mut u32,
        max_w: u32,
        max_h: u32,
        changed: &mut bool,
    ) {
        *changed |= Self::slider_line(
            ui,
            "Width",
            egui::Slider::new(width, 0..=max_w).custom_formatter(|v, _| Self::size_text(v)),
        );
        *changed |= Self::slider_line(
            ui,
            "Height",
            egui::Slider::new(height, 0..=max_h).custom_formatter(|v, _| Self::size_text(v)),
        );
        ui.horizontal(|ui| {
            ui.add_space(SLIDER_LABEL_WIDTH);
            if ui.small_button("Match notch").clicked() {
                *width = 0;
                *height = 0;
                *changed = true;
            }
        });
    }

    fn color_row(ui: &mut egui::Ui, label: &str, value: &mut [f32; 4], changed: &mut bool) {
        Self::row_inline(ui, label, |ui| {
            if color_picker::color_picker_button(ui, label, value).changed() {
                *changed = true;
            }
        });
    }

    fn size_text(value: f64) -> String {
        if value <= 0.0 {
            "auto".to_string()
        } else {
            format!("{}", value.round() as i64)
        }
    }
}

/// What a click on the title bar's buttons asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TitleAction {
    None,
    Hide,
    Minimize,
    /// The pin was toggled; the new state is already in the config.
    Pin,
}

impl eframe::App for SettingsApp {
    /// The window is transparent so the blur behind it shows; every pixel of
    /// colour comes from the panels' translucent fills.
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0; 4]
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if SETTINGS_HWND.load(std::sync::atomic::Ordering::Relaxed) == 0 {
            unsafe {
                let _ = find_settings_hwnd();
            }
        }
        // The blur is asked for once, as soon as there is a window to blur.
        if self.blurred.is_none() {
            if let Some(hwnd) = unsafe { find_settings_hwnd() } {
                self.blurred = Some(glass::apply(hwnd));
            }
        }

        if ctx.input(|i| i.viewport().close_requested()) {
            // Closing puts Venu back in the tray rather than ending it.
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            self.on_screen = false;
        }

        if crate::tray::SHOW_REQUESTED.swap(false, std::sync::atomic::Ordering::SeqCst) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            self.on_screen = true;
            ctx.request_repaint();
        }

        let mut config_changed = false;
        let mut cfg_guard = self.config.write();

        // -- palette -----------------------------------------------------
        let palette = theme::Palette::glass(
            cfg_guard.settings_window.glass,
            self.blurred.unwrap_or(true),
            cfg_guard.settings_window.accent,
        );
        theme::set(palette);
        if self.palette != Some(palette) {
            theme::apply_style(ctx, palette);
            self.palette = Some(palette);
        }

        // -- keyboard ----------------------------------------------------
        let search_id = egui::Id::new("settings_search");
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::F)) {
            ctx.memory_mut(|m| m.request_focus(search_id));
        }
        if !self.query.is_empty() && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.query.clear();
            ctx.memory_mut(|m| m.surrender_focus(search_id));
        }

        // -- transition --------------------------------------------------
        let dt = ctx.input(|i| i.stable_dt).clamp(0.0, 0.1);
        if self.slide < 1.0 {
            self.slide = (self.slide + dt / 0.22).min(1.0);
            ctx.request_repaint();
        }
        let travel = ease_out(self.slide);
        let before = (self.active, self.tabs[self.active]);

        egui::SidePanel::left("nav")
            .exact_width(SIDEBAR_WIDTH)
            .resizable(false)
            .show_separator_line(false)
            .frame(Frame::none().fill(theme::sidebar()).inner_margin(Margin {
                left: 12.0,
                right: 12.0,
                top: 12.0,
                bottom: 10.0,
            }))
            .show(ctx, |ui| {
                Self::draw_brand(ui, self.logo.as_ref());
                ui.add_space(10.0);
                Self::draw_search(ui, &mut self.query, search_id);
                ui.add_space(14.0);

                ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
                    ui.horizontal(|ui| {
                        ui.add_space(12.0);
                        ui.label(
                            RichText::new(concat!("Venu ", env!("CARGO_PKG_VERSION")))
                                .font(egui::FontId::monospace(10.0))
                                .color(theme::text_tertiary()),
                        );
                    });
                    ui.add_space(4.0);
                    ui.with_layout(Layout::top_down(Align::Min), |ui| {
                        // The rail scrolls like the page beside it does, so
                        // a long list of results is never out of reach.
                        egui::ScrollArea::vertical()
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                ui.set_min_width(ui.available_width());
                                if self.query.trim().is_empty() {
                                    if let Some(page) = Self::draw_nav(ui, self.active) {
                                        self.active = page;
                                    }
                                } else if let Some((page, tab)) =
                                    Self::draw_results(ui, &self.query)
                                {
                                    self.active = page;
                                    self.tabs[page] = tab;
                                    self.query.clear();
                                }
                            });
                    });
                });
            });

        // The preview strip only means anything on the edge-marquee pages, so
        // it is only there for them — and it grows and shrinks rather than
        // appearing, which keeps the content above it from jumping.
        let wants_preview = PAGES[self.active].title == "Edge Marquee";
        let strip =
            ctx.animate_bool_with_time(egui::Id::new("preview_strip"), wants_preview, 0.18) * 52.0;
        if strip > 0.5 {
            egui::TopBottomPanel::bottom("preview")
                .exact_height(strip)
                .show_separator_line(false)
                .frame(
                    Frame::none()
                        .fill(theme::bg())
                        .inner_margin(Margin::symmetric(28.0, 8.0)),
                )
                .show(ctx, |ui| {
                    ui.set_clip_rect(ui.max_rect());
                    let rect = ui.max_rect();
                    ui.painter().hline(
                        rect.x_range(),
                        rect.top() - 8.0,
                        Stroke::new(1.0, theme::divider()),
                    );
                    ui.add_space(4.0);
                    Self::draw_preview_bar(ui, &cfg_guard);
                });
        }

        egui::CentralPanel::default()
            .frame(Frame::none().fill(theme::bg()).inner_margin(Margin {
                left: 28.0,
                right: 12.0,
                top: 8.0,
                bottom: 0.0,
            }))
            .show(ctx, |ui| {
                let page = &PAGES[self.active];

                let mut pinned = cfg_guard.settings_window.pinned;
                match Self::draw_title_bar(ui, page, &mut pinned) {
                    TitleAction::None => {}
                    TitleAction::Hide => {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
                        self.on_screen = false;
                    }
                    TitleAction::Minimize => {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                    }
                    TitleAction::Pin => {
                        cfg_guard.settings_window.pinned = pinned;
                        ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(if pinned {
                            egui::WindowLevel::AlwaysOnTop
                        } else {
                            egui::WindowLevel::Normal
                        }));
                        config_changed = true;
                    }
                }

                ui.label(
                    RichText::new(page.blurb)
                        .size(11.5)
                        .color(theme::text_secondary()),
                );
                ui.add_space(14.0);

                if page.tabs.len() > 1 {
                    if let Some(tab) =
                        Self::draw_tabs(ui, self.active, page.tabs, self.tabs[self.active])
                    {
                        self.tabs[self.active] = tab;
                    }
                    ui.add_space(6.0);
                }

                // Content slides in by moving its own margins. Doing it this
                // way rather than by translating a layer keeps every widget's
                // interaction rect where it is painted.
                let shift = self.slide_from * (1.0 - travel);
                let tab = self.tabs[self.active];
                Frame::none()
                    .inner_margin(Margin {
                        left: 16.0 + shift.x * 16.0,
                        right: 16.0 - shift.x * 16.0,
                        top: 12.0 + shift.y * 14.0,
                        bottom: 0.0,
                    })
                    .outer_margin(Margin {
                        left: -16.0,
                        right: 0.0,
                        top: 0.0,
                        bottom: 0.0,
                    })
                    .show(ui, |ui| {
                        egui::ScrollArea::vertical()
                            .id_salt(("page", self.active, tab))
                            // Without this the content column shrinks to its
                            // widest widget, so right-aligned controls drift
                            // and rows jump as tabs change.
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                ui.multiply_opacity(travel);
                                // Room for the scroll bar, so it never sits
                                // on top of a right-aligned control.
                                ui.set_max_width(ui.available_width() - 14.0);

                                let mut cx = PageCtx {
                                    cfg: &mut cfg_guard,
                                    changed: &mut config_changed,
                                    temp_text: &mut self.temp_text,
                                    preview: &mut self.preview,
                                };
                                (page.tabs[tab].draw)(ui, &mut cx);
                                ui.add_space(28.0);
                            });
                    });
            });

        let after = (self.active, self.tabs[self.active]);
        if after != before {
            self.slide = 0.0;
            self.slide_from = if after.0 != before.0 {
                Vec2::new(0.0, 1.0)
            } else if after.1 > before.1 {
                Vec2::new(1.0, 0.0)
            } else {
                Vec2::new(-1.0, 0.0)
            };
        }

        // A hairline round the whole window and down the rail's edge. Glass
        // over glass has no edge of its own, and without these the window
        // dissolves into whatever is behind it.
        let frame = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("window_frame"),
        ));
        let screen = ctx.screen_rect();
        frame.vline(
            screen.left() + SIDEBAR_WIDTH,
            screen.y_range(),
            Stroke::new(1.0, theme::divider()),
        );
        frame.rect_stroke(
            screen.shrink(0.5),
            Rounding::same(WINDOW_ROUNDING),
            Stroke::new(1.0, Color32::from_white_alpha(30)),
        );

        if config_changed {
            cfg_guard.save();
        }

        // Nothing on this window animates on a clock of its own: egui asks for
        // its own frames while a transition is running, and the tray wakes the
        // context directly when it wants the window back. What is left is the
        // config the notch shares with it — an inline focus edit or the
        // click-through chord can change it from the other thread — so while
        // the window is on screen it re-reads that a few times a second, and
        // while it is in the tray it does not run at all.
        let minimized = ctx.input(|i| i.viewport().minimized.unwrap_or(false));
        let page = &PAGES[self.active];
        if self.on_screen && !minimized && page.tabs[self.tabs[self.active]].title == "Stats" {
            crate::stats::request_sampling();
        }
        if self.on_screen && !minimized {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        } else if SETTINGS_HWND.load(std::sync::atomic::Ordering::Relaxed) == 0 {
            // The window handle is found by walking this process's windows,
            // which only works once one exists. Keep looking, slowly, until it
            // does — the tray needs it to restore the window.
            ctx.request_repaint_after(std::time::Duration::from_secs(1));
        }
    }
}

fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let f = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_premultiplied(
        f(a.r(), b.r()),
        f(a.g(), b.g()),
        f(a.b(), b.b()),
        f(a.a(), b.a()),
    )
}

/// Small line glyphs for the window's own controls.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Glyph {
    Search,
    Close,
    Minimize,
    Pin,
}

fn paint_glyph(painter: &egui::Painter, glyph: Glyph, center: egui::Pos2, color: Color32) {
    let stroke = Stroke::new(1.4, color);
    let p = |x: f32, y: f32| center + Vec2::new(x, y);
    match glyph {
        Glyph::Search => {
            painter.circle_stroke(p(-1.0, -1.0), 4.2, stroke);
            painter.line_segment([p(2.2, 2.2), p(5.0, 5.0)], stroke);
        }
        Glyph::Close => {
            painter.line_segment([p(-4.0, -4.0), p(4.0, 4.0)], stroke);
            painter.line_segment([p(4.0, -4.0), p(-4.0, 4.0)], stroke);
        }
        Glyph::Minimize => {
            painter.line_segment([p(-4.5, 0.5), p(4.5, 0.5)], stroke);
        }
        Glyph::Pin => {
            // A push pin seen from the side: head, collar, needle.
            painter.rect_stroke(
                Rect::from_min_max(p(-2.6, -5.5), p(2.6, -0.8)),
                Rounding::same(1.2),
                stroke,
            );
            painter.line_segment([p(-4.6, -0.8), p(4.6, -0.8)], stroke);
            painter.line_segment([p(0.0, -0.8), p(0.0, 5.5)], stroke);
        }
    }
}

/// The rail's page glyphs, drawn as strokes so they follow the palette.
fn paint_icon(painter: &egui::Painter, icon: Icon, center: egui::Pos2, color: Color32) {
    let stroke = Stroke::new(1.35, color);
    let (cx, cy) = (center.x, center.y);
    let p = egui::pos2;

    match icon {
        Icon::Notch => {
            // A capsule hanging from a bezel line.
            painter.line_segment([p(cx - 7.0, cy - 4.5), p(cx + 7.0, cy - 4.5)], stroke);
            let pill = Rect::from_center_size(p(cx, cy), egui::vec2(11.0, 6.5));
            painter.rect_stroke(pill, Rounding::same(3.2), stroke);
        }
        Icon::Palette => {
            painter.circle_stroke(center, 5.5, stroke);
            painter.circle_filled(p(cx - 2.2, cy - 1.8), 1.1, color);
            painter.circle_filled(p(cx + 2.2, cy - 1.8), 1.1, color);
            painter.circle_filled(p(cx, cy + 2.2), 1.1, color);
        }
        Icon::Layers => {
            let r1 = Rect::from_center_size(center + egui::vec2(-1.5, -1.5), egui::vec2(9.5, 7.5));
            let r2 = Rect::from_center_size(center + egui::vec2(1.5, 1.5), egui::vec2(9.5, 7.5));
            painter.rect_stroke(r1, Rounding::same(2.0), stroke);
            painter.rect_stroke(r2, Rounding::same(2.0), stroke);
        }
        Icon::Bell => {
            painter.line_segment([p(cx - 4.6, cy + 2.6), p(cx + 4.6, cy + 2.6)], stroke);
            painter.line_segment([p(cx - 3.4, cy + 2.6), p(cx - 2.6, cy - 2.4)], stroke);
            painter.line_segment([p(cx + 3.4, cy + 2.6), p(cx + 2.6, cy - 2.4)], stroke);
            painter.line_segment([p(cx - 2.6, cy - 2.4), p(cx + 2.6, cy - 2.4)], stroke);
            painter.circle_stroke(p(cx, cy - 3.9), 0.9, stroke);
            painter.circle_filled(p(cx, cy + 4.3), 1.1, color);
        }
        Icon::Monitor => {
            let screen =
                Rect::from_center_size(center + egui::vec2(0.0, -1.2), egui::vec2(13.0, 9.0));
            painter.rect_stroke(screen, Rounding::same(2.0), stroke);
            painter.line_segment([p(cx, cy + 3.3), p(cx, cy + 5.2)], stroke);
            painter.line_segment([p(cx - 2.8, cy + 5.2), p(cx + 2.8, cy + 5.2)], stroke);
        }
        Icon::Bolt => {
            let pts = [
                p(cx + 1.6, cy - 5.4),
                p(cx - 3.2, cy + 0.6),
                p(cx - 0.4, cy + 0.6),
                p(cx - 1.6, cy + 5.4),
                p(cx + 3.2, cy - 0.7),
                p(cx + 0.4, cy - 0.7),
            ];
            for i in 0..pts.len() {
                painter.line_segment([pts[i], pts[(i + 1) % pts.len()]], stroke);
            }
        }
        Icon::Gear => {
            painter.circle_stroke(center, 3.8, stroke);
            painter.circle_filled(center, 1.2, color);
            for i in 0..6 {
                let angle = (i as f32) * (std::f32::consts::PI / 3.0);
                let p1 = center + egui::vec2(angle.cos() * 3.8, angle.sin() * 3.8);
                let p2 = center + egui::vec2(angle.cos() * 5.8, angle.sin() * 5.8);
                painter.line_segment([p1, p2], stroke);
            }
        }
    }
}
