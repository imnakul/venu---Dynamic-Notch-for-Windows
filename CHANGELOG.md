# Changelog

What's new in each version of Venu, written for the people who use it.

## [0.5.1] - 2026-10-21

### Fixed
- **Glass notch looks like one piece again.** On the see-through themes, a
  faint band showed across the top of the resting pill, and the open notch
  looked like a separate darker panel under a lighter strip. The notch's
  shadow was showing through the glass; it now only falls around the notch.

## [0.5.0] - 2026-10-21

The biggest update yet: a redesigned Settings window, three new slides, border
effects for the notch, and a lighter, smoother notch overall.

### New slides
- **Focus Timer.** A Pomodoro timer right in the notch: focus rounds, short
  breaks and a long break after the set. Start, pause and skip from the open
  notch; the resting pill shows the countdown. The notch tells you when each
  round ends, and breaks can start on their own.
- **Calendar.** See what's next without opening anything. Paste your Google
  Calendar or Outlook iCal link (or point it at an `.ics` file) and the notch
  shows your next meeting, how long until it starts, and what comes after.
  Recurring meetings, moved meetings and all-day events are all understood.
- **Quick Toggles.** One click to mute your microphone, mute your sound, switch
  Windows between dark and light mode, or keep your PC awake.

### Border effects
- The notch can now play an effect around its edge: **Rainbow**, **Glass
  reflection**, **Colour sweep** (left to right, right to left, or there and
  back), **Breathing glow**, **Aurora**, **Comet**, **Neon sign**,
  **Heartbeat** and **Fire**.
- Play an effect every so often (you choose how often, from 5 seconds to 10
  minutes) or **continuously**.
- Choose where it plays: **always**, **only on the resting pill**, or **only on
  the open panel**.
- A **live preview** in Settings shows the effect on a miniature notch as you
  change it.
- Effects cost nothing between runs, and a continuous effect is drawn at a
  gentle 30 frames a second.

### Alerts
- **Battery alerts.** A quick alert when you plug your charger in or out, and
  when your battery runs low (you pick the level). Desktops stay quiet.
- **Cleaner notification settings.** Add any app by name, remove apps with one
  click, pick each app's colour, and send yourself a test alert. The local
  address for scripts and AI agents now comes with a ready-to-copy example.

### A new Settings window
- **Glass design.** The window is frosted glass over your desktop, with a
  *Glass strength* slider to make it clearer or more solid.
- **Your accent colour.** Pick from eight colours or any colour you like;
  switches, tabs, sliders and the selected page all follow it.
- **Easier to find things.** Seven pages in three groups (Notch, On screen,
  App), each split into tabs, plus a **search box** (press Ctrl+F) that jumps
  straight to any setting.
- **Floats beside your work.** A fixed-size window that stays above other
  windows (unpin it if you prefer), with its own title bar.
- **Sliders everywhere.** Every number is now a slider, and they all line up.
- **Switches instead of checkboxes**, smooth gliding tabs, and Geist Mono for
  numbers.
- **Simpler wallpaper picker.** Browse for an image and see exactly how the
  notch will frame it, with zoom.

### The notch itself
- **A smoother shape.** The corners where the notch meets the top of your
  screen now curve continuously, so the notch looks like part of the display,
  open or closed.
- **Uses less of your PC.** The notch only redraws when something on it
  actually changes, and reads GPU activity less often.
- **New icon.** Venu now uses its own logo in the taskbar, the tray, the
  Start menu and the installer.

## [0.4.3]

- Whole-PC Stats slide (CPU, memory, GPU and power) and Geist typography.
- Fixed a crash at startup caused by a bad Devanagari font.
