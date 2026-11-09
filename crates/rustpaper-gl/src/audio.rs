// Audio spectrum for audio-reactive wallpapers (`g_AudioSpectrum16Left`...,
// particle emitters with `audioprocessingmode`).
//
// The analysis is pure (samples in, bands out) so it can be tested; capturing
// the system audio is the live engine's job (PipeWire monitor of the default
// output). Without audio (CLI, harness) every band is 0 — silence, which is
// also what the reference WE captures hear.

use rustfft::{FftPlanner, num_complex::Complex};

/// Number of bands kept; 32/16-band versions are averages of these.
pub const BANDS: usize = 64;
/// Samples per FFT window (per channel).
pub const WINDOW: usize = 2048;

/// Spectrum of one frame, 0..1 per band, low to high frequency.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AudioFrame {
    pub left: [f32; BANDS],
    pub right: [f32; BANDS],
}

impl Default for AudioFrame {
    fn default() -> Self {
        AudioFrame {
            left: [0.0; BANDS],
            right: [0.0; BANDS],
        }
    }
}

impl AudioFrame {
    /// `n`-band version (16, 32 or 64) of one channel: averages of the 64.
    pub fn bands(&self, right: bool, n: usize) -> Vec<f32> {
        let src = if right { &self.right } else { &self.left };
        let group = (BANDS / n.max(1)).max(1);
        src.chunks(group)
            .map(|c| c.iter().sum::<f32>() / c.len() as f32)
            .collect()
    }

    /// Response of a WE "audio processing" setting: average of 16-band bins
    /// `start..=end` on the chosen channel (1 left, 2 right, 3 both), mapped
    /// through `bounds` (smoothstep) and `exponent`.
    pub fn response(
        &self,
        mode: u32,
        start: usize,
        end: usize,
        bounds: (f32, f32),
        exponent: f32,
    ) -> f32 {
        let pick = |right: bool| {
            let b = self.bands(right, 16);
            let (s, e) = (start.min(15), end.clamp(start.min(15), 15));
            b[s..=e].iter().sum::<f32>() / (e - s + 1) as f32
        };
        let v = match mode {
            1 => pick(false),
            2 => pick(true),
            _ => (pick(false) + pick(true)) * 0.5,
        };
        let t = ((v - bounds.0) / (bounds.1 - bounds.0).max(1e-4)).clamp(0.0, 1.0);
        let s = t * t * (3.0 - 2.0 * t);
        s.powf(exponent.max(0.01))
    }
}

/// FFT + band mapping with WE-like smoothing (instant attack, slow decay).
pub struct Analyzer {
    fft: std::sync::Arc<dyn rustfft::Fft<f32>>,
    window: Vec<f32>,
    edges: Vec<usize>,
    pub frame: AudioFrame,
}

impl Analyzer {
    pub fn new(sample_rate: f32) -> Analyzer {
        let fft = FftPlanner::new().plan_fft_forward(WINDOW);
        // Hann window
        let window = (0..WINDOW)
            .map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / (WINDOW - 1) as f32).cos())
            .collect();
        // log-spaced band edges between 30 Hz and 16 kHz, as FFT bin indices
        let bin = |hz: f32| ((hz / sample_rate * WINDOW as f32) as usize).clamp(1, WINDOW / 2 - 1);
        let (lo, hi) = (30.0f32.ln(), 16_000.0f32.ln());
        let edges = (0..=BANDS)
            .map(|k| bin((lo + (hi - lo) * k as f32 / BANDS as f32).exp()))
            .collect();
        Analyzer {
            fft,
            window,
            edges,
            frame: AudioFrame::default(),
        }
    }

    /// Feeds the latest `WINDOW` interleaved stereo frames (L, R, L, R...).
    pub fn analyze(&mut self, interleaved: &[f32]) -> AudioFrame {
        let frames = interleaved.len() / 2;
        if frames < WINDOW {
            return self.frame;
        }
        let tail = &interleaved[(frames - WINDOW) * 2..];
        for ch in 0..2 {
            let mut buf: Vec<Complex<f32>> = (0..WINDOW)
                .map(|i| Complex::new(tail[i * 2 + ch] * self.window[i], 0.0))
                .collect();
            self.fft.process(&mut buf);
            let out = if ch == 0 {
                &mut self.frame.left
            } else {
                &mut self.frame.right
            };
            for (b, slot) in out.iter_mut().enumerate() {
                let (a, z) = (self.edges[b], self.edges[b + 1].max(self.edges[b] + 1));
                let mag =
                    buf[a..z].iter().map(|c| c.norm()).fold(0.0, f32::max) / (WINDOW as f32 * 0.25);
                // -60 dB .. 0 dB -> 0 .. 1
                let level = ((20.0 * mag.max(1e-6).log10() + 60.0) / 60.0).clamp(0.0, 1.0);
                *slot = level.max(*slot * 0.85);
            }
        }
        self.frame
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tone_lights_its_band_and_silence_decays() {
        let rate = 48_000.0;
        let mut a = Analyzer::new(rate);
        // 1 kHz sine, both channels
        let tone: Vec<f32> = (0..WINDOW * 2)
            .flat_map(|i| {
                let v = (std::f32::consts::TAU * 1000.0 * i as f32 / rate).sin() * 0.5;
                [v, v]
            })
            .collect();
        let f = a.analyze(&tone);
        let peak = (0..BANDS)
            .max_by(|&x, &y| f.left[x].total_cmp(&f.left[y]))
            .unwrap();
        // 1 kHz sits around the middle of a 30 Hz..16 kHz log scale
        assert!((30..45).contains(&peak), "peak band {peak}");
        assert!(f.left[peak] > 0.7 && f.left[2] < 0.3);
        let silent = a.analyze(&vec![0.0; WINDOW * 2]);
        assert!(
            silent.left[peak] < f.left[peak] && silent.left[peak] > 0.5,
            "slow decay"
        );
        assert_eq!(silent.bands(false, 16).len(), 16);
        assert!(silent.response(3, 8, 12, (0.0, 1.0), 1.0) > 0.0);
    }
}
