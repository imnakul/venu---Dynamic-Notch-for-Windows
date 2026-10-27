//! The notch's right-click menu: a small card the notch draws itself, in its
//! own theme, with a highlight that slides between items and a quick scale
//! and fade in and out.
//!
//! This module is the model only — rows, layout, placement and motion — and
//! has no Win32 or Direct2D in it, so it is unit-tested on its own. The
//! window feeds it the cursor and clicks; the painter draws it
//! (`paint_panels::paint_menu`). It lives inside the notch's existing layered
//! window, so it costs no extra window or surface, and nothing at all while
//! it is closed.

use crate::notch::anim::{smoothstep, Spring};
use crate::notch::layout::Rect;

/// Card width, in pixels.
pub const MENU_W: f32 = 236.0;
/// One item.
pub const ROW_H: f32 = 30.0;
/// A separator.
pub const SEP_H: f32 = 9.0;
/// Padding inside the card, around the rows.
pub const PAD: f32 = 6.0;
/// Card corner radius.
pub const RADIUS: f32 = 12.0;
/// Tallest a menu can be; the window frame reserves this much below the top.
pub const MAX_H: f32 = 440.0;

/// Something the menu can ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuCommand {
    Settings,
    TogglePin,
    PauseHover,
    ResumeHover,
    NextPage,
    PreviousPage,
    ToggleClickThrough,
    Close,
    Quit,
    /// Shelf tile actions, by path, so a shelf edited while the menu was up
    /// can never redirect the action to a different item.
    Open(String),
    CopyFile(String),
    CopyPath(String),
    Reveal(String),
    Pin(String),
    Unpin(String),
    ClearClipboard,
}

/// Segoe Fluent Icons / Segoe MDL2 Assets code points (the two share them).
pub mod glyph {
    pub const SETTINGS: &str = "\u{E713}";
    pub const PIN: &str = "\u{E718}";
    pub const UNPIN: &str = "\u{E77A}";
    pub const PAUSE: &str = "\u{E769}";
    pub const POINTER: &str = "\u{E7C9}";
    pub const NEXT: &str = "\u{E76C}";
    pub const PREVIOUS: &str = "\u{E76B}";
    pub const CLOSE: &str = "\u{E8BB}";
    pub const POWER: &str = "\u{E7E8}";
    pub const OPEN: &str = "\u{E8A7}";
    pub const COPY: &str = "\u{E8C8}";
    pub const LINK: &str = "\u{E71B}";
    pub const FOLDER: &str = "\u{E838}";
    pub const CLEAR: &str = "\u{E74D}";
    pub const ADD: &str = "\u{E710}";
}

/// One half of a two-button row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Half {
    pub command: MenuCommand,
    pub label: &'static str,
    pub glyph: &'static str,
}

/// One row of the menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuRow {
    Item {
        command: MenuCommand,
        label: String,
        glyph: &'static str,
        /// A switch on the right, on or off, for settings that toggle.
        toggle: Option<bool>,
        /// Shown in a warning tone when hovered (Quit, Remove).
        danger: bool,
    },
    /// Two commands side by side, e.g. Previous | Next.
    Pair(Half, Half),
    Separator,
}

impl MenuRow {
    pub fn item(command: MenuCommand, label: &str, glyph: &'static str) -> Self {
        MenuRow::Item {
            command,
            label: label.to_owned(),
            glyph,
            toggle: None,
            danger: false,
        }
    }

    pub fn toggle(command: MenuCommand, label: &str, glyph: &'static str, on: bool) -> Self {
        MenuRow::Item {
            command,
            label: label.to_owned(),
            glyph,
            toggle: Some(on),
            danger: false,
        }
    }

    pub fn danger(command: MenuCommand, label: &str, glyph: &'static str) -> Self {
        MenuRow::Item {
            command,
            label: label.to_owned(),
            glyph,
            toggle: None,
            danger: true,
        }
    }

    fn height(&self) -> f32 {
        match self {
            MenuRow::Separator => SEP_H,
            _ => ROW_H,
        }
    }
}

/// Which item: a row, and for a pair, which half (0 left, 1 right).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Slot {
    pub row: usize,
    pub half: u8,
}

/// The card's size for these rows.
pub fn size(rows: &[MenuRow]) -> (f32, f32) {
    let h: f32 = rows.iter().map(MenuRow::height).sum();
    (MENU_W, (h + PAD * 2.0).min(MAX_H))
}

