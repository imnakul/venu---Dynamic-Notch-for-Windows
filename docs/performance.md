# CPU investigation

The performance change in PR #1 is present in main and v0.4.1. It gates notch
repaints, but three paths still cause unnecessary work:

- `NotchState::tick` advanced `marquee_offset` whenever the panel was open and
  the deck contained Moving text, including when another slide was visible.
  It also advanced the offset with marquee scrolling disabled. The offset is
  part of `FrameKey`, so these invisible changes caused a full notch repaint
  every tick, bypassing the gate. Visibility now matches the painter, including
  carousel transitions, and static marquees do not advance.
- The overlay loop rendered immediately after every message wake. Its wait
  timeout did not limit rendering when messages arrived early. The loop now
  pumps messages immediately but waits for a frame deadline before ticking
  the renderers. Input traffic cannot increase the intended render cadence.
- Scrolling edge strips still created a text format, shaped a probe, and shaped
  at least 50 repeated phrases per frame, per edge. Text layouts are now cached
  by text, font, spacing, dimensions and edge, and repetitions cover only the
  visible extent plus two copies. Color and speed changes reuse the layout;
  text/font/spacing/geometry changes rebuild it. Zero-speed strips repaint on
  changes and the two-second layout heartbeat instead of every frame.

These fixes do not remove the work needed for visible animation. Moving text,
notification glows and FlashScreen still animate. The notch's ambient pulse and
glass backdrop still repaint at roughly 10 fps, and each such frame still does
Direct2D drawing and a layered-window blit. A collapsed pulse-free slide used to
repaint on the one-second clock key even when it showed no clock; the clock key
now only counts while a Clock slide is visible (see below).
Those are further optimization candidates if profiling shows they dominate.

## Follow-up: brushes, clock key, GPU counter

- Every fill and stroke used to create its own `ID2D1SolidColorBrush`, a few
  hundred COM objects per painted frame. The painter now keeps one per render
  target and recolours it between draw calls.
- `FrameKey::clock` is zero unless a Clock slide is on screen, so a collapsed
  Status, Media or Notifications pill no longer repaints every second. The
  Usage slide had been relying on that repaint to show `/usage` posts, so it
  now has its own revision counter in the key.
- The `GPU Engine(*)` PDH wildcard is collected every other second instead of
  every second, into a reused buffer, without copying each instance name.
  CPU and RAM still sample every second.

Not measured yet; use the procedure below.

## Validation

Five tests exercise the actual state implementation: hidden slides, static
marquees, visible marquees in both sizes, carousel entry, and reverse scrolling.
The hidden-slide and static-marquee tests fail against the original main code;
all five pass with the fix. CI now runs `cargo test --locked` on Windows.

Development validation ran those state tests in a small Linux harness importing
the repository's `config.rs`, `anim.rs` and `state.rs`. All Windows Rust source,
including tests, passed `cargo check --locked --tests --target
x86_64-pc-windows-msvc` using a temporary manifest with resource embedding
excluded: the Linux environment lacks `llvm-rc`. Formatting and patch whitespace
checks passed. Windows linking, visual behavior and CPU usage have not been
measured here; no CPU percentage improvement is claimed.

## Measure on Windows

Build main and this branch with `cargo build --release --locked`. Fully exit Venu
using its tray menu before switching binaries; closing Settings only hides it,
and the single-instance mutex otherwise leaves the old executable running.
Keep the same saved configuration and use the same machine for both builds.

For each build, allow 10 seconds to settle and measure these cases separately:

1. Settings closed, notch collapsed on Status, no edge strips, no active toast
   or flash, pointer away from the notch.
2. The same, with Status pinned open and Moving text still in the deck.
3. Moving text active with its scrolling switch off, then with scrolling on.
4. One edge strip enabled, then all four, then speed set to zero.
5. Repeat with Frosted/Blurred/Acrylic, and with mouse movement over the notch.
6. Open Settings separately to distinguish its rendering cost.

This PowerShell sample reports average CPU over 30 seconds, normalized across
logical processors like Task Manager:

```powershell
$venuProcess = @(Get-Process -Name venu -ErrorAction Stop)
if ($venuProcess.Count -ne 1) { throw "Expected exactly one Venu process" }
$venuProcess = $venuProcess[0]
$logicalProcessors = (Get-CimInstance Win32_ComputerSystem).NumberOfLogicalProcessors
$cpuStart = $venuProcess.TotalProcessorTime.TotalSeconds
$timer = [Diagnostics.Stopwatch]::StartNew()
Start-Sleep -Seconds 30
$venuProcess.Refresh()
$elapsed = $timer.Elapsed.TotalSeconds
$cpuDelta = $venuProcess.TotalProcessorTime.TotalSeconds - $cpuStart
"Average CPU: {0:N2}%" -f (100 * $cpuDelta / $elapsed / $logicalProcessors)
```

Repeat each sample three times. If CPU remains high, record a CPU Usage sampling
trace with Windows Performance Recorder and inspect Venu's stacks in Windows
Performance Analyzer. In particular, distinguish the overlay thread's
DirectWrite/GDI/Direct2D work from the Settings thread's egui/OpenGL work and
the media poller's WinRT calls before choosing the next optimization.
