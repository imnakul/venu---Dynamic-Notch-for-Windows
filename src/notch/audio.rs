//! The live music wave: equaliser bars that follow what is actually playing.
//!
//! Real per-band levels would need the audio stream itself (a loopback
//! capture and an FFT on every frame). Windows already keeps a peak meter
//! for every output device, the same one the volume mixer shows, and reading
//! it is a shared-memory read. The bars here are that one live level shaped
//! into a few bars that move at slightly different rates, so they rise on a
//! beat, fall in a quiet passage and lie flat in silence.
//!
//! Sampled at [`WAVE_FRAME`] and only while a wave is on screen and something
//! is playing, so a paused or hidden player costs nothing.

use std::time::{Duration, Instant};

use windows::Win32::Media::Audio::Endpoints::IAudioMeterInformation;
use windows::Win32::Media::Audio::{eConsole, eRender, IMMDeviceEnumerator, MMDeviceEnumerator};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_ALL, CLSCTX_INPROC_SERVER};

/// How often the bars move: 25 frames a second reads as fluid for a meter
/// and keeps the repaint rate well under the display's.
pub const WAVE_FRAME: Duration = Duration::from_millis(40);

/// The default output can change (headphones plugged in), so the meter is
/// re-acquired this often while in use.
const REACQUIRE: Duration = Duration::from_secs(4);

/// Bars in the wave.
pub const BARS: usize = crate::notch::layout::WAVE_BARS;

/// Bar rest height when silent, as a fraction of full.
const REST: f32 = 0.22;

pub struct AudioMeter {
    meter: Option<IAudioMeterInformation>,
    acquired_at: Option<Instant>,
    sampled_at: Option<Instant>,
    /// Smoothed level, 0..1.
    envelope: f32,
    clock: f32,
    bars: [f32; BARS],
}

impl Default for AudioMeter {
    fn default() -> Self {
        Self {
            meter: None,
            acquired_at: None,
            sampled_at: None,
            envelope: 0.0,
            clock: 0.0,
            bars: [REST; BARS],
        }
    }
}

impl AudioMeter {
    /// Advance the wave. `live` is whether it is on screen and playing; when
    /// it is not, the bars settle at rest and the meter is released.
    /// Returns the bar heights, 0..1.
    pub fn tick(&mut self, live: bool) -> [f32; BARS] {
        if !live {
            self.meter = None;
            self.acquired_at = None;
            self.sampled_at = None;
            self.envelope = 0.0;
            self.bars = [REST; BARS];
            return self.bars;
        }

        let now = Instant::now();
        let dt = match self.sampled_at {
            Some(at) if now.duration_since(at) < WAVE_FRAME => return self.bars,
            Some(at) => now.duration_since(at).as_secs_f32().min(0.2),
            None => WAVE_FRAME.as_secs_f32(),
        };
        self.sampled_at = Some(now);

        if self
            .acquired_at
            .is_none_or(|at| now.duration_since(at) >= REACQUIRE)
        {
            self.meter = acquire();
            self.acquired_at = Some(now);
        }
        let peak = self
            .meter
            .as_ref()
            .and_then(|m| unsafe { m.GetPeakValue() }.ok())
            .unwrap_or(0.0);

        self.clock += dt;
        self.envelope = follow(self.envelope, peak, dt);
        self.bars = shape(self.envelope, self.clock);
        self.bars
    }
}

fn acquire() -> Option<IAudioMeterInformation> {
    unsafe {
        let devices: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_INPROC_SERVER).ok()?;
        let device = devices.GetDefaultAudioEndpoint(eRender, eConsole).ok()?;
        device
            .Activate::<IAudioMeterInformation>(CLSCTX_ALL, None)
            .ok()
    }
}

/// A meter's ballistics: rise almost at once, fall back over a few hundred
/// milliseconds. The square root makes quiet music visible without loud
/// music pinning the bars.
pub fn follow(envelope: f32, peak: f32, dt: f32) -> f32 {
    let target = peak.clamp(0.0, 1.0).sqrt();
    if target > envelope {
        envelope + (target - envelope) * (dt * 30.0).min(1.0)
    } else {
        envelope + (target - envelope) * (dt * 5.0).min(1.0)
    }
}

/// Bar heights for a level: each bar sways at its own rate within the level,
/// so they never move in lockstep, and all of them lie at rest in silence.
pub fn shape(level: f32, clock: f32) -> [f32; BARS] {
    const RATES: [f32; BARS] = [7.3, 9.7, 6.1];
    const PHASES: [f32; BARS] = [0.0, 2.1, 4.2];
    let mut bars = [REST; BARS];
    for (i, bar) in bars.iter_mut().enumerate() {
        let sway = 0.5 + 0.5 * (clock * RATES[i] + PHASES[i]).sin();
        *bar = (REST + level * (0.45 + 0.55 * sway) * (1.0 - REST)).clamp(REST, 1.0);
    }
    bars
}

/// Bars quantised for the frame key: a change smaller than a pixel at any
/// size the bars are drawn is not worth a repaint.
pub fn key(bars: &[f32; BARS]) -> [u8; BARS] {
    bars.map(|b| (b.clamp(0.0, 1.0) * 40.0) as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silence_rests_and_sound_lifts_every_bar() {
        assert_eq!(shape(0.0, 1.0), [REST; BARS]);
        let loud = shape(1.0, 1.0);
        assert!(loud.iter().all(|&b| b > REST + 0.2 && b <= 1.0));
    }

    #[test]
    fn the_envelope_rises_fast_and_falls_slowly() {
        let up = follow(0.0, 1.0, 0.04);
        assert!(up > 0.9);
        let down = follow(1.0, 0.0, 0.04);
        assert!(down > 0.7 && down < 1.0);
    }
}
