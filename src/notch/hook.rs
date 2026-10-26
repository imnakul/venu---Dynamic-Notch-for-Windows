//! The mouse hook, on a thread of its own.
//!
//! A `WH_MOUSE_LL` hook is called *synchronously on the thread that installed
//! it*, for every mouse event in the system. Windows delivers it through that
//! thread's message queue and waits for the callback to return before the
//! event continues to whatever is underneath. So if the installing thread is
//! busy — rendering, or sleeping between frames — the cursor stalls for as
//! long as the thread takes to get back to its pump. That is a system-wide
//! stutter, not a stutter in our window.
//!
//! The hook therefore lives here, on a dedicated thread whose entire job is to
//! block in `GetMessageW`. The callback touches nothing but atomics and
//! returns immediately, so it never becomes the slow link in mouse input.
//! Everything the callback needs to make a decision is published as plain
//! integers by the render thread.
//!
//! It watches for four things: the wheel, for switching slides; a
//! left-plus-right button chord over the notch, which toggles click-through;
//! the user's chosen mouse trigger (middle click, a side button, or a
//! modifier plus click) over the notch, which opens or closes it; and, while
//! the notch is held open by a click, any press outside it, which closes it.
//! The chord has to be caught here rather than in the window procedure,
//! because the whole point of click-through is that the window stops
//! receiving mouse messages — once it is on, no click could ever reach the
//! notch to turn it back off again.

use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::Once;

use std::sync::atomic::AtomicU8;

use windows::Win32::Foundation::{LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, VK_CONTROL, VK_LBUTTON, VK_MENU, VK_RBUTTON, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetMessageW, SetWindowsHookExW, MSG, MSLLHOOKSTRUCT, WH_MOUSE_LL,
    WINDOWS_HOOK_ID, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEWHEEL,
    WM_RBUTTONDOWN, WM_RBUTTONUP, WM_XBUTTONDOWN, WM_XBUTTONUP,
};

use crate::config::MouseTrigger;

/// One wheel notch, in the units Windows reports.
const WHEEL_DELTA: i32 = 120;

/// Accumulated notches waiting to be consumed by the render thread.
static WHEEL: AtomicI32 = AtomicI32::new(0);

/// Screen-space bounds of the notch. Only read by the callback.
static BOUND_L: AtomicI32 = AtomicI32::new(0);
static BOUND_T: AtomicI32 = AtomicI32::new(0);
static BOUND_R: AtomicI32 = AtomicI32::new(0);
static BOUND_B: AtomicI32 = AtomicI32::new(0);

/// False whenever the panel is closed or wheel-switching is turned off, in
/// which case the callback does nothing at all beyond a single atomic load.
static WHEEL_ACTIVE: AtomicBool = AtomicBool::new(false);

/// False whenever there is no notch on screen to toggle.
static CHORD_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Live button state, tracked here because a click-through window never sees
/// the presses itself.
static L_DOWN: AtomicBool = AtomicBool::new(false);
static R_DOWN: AtomicBool = AtomicBool::new(false);

/// Latched when both buttons were held together over the notch, until the
/// render thread consumes it.
static CHORD: AtomicBool = AtomicBool::new(false);

/// While a chord is in progress every remaining button event belongs to it and
/// must not reach anything else — otherwise letting go would drop a stray
/// right-click on whatever is underneath.
static SWALLOWING: AtomicBool = AtomicBool::new(false);

/// The user's mouse trigger, as a [`MouseTrigger`] discriminant. Zero is off.
static TRIGGER_KIND: AtomicU8 = AtomicU8::new(0);
/// Latched when the trigger fired over the notch.
static TRIGGERED: AtomicBool = AtomicBool::new(false);
/// The release that ends a swallowed trigger press, so it is swallowed too
/// instead of reaching whatever is underneath on its own: one of the
/// `RELEASE_*` values, or zero.
static SWALLOW_RELEASE: AtomicU8 = AtomicU8::new(0);
const RELEASE_LEFT: u8 = 1;
const RELEASE_MIDDLE: u8 = 2;
const RELEASE_X: u8 = 3;

/// While the notch is held open by a click, a press anywhere outside it
/// closes it. Never swallowed: the click still does what it was for.
static WATCH_OUTSIDE: AtomicBool = AtomicBool::new(false);
static OUTSIDE: AtomicBool = AtomicBool::new(false);

static INSTALL: Once = Once::new();

