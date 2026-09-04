<p align="center">
  <img src="assets/file_00000000305c821187409948f43d9797.png" alt="Venu logo" width="120">
</p>

<h1 align="center">Venu</h1>

<p align="center">
  <strong>Dynamic Notch for Windows</strong>
</p>

<p align="center">
  A native desktop companion for Windows 10 and Windows 11 that brings media controls, notifications, system information, reminders, and AI coding usage into a compact glass notch.
</p>

<p align="center">
  <a href="https://github.com/imnakul/venu---Dynamic-Notch-for-Windows/releases"><img src="https://img.shields.io/github/v/release/imnakul/venu---Dynamic-Notch-for-Windows?style=flat-square&label=download" alt="Latest release"></a>
  <a href="https://github.com/imnakul/venu---Dynamic-Notch-for-Windows/actions/workflows/ci.yml"><img src="https://img.shields.io/github/actions/workflow/status/imnakul/venu---Dynamic-Notch-for-Windows/ci.yml?branch=main&label=build&style=flat-square" alt="Build status"></a>
  <img src="https://img.shields.io/badge/platform-Windows%2010%2F11-0A0A0D.svg?style=flat-square" alt="Platform: Windows">
  <img src="https://img.shields.io/badge/built%20with-Rust-orange.svg?style=flat-square" alt="Built with Rust">
  <img src="https://img.shields.io/badge/license-MIT-blue.svg?style=flat-square" alt="License: MIT">
</p>

<p align="center">
  <img src="assets/file_00000000782082119e0fbfaad14b1fa3.png" alt="Venu Dynamic Notch for Windows showing normal and expanded glass notch states, media controls, AI coding usage tracking for Claude Code, OpenAI Codex and Antigravity, desktop alerts, and system information" width="1200">
</p>

## What is Venu?

Venu brings a Dynamic Island-style experience to Windows and turns the unused space around the top of your desktop into a useful, contextual information surface.

See the time, control media, read notifications, track your current focus, display reminders, monitor AI coding tool usage, and receive alerts from developer tools without constantly switching windows.

Venu is designed to feel native, lightweight, and unobtrusive. It runs locally, uses hardware-accelerated rendering, and does not require an account or cloud service.

## Why Venu?

Your desktop is where you already work. Venu puts useful information where you can see it without adding another full application window.

- Dynamic Notch for Windows
- Live media controls and now-playing information
- Windows desktop notifications
- Claude, Codex, and Antigravity usage tracking
- AI coding workflow notifications
- Scrolling edge marquee for reminders, notes, and text
- Custom glass, acrylic, blur, transparent, dark, and light surfaces
- Wallpaper-aware adaptive colors
- Native Windows system tray integration
- Local configuration with no telemetry or cloud dependency

## AI Coding Usage Tracker

Venu can display AI coding tool usage directly inside the Windows Dynamic Notch.

Supported usage views include:

- **Claude Code**: context window, session cost, and rate limits
- **OpenAI Codex**: usage information directly in the notch
- **Google Antigravity**: usage information directly in the notch

This makes Venu especially useful for developers who spend most of their day working with AI coding agents. Instead of opening separate dashboards or repeatedly checking terminal output, important usage information can stay visible in the desktop HUD.

Venu also includes notification hooks for developer tools and AI agents such as Claude Code, Codex, and Antigravity, allowing scripts and development workflows to send events directly to the Dynamic Notch.

## Features

### Dynamic Notch / Desktop HUD

A compact Dynamic Island-style panel that expands when you need it and stays out of the way when you do not.

- Smooth spring-based expansion and collapse
- Top or bottom placement
- Configurable monitor, width, height, and offset
- Multiple information slides
- Mouse wheel navigation
- Pin mode for keeping the panel expanded
- Optional click-through mode
- Automatic mode that can prioritize active notifications, media, or the clock

Available slides include:

- **Status**: Focus and today's priorities
- **Clock**: Large digital clock and date
- **Media**: Current track, artist, artwork, and playback controls
- **Notifications**: Notification center and unread indicators
- **Wallpaper**: Image preview with optional caption
- **Marquee**: Moving reminder or custom text
- **AI Usage**: Claude, Codex, and Antigravity usage information

### Media Controls

Control media playback without leaving your current application.

Venu integrates with Windows System Media Transport Controls to show the currently playing track, artist, artwork, playback state, and controls for play, pause, previous, and next.

### Notifications

Turn the Dynamic Notch into a lightweight notification hub.

- Live desktop alerts
- Unread notification indicators
- Notification center
- Individual dismissal
- Clear all notifications
- Application-specific colors
- Custom animated notification glow styles
- Local webhook support for external tools and scripts

### AI Agent and Developer Notifications

Venu can act as a small desktop HUD for your development workflow.

CLI hooks in `scripts/notch-hooks/` allow external tools, build scripts, AI agents, and development workflows to send instant notifications to Venu.

This can be useful for events such as:

- AI coding agent task completion
- Long-running build completion
- Test results
- Deployment status
- Background scripts finishing
- Agent attention or approval requests

### Edge Marquee

Display scrolling text around the edges of your Windows desktop.

- Top, bottom, left, and right edges
- Custom text and reminders
- Adjustable speed and direction
- Configurable thickness and spacing
- Custom colors and opacity
- Always-on-top support
- Click-through mode
- Unicode support including Latin, Devanagari, CJK, and symbols

Use it for reminders, quotes, status information, temporary notes, or anything you want to keep visible while working.

### Glass and Surface Themes

Venu includes multiple visual treatments designed to work with different Windows desktops and wallpapers.

- Dark / Obsidian
- Light
- Frosted Glass
- Transparent Glass
- Blurred
- Windows Fluent Acrylic

