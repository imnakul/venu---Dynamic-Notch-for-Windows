//! Settings for how the notch is driven — opening, the wheel, the mouse
//! trigger and global shortcuts — and for the pages added alongside them:
//! Now Playing, the Shelf and Clipboard.
//!
//! A child module of `gui`, so it uses the same private layout helpers as
//! every other page.

use eframe::egui::{self, Color32, RichText};

use super::{filedlg, theme, PageCtx, SettingsApp, ToggleExt};
use crate::config::{
    AppConfig, ClipboardConfig, Hotkey, InputConfig, MouseTrigger, OpenWith, ScrollPages,
    ShelfConfig, SlideKind,
};
use crate::notch::keys::{self, HotkeyStatus};

/// Colour for a problem the user can fix right here.
const WARNING: Color32 = Color32::from_rgb(244, 160, 92);

impl SettingsApp {
    // -- General › Controls ----------------------------------------------------

    pub(super) fn tab_notch_controls(ui: &mut egui::Ui, cx: &mut PageCtx<'_>) {
        ui.label(
            RichText::new(
                "Everything about driving the notch, in one place. If it ever ends up in a \
                 state you cannot use, Reset controls (here, at the top right, or in the tray \
                 menu) puts all of this back.",
            )
            .size(11.0)
            .color(theme::text_secondary()),
        );
        ui.add_space(8.0);
        if ui.button("Reset controls").clicked() {
            cx.cfg.notch.input.reset_behaviour();
            cx.cfg.notch.click_through = false;
            *cx.changed = true;
        }
        ui.add_space(14.0);

        Self::sec_opening(ui, cx);
        Self::divider(ui);
        Self::sec_closing(ui, cx);
        Self::divider(ui);
        Self::sec_click_through(ui, cx);
        Self::divider(ui);
        Self::sec_menu_and_pages(ui, cx);
        Self::divider(ui);
        Self::sec_shortcuts(ui, cx);
        cx.cfg.notch.input.clean();
    }