/// Install the hook on its own thread. Idempotent; safe to call every frame.
pub fn install() {
    INSTALL.call_once(|| {
        std::thread::Builder::new()
            .name("notch-mouse-hook".into())
            .spawn(|| unsafe {
                let instance = match GetModuleHandleW(None) {
                    Ok(h) => h,
                    Err(e) => {
                        eprintln!("[notch] hook thread: no module handle: {e:?}");
                        return;
                    }
                };

                let hook = match SetWindowsHookExW(
                    WINDOWS_HOOK_ID(WH_MOUSE_LL.0),
                    Some(proc),
                    instance,
                    0,
                ) {
                    Ok(h) => h,
                    Err(e) => {
                        eprintln!(
                            "[notch] mouse hook unavailable ({e:?}); \
                             wheel switching and the click-through chord are off"
                        );
                        return;
                    }
                };
                let _ = hook;

                // A low-level hook is only serviced while its thread pumps.
                // This loop never exits; the thread dies with the process.
                let mut msg = MSG::default();
                while GetMessageW(&mut msg, None, 0, 0).as_bool() {}
            })
            .ok();
    });
}

/// What the callback needs to know, published once per frame.
#[derive(Debug, Clone, Copy, Default)]
pub struct Watch {
    /// Scroll over the notch switches slides.
    pub wheel: bool,
    /// The left-plus-right chord toggles click-through.
    pub chord: bool,
    /// The mouse gesture that opens or closes the notch.
    pub trigger: MouseTrigger,
    /// The notch is held open by a click: a press outside it closes it.
    pub outside: bool,
    /// Screen-space bounds of the notch.
    pub bounds: (i32, i32, i32, i32),
}

/// Publish what the callback needs to know, once per frame.
pub fn publish(watch: Watch) {
    let (left, top, right, bottom) = watch.bounds;
    BOUND_L.store(left, Ordering::Relaxed);
    BOUND_T.store(top, Ordering::Relaxed);
    BOUND_R.store(right, Ordering::Relaxed);
    BOUND_B.store(bottom, Ordering::Relaxed);
    TRIGGER_KIND.store(trigger_code(watch.trigger), Ordering::Release);
    WATCH_OUTSIDE.store(watch.outside, Ordering::Release);
    WHEEL_ACTIVE.store(watch.wheel, Ordering::Release);
    CHORD_ACTIVE.store(watch.chord, Ordering::Release);
}

/// Stand everything down, for when there is no notch at all.
pub fn silence() {
    publish(Watch::default());
}

/// Consume everything scrolled since the last call.
pub fn take_wheel() -> i32 {
    WHEEL.swap(0, Ordering::AcqRel)
}

/// Consume a pending left-plus-right press over the notch, if there was one.
pub fn take_chord() -> bool {
    CHORD.swap(false, Ordering::AcqRel)
}

/// Consume a pending mouse-trigger press over the notch.
pub fn take_trigger() -> bool {
    TRIGGERED.swap(false, Ordering::AcqRel)
}

/// Consume a pending press outside the notch.
pub fn take_outside() -> bool {
    OUTSIDE.swap(false, Ordering::AcqRel)
}

/// Whether a mouse button is held right now, e.g. mid-drag. Hover never opens
/// the notch then: dragging a window or a file to the top of the screen is
/// not a request to open it.
pub fn button_held() -> bool {
    let tracked = L_DOWN.load(Ordering::Acquire) || R_DOWN.load(Ordering::Acquire);
    // Confirmed against the buttons themselves, so a release some other hook
    // swallowed before it reached this one cannot leave hover switched off.
    tracked && (key_down(VK_LBUTTON.0) || key_down(VK_RBUTTON.0))
}

fn trigger_code(trigger: MouseTrigger) -> u8 {
    match trigger {
        MouseTrigger::Off => 0,
        MouseTrigger::MiddleClick => 1,
        MouseTrigger::BackButton => 2,
        MouseTrigger::ForwardButton => 3,
        MouseTrigger::CtrlClick => 4,
        MouseTrigger::AltClick => 5,
        MouseTrigger::ShiftClick => 6,
    }
}

/// Whether a key or button is down. Asked on a left press while a modifier
/// trigger is configured and by `button_held`, never on mouse moves.
fn key_down(vk: u16) -> bool {
    unsafe { GetAsyncKeyState(vk as i32) as u16 & 0x8000 != 0 }
}

/// Whether this button press is the configured trigger. `x_button` is 1 for
/// back and 2 for forward on side-button presses, zero otherwise.
fn is_trigger(msg: u32, x_button: u16) -> bool {
    match TRIGGER_KIND.load(Ordering::Acquire) {
        1 => msg == WM_MBUTTONDOWN,
        2 => msg == WM_XBUTTONDOWN && x_button == 1,
        3 => msg == WM_XBUTTONDOWN && x_button == 2,
        4 => msg == WM_LBUTTONDOWN && key_down(VK_CONTROL.0),
        5 => msg == WM_LBUTTONDOWN && key_down(VK_MENU.0),
        6 => msg == WM_LBUTTONDOWN && key_down(VK_SHIFT.0),
        _ => false,
    }
}