/// Where the card goes: just below and right of the click, kept inside
/// `frame` with a margin. Flips above the click when there is no room below.
pub fn place(anchor: (f32, f32), size: (f32, f32), frame: Rect) -> Rect {
    let margin = 8.0;
    let (w, h) = size;
    let mut left = anchor.0 - 14.0;
    let mut top = anchor.1 + 8.0;
    if top + h > frame.bottom - margin {
        top = anchor.1 - 8.0 - h;
    }
    left = left.clamp(
        frame.left + margin,
        (frame.right - margin - w).max(frame.left + margin),
    );
    top = top.clamp(
        frame.top + margin,
        (frame.bottom - margin - h).max(frame.top + margin),
    );
    Rect::new(left, top, left + w, top + h)
}

/// The open menu, its motion included.
#[derive(Debug, Clone)]
pub struct MenuView {
    pub rows: Vec<MenuRow>,
    pub rect: Rect,
    /// 0 closed, 1 open.
    pub open: Spring,
    pub closing: bool,
    pub hot: Option<Slot>,
    /// The highlight: it slides between items rather than jumping.
    pub hl_top: Spring,
    pub hl_left: Spring,
    pub hl_right: Spring,
    pub hl_alpha: Spring,
    /// The item just chosen, lit in the accent while the card fades.
    pub pressed: Option<Slot>,
    /// The item picked with the arrow keys, used while the cursor is off
    /// the card.
    keyboard: Option<Slot>,
}

impl MenuView {
    pub fn new(rows: Vec<MenuRow>, anchor: (f32, f32), frame: Rect) -> Self {
        let rect = place(anchor, size(&rows), frame);
        Self {
            rows,
            rect,
            // Quick and just under critical: it arrives with a soft settle.
            open: Spring::new(0.0, 420.0, 34.0),
            closing: false,
            hot: None,
            hl_top: Spring::new(0.0, 520.0, 42.0),
            hl_left: Spring::new(0.0, 520.0, 42.0),
            hl_right: Spring::new(0.0, 520.0, 42.0),
            hl_alpha: Spring::new(0.0, 380.0, 36.0),
            pressed: None,
            keyboard: None,
        }
    }

    /// Every item in order, a pair counting as two.
    fn slots(&self) -> Vec<Slot> {
        self.rows
            .iter()
            .enumerate()
            .flat_map(|(row, kind)| match kind {
                MenuRow::Separator => Vec::new(),
                MenuRow::Item { .. } => vec![Slot { row, half: 0 }],
                MenuRow::Pair(..) => vec![Slot { row, half: 0 }, Slot { row, half: 1 }],
            })
            .collect()
    }

    /// Move the highlight with the arrow keys, wrapping at the ends.
    pub fn step_keyboard(&mut self, down: bool) {
        if self.closing {
            return;
        }
        let slots = self.slots();
        if slots.is_empty() {
            return;
        }
        let current = self.hot.and_then(|h| slots.iter().position(|s| *s == h));
        let next = match (current, down) {
            (None, true) => 0,
            (None, false) => slots.len() - 1,
            (Some(i), true) => (i + 1) % slots.len(),
            (Some(i), false) => (i + slots.len() - 1) % slots.len(),
        };
        self.keyboard = Some(slots[next]);
        self.hot = self.keyboard;
    }

    /// Enter: run the highlighted item.
    pub fn choose_highlighted(&mut self) -> Option<MenuCommand> {
        let slot = self.hot?;
        let command = self.command(slot)?;
        self.pressed = Some(slot);
        self.close();
        Some(command)
    }

    /// Top of each row, in order.
    fn row_tops(&self) -> impl Iterator<Item = (usize, f32, &MenuRow)> {
        let mut y = self.rect.top + PAD;
        self.rows.iter().enumerate().map(move |(i, row)| {
            let top = y;
            y += row.height();
            (i, top, row)
        })
    }

    /// The rectangle an item's highlight fills.
    pub fn slot_rect(&self, slot: Slot) -> Option<Rect> {
        let (_, top, row) = self.row_tops().find(|(i, _, _)| *i == slot.row)?;
        let left = self.rect.left + PAD;
        let right = self.rect.right - PAD;
        match row {
            MenuRow::Separator => None,
            MenuRow::Item { .. } => Some(Rect::new(left, top, right, top + ROW_H)),
            MenuRow::Pair(..) => {
                let mid = (left + right) * 0.5;
                Some(if slot.half == 0 {
                    Rect::new(left, top, mid - 2.0, top + ROW_H)
                } else {
                    Rect::new(mid + 2.0, top, right, top + ROW_H)
                })
            }
        }
    }