    /// A labelled group of on/off switches, several of which can be on. Uses
    /// the same switch as every other setting, so an option reads as on or
    /// off at a glance instead of as a button that might be a choice.
    fn switches(
        ui: &mut egui::Ui,
        label: &str,
        items: &mut [(&str, &mut bool)],
        changed: &mut bool,
    ) {
        ui.label(RichText::new(label).size(13.0).color(theme::text_primary()));
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(22.0, 10.0);
            for (name, on) in items.iter_mut() {
                *changed |= ui.toggle(on, *name).changed();
            }
        });
        ui.add_space(10.0);
    }

    fn note(ui: &mut egui::Ui, text: &str) {
        ui.label(
            RichText::new(text)
                .size(11.0)
                .color(theme::text_secondary()),
        );
        ui.add_space(12.0);
    }

    fn sec_opening(ui: &mut egui::Ui, cx: &mut PageCtx<'_>) {
        Self::section_title(ui, "OPEN");
        let input = &mut cx.cfg.notch.input;
        let mut hover = input.open_with.hovers();
        let mut click = input.open_with.clicks();
        let mut edited = false;
        Self::switches(
            ui,
            "Open with",
            &mut [("Hover", &mut hover), ("Click", &mut click)],
            &mut edited,
        );
        if edited {
            // One of the two always stays on.
            input.open_with = match (hover, click) {
                (true, true) => OpenWith::Both,
                (true, false) => OpenWith::Hover,
                (false, true) => OpenWith::Click,
                (false, false) if input.open_with.hovers() => OpenWith::Click,
                (false, false) => OpenWith::Hover,
            };
            *cx.changed = true;
        }
        Self::note(
            ui,
            match input.open_with {
                OpenWith::Click => "Click the pill to open it. Hovering does nothing at all.",
                OpenWith::Hover => "Rest the cursor on the pill to open it. Clicks do not open it.",
                OpenWith::Both => "Either opens it: a click at once, a hover after the delay.",
            },
        );

        if input.open_with.hovers() {
            *cx.changed |= Self::slider_line(
                ui,
                "Delay",
                egui::Slider::new(
                    &mut input.hover_delay_ms,
                    0..=InputConfig::MAX_HOVER_DELAY_MS,
                )
                .suffix(" ms"),
            );
            ui.add_space(6.0);
            *cx.changed |= ui
                .toggle(
                    &mut input.hover_in_fullscreen,
                    "Also open on hover over full-screen apps and games",
                )
                .changed();
            ui.add_space(12.0);
        }

        if input.open_with.clicks() {
            let trigger = input.mouse_trigger;
            let shown = |kind: MouseTrigger| -> &'static str {
                match kind {
                    MouseTrigger::Off => "Plain click",
                    other => other.label(),
                }
            };
            Self::row_inline(ui, "Click with", |ui| {
                egui::ComboBox::from_id_salt("mouse_trigger")
                    .selected_text(shown(trigger))
                    .show_ui(ui, |ui| {
                        for kind in MouseTrigger::ALL {
                            if ui.selectable_label(trigger == kind, shown(kind)).clicked() {
                                input.mouse_trigger = kind;
                                *cx.changed = true;
                            }
                        }
                    });
            });
            Self::note(
                ui,
                if trigger == MouseTrigger::Off {
                    "Any click on the pill opens it."
                } else {
                    "Only this gesture opens it; a plain click on the pill does nothing. It \
                     works even while clicks pass through."
                },
            );
        }
    }

    fn sec_closing(ui: &mut egui::Ui, cx: &mut PageCtx<'_>) {
        Self::section_title(ui, "CLOSE");
        let input = &mut cx.cfg.notch.input;
        Self::switches(
            ui,
            "Close with",
            &mut [
                ("Esc key", &mut input.close_on_esc),
                ("Click outside", &mut input.close_on_outside_click),
                ("Cursor leaves", &mut input.close_on_leave),
            ],
            cx.changed,
        );
        input.clean();
        Self::note(
            ui,
            "Any that are on close it. Esc is only taken while the notch is open, so it \
             reaches your other apps the rest of the time. At least one stays on, so the \
             notch can always be closed.",
        );
        if input.close_on_leave {
            *cx.changed |= Self::slider_line(
                ui,
                "After",
                egui::Slider::new(&mut cx.cfg.notch.collapse_delay_ms, 0..=1500).suffix(" ms"),
            );
            ui.add_space(12.0);
        }
    }

    fn sec_click_through(ui: &mut egui::Ui, cx: &mut PageCtx<'_>) {
        Self::section_title(ui, "CLICKS PASS THROUGH");
        *cx.changed |= ui
            .toggle(
                &mut cx.cfg.notch.click_through,
                "Let clicks go to the window underneath",
            )
            .changed();
        ui.add_space(4.0);
        Self::note(
            ui,
            "The notch still opens on hover and still answers the wheel, its right-click \
             menu and your shortcuts. To switch this back off: the right-click menu, both \
             mouse buttons together on the notch, the tray icon's menu, or the button at the \
             top right of this window.",
        );
    }

    fn sec_menu_and_pages(ui: &mut egui::Ui, cx: &mut PageCtx<'_>) {
        Self::section_title(ui, "RIGHT-CLICK MENU");
        let input = &mut cx.cfg.notch.input;
        Self::switches(
            ui,
            "Opens on",
            &mut [
                ("Closed pill", &mut input.menu_on_pill),
                ("Open panel", &mut input.menu_on_panel),
            ],
            cx.changed,
        );
        Self::note(
            ui,
            "Settings, keep open, pause hover, click-through, pages and quit. It works even \
             while clicks pass through.",
        );

        Self::section_title(ui, "CHANGE PAGES WITH THE WHEEL");
        let scroll = cx.cfg.notch.scroll_pages();
        let mut closed = scroll.applies(false);
        let mut open = scroll.applies(true);
        let mut edited = false;
        Self::switches(
            ui,
            "Scroll on",
            &mut [("Closed pill", &mut closed), ("Open panel", &mut open)],
            &mut edited,
        );
        if edited {
            let mode = ScrollPages::from_parts(closed, open);
            cx.cfg.notch.input.scroll = Some(mode);
            // Kept in step for anything that reads the old switch.
            cx.cfg.notch.scroll_to_switch = mode != ScrollPages::Off;
            *cx.changed = true;
        }
        Self::note(
            ui,
            "On the closed pill, scrolling changes the face it rests on; on the open panel, \
             the page.",
        );
    }

    fn sec_shortcuts(ui: &mut egui::Ui, cx: &mut PageCtx<'_>) {
        Self::section_title(ui, "KEYBOARD SHORTCUTS");
        ui.label(
            RichText::new(
                "Work from any app. Click a shortcut, then press the keys you want; it needs \
                 Ctrl, Alt or Shift. Esc cancels.",
            )
            .size(11.0)
            .color(theme::text_secondary()),
        );
        ui.add_space(10.0);

        let input = &mut cx.cfg.notch.input;
        let rows: [(&str, &mut Option<Hotkey>); 3] = [
            ("Open or close the notch", &mut input.hotkey_toggle),
            ("Clicks pass through", &mut input.hotkey_click_through),
            ("Pause hover", &mut input.hotkey_pause_hover),
        ];
        for (index, (label, slot)) in rows.into_iter().enumerate() {
            Self::hotkey_row(ui, label, index, slot, cx.recording_hotkey, cx.changed);
        }
    }

    /// One shortcut: its keys as a button that records new ones, a clear
    /// button, and a note if Windows would not take it.
    fn hotkey_row(
        ui: &mut egui::Ui,
        label: &str,
        index: usize,
        slot: &mut Option<Hotkey>,
        recording: &mut Option<usize>,
        changed: &mut bool,
    ) {
        let listening = *recording == Some(index);
        let mut rejected = false;
        if listening {
            let events = ui.input(|i| i.events.clone());
            for event in events {
                let egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } = event
                else {
                    continue;
                };
                if key == egui::Key::Escape {
                    *recording = None;
                    break;
                }
                let candidate = Hotkey {
                    ctrl: modifiers.ctrl,
                    alt: modifiers.alt,
                    shift: modifiers.shift,
                    win: false,
                    key: key.name().to_owned(),
                };
                if keys::registration(&candidate).is_some() {
                    *slot = Some(candidate);
                    *recording = None;
                    *changed = true;
                    break;
                }
                rejected = true;
            }
        }

        Self::row_inline(ui, label, |ui| {
            if slot.is_some() && ui.small_button("Clear").clicked() {
                *slot = None;
                *recording = None;
                *changed = true;
            }
            let text = if *recording == Some(index) {
                "Press keys…".to_owned()
            } else {
                slot.as_ref()
                    .map(Hotkey::label)
                    .unwrap_or_else(|| "Not set".to_owned())
            };
            let picked = *recording == Some(index);
            if Self::segment(ui, &text, picked)
                .on_hover_text("Click, then press the shortcut")
                .clicked()
            {
                *recording = if picked { None } else { Some(index) };
            }
        });

        let note = if rejected {
            Some((
                "Add Ctrl, Alt or Shift to a letter, number or F-key.",
                WARNING,
            ))
        } else if slot.is_some() && keys::status(index) == HotkeyStatus::InUse {
            Some((
                "Another app already uses this shortcut, so Windows gave it to them. Pick another.",
                WARNING,
            ))
        } else {
            None
        };
        if let Some((text, colour)) = note {
            ui.label(RichText::new(text).size(11.0).color(colour));
            ui.add_space(8.0);
        }
    }

    // -- Slides › Now Playing -------------------------------------------------

    // -- General › Features ---------------------------------------------------

    /// Which features the notch carries and in what order: one row each,
    /// dragged by its grip to reorder, with a switch to show or hide it.
    pub(super) fn tab_notch_features(ui: &mut egui::Ui, cx: &mut PageCtx<'_>) {
        Self::note(
            ui,
            "Drag a row to change the order the notch and the scroll wheel go through. A \
             feature you switch off keeps its place for when you switch it back on. Each \
             one's own settings are under Slides.",
        );
        Self::feature_list(ui, cx.cfg, cx.changed);
    }

    fn feature_list(ui: &mut egui::Ui, cfg: &mut AppConfig, changed: &mut bool) {
        let drag_id = egui::Id::new("feature_list_drag");
        let dragging: Option<SlideKind> = ui.ctx().data(|d| d.get_temp(drag_id)).flatten();
        let mut next_drag = dragging;

        let mut order = cfg.notch.deck_order();
        let mut on: Vec<SlideKind> = cfg.notch.slides.clone();
        let on_count = on.len();
        let mut rows: Vec<(SlideKind, egui::Rect)> = Vec::with_capacity(order.len());
        let mut edited = false;
        let mut nudge: Option<(SlideKind, bool)> = None;

        for &kind in &order {
            let row = Self::feature_row(ui, kind, &mut on, on_count, dragging == Some(kind));
            edited |= row.toggled;
            if row.handle.drag_started() {
                next_drag = Some(kind);
            }
            if row.handle.drag_stopped() {
                next_drag = None;
            }
            // Keyboard: focus a row (Tab), then Alt+Up / Alt+Down moves it.
            if row.handle.has_focus() {
                let (up, down) = ui.input(|i| {
                    (
                        i.modifiers.alt && i.key_pressed(egui::Key::ArrowUp),
                        i.modifiers.alt && i.key_pressed(egui::Key::ArrowDown),
                    )
                });
                if up || down {
                    nudge = Some((kind, down));
                }
            }
            rows.push((kind, row.rect));
            ui.add_space(6.0);
        }

        // While dragging, the row trades places with a neighbour as soon as
        // the pointer passes that neighbour's middle, so the list reflows
        // under the cursor rather than on release.
        if let (Some(kind), Some(pointer)) = (next_drag, ui.ctx().pointer_interact_pos()) {
            if let Some(index) = order.iter().position(|k| *k == kind) {
                let above = index.checked_sub(1).map(|i| rows[i].1.center().y);
                let below = rows.get(index + 1).map(|(_, r)| r.center().y);
                if above.is_some_and(|y| pointer.y < y) {
                    order.swap(index, index - 1);
                    edited = true;
                } else if below.is_some_and(|y| pointer.y > y) {
                    order.swap(index, index + 1);
                    edited = true;
                }
            }
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        }
        if let Some((kind, down)) = nudge {
            if let Some(index) = order.iter().position(|k| *k == kind) {
                let target = if down {
                    (index + 1).min(order.len() - 1)
                } else {
                    index.saturating_sub(1)
                };
                if target != index {
                    order.swap(index, target);
                    edited = true;
                }
            }
        }
        // A drag that ended outside the window never reports a stop.
        if !ui.input(|i| i.pointer.any_down()) {
            next_drag = None;
        }
        ui.ctx().data_mut(|d| d.insert_temp(drag_id, next_drag));

        if edited && cfg.notch.set_deck(order, |kind| on.contains(&kind)) {
            *changed = true;
        }
    }

    /// One row: grip, name and switch. The grip and name are the drag handle;
    /// the switch is left to itself so a click on it never starts a drag.
    fn feature_row(
        ui: &mut egui::Ui,
        kind: SlideKind,
        on: &mut Vec<SlideKind>,
        on_count: usize,
        lifted: bool,
    ) -> FeatureRow {
        let background = ui.painter().add(egui::Shape::Noop);
        let mut shown = on.contains(&kind);
        // The last feature that is on stays on: the notch needs a face.
        let locked = shown && on_count <= 1;
        let mut toggled = false;
        let mut switch_left = f32::INFINITY;

        let inner = ui.horizontal(|ui| {
            ui.set_min_height(40.0);
            ui.add_space(12.0);
            let (grip, _) = ui.allocate_exact_size(egui::vec2(10.0, 16.0), egui::Sense::hover());
            let dot = if lifted {
                theme::accent()
            } else {
                theme::text_tertiary()
            };
            for col in 0..2 {
                for line in 0..3 {
                    ui.painter().circle_filled(
                        egui::pos2(
                            grip.left() + 2.0 + col as f32 * 6.0,
                            grip.top() + 2.5 + line as f32 * 5.5,
                        ),
                        1.4,
                        dot,
                    );
                }
            }
            ui.add_space(12.0);
            ui.label(RichText::new(kind.label()).size(13.0).color(if shown {
                theme::text_primary()
            } else {
                theme::text_tertiary()
            }));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(12.0);
                let switch = ui
                    .add_enabled_ui(!locked, |ui| ui.toggle(&mut shown, ""))
                    .inner
                    .on_hover_text(if locked {
                        "At least one feature stays on"
                    } else if shown {
                        "Hide from the notch"
                    } else {
                        "Show in the notch"
                    });
                switch_left = switch.rect.left();
                if switch.changed() {
                    toggled = true;
                }
            });
        });
        let rect = inner.response.rect;

        let handle_rect = egui::Rect::from_min_max(
            rect.min,
            egui::pos2((switch_left - 8.0).min(rect.right()), rect.bottom()),
        );
        let handle = ui
            .interact(
                handle_rect,
                egui::Id::new(("feature_row", kind)),
                egui::Sense::click_and_drag(),
            )
            .on_hover_cursor(egui::CursorIcon::Grab);
        handle.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::WidgetType::Button,
                true,
                format!("{}, drag or Alt+arrow keys to reorder", kind.label()),
            )
        });

        let hover = ui.ctx().animate_bool_with_time(
            egui::Id::new(("feature_row_hover", kind)),
            handle.hovered() || lifted,
            0.12,
        );
        let fill = if lifted {
            theme::surface_active()
        } else {
            lerp(theme::surface(), theme::surface_hover(), hover)
        };
        let stroke = if lifted || handle.has_focus() {
            egui::Stroke::new(1.0_f32, theme::accent().gamma_multiply(0.7))
        } else {
            egui::Stroke::new(1.0_f32, theme::divider())
        };
        ui.painter().set(
            background,
            egui::Shape::rect_filled(rect, egui::Rounding::same(10.0), fill),
        );
        ui.painter()
            .rect_stroke(rect, egui::Rounding::same(10.0), stroke);

        if toggled {
            if shown {
                on.push(kind);
            } else {
                on.retain(|k| *k != kind);
            }
        }
        FeatureRow {
            rect,
            handle,
            toggled,
        }
    }

    pub(super) fn tab_slides_media(ui: &mut egui::Ui, cx: &mut PageCtx<'_>) {
        Self::section_title(ui, "NOW PLAYING");
        let media = &mut cx.cfg.notch.media;
        *cx.changed |= ui
            .toggle(&mut media.live_wave, "Bars follow the music")
            .changed();
        ui.add_space(4.0);
        ui.label(
            RichText::new(
                "The little equaliser on the pill, the media pod and the player moves with \
                 what is actually playing. It redraws 25 times a second while music plays and \
                 not at all otherwise; off, the bars stand still.",
            )
            .size(11.0)
            .color(theme::text_secondary()),
        );
        ui.add_space(14.0);

        Self::row_stacked(
            ui,
            "Player size",
            Some(
                "Auto opens a centred player: the cover or video thumbnail large in the middle, \
                 title, channel and the timeline under it. A short, wide size lays it out side \
                 by side instead.",
            ),
            |ui| {
                Self::size_pair(
                    ui,
                    &mut media.panel_width,
                    &mut media.panel_height,
                    1400,
                    700,
                    cx.changed,
                );
            },
        );
    }

    // -- Slides › Shelf ---------------------------------------------------------

    pub(super) fn tab_slides_shelf(ui: &mut egui::Ui, cx: &mut PageCtx<'_>) {
        Self::section_title(ui, "PINNED");
        ui.label(
            RichText::new(
                "Files, folders, apps and scripts, one click away in the notch. Venu stores \
                 only the path: nothing is copied or moved. You can also drop files on the \
                 notch, and right-click an item there for more.",
            )
            .size(11.0)
            .color(theme::text_secondary()),
        );
        ui.add_space(10.0);

        let shelf: &mut ShelfConfig = &mut cx.cfg.notch.shelf;
        let mut remove = None;
        for (index, path) in shelf.items.iter().enumerate() {
            ui.horizontal(|ui| {
                let exists = std::path::Path::new(path).exists();
                let name = crate::notch::shelf::display_name(path);
                ui.label(RichText::new(name).size(12.5).color(if exists {
                    theme::text_primary()
                } else {
                    theme::text_tertiary()
                }))
                .on_hover_text(path.as_str());
                if !exists {
                    ui.label(RichText::new("missing").size(10.5).color(WARNING));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("Remove").clicked() {
                        remove = Some(index);
                    }
                });
            });
            ui.add_space(4.0);
        }
        if let Some(index) = remove {
            shelf.items.remove(index);
            *cx.changed = true;
        }
        if shelf.items.is_empty() {
            ui.label(
                RichText::new("Nothing pinned yet.")
                    .size(12.0)
                    .color(theme::text_tertiary()),
            );
        }
        ui.add_space(8.0);
        let full = shelf.items.len() >= ShelfConfig::MAX_ITEMS;
        ui.horizontal(|ui| {
            if ui
                .add_enabled(!full, egui::Button::new("Add files or apps…"))
                .clicked()
            {
                for path in filedlg::pick_files("Pin to the Shelf") {
                    *cx.changed |= shelf.pin(&path);
                }
            }
            if ui
                .add_enabled(!full, egui::Button::new("Add a folder…"))
                .clicked()
            {
                if let Some(path) = filedlg::pick_folder("Pin a folder to the Shelf") {
                    *cx.changed |= shelf.pin(&path);
                }
            }
        });
        if full {
            ui.label(
                RichText::new(format!(
                    "The shelf holds up to {} items.",
                    ShelfConfig::MAX_ITEMS
                ))
                .size(11.0)
                .color(theme::text_tertiary()),
            );
        }

        Self::divider(ui);
        Self::section_title(ui, "FOLDER PEEK");
        ui.label(
            RichText::new(
                "Show the newest files of one folder beside the pins, such as Screenshots or \
                 Downloads. Click one to open it; right-click to copy it, ready to paste into \
                 a chat or a document. Venu only looks again when Windows says the folder \
                 changed.",
            )
            .size(11.0)
            .color(theme::text_secondary()),
        );
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            let shown = if shelf.folder.trim().is_empty() {
                "No folder".to_owned()
            } else {
                shelf.folder.clone()
            };
            ui.label(RichText::new(shown).size(12.0).color(theme::text_primary()));
        });
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui.button("Choose folder…").clicked() {
                if let Some(path) = filedlg::pick_folder("Folder to peek into") {
                    shelf.folder = path;
                    *cx.changed = true;
                }
            }
            if let Some(screens) = screenshots_folder() {
                if shelf.folder != screens && ui.button("Use Screenshots").clicked() {
                    shelf.folder = screens;
                    *cx.changed = true;
                }
            }
            if !shelf.folder.is_empty() && ui.small_button("Clear").clicked() {
                shelf.folder.clear();
                *cx.changed = true;
            }
        });
        ui.add_space(12.0);
        if !shelf.folder.is_empty() {
            *cx.changed |= Self::slider_line(
                ui,
                "Show",
                egui::Slider::new(&mut shelf.folder_count, 1..=ShelfConfig::MAX_FOLDER_COUNT)
                    .suffix(" files"),
            );
        }
        Self::deck_note(ui, cx, SlideKind::Shelf);
    }

    // -- Slides › Clipboard -----------------------------------------------------

    pub(super) fn tab_slides_clipboard(ui: &mut egui::Ui, cx: &mut PageCtx<'_>) {
        Self::section_title(ui, "CLIPBOARD HISTORY");
        let clip: &mut ClipboardConfig = &mut cx.cfg.notch.clipboard;
        if ui
            .toggle(&mut clip.enabled, "Remember text I copy")
            .changed()
        {
            *cx.changed = true;
            // Switching it on is asking to see it: put the page in the deck.
            if clip.enabled && !cx.cfg.notch.slides.contains(&SlideKind::Clipboard) {
                cx.cfg.notch.slides.push(SlideKind::Clipboard);
            }
        }
        ui.add_space(4.0);
        ui.label(
            RichText::new(
                "The last few things you copied, one click to copy again. Kept in memory only: \
                 never written to disk, gone when Venu closes or this is switched off. Anything \
                 a password manager marks as private is skipped.",
            )
            .size(11.0)
            .color(theme::text_secondary()),
        );
        ui.add_space(12.0);
        let clip = &mut cx.cfg.notch.clipboard;
        if clip.enabled {
            *cx.changed |= Self::slider_line(
                ui,
                "Keep",
                egui::Slider::new(&mut clip.max_items, 1..=ClipboardConfig::MAX).suffix(" items"),
            );
            ui.add_space(6.0);
            if ui.button("Clear history now").clicked() {
                crate::notch::clipboard::clear();
            }
            ui.add_space(6.0);
        }
        Self::deck_note(ui, cx, SlideKind::Clipboard);
    }

    /// A page only shows if its slide is in the carousel; offer to add it.
    fn deck_note(ui: &mut egui::Ui, cx: &mut PageCtx<'_>, kind: SlideKind) {
        if cx.cfg.notch.slides.contains(&kind) {
            return;
        }
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!(
                    "{} is switched off in General › Features.",
                    kind.label()
                ))
                .size(11.5)
                .color(theme::text_secondary()),
            );
            if ui.small_button("Switch it on").clicked() {
                cx.cfg.notch.show_feature(kind);
                *cx.changed = true;
            }
        });
    }
}

/// The user's Screenshots folder, if it exists.
fn screenshots_folder() -> Option<String> {
    let mut path = dirs::picture_dir()?;
    path.push("Screenshots");
    path.is_dir().then(|| path.to_string_lossy().into_owned())
}

/// What [`SettingsApp::feature_row`] reports back to the list.
struct FeatureRow {
    rect: egui::Rect,
    handle: egui::Response,
    toggled: bool,
}

/// Blend two colours, `t` from 0 (`a`) to 1 (`b`).
fn lerp(a: Color32, b: Color32, t: f32) -> Color32 {
    let mix = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t.clamp(0.0, 1.0)).round() as u8;
    Color32::from_rgba_premultiplied(
        mix(a.r(), b.r()),
        mix(a.g(), b.g()),
        mix(a.b(), b.b()),
        mix(a.a(), b.a()),
    )
}
