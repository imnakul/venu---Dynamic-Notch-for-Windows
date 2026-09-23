use eframe::egui::{
    self, Align, Align2, Color32, Frame, Layout, Margin, Rect, RichText, Rounding, Sense, Stroke,
    Vec2,
};
use parking_lot::RwLock;
use std::sync::Arc;

use crate::config::{
    AppConfig, FlashAnim, FlashBackground, FlashContent, FlashLayout, NotchAlign, NotchTheme,
    SlideKind, StatusItem, UiTheme,
};

mod color_picker;
mod filedlg;
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
    let mut fonts = egui::FontDefinitions::default();

    fonts.font_data.insert(
        "PlusJakartaSans".to_owned(),
        egui::FontData::from_static(include_bytes!("../../PlusJakartaSans.ttf")),
    );

    fonts
        .families
        .entry(egui::FontFamily::Proportional)
        .or_default()
        .insert(0, "PlusJakartaSans".to_owned());

    fonts
        .families
        .entry(egui::FontFamily::Monospace)
        .or_default()
        .insert(0, "PlusJakartaSans".to_owned());

    ctx.set_fonts(fonts);
}

/// The things this app actually is, from the user's point of view: the
/// notch, the border marquee, the flash screen, and the app itself. Every
/// page belongs to one.
///
/// Grouping is the whole point of the rail. A flat list of a dozen tabs makes
/// the reader scan; short lists make them recognise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Group {
    Notch,
    Marquee,
    Flash,
    App,
}

impl Group {
    fn label(self) -> &'static str {
        match self {
            Group::Notch => "NOTCH",
            Group::Marquee => "EDGE MARQUEE",
            Group::Flash => "FLASH SCREEN",
            Group::App => "APP",
        }
    }
}

/// Everything the shell needs to know about one page of settings.
///
/// This is the scalability story: a new feature is one entry in [`PAGES`] and
/// one function. Nothing else in this file has to learn about it — not the
/// rail, not the header, not the transition, not the scroll area.
struct Page {
    group: Group,
    /// Shown in the rail and as the page's own heading.
    title: &'static str,
    /// One line under the heading saying what this page is for. Worth writing
    /// properly: it is the only chance to explain a setting before the user
    /// has to guess from its name.
    blurb: &'static str,
    draw: fn(&mut egui::Ui, &mut PageCtx<'_>),
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
        title: "Overview",
        blurb: "A pill fused to the top of your screen. Hover it and it springs open; scroll \
                inside to move between slides; it collapses onto whichever slide you left it on.",
        draw: SettingsApp::page_notch_overview,
    },
    Page {
        group: Group::Notch,
        title: "Slides",
        blurb: "Which faces the notch carries, and the order the wheel walks through them.",
        draw: SettingsApp::page_notch_slides,
    },
    Page {
        group: Group::Notch,
        title: "Status",
        blurb: "What you are working on right now. One line, plus a short list — not a notes \
                app and not a backlog.",
        draw: SettingsApp::page_notch_status,
    },
    Page {
        group: Group::Notch,
        title: "Wallpaper",
        blurb: "One image you want put back in front of you now and then. Drag the preview to \
                choose which part of it the panel shows.",
        draw: SettingsApp::page_notch_wallpaper,
    },
    Page {
        group: Group::Notch,
        title: "Moving Text",
        blurb: "How the notch sizes itself while it is showing the moving text. Everything here \
                is an override — leave it on auto and the slide follows the notch's own settings.",
        draw: SettingsApp::page_notch_marquee,
    },
    Page {
        group: Group::Notch,
        title: "Size & Place",
        blurb: "How large the notch is when shut and when open, and where on which screen it \
                sits.",
        draw: SettingsApp::page_notch_layout,
    },
    Page {
        group: Group::Notch,
        title: "Appearance",
        blurb: "Finish, accent and type for the notch itself.",
        draw: SettingsApp::page_notch_look,
    },
    Page {
        group: Group::Notch,
        title: "Notifications",
        blurb: "Dynamic Notification Center. Filter allowed apps like Antigravity, Codex & Claude, and customize live alert toasts.",
        draw: SettingsApp::page_notch_notifications,
    },
    Page {
        group: Group::Marquee,
        title: "Overview",
        blurb: "Strips of scrolling text pinned to the edges of the screen. Separate from the \
                notch, and switched on and off separately.",
        draw: SettingsApp::page_edge_overview,
    },
    Page {
        group: Group::Marquee,
        title: "Message",
        blurb: "The words that scroll, and how far apart the repeats sit.",
        draw: SettingsApp::page_edge_message,
    },
    Page {
        group: Group::Marquee,
        title: "Appearance",
        blurb: "Type and colour for the strips.",
        draw: SettingsApp::page_edge_look,
    },
    Page {
        group: Group::Marquee,
        title: "Motion",
        blurb: "How fast the text travels, which way, and how the strips behave against other \
                windows.",
        draw: SettingsApp::page_edge_motion,
    },
    Page {
        group: Group::Flash,
        title: "FlashScreen",
        blurb: "Every few minutes, for a few seconds, a line of text or an image appears in the \
                middle of the screen — then leaves on its own. Nothing to dismiss, nothing to \
                forget.",
        draw: SettingsApp::page_flashscreen,
    },
    Page {
        group: Group::App,
        title: "Preferences",
        blurb: "The settings window itself, and where your settings are kept.",
        draw: SettingsApp::page_app_prefs,
    },
];