    /// The item under a point.
    pub fn slot_at(&self, x: f32, y: f32) -> Option<Slot> {
        if !self.rect.contains(x, y) || self.closing {
            return None;
        }
        self.row_tops().find_map(|(i, top, row)| {
            if y < top || y >= top + row.height() {
                return None;
            }
            match row {
                MenuRow::Separator => None,
                MenuRow::Item { .. } => Some(Slot { row: i, half: 0 }),
                MenuRow::Pair(..) => {
                    let mid = (self.rect.left + self.rect.right) * 0.5;
                    Some(Slot {
                        row: i,
                        half: u8::from(x >= mid),
                    })
                }
            }
        })
    }

    /// What an item does.
    pub fn command(&self, slot: Slot) -> Option<MenuCommand> {
        match self.rows.get(slot.row)? {
            MenuRow::Item { command, .. } => Some(command.clone()),
            MenuRow::Pair(left, right) => Some(if slot.half == 0 {
                left.command.clone()
            } else {
                right.command.clone()
            }),
            MenuRow::Separator => None,
        }
    }

    /// A click inside the card: the command to run, if it landed on one. The
    /// card starts closing either way.
    pub fn choose(&mut self, x: f32, y: f32) -> Option<MenuCommand> {
        let slot = self.slot_at(x, y)?;
        let command = self.command(slot)?;
        self.pressed = Some(slot);
        self.close();
        Some(command)
    }

    pub fn close(&mut self) {
        self.closing = true;
    }

    /// Advance the motion with the cursor where it is. Returns false once it
    /// has closed completely and can be dropped.
    pub fn step(&mut self, cursor: Option<(f32, f32)>, dt: f32) -> bool {
        let target = if self.closing { 0.0 } else { 1.0 };
        self.open.step(target, dt);

        if !self.closing {
            let pointed = cursor.and_then(|(x, y)| self.slot_at(x, y));
            if pointed.is_some() {
                // The mouse takes over from the keys.
                self.keyboard = pointed;
            }
            let hot = pointed.or(self.keyboard);
            if let Some(rect) = hot.and_then(|slot| self.slot_rect(slot)) {
                if self.hot.is_none() && self.hl_alpha.value < 0.3 {
                    // Arriving from outside: appear in place, don't fly in.
                    self.hl_top.set(rect.top);
                    self.hl_left.set(rect.left);
                    self.hl_right.set(rect.right);
                }
            }
            self.hot = hot;
        }
        let shown = self.pressed.or(self.hot);
        if let Some(rect) = shown.and_then(|slot| self.slot_rect(slot)) {
            self.hl_top.step(rect.top, dt);
            self.hl_left.step(rect.left, dt);
            self.hl_right.step(rect.right, dt);
            self.hl_alpha.step(1.0, dt);
        } else {
            self.hl_alpha.step(0.0, dt);
        }

        !(self.closing && self.open.value <= 0.01)
    }

    /// How visible the card is, 0..1.
    pub fn alpha(&self) -> f32 {
        smoothstep(0.0, 1.0, self.open.value.clamp(0.0, 1.0))
    }

    /// The highlight's current rectangle.
    pub fn highlight(&self) -> Rect {
        Rect::new(
            self.hl_left.value,
            self.hl_top.value,
            self.hl_right.value,
            self.hl_top.value + ROW_H,
        )
    }