/// Whether the published bounds contain this point.
fn within(pt: POINT) -> bool {
    pt.x >= BOUND_L.load(Ordering::Relaxed)
        && pt.x <= BOUND_R.load(Ordering::Relaxed)
        && pt.y >= BOUND_T.load(Ordering::Relaxed)
        && pt.y <= BOUND_B.load(Ordering::Relaxed)
}

unsafe extern "system" fn proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code < 0 {
        return CallNextHookEx(None, code, wparam, lparam);
    }

    // Moves are the overwhelming majority of what arrives here and match
    // neither arm, so the common case costs one integer compare.
    match wparam.0 as u32 {
        WM_MOUSEWHEEL if WHEEL_ACTIVE.load(Ordering::Acquire) => {
            let info = &*(lparam.0 as *const MSLLHOOKSTRUCT);
            if within(info.pt) {
                let delta = ((info.mouseData >> 16) & 0xFFFF) as i16 as i32;
                WHEEL.fetch_add(delta / WHEEL_DELTA, Ordering::AcqRel);
                // Swallow, so the window underneath does not scroll as well.
                return LRESULT(1);
            }
        }

        msg @ (WM_LBUTTONDOWN | WM_LBUTTONUP | WM_RBUTTONDOWN | WM_RBUTTONUP) => {
            let left = msg == WM_LBUTTONDOWN || msg == WM_LBUTTONUP;
            let down = msg == WM_LBUTTONDOWN || msg == WM_RBUTTONDOWN;

            // Record first, so the "is the other one held?" test below reads a
            // state that already includes this event.
            if left {
                L_DOWN.store(down, Ordering::Release);
            } else {
                R_DOWN.store(down, Ordering::Release);
            }

            if msg == WM_LBUTTONUP && SWALLOW_RELEASE.load(Ordering::Acquire) == RELEASE_LEFT {
                SWALLOW_RELEASE.store(0, Ordering::Release);
                return LRESULT(1);
            }

            if SWALLOWING.load(Ordering::Acquire) {
                // Both buttons back up: the chord is over.
                if !L_DOWN.load(Ordering::Acquire) && !R_DOWN.load(Ordering::Acquire) {
                    SWALLOWING.store(false, Ordering::Release);
                }
                return LRESULT(1);
            }

            if down {
                let info = &*(lparam.0 as *const MSLLHOOKSTRUCT);
                let inside = within(info.pt);
                if !inside && WATCH_OUTSIDE.load(Ordering::Acquire) {
                    OUTSIDE.store(true, Ordering::Release);
                }

                if CHORD_ACTIVE.load(Ordering::Acquire) {
                    let other = if left {
                        R_DOWN.load(Ordering::Acquire)
                    } else {
                        L_DOWN.load(Ordering::Acquire)
                    };
                    if other && inside {
                        CHORD.store(true, Ordering::Release);
                        SWALLOWING.store(true, Ordering::Release);
                        return LRESULT(1);
                    }
                }

                if msg == WM_LBUTTONDOWN && inside && is_trigger(msg, 0) {
                    TRIGGERED.store(true, Ordering::Release);
                    SWALLOW_RELEASE.store(RELEASE_LEFT, Ordering::Release);
                    return LRESULT(1);
                }
            }
        }

        msg @ (WM_MBUTTONDOWN | WM_XBUTTONDOWN) => {
            let info = &*(lparam.0 as *const MSLLHOOKSTRUCT);
            let inside = within(info.pt);
            if !inside && WATCH_OUTSIDE.load(Ordering::Acquire) {
                OUTSIDE.store(true, Ordering::Release);
            }
            let x_button = ((info.mouseData >> 16) & 0xFFFF) as u16;
            if inside && is_trigger(msg, x_button) {
                TRIGGERED.store(true, Ordering::Release);
                let release = if msg == WM_MBUTTONDOWN {
                    RELEASE_MIDDLE
                } else {
                    RELEASE_X
                };
                SWALLOW_RELEASE.store(release, Ordering::Release);
                return LRESULT(1);
            }
        }

        msg @ (WM_MBUTTONUP | WM_XBUTTONUP) => {
            let release = if msg == WM_MBUTTONUP {
                RELEASE_MIDDLE
            } else {
                RELEASE_X
            };
            if SWALLOW_RELEASE.load(Ordering::Acquire) == release {
                SWALLOW_RELEASE.store(0, Ordering::Release);
                return LRESULT(1);
            }
        }

        _ => {}
    }

    CallNextHookEx(None, code, wparam, lparam)
}