pub struct SettingsApp {
    config: Arc<RwLock<AppConfig>>,
    temp_text: String,
    /// Index into [`PAGES`].
    active: usize,
    /// Runs 0 to 1 while a page slides in. Held at 1 the rest of the time, so
    /// a settled window does no animation work at all.
    slide: f32,
    /// Which way the incoming page travels: down the rail is +1, up is -1.
    /// Direction is what makes the movement mean something rather than just
    /// being decoration.
    slide_dir: f32,
    /// The palette in force last frame. The style is only rebuilt when this
    /// changes, which is during a theme cross-fade and never otherwise.
    palette: Option<theme::Palette>,
    /// Decoded wallpaper, kept between frames so the preview does not re-read
    /// the file sixty times a second.
    preview: wallpaper::PreviewCache,
    /// Whether the window is on screen. Venu is a tray app: most of the time
    /// this is false, and a hidden window has nothing worth redrawing.
    on_screen: bool,
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
        style.visuals.window_rounding = Rounding::same(10.0);
        style.spacing.item_spacing = Vec2::new(8.0, 8.0);
        style.spacing.button_padding = Vec2::new(12.0, 6.0);
        cc.egui_ctx.set_style(style);

        let current_text = config.read().text.clone();
        Self {
            config,
            temp_text: current_text,
            active: 0,
            slide: 1.0,
            slide_dir: 1.0,
            palette: None,
            preview: wallpaper::PreviewCache::new(),
            on_screen,
        }
    }

    fn section_title(ui: &mut egui::Ui, title: &str) {
        ui.label(
            RichText::new(title)
                .size(11.0)
                .color(theme::text_tertiary())
                .strong(),
        );
        ui.add_space(10.0);
    }

    fn divider(ui: &mut egui::Ui) {
        ui.add_space(4.0);
        ui.separator();
        ui.add_space(14.0);
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
        add_contents(ui);
        ui.add_space(18.0);
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

    /// The navigation rail.
    ///
    /// The selected row is painted as one pill that *slides* between rows
    /// rather than being redrawn in place. That is why the pill's shape is
    /// reserved before the loop and filled in after: its resting place is not
    /// known until every row has been laid out, but it has to be painted
    /// underneath the labels.
    fn draw_nav(ui: &mut egui::Ui, active: &mut usize) {
        let pill = ui.painter().add(egui::Shape::Noop);
        let bar = ui.painter().add(egui::Shape::Noop);
        let mut target: Option<Rect> = None;

        let mut group: Option<Group> = None;
        for (index, page) in PAGES.iter().enumerate() {
            if group != Some(page.group) {
                ui.add_space(if group.is_some() { 16.0 } else { 2.0 });
                ui.label(
                    RichText::new(page.group.label())
                        .size(9.5)
                        .color(theme::text_tertiary())
                        .strong(),
                );
                ui.add_space(7.0);
                group = Some(page.group);
            }

            let selected = *active == index;
            let width = ui.available_width();
            let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 30.0), Sense::click());

            if selected {
                target = Some(rect);
            } else {
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
                        theme::surface_hover().gamma_multiply(hover),
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

            let item_color = if selected {
                theme::accent()
            } else {
                theme::text_secondary()
            };

            let icon_pos = rect.left_center() + Vec2::new(14.0 + sink, 0.0);
            paint_sidebar_hugeicon(ui.painter(), page.group, page.title, icon_pos, item_color);

            ui.painter().text(
                rect.left_center() + Vec2::new(30.0 + sink, 0.0),
                Align2::LEFT_CENTER,
                page.title,
                egui::FontId::proportional(13.0),
                item_color,
            );

            if response.clicked() {
                *active = index;
            }

            ui.add_space(1.0);
        }

        if let Some(target) = target {
            // Only the vertical position is animated: the rows are all the
            // same width and height, so nothing else has anywhere to travel.
            let y = ui
                .ctx()
                .animate_value_with_time(egui::Id::new("nav_pill"), target.top(), 0.18);
            let moved = Rect::from_min_size(egui::pos2(target.left(), y), target.size());

            ui.painter().set(
                pill,
                egui::Shape::rect_filled(moved, Rounding::same(8.0), theme::accent_wash()),
            );
            ui.painter().set(
                bar,
                egui::Shape::rect_filled(
                    Rect::from_min_size(
                        moved.left_top() + Vec2::new(0.0, 7.0),
                        Vec2::new(2.5, moved.height() - 14.0),
                    ),
                    Rounding::same(2.0),
                    theme::accent(),
                ),
            );
        }
    }

    /// The heading every page opens with. Uniform on purpose: the reader
    /// should be able to tell where they are without reading, by shape alone.
    fn page_header(ui: &mut egui::Ui, page: &Page) {
        ui.label(
            RichText::new(page.title)
                .size(19.0)
                .color(theme::text_primary())
                .strong(),
        );
        ui.add_space(5.0);
        ui.label(
            RichText::new(page.blurb)
                .size(11.5)
                .color(theme::text_secondary()),
        );
        ui.add_space(6.0);
        let rect = ui.max_rect();
        let (line, _) = ui.allocate_exact_size(Vec2::new(rect.width(), 1.0), Sense::hover());
        ui.painter().hline(
            line.x_range(),
            line.center().y,
            Stroke::new(1.0, theme::divider()),
        );
        ui.add_space(18.0);
    }

    // -- pages -----------------------------------------------------------
    //
    // Each one is a thin arrangement of the sections below it. Keeping the
    // sections separate from the pages is what lets a section be moved to a
    // different page without touching its body.

    fn page_notch_overview(ui: &mut egui::Ui, cx: &mut PageCtx<'_>) {
        Self::sec_notch_basics(ui, cx.cfg, cx.changed);
    }

    fn page_notch_slides(ui: &mut egui::Ui, cx: &mut PageCtx<'_>) {
        Self::sec_notch_deck(ui, cx.cfg, cx.changed);
    }

    fn page_notch_status(ui: &mut egui::Ui, cx: &mut PageCtx<'_>) {
        Self::sec_status_now(ui, cx.cfg, cx.changed);
        Self::sec_status_deck(ui, cx.cfg, cx.changed);
    }

    fn page_notch_wallpaper(ui: &mut egui::Ui, cx: &mut PageCtx<'_>) {
        Self::sec_wallpaper(ui, cx.preview, cx.cfg, cx.changed);
    }

    fn page_notch_marquee(ui: &mut egui::Ui, cx: &mut PageCtx<'_>) {
        Self::sec_notch_marquee(ui, cx.cfg, cx.changed);
    }

    fn page_notch_layout(ui: &mut egui::Ui, cx: &mut PageCtx<'_>) {
        Self::sec_notch_size(ui, cx.cfg, cx.changed);
        Self::divider(ui);
        Self::sec_notch_place(ui, cx.cfg, cx.changed);
    }

    fn page_notch_look(ui: &mut egui::Ui, cx: &mut PageCtx<'_>) {
        Self::sec_notch_look(ui, cx.cfg, cx.changed);
    }

    fn page_notch_notifications(ui: &mut egui::Ui, cx: &mut PageCtx<'_>) {
        Self::sec_notch_notifications(ui, cx.cfg, cx.changed);
    }

    fn page_edge_overview(ui: &mut egui::Ui, cx: &mut PageCtx<'_>) {
        Self::sec_edge_master(ui, cx.cfg, cx.changed);
        Self::divider(ui);
        Self::sec_edge_edges(ui, cx.cfg, cx.changed);
        Self::divider(ui);
        Self::sec_edge_margins(ui, cx.cfg, cx.changed);
    }

    fn page_edge_message(ui: &mut egui::Ui, cx: &mut PageCtx<'_>) {
        Self::sec_marquee_message(ui, cx.temp_text, cx.cfg, cx.changed);
    }

    fn page_edge_look(ui: &mut egui::Ui, cx: &mut PageCtx<'_>) {
        Self::sec_edge_type(ui, cx.cfg, cx.changed);
        Self::divider(ui);
        Self::sec_edge_color(ui, cx.cfg, cx.changed);
    }

    fn page_edge_motion(ui: &mut egui::Ui, cx: &mut PageCtx<'_>) {
        Self::sec_edge_motion(ui, cx.cfg, cx.changed);
        Self::divider(ui);
        Self::sec_edge_window(ui, cx.cfg, cx.changed);
    }

    fn page_flashscreen(ui: &mut egui::Ui, cx: &mut PageCtx<'_>) {
        Self::sec_flash_master(ui, cx.cfg, cx.changed);
        Self::divider(ui);
        Self::sec_flash_schedule(ui, cx.cfg, cx.changed);
        Self::divider(ui);
        Self::sec_flash_content(ui, cx.cfg, cx.changed);
        if cx.cfg.flash.content != FlashContent::Image {
            Self::divider(ui);
            Self::sec_flash_type(ui, cx.cfg, cx.changed);
        }
        Self::divider(ui);
        Self::sec_flash_backdrop(ui, cx.cfg, cx.changed);
        Self::divider(ui);
        Self::sec_flash_anim(ui, cx.cfg, cx.changed);
        Self::divider(ui);
        Self::sec_flash_behavior(ui, cx.cfg, cx.changed);
    }

    fn page_app_prefs(ui: &mut egui::Ui, cx: &mut PageCtx<'_>) {
        Self::section_title(ui, "APPEARANCE");

        Self::row_stacked(
            ui,
            "Window theme",
            Some("System follows whatever Windows is set to."),
            |ui| {
                ui.horizontal(|ui| {
                    for option in UiTheme::ALL {
                        let picked = cx.cfg.ui_theme == option;
                        if Self::segment(ui, option.label(), picked).clicked() {
                            cx.cfg.ui_theme = option;
                            *cx.changed = true;
                        }
                    }
                });
            },
        );

        Self::divider(ui);
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
                    .checkbox(&mut enabled, "")
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

        Self::divider(ui);
        Self::section_title(ui, "YOUR SETTINGS");

        ui.label(
            RichText::new("Everything on these pages is saved here, as soon as you change it.")
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

            Frame::default()
                .fill(bg_color)
                .rounding(Rounding::same(6.0))
                .inner_margin(Margin::symmetric(14.0, 6.0))
                .show(ui, |ui| {
                    ui.set_clip_rect(ui.max_rect());
                    let spacing_str = " ".repeat(cfg.phrase_spacing as usize);
                    let display_text =
                        format!("{}{}{}{}", cfg.text, spacing_str, cfg.text, spacing_str);

                    let mut text_rt = RichText::new(&display_text)
                        .size(cfg.font.size.clamp(13.0, 22.0))
                        .color(fg_color);
                    if cfg.font.bold {
                        text_rt = text_rt.strong();
                    }
                    if cfg.font.italic {
                        text_rt = text_rt.italics();
                    }
                    ui.label(text_rt);
                });
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
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Panel")
                            .size(12.0)
                            .color(theme::text_secondary()),
                    );
                    if ui
                        .add(
                            egui::DragValue::new(&mut cfg.marquee.font_size)
                                .range(0.0..=120.0)
                                .speed(0.5)
                                .custom_formatter(|v, _| Self::size_text(v)),
                        )
                        .changed()
                    {
                        *changed = true;
                    }
                    ui.add_space(10.0);
                    ui.label(
                        RichText::new("Pill")
                            .size(12.0)
                            .color(theme::text_secondary()),
                    );
                    if ui
                        .add(
                            egui::DragValue::new(&mut cfg.marquee.pill_font_size)
                                .range(0.0..=64.0)
                                .speed(0.5)
                                .custom_formatter(|v, _| Self::size_text(v)),
                        )
                        .changed()
                    {
                        *changed = true;
                    }
                    ui.add_space(12.0);
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
            .checkbox(&mut cfg.overlay_enabled, "Show the edge marquee")
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
                    if ui.checkbox(&mut cfg.edges.top, "Top").changed() {
                        *changed = true;
                    }
                    if ui.checkbox(&mut cfg.edges.right, "Right").changed() {
                        *changed = true;
                    }
                    ui.end_row();

                    if ui.checkbox(&mut cfg.edges.bottom, "Bottom").changed() {
                        *changed = true;
                    }
                    if ui.checkbox(&mut cfg.edges.left, "Left").changed() {
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

        egui::Grid::new("padding_grid")
            .num_columns(4)
            .spacing([12.0, 10.0])
            .show(ui, |ui| {
                ui.label(
                    RichText::new("Top")
                        .size(12.0)
                        .color(theme::text_secondary()),
                );
                if ui
                    .add(egui::DragValue::new(&mut cfg.padding.top).range(0..=800))
                    .changed()
                {
                    *changed = true;
                }
                ui.label(
                    RichText::new("Right")
                        .size(12.0)
                        .color(theme::text_secondary()),
                );
                if ui
                    .add(egui::DragValue::new(&mut cfg.padding.right).range(0..=800))
                    .changed()
                {
                    *changed = true;
                }
                ui.end_row();

                ui.label(
                    RichText::new("Bottom")
                        .size(12.0)
                        .color(theme::text_secondary()),
                );
                if ui
                    .add(egui::DragValue::new(&mut cfg.padding.bottom).range(0..=800))
                    .changed()
                {
                    *changed = true;
                }
                ui.label(
                    RichText::new("Left")
                        .size(12.0)
                        .color(theme::text_secondary()),
                );
                if ui
                    .add(egui::DragValue::new(&mut cfg.padding.left).range(0..=800))
                    .changed()
                {
                    *changed = true;
                }
                ui.end_row();
            });
    }

    fn sec_edge_type(ui: &mut egui::Ui, cfg: &mut AppConfig, changed: &mut bool) {
        Self::section_title(ui, "TYPOGRAPHY");

        Self::row_inline(ui, "Font Family", |ui| {
            let fonts = [
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
            if ui.checkbox(&mut cfg.font.bold, "Bold").changed() {
                *changed = true;
            }
            ui.add_space(16.0);
            if ui.checkbox(&mut cfg.font.italic, "Italic").changed() {
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
            if ui.checkbox(&mut cfg.animation.reverse, "").changed() {
                *changed = true;
            }
        });
    }

    fn sec_edge_window(ui: &mut egui::Ui, cfg: &mut AppConfig, changed: &mut bool) {
        Self::section_title(ui, "OVERLAY BEHAVIOR");

        Self::row_inline(ui, "Always On Top", |ui| {
            if ui
                .checkbox(&mut cfg.always_on_top, "")
                .on_hover_text("Keep the text border above all open windows")
                .changed()
            {
                *changed = true;
            }
        });

        Self::row_inline(ui, "Click-Through Mode", |ui| {
            if ui
                .checkbox(&mut cfg.click_through, "")
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
            .checkbox(&mut cfg.flash.enabled, "Show FlashScreen reminders")
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
            if ui.checkbox(&mut cfg.flash.font.bold, "Bold").changed() {
                *changed = true;
            }
            ui.add_space(16.0);
            if ui.checkbox(&mut cfg.flash.font.italic, "Italic").changed() {
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
                        .checkbox(&mut cfg.flash.shine, "Shine sweep")
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
                .checkbox(&mut cfg.flash.always_on_top, "")
                .on_hover_text("Keep the flash above all open windows")
                .changed()
            {
                *changed = true;
            }
        });

        Self::row_inline(ui, "Click-Through Mode", |ui| {
            if ui
                .checkbox(&mut cfg.flash.click_through, "")
                .on_hover_text("Let clicks pass through the flash to whatever is underneath")
                .changed()
            {
                *changed = true;
            }
        });
    }

    fn sec_notch_basics(ui: &mut egui::Ui, cfg: &mut AppConfig, changed: &mut bool) {
        if ui
            .checkbox(&mut cfg.notch.enabled, "Show the notch")
            .changed()
        {
            *changed = true;
        }
        ui.add_space(6.0);
        if ui
            .checkbox(&mut cfg.notch.always_on_top, "Keep above other windows")
            .changed()
        {
            *changed = true;
        }
        ui.add_space(6.0);
        if ui
            .checkbox(&mut cfg.notch.click_through, "Let clicks pass through")
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
            .checkbox(
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

    fn sec_notch_notifications(ui: &mut egui::Ui, cfg: &mut AppConfig, changed: &mut bool) {
        Self::section_title(ui, "DYNAMIC NOTIFICATIONS");

        if ui
            .checkbox(
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

        Self::divider(ui);
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

        Self::divider(ui);
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

        Self::divider(ui);
        Self::section_title(ui, "PROGRAM COLORS");

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

        Self::divider(ui);
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
            cfg.notch.slides.remove(i);
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

        Self::row_stacked(ui, "Collapsed", Some("The resting pill."), |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("W").size(12.0).color(theme::text_secondary()));
                if ui
                    .add(egui::DragValue::new(&mut cfg.notch.collapsed_width).range(60..=900))
                    .changed()
                {
                    *changed = true;
                }
                ui.add_space(10.0);
                ui.label(RichText::new("H").size(12.0).color(theme::text_secondary()));
                if ui
                    .add(egui::DragValue::new(&mut cfg.notch.collapsed_height).range(18..=120))
                    .changed()
                {
                    *changed = true;
                }
            });
        });

        Self::row_stacked(ui, "Expanded", Some("The panel it opens into."), |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("W").size(12.0).color(theme::text_secondary()));
                if ui
                    .add(egui::DragValue::new(&mut cfg.notch.expanded_width).range(320..=2200))
                    .changed()
                {
                    *changed = true;
                }
                ui.add_space(10.0);
                ui.label(RichText::new("H").size(12.0).color(theme::text_secondary()));
                if ui
                    .add(egui::DragValue::new(&mut cfg.notch.expanded_height).range(120..=900))
                    .changed()
                {
                    *changed = true;
                }
            });
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
                ui.horizontal(|ui| {
                    ui.label(RichText::new("X").size(12.0).color(theme::text_secondary()));
                    if ui
                        .add(egui::DragValue::new(&mut cfg.notch.offset_x).range(-2000..=2000))
                        .changed()
                    {
                        *changed = true;
                    }
                    ui.add_space(10.0);
                    ui.label(RichText::new("Y").size(12.0).color(theme::text_secondary()));
                    if ui
                        .add(egui::DragValue::new(&mut cfg.notch.offset_y).range(0..=1200))
                        .changed()
                    {
                        *changed = true;
                    }
                });
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
            .checkbox(&mut cfg.notch.clock_24h, "24-hour clock")
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
                if ui.checkbox(&mut item.done, "").changed() {
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
        Self::row_stacked(ui, "Image", Some("PNG, JPG, BMP, GIF or WebP."), |ui| {
            // Buttons first, in right-to-left, so the path field gets exactly
            // the width that is actually left over. Subtracting a guessed
            // button width is what pushed controls off the panel before.
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui
                    .add_enabled(cfg.wallpaper.has_image(), egui::Button::new("Clear"))
                    .clicked()
                {
                    cfg.wallpaper.path.clear();
                    *changed = true;
                }
                if ui.button("Browse...").clicked() {
                    if let Some(picked) =
                        filedlg::pick_image(&cfg.wallpaper.path, "Choose a wallpaper")
                    {
                        cfg.wallpaper.path = picked;
                        // A new picture invalidates the old framing.
                        cfg.wallpaper.focus_x = 0.5;
                        cfg.wallpaper.focus_y = 0.5;
                        cfg.wallpaper.zoom = 1.0;
                        *changed = true;
                    }
                }
                if ui
                    .add(
                        egui::TextEdit::singleline(&mut cfg.wallpaper.path)
                            .desired_width(ui.available_width())
                            .hint_text("C:\\Users\\...\\reminder.jpg"),
                    )
                    .changed()
                {
                    *changed = true;
                }
            });
        });

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
            "Framing",
            Some("Drag to reposition. This is exactly what the open notch will show."),
            |ui| {
                if wallpaper::preview(
                    ui,
                    cache,
                    &mut cfg.wallpaper,
                    panel_w / panel_h.max(1.0),
                    "Choose an image to see it framed here",
                ) {
                    *changed = true;
                }

                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Zoom")
                            .size(12.0)
                            .color(theme::text_secondary()),
                    );
                    if ui
                        .add(
                            egui::Slider::new(&mut cfg.wallpaper.zoom, 1.0..=4.0)
                                .fixed_decimals(2)
                                .suffix("x"),
                        )
                        .changed()
                    {
                        *changed = true;
                    }
                    if ui.small_button("Reset").clicked() {
                        cfg.wallpaper.focus_x = 0.5;
                        cfg.wallpaper.focus_y = 0.5;
                        cfg.wallpaper.zoom = 1.0;
                        *changed = true;
                    }
                });
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
        ui.horizontal(|ui| {
            ui.label(RichText::new("W").size(12.0).color(theme::text_secondary()));
            if ui
                .add(
                    egui::DragValue::new(width)
                        .range(0..=max_w)
                        .speed(2.0)
                        .custom_formatter(|v, _| Self::size_text(v)),
                )
                .changed()
            {
                *changed = true;
            }
            ui.add_space(10.0);
            ui.label(RichText::new("H").size(12.0).color(theme::text_secondary()));
            if ui
                .add(
                    egui::DragValue::new(height)
                        .range(0..=max_h)
                        .speed(2.0)
                        .custom_formatter(|v, _| Self::size_text(v)),
                )
                .changed()
            {
                *changed = true;
            }
            ui.add_space(12.0);
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

impl eframe::App for SettingsApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if SETTINGS_HWND.load(std::sync::atomic::Ordering::Relaxed) == 0 {
            unsafe {
                let _ = find_settings_hwnd();
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

        // -- theme -------------------------------------------------------
        //
        // Switching theme cross-fades rather than snapping. A fifth of a
        // second is long enough to read as one surface changing colour and
        // short enough that nobody waits for it.
        let want_dark = theme::resolve(cfg_guard.ui_theme, ctx);
        let blend = ctx.animate_bool_with_time(egui::Id::new("ui_theme"), want_dark, 0.22);
        let palette = theme::Palette::lerp(theme::LIGHT, theme::DARK, blend);
        theme::set(palette);
        if self.palette != Some(palette) {
            theme::apply_style(ctx, palette);
            self.palette = Some(palette);
        }

        // -- page transition ---------------------------------------------
        let dt = ctx.input(|i| i.stable_dt).clamp(0.0, 0.1);
        if self.slide < 1.0 {
            self.slide = (self.slide + dt / 0.2).min(1.0);
            ctx.request_repaint();
        }
        let travel = ease_out(self.slide);

        egui::TopBottomPanel::top("header")
            .frame(
                Frame::default()
                    .fill(theme::bg())
                    .inner_margin(Margin::symmetric(24.0, 14.0)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new("Venu")
                                .size(16.0)
                                .color(theme::text_primary())
                                .strong(),
                        );
                        ui.label(
                            RichText::new("Dynamic Notch for Windows")
                                .size(11.0)
                                .color(theme::text_secondary()),
                        );
                    });

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui
                            .button(RichText::new("Hide to Tray").size(12.0))
                            .on_hover_text("Hide the settings window to the system tray")
                            .clicked()
                        {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
                        }

                        ui.add_space(6.0);

                        // The theme also lives on the Preferences page, which
                        // is its proper home. It is repeated here because it
                        // is the one setting people reach for out of habit
                        // rather than intent, and hunting for it is friction.
                        let next = match cfg_guard.ui_theme {
                            UiTheme::System => UiTheme::Light,
                            UiTheme::Light => UiTheme::Dark,
                            UiTheme::Dark => UiTheme::System,
                        };
                        if ui
                            .button(RichText::new(cfg_guard.ui_theme.label()).size(12.0))
                            .on_hover_text(format!("Switch to {}", next.label().to_lowercase()))
                            .clicked()
                        {
                            cfg_guard.ui_theme = next;
                            config_changed = true;
                        }
                    });
                });

                let rect = ui.max_rect();
                ui.painter().hline(
                    rect.x_range(),
                    rect.bottom(),
                    Stroke::new(1.0, theme::divider()),
                );
            });

        // The preview strip only means anything on the edge-marquee pages, so
        // it is only there for them — and it grows and shrinks rather than
        // appearing, which keeps the content below from jumping.
        let wants_preview = PAGES[self.active].group == Group::Marquee;
        let strip =
            ctx.animate_bool_with_time(egui::Id::new("preview_strip"), wants_preview, 0.18) * 52.0;
        if strip > 0.5 {
            egui::TopBottomPanel::bottom("preview")
                .exact_height(strip)
                .frame(
                    Frame::default()
                        .fill(theme::bg())
                        .inner_margin(Margin::symmetric(24.0, 8.0)),
                )
                .show(ctx, |ui| {
                    ui.set_clip_rect(ui.max_rect());
                    let rect = ui.max_rect();
                    ui.painter().hline(
                        rect.x_range(),
                        rect.top(),
                        Stroke::new(1.0, theme::divider()),
                    );
                    ui.add_space(4.0);
                    Self::draw_preview_bar(ui, &cfg_guard);
                });
        }

        egui::SidePanel::left("nav")
            .exact_width(186.0)
            .resizable(false)
            .frame(
                Frame::default()
                    .fill(theme::sidebar())
                    .inner_margin(Margin::symmetric(12.0, 16.0)),
            )
            .show(ctx, |ui| {
                let before = self.active;
                // The rail carries one row per page, and there are more pages
                // than fit a short window — so the rail scrolls like the page
                // beside it does, and the last group is never out of reach.
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        Self::draw_nav(ui, &mut self.active);
                    });
                if self.active != before {
                    // Direction carries meaning: pages entering from the right
                    // are further down the rail. Without it the movement is
                    // decoration; with it, it is a small map.
                    self.slide_dir = if self.active > before { 1.0 } else { -1.0 };
                    self.slide = 0.0;
                }
            });

        // Long lines are hard to read and right-aligned controls end up an
        // arm's length from their labels, so the column is capped and centred
        // however wide the window gets.
        let column = 720.0;
        let gutter = (((ctx.available_rect().width() - column) * 0.5).max(0.0) + 30.0).floor();
        // The page slides in horizontally by moving its own margins. Doing it
        // this way rather than by translating a layer keeps every widget's
        // interaction rect where it is painted.
        let shift = (1.0 - travel) * 24.0 * self.slide_dir;

        egui::CentralPanel::default()
            .frame(Frame::default().fill(theme::bg()).inner_margin(Margin {
                left: gutter + shift,
                right: (gutter - shift).max(0.0),
                top: 22.0,
                bottom: 24.0,
            }))
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    // Without this the content column shrinks to its widest
                    // widget, so right-aligned controls drift and rows jump
                    // as pages change.
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.multiply_opacity(travel);

                        let page = &PAGES[self.active];
                        Self::page_header(ui, page);

                        let mut cx = PageCtx {
                            cfg: &mut cfg_guard,
                            changed: &mut config_changed,
                            temp_text: &mut self.temp_text,
                            preview: &mut self.preview,
                        };
                        (page.draw)(ui, &mut cx);
                    });
            });

        if config_changed {
            cfg_guard.save();
        }

        // Nothing on this window animates on a clock of its own: egui asks for
        // its own frames while a cross-fade or the page slide is running, and
        // the tray wakes the context directly when it wants the window back.
        // What is left is the config the notch shares with it — an inline
        // focus edit or the click-through chord can change it from the other
        // thread — so while the window is on screen it re-reads that a few
        // times a second, and while it is in the tray it does not run at all.
        //
        // This used to ask for a frame every 50ms unconditionally, which had
        // the whole settings UI rebuilding, laying out and tessellating twenty
        // times a second for the entire time Venu was running, hidden or not.
        let minimized = ctx.input(|i| i.viewport().minimized.unwrap_or(false));
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