    /// Everything that changes what it looks like, quantised, for the
    /// notch's frame key.
    pub fn key(&self) -> [i32; 6] {
        [
            (self.open.value * 200.0) as i32,
            (self.hl_top.value * 4.0) as i32,
            (self.hl_left.value * 4.0) as i32,
            (self.hl_alpha.value * 100.0) as i32,
            self.pressed
                .map_or(-1, |s| s.row as i32 * 2 + s.half as i32),
            self.rect.top as i32 * 4096 + self.rect.left as i32,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows() -> Vec<MenuRow> {
        vec![
            MenuRow::item(MenuCommand::Settings, "Open Settings", glyph::SETTINGS),
            MenuRow::Separator,
            MenuRow::toggle(MenuCommand::TogglePin, "Keep open", glyph::PIN, false),
            MenuRow::Pair(
                Half {
                    command: MenuCommand::PreviousPage,
                    label: "Previous",
                    glyph: glyph::PREVIOUS,
                },
                Half {
                    command: MenuCommand::NextPage,
                    label: "Next",
                    glyph: glyph::NEXT,
                },
            ),
            MenuRow::Separator,
            MenuRow::danger(MenuCommand::Quit, "Quit Venu", glyph::POWER),
        ]
    }

    fn frame() -> Rect {
        Rect::new(0.0, 0.0, 900.0, 500.0)
    }

    #[test]
    fn the_card_fits_its_rows_and_stays_inside_the_frame() {
        let (w, h) = size(&rows());
        assert_eq!(w, MENU_W);
        assert_eq!(h, ROW_H * 4.0 + SEP_H * 2.0 + PAD * 2.0);

        let near_right = place((890.0, 60.0), (w, h), frame());
        assert!(near_right.right <= 892.0);
        let near_bottom = place((300.0, 480.0), (w, h), frame());
        assert!(near_bottom.bottom <= 492.0);
        assert!(near_bottom.top < 480.0, "flips above the click");
    }

    #[test]
    fn items_are_found_where_they_are_drawn() {
        let menu = MenuView::new(rows(), (100.0, 50.0), frame());
        let top = menu.rect.top + PAD;
        let mid_x = (menu.rect.left + menu.rect.right) * 0.5;
        assert_eq!(
            menu.slot_at(mid_x, top + 10.0),
            Some(Slot { row: 0, half: 0 })
        );
        // The separator is not an item.
        assert_eq!(menu.slot_at(mid_x, top + ROW_H + 4.0), None);
        // The pair splits down the middle.
        let pair_y = top + ROW_H + SEP_H + ROW_H + 10.0;
        assert_eq!(
            menu.command(menu.slot_at(mid_x - 20.0, pair_y).unwrap()),
            Some(MenuCommand::PreviousPage)
        );
        assert_eq!(
            menu.command(menu.slot_at(mid_x + 20.0, pair_y).unwrap()),
            Some(MenuCommand::NextPage)
        );
    }

    #[test]
    fn choosing_runs_once_and_the_card_closes_away() {
        let mut menu = MenuView::new(rows(), (100.0, 50.0), frame());
        for _ in 0..30 {
            assert!(menu.step(None, 1.0 / 60.0));
        }
        assert!(menu.alpha() > 0.95);
        let y = menu.rect.top + PAD + 10.0;
        assert_eq!(
            menu.choose(menu.rect.left + 40.0, y),
            Some(MenuCommand::Settings)
        );
        // Closing: no second choice.
        assert_eq!(menu.choose(menu.rect.left + 40.0, y), None);
        let mut alive = true;
        for _ in 0..120 {
            alive = menu.step(None, 1.0 / 60.0);
        }
        assert!(!alive);
    }

    #[test]
    fn arrow_keys_walk_the_items_and_enter_picks_one() {
        let mut menu = MenuView::new(rows(), (100.0, 50.0), frame());
        menu.step_keyboard(true);
        assert_eq!(menu.hot, Some(Slot { row: 0, half: 0 }));
        menu.step_keyboard(true);
        menu.step_keyboard(true);
        // Past the separator, into the pair's left half.
        assert_eq!(menu.hot, Some(Slot { row: 3, half: 0 }));
        menu.step_keyboard(false);
        menu.step_keyboard(false);
        menu.step_keyboard(false);
        // Wraps from the first to the last.
        assert_eq!(menu.hot, Some(Slot { row: 5, half: 0 }));
        menu.step(None, 1.0 / 60.0);
        assert_eq!(
            menu.hot,
            Some(Slot { row: 5, half: 0 }),
            "kept with the cursor away"
        );
        assert_eq!(menu.choose_highlighted(), Some(MenuCommand::Quit));
    }

    #[test]
    fn the_highlight_slides_between_items_but_appears_in_place() {
        let mut menu = MenuView::new(rows(), (100.0, 50.0), frame());
        let x = menu.rect.left + 40.0;
        let first = menu.rect.top + PAD + 10.0;
        menu.step(Some((x, first)), 1.0 / 60.0);
        assert_eq!(menu.highlight().top, menu.rect.top + PAD, "no fly-in");
        let third = menu.rect.top + PAD + ROW_H + SEP_H + 10.0;
        menu.step(Some((x, third)), 1.0 / 60.0);
        let mid = menu.highlight().top;
        assert!(mid > menu.rect.top + PAD && mid < menu.rect.top + PAD + ROW_H + SEP_H);
    }
}