The notch can sample the desktop wallpaper and extract dominant colors for adaptive accents and themes.

### Modern Color Editor

Venu includes a precision color editor instead of relying on the default Windows color picker.

- 2D saturation and value canvas
- Hue spectrum slider
- Alpha slider
- HEX input
- RGB input
- HSV input
- Curated color swatches
- Live opacity and color preview

### System Tray and Windows Integration

Venu is designed to run quietly in the background.

- System tray support
- Quick controls from the tray
- Single-instance application behavior
- Launch on startup (quiet, tray-only sign-in start)
- Native Windows file dialogs
- Windows 10 and Windows 11 support
- Local JSON configuration

## Privacy First

Venu is designed as a local-first Windows application.

- No account required
- No telemetry
- No cloud dependency
- No mandatory online service
- Configuration stored locally on your PC

AI usage information is displayed locally by Venu based on the integrations and local data available to the application.

## Installation

### Installer (recommended)

Download `venu-setup-x.y.z.exe` from the [GitHub Releases](https://github.com/imnakul/venu---Dynamic-Notch-for-Windows/releases) page and run it. The installer:

- installs Venu per-user into `%LOCALAPPDATA%\Programs\Venu` — no administrator rights needed
- adds a Start Menu shortcut (and optionally a desktop shortcut)
- offers to enable **Launch on startup**, so Venu is already running quietly in the tray the next time you turn your PC on
- ships an uninstaller that removes all of the above

### Portable

Download `venu-x.y.z.exe` from the Releases page and run it directly. No installation, no admin rights.

Either way, **Launch on startup** can be toggled any time from **Settings > App > Preferences**. The setting lives under your user profile (`HKCU\...\CurrentVersion\Run`), keeps pointing at the copy of Venu you last ran, and a sign-in start stays quiet in the tray instead of opening the settings window. Only one Venu runs at a time.

### Build from Source

Venu is built with Rust and targets Windows 10 and Windows 11.

Install the stable [Rust toolchain](https://www.rust-lang.org/tools/install), then run:

```bash
git clone https://github.com/imnakul/venu---Dynamic-Notch-for-Windows.git
cd venu---Dynamic-Notch-for-Windows
cargo build --release
```

The compiled executable will be available at:

```text
target/release/venu.exe
```

To build the Windows installer as well, install [Inno Setup 6](https://jrsoftware.org/isinfo.php) and run:

```bash
iscc packaging/venu.iss
```

The setup executable is written to `dist/venu-setup-<version>.exe`. A tooling-free per-user install/uninstall is also available as `scripts/install.ps1` / `scripts/uninstall.ps1`.

## How to Use Venu

### Dynamic Notch

1. Move your cursor to the configured edge of the screen.
2. The Dynamic Notch expands automatically.
3. Scroll over the notch to move between active slides.
4. Use the pin control to keep it expanded.
5. Configure placement, size, monitor, and behavior from Settings > Notch.

### AI Usage Tracker

Open the AI Usage slide to view supported AI coding tool usage directly in the notch. Configure the available integrations from the corresponding Venu settings.

### Edge Marquee

1. Open Settings > Text.
2. Enter the text you want to display.
3. Open Settings > Appearance and select the screen edges.
4. Adjust thickness, spacing, color, and opacity.
5. Configure speed, direction, and click-through behavior under Settings > Behavior.

### Colors and Themes

1. Open any Venu color setting.
2. Select the color capsule to open the color editor.
3. Adjust saturation, brightness, hue, and opacity.
4. Use HEX, RGB, or HSV input when precise values are required.
5. Select a surface theme such as Dark, Frosted, Transparent, Blurred, or Acrylic.

## Configuration

Venu automatically stores preferences and state locally at:

```text
%APPDATA%\venu\config.json
```

To reset Venu to its default configuration, close the application and delete this file.

## Technology

Venu is built as a native Windows desktop application with a focus on performance and responsive visual rendering.

- **Language**: Rust 2021
- **GUI**: egui and eframe
- **Graphics**: Direct2D and DirectWrite through Windows bindings
- **Backdrop**: GDI screen capture with DWM capture exclusion and Direct2D interpolation
- **Media**: Windows System Media Transport Controls (SMTC / WinRT)
- **Typography**: Plus Jakarta Sans and Noto Sans Devanagari

## For Developers

Venu is also designed to be useful inside a developer workflow.

External applications can communicate with the Venu notification system through local hooks and the notification webhook. This makes it possible to integrate Venu with scripts, CI workflows, AI coding agents, development tools, and other local applications.

The goal is simple: when something important finishes in the background, your desktop should be able to tell you without forcing you to keep another terminal or dashboard open.

## Roadmap

Venu is actively evolving. Planned and experimental areas include improvements to:

- AI coding tool integrations
- Dynamic Notch information surfaces
- Notification integrations
- Windows desktop integration
- Customization and themes
- Developer automation hooks

## Contributing

Contributions, ideas, and bug reports are welcome.

Before submitting changes, run:

```bash
cargo fmt --check
cargo check
```

Please keep changes focused and ensure the project builds without warnings where practical.

## License

Venu is licensed under the [MIT License](LICENSE).

## Keywords

Venu, Dynamic Notch for Windows, Dynamic Island for Windows, Windows Dynamic Island, Windows notch, Windows desktop HUD, Windows desktop companion, Windows notification HUD, Windows media controls, Windows desktop customization, Windows productivity tool, Windows AI coding tools, Claude Code usage tracker, Claude usage tracker, Codex usage tracker, OpenAI Codex usage, Google Antigravity usage tracker, Antigravity usage tracker, AI coding agent notifications, Windows developer tools, Windows Rust application, Rust desktop application, Windows 10, Windows 11.