/// Renders authentic Hugeicons stroke vector icons for each sidebar rail item.
fn paint_sidebar_hugeicon(
    painter: &egui::Painter,
    group: Group,
    title: &str,
    center: egui::Pos2,
    color: egui::Color32,
) {
    let stroke = egui::Stroke::new(1.35, color);
    let (cx, cy) = (center.x, center.y);

    match (group, title) {
        (Group::Notch, "Overview") => {
            // Hugeicons: Notch Capsule Dashboard
            let rect = egui::Rect::from_center_size(center, egui::vec2(13.0, 7.0));
            painter.rect_stroke(rect, egui::Rounding::same(3.5), stroke);
            painter.circle_filled(center, 1.2, color);
        }
        (Group::Notch, "Slides") => {
            // Hugeicons: Layers / Stacked Cards
            let r1 =
                egui::Rect::from_center_size(center + egui::vec2(-1.5, -1.5), egui::vec2(9.5, 7.5));
            let r2 =
                egui::Rect::from_center_size(center + egui::vec2(1.5, 1.5), egui::vec2(9.5, 7.5));
            painter.rect_stroke(r1, egui::Rounding::same(2.0), stroke);
            painter.rect_stroke(r2, egui::Rounding::same(2.0), stroke);
        }
        (Group::Notch, "Status") => {
            // Hugeicons: Checklist Task
            let rect = egui::Rect::from_center_size(center, egui::vec2(12.5, 12.5));
            painter.rect_stroke(rect, egui::Rounding::same(3.0), stroke);
            painter.line_segment(
                [egui::pos2(cx - 3.2, cy), egui::pos2(cx - 1.0, cy + 2.2)],
                stroke,
            );
            painter.line_segment(
                [
                    egui::pos2(cx - 1.0, cy + 2.2),
                    egui::pos2(cx + 3.2, cy - 2.2),
                ],
                stroke,
            );
        }
        (Group::Notch, "Wallpaper") => {
            // Hugeicons: Photo Frame
            let rect = egui::Rect::from_center_size(center, egui::vec2(13.0, 11.0));
            painter.rect_stroke(rect, egui::Rounding::same(2.5), stroke);
            painter.line_segment(
                [
                    egui::pos2(cx - 4.2, cy + 2.8),
                    egui::pos2(cx - 1.0, cy - 1.0),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    egui::pos2(cx - 1.0, cy - 1.0),
                    egui::pos2(cx + 2.0, cy + 2.0),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    egui::pos2(cx + 1.0, cy + 1.0),
                    egui::pos2(cx + 4.2, cy - 1.5),
                ],
                stroke,
            );
            painter.circle_filled(egui::pos2(cx + 2.5, cy - 2.5), 1.1, color);
        }
        (Group::Notch, "Moving Text") => {
            // Hugeicons: Typographic 'T' with motion speed dashes
            painter.line_segment(
                [
                    egui::pos2(cx - 3.2, cy - 4.0),
                    egui::pos2(cx + 3.2, cy - 4.0),
                ],
                stroke,
            );
            painter.line_segment([egui::pos2(cx, cy - 4.0), egui::pos2(cx, cy + 4.0)], stroke);
            painter.line_segment(
                [
                    egui::pos2(cx - 5.5, cy + 1.2),
                    egui::pos2(cx - 2.5, cy + 1.2),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    egui::pos2(cx - 6.5, cy - 1.5),
                    egui::pos2(cx - 4.5, cy - 1.5),
                ],
                stroke,
            );
        }
        (Group::Notch, "Size & Place") => {
            // Hugeicons: Maximize Frame Corner Brackets
            let s = 5.0;
            painter.line_segment(
                [egui::pos2(cx - s, cy - s + 3.0), egui::pos2(cx - s, cy - s)],
                stroke,
            );
            painter.line_segment(
                [egui::pos2(cx - s, cy - s), egui::pos2(cx - s + 3.0, cy - s)],
                stroke,
            );
            painter.line_segment(
                [egui::pos2(cx + s, cy + s - 3.0), egui::pos2(cx + s, cy + s)],
                stroke,
            );
            painter.line_segment(
                [egui::pos2(cx + s, cy + s), egui::pos2(cx + s - 3.0, cy + s)],
                stroke,
            );
            painter.circle_filled(center, 1.2, color);
        }
        (Group::Notch, "Appearance") => {
            // Hugeicons: Paint Palette
            painter.circle_stroke(center, 5.5, stroke);
            painter.circle_filled(egui::pos2(cx - 2.2, cy - 1.8), 1.1, color);
            painter.circle_filled(egui::pos2(cx + 2.2, cy - 1.8), 1.1, color);
            painter.circle_filled(egui::pos2(cx, cy + 2.2), 1.1, color);
        }
        (Group::Notch, "Notifications") => {
            // Hugeicons: Notification Bell
            painter.line_segment(
                [
                    egui::pos2(cx - 4.2, cy + 2.5),
                    egui::pos2(cx + 4.2, cy + 2.5),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    egui::pos2(cx - 3.2, cy + 2.5),
                    egui::pos2(cx - 2.2, cy - 2.2),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    egui::pos2(cx + 3.2, cy + 2.5),
                    egui::pos2(cx + 2.2, cy - 2.2),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    egui::pos2(cx - 2.2, cy - 2.2),
                    egui::pos2(cx + 2.2, cy - 2.2),
                ],
                stroke,
            );
            painter.circle_filled(egui::pos2(cx, cy + 3.8), 1.1, color);
            painter.circle_stroke(egui::pos2(cx, cy - 3.5), 0.9, stroke);
        }
        (Group::Marquee, "Overview") => {
            // Hugeicons: Desktop Display Monitor
            let screen =
                egui::Rect::from_center_size(center + egui::vec2(0.0, -1.2), egui::vec2(13.0, 9.0));
            painter.rect_stroke(screen, egui::Rounding::same(2.0), stroke);
            painter.line_segment([egui::pos2(cx, cy + 3.3), egui::pos2(cx, cy + 5.2)], stroke);
            painter.line_segment(
                [
                    egui::pos2(cx - 2.8, cy + 5.2),
                    egui::pos2(cx + 2.8, cy + 5.2),
                ],
                stroke,
            );
        }
        (Group::Marquee, "Message") => {
            // Hugeicons: Chat Message Bubble
            let bubble =
                egui::Rect::from_center_size(center + egui::vec2(0.0, -0.8), egui::vec2(12.5, 9.5));
            painter.rect_stroke(bubble, egui::Rounding::same(2.5), stroke);
            painter.line_segment(
                [
                    egui::pos2(cx - 3.2, cy - 0.8),
                    egui::pos2(cx + 3.2, cy - 0.8),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    egui::pos2(cx - 2.8, cy + 4.0),
                    egui::pos2(cx - 1.0, cy + 5.8),
                ],
                stroke,
            );
        }
        (Group::Marquee, "Appearance") => {
            // Hugeicons: Paintbrush
            painter.line_segment(
                [
                    egui::pos2(cx - 3.8, cy + 3.8),
                    egui::pos2(cx + 3.2, cy - 3.2),
                ],
                egui::Stroke::new(1.6, color),
            );
            painter.circle_filled(egui::pos2(cx - 3.8, cy + 3.8), 1.8, color);
            painter.line_segment(
                [
                    egui::pos2(cx + 1.8, cy - 1.8),
                    egui::pos2(cx + 4.2, cy - 4.2),
                ],
                stroke,
            );
        }
        (Group::Marquee, "Motion") => {
            // Hugeicons: Activity Wave
            painter.line_segment([egui::pos2(cx - 5.5, cy), egui::pos2(cx - 2.8, cy)], stroke);
            painter.line_segment(
                [egui::pos2(cx - 2.8, cy), egui::pos2(cx - 1.0, cy - 4.0)],
                stroke,
            );
            painter.line_segment(
                [
                    egui::pos2(cx - 1.0, cy - 4.0),
                    egui::pos2(cx + 1.2, cy + 4.0),
                ],
                stroke,
            );
            painter.line_segment(
                [egui::pos2(cx + 1.2, cy + 4.0), egui::pos2(cx + 3.0, cy)],
                stroke,
            );
            painter.line_segment([egui::pos2(cx + 3.0, cy), egui::pos2(cx + 5.5, cy)], stroke);
        }
        (Group::Flash, "FlashScreen") => {
            // Hugeicons: Flash bolt
            let pts = [
                egui::pos2(cx + 1.6, cy - 5.4),
                egui::pos2(cx - 3.2, cy + 0.6),
                egui::pos2(cx - 0.4, cy + 0.6),
                egui::pos2(cx - 1.6, cy + 5.4),
                egui::pos2(cx + 3.2, cy - 0.7),
                egui::pos2(cx + 0.4, cy - 0.7),
            ];
            for i in 0..pts.len() {
                painter.line_segment([pts[i], pts[(i + 1) % pts.len()]], stroke);
            }
        }
        (Group::App, "Preferences") => {
            // Hugeicons: Settings Gear
            painter.circle_stroke(center, 3.8, stroke);
            painter.circle_filled(center, 1.2, color);
            for i in 0..6 {
                let angle = (i as f32) * (std::f32::consts::PI / 3.0);
                let p1 = center + egui::vec2(angle.cos() * 3.8, angle.sin() * 3.8);
                let p2 = center + egui::vec2(angle.cos() * 5.8, angle.sin() * 5.8);
                painter.line_segment([p1, p2], stroke);
            }
        }
        _ => {
            painter.circle_stroke(center, 3.0, stroke);
        }
    }
}
