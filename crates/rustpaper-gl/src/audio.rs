// Audio spectrum for audio-reactive wallpapers (`g_AudioSpectrum16Left`...,
// particle emitters with `audioprocessingmode`).
//
// The analysis is pure (samples in, bands out) so it can be tested; capturing
// the system audio is the live engine's job (PipeWire monitor of the default
// output). The CLI replays a recorded file instead (`AudioTrack`,
// `rustpaper-render --audio`); without one every band is 0 (silence).
//
// The model is fitted to WE itself (harness/calibration/run_audio.sh: a scene
// that shows g_AudioSpectrum* as gray cells, captured while a sweep of tones
// plays; fit_audio.py). What WE does, measured 2026-11-09:
//   * band k is a fixed frequency: 45 Hz -> 0, 250 -> 8, 1 kHz -> 32,
//     4 kHz -> 47, 11 kHz -> 61 (ANCHORS); nothing above ~13 kHz;
//   * values are LINEAR magnitudes, not dB (amplitude 0.5 / 0.25 / 0.0625 after
//     a full-scale tone show 0.47 / 0.25 / 0.08);
//   * divided by an adaptive reference shared by all bands: a quiet tone that
//     follows silence shows ~1.0, the same tone after a louder one ~0.5;
//   * no temporal smoothing: a band drops from 1 to 0 within ~70 ms;
//   * a tone also lights its neighbours (~0.3 one band below, ~0.4 above);
//   * the 16/32-band arrays are the maximum of each group of 64 bands.
// Fitted RMSE per band against WE: 0.107 (the previous dB + slow-decay
// analyzer: 0.224). Remaining gap: WE lights more bands for tones below
// ~700 Hz than a 2048-sample FFT can explain.

use rustfft::{FftPlanner, num_complex::Complex};

/// Number of bands kept; 32/16-band versions are maxima of these.
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
    /// `n`-band version (16, 32 or 64) of one channel: the maximum of each
    /// group of the 64 (closer to WE than the average: RMSE 0.10 vs 0.15).
    pub fn bands(&self, right: bool, n: usize) -> Vec<f32> {
        let src = if right { &self.right } else { &self.left };
        let group = (BANDS / n.max(1)).max(1);
        src.chunks(group)
            .map(|c| c.iter().copied().fold(0.0, f32::max))
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

/// Tone frequency (Hz) -> the band WE lights for it (sweep, 2026-11-09).
const ANCHORS: [(f32, f32); 16] = [
    (45.0, 0.0),
    (90.0, 1.0),
    (125.0, 2.0),
    (180.0, 5.0),
    (250.0, 8.0),
    (350.0, 13.0),
    (500.0, 20.0),
    (700.0, 29.0),
    (1000.0, 32.0),
    (1400.0, 35.0),
    (2000.0, 39.0),
    (2800.0, 43.0),
    (4000.0, 47.0),
    (5600.0, 51.0),
    (8000.0, 56.0),
    (11000.0, 61.0),
];
/// How much of a band shows in the 1st / 2nd band below and above it.
const SPREAD_BELOW: [f32; 2] = [0.3, 0.1];
const SPREAD_ABOVE: [f32; 2] = [0.4, 0.15];
/// The lowest bands take 3x more of the bands above them, and every band is
/// smoothed per analysis step. Measured with make_audio.py's cells while
/// CITRUS 2719898311's own music played (harness-out/calib/audio_citrus):
/// WE's band 0 averaged 0.36 and moved in small steps (0.075, 90 of 225
/// frames) while ours averaged 0.19 and jumped (0.169, 28 frames). With these
/// it is 0.36 / 0.084 / 34 frames; the tone sweep and level response do not
/// regress (1 kHz at 0.125: 0.380 = WE). Gate 2026-11-21: CITRUS FAIL ->
/// PASS (energy 1.68 -> 1.19), no regression.
const LOW_BANDS: usize = 8;
const LOW_SPREAD: f32 = 3.0;
const BAND_SMOOTH: f32 = 0.3;
/// Adaptive reference, per analysis step (60 Hz): rises toward the loudest
/// band by ATTACK, otherwise decays by RELEASE (~33 s time constant; the fit
/// barely distinguishes it from no decay, but a live wallpaper must recover
/// after a loud moment), never below REF_MIN (silence stays dark).
const ATTACK: f32 = 0.05;
const RELEASE: f32 = 0.9995;
const REF_MIN: f32 = 0.01;

/// Center frequency of band `k`: log-linear between the anchors.
fn band_center(k: usize) -> f32 {
    let k = k as f32;
    let seg = ANCHORS
        .windows(2)
        .find(|w| k <= w[1].1)
        .unwrap_or(&ANCHORS[ANCHORS.len() - 2..]);
    let ((f0, b0), (f1, b1)) = (seg[0], seg[1]);
    let t = (k - b0) / (b1 - b0);
    (f0.ln() + t * (f1.ln() - f0.ln())).exp()
}

/// FFT + WE's band mapping, spread and adaptive normalization.
pub struct Analyzer {
    fft: std::sync::Arc<dyn rustfft::Fft<f32>>,
    window: Vec<f32>,
    /// FFT bin range of each band, plus its center as a fractional bin.
    bins: Vec<(usize, usize, f32)>,
    reference: f32,
    pub frame: AudioFrame,
}

impl Analyzer {
    pub fn new(sample_rate: f32) -> Analyzer {
        let fft = FftPlanner::new().plan_fft_forward(WINDOW);
        // Hann window
        let window = (0..WINDOW)
            .map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / (WINDOW - 1) as f32).cos())
            .collect();
        let to_bin = |hz: f32| hz / sample_rate * WINDOW as f32;
        let centers: Vec<f32> = (0..BANDS).map(band_center).collect();
        // edges halfway (geometrically) between band centers
        let edge = |k: usize| -> f32 {
            match k {
                0 => centers[0] / (centers[1] / centers[0]).sqrt(),
                k if k == BANDS => {
                    centers[BANDS - 1] * (centers[BANDS - 1] / centers[BANDS - 2]).sqrt()
                }
                k => (centers[k - 1] * centers[k]).sqrt(),
            }
        };
        let clamp = |b: f32| (b as usize).clamp(1, WINDOW / 2 - 2);
        let bins = (0..BANDS)
            .map(|k| {
                (
                    clamp(to_bin(edge(k))),
                    clamp(to_bin(edge(k + 1))),
                    to_bin(centers[k]),
                )
            })
            .collect();
        Analyzer {
            fft,
            window,
            bins,
            reference: REF_MIN,
            frame: AudioFrame::default(),
        }
    }

    /// Feeds the latest `WINDOW` interleaved stereo frames (L, R, L, R...).
    /// Call it at ~60 Hz: the reference's attack/release are per call.
    pub fn analyze(&mut self, interleaved: &[f32]) -> AudioFrame {
        let frames = interleaved.len() / 2;
        if frames < WINDOW {
            return self.frame;
        }
        let tail = &interleaved[(frames - WINDOW) * 2..];
        let mut raw = [[0.0f32; BANDS]; 2];
        for (ch, out) in raw.iter_mut().enumerate() {
            let mut buf: Vec<Complex<f32>> = (0..WINDOW)
                .map(|i| Complex::new(tail[i * 2 + ch] * self.window[i], 0.0))
                .collect();
            self.fft.process(&mut buf);
            // a full-scale sine peaks at WINDOW / 4 with a Hann window
            let mag = |i: usize| buf[i].norm() / (WINDOW as f32 * 0.25);
            let mut bands = [0.0f32; BANDS];
            for (b, &(lo, hi, center)) in self.bins.iter().enumerate() {
                // bands narrower than a bin (the bass) read the spectrum
                // interpolated at their center
                let i = center as usize;
                let interp = mag(i) + (mag(i + 1) - mag(i)) * (center - i as f32);
                bands[b] = (lo..hi).map(mag).fold(interp, f32::max);
            }
            for k in 0..BANDS {
                let mut v = bands[k];
                for (d, w) in SPREAD_BELOW.iter().enumerate() {
                    if let Some(&above) = bands.get(k + d + 1) {
                        let w = if k < LOW_BANDS { w * LOW_SPREAD } else { *w };
                        v = v.max(above * w.min(1.0));
                    }
                }
                for (d, w) in SPREAD_ABOVE.iter().enumerate() {
                    if let Some(below) = k.checked_sub(d + 1) {
                        v = v.max(bands[below] * w);
                    }
                }
                out[k] = v;
            }
        }
        let peak = raw.iter().flatten().copied().fold(0.0, f32::max);
        self.reference = if peak > self.reference {
            self.reference + (peak - self.reference) * ATTACK
        } else {
            (self.reference * RELEASE).max(REF_MIN)
        };
        for (dst, src) in [&mut self.frame.left, &mut self.frame.right]
            .into_iter()
            .zip(raw)
        {
            for (d, v) in dst.iter_mut().zip(src) {
                let x = (v / self.reference).min(1.0);
                *d += (x - *d) * BAND_SMOOTH;
            }
        }
        self.frame
    }
}

/// WE publishes a new spectrum only every ~75 ms (~13 Hz), not every frame.
/// Measured 2026-11-10 on the make_audio.py cells while real music played:
/// 187 changes in 421 frames at 30 fps, gaps of 1 / 2 / 3 frames about
/// equally often (irregular, as if driven by audio packets). In between,
/// shaders keep reading the previous values. Updating at 60 Hz gave
/// audio-driven scenes ~2x WE's motion energy (CITRUS 2719898311: the ring
/// glow far from the ring changed 0.68 per frame vs WE's 0.36).
pub const PUBLISH_INTERVAL: f32 = 0.075;

/// Holds the analyzer's output between WE-style publications: feed it every
/// 60 Hz analysis step, read what WE would show.
#[derive(Default)]
pub struct Publisher {
    shown: AudioFrame,
    /// Seconds of analysis since the last publication.
    since: f32,
    started: bool,
}

impl Publisher {
    pub fn step(&mut self, fresh: AudioFrame, dt: f32) -> AudioFrame {
        self.since += dt;
        if !self.started || self.since >= PUBLISH_INTERVAL {
            self.started = true;
            // keep the remainder: 60 Hz steps then alternate 4 / 5 (13.3 Hz)
            self.since = (self.since - PUBLISH_INTERVAL).max(0.0);
            self.shown = fresh;
        }
        self.shown
    }
}

/// A recorded audio file replayed in step with the scene clock (the harness
/// records what WE heard, `we_audio.wav`, and renders Rustpaper with it).
/// The analyzer advances in fixed 1/60 s steps, like the live capture
/// (`audio_capture.rs` analyzes every 800 frames at 48 kHz), so the decay
/// smoothing behaves the same.
pub struct AudioTrack {
    samples: Vec<f32>,
    rate: f32,
    analyzer: Analyzer,
    publisher: Publisher,
    /// Audio time of the last analysis step, seconds.
    pos: f32,
}

impl AudioTrack {
    pub const STEP: f32 = 1.0 / 60.0;
    /// WE shows the spectrum of a sound ~0.167 s after it plays (fitted on
    /// the calibration capture), so a replayed recording is delayed as much.
    pub const WE_LATENCY: f32 = 0.167;

    /// Interleaved stereo samples at `rate` Hz.
    pub fn new(samples: Vec<f32>, rate: f32) -> AudioTrack {
        AudioTrack {
            samples,
            rate,
            analyzer: Analyzer::new(rate),
            publisher: Publisher::default(),
            pos: -1.0,
        }
    }

    /// Decodes any file ffmpeg reads to 48 kHz stereo f32.
    pub fn load(path: &std::path::Path) -> Result<AudioTrack, String> {
        let out = std::process::Command::new("ffmpeg")
            .args(["-v", "error", "-i"])
            .arg(path)
            .args(["-f", "f32le", "-ac", "2", "-ar", "48000", "-"])
            .output()
            .map_err(|e| format!("starting ffmpeg: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "ffmpeg could not decode {}: {}",
                path.display(),
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        let samples = out
            .stdout
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| f32::from_le_bytes(*b))
            .collect();
        Ok(AudioTrack::new(samples, 48_000.0))
    }

    /// Spectrum WE would show at audio time `t` seconds (`WE_LATENCY` late;
    /// before the start or after the end of the recording counts as silence). Times must not go backwards by
    /// more than a step; going back restarts the analysis.
    pub fn frame_at(&mut self, t: f32) -> AudioFrame {
        let t = t - Self::WE_LATENCY;
        if t + Self::STEP < self.pos {
            self.analyzer = Analyzer::new(self.rate);
            self.publisher = Publisher::default();
            self.pos = -1.0;
        }
        if self.pos < 0.0 {
            // warm up from a second before, enough for the decay to settle
            self.pos = (t - 1.0).max(0.0) - Self::STEP;
        }
        while self.pos + Self::STEP <= t {
            self.pos += Self::STEP;
            let end = (self.pos * self.rate) as usize * 2;
            let start = end.saturating_sub(WINDOW * 2);
            let mut window = vec![0.0f32; WINDOW * 2];
            let have = &self.samples[start.min(self.samples.len())..end.min(self.samples.len())];
            let off = WINDOW * 2 - (end - start);
            window[off..off + have.len()].copy_from_slice(have);
            let fresh = self.analyzer.analyze(&window);
            self.publisher.step(fresh, Self::STEP);
        }
        self.publisher.shown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publisher_updates_at_we_rate() {
        let mut p = Publisher::default();
        let mut changes = 0;
        let mut last = -1.0;
        for i in 0..600 {
            // 10 s of 60 Hz steps, each with a different value
            let mut f = AudioFrame::default();
            f.left[0] = i as f32;
            let shown = p.step(f, 1.0 / 60.0).left[0];
            if shown != last {
                changes += 1;
                last = shown;
            }
        }
        // ~13.3 Hz: 133 publications in 10 s
        assert!((130..=136).contains(&changes), "{changes}");
    }

    fn sine(hz: f32, amp: f32, frames: usize) -> Vec<f32> {
        (0..frames)
            .flat_map(|i| {
                let v = (std::f32::consts::TAU * hz * i as f32 / 48_000.0).sin() * amp;
                [v, v]
            })
            .collect()
    }

    fn peak(v: &[f32; BANDS]) -> usize {
        (0..BANDS).max_by(|&x, &y| v[x].total_cmp(&v[y])).unwrap()
    }

    #[test]
    fn tones_land_on_we_bands_with_neighbours_and_smoothing() {
        let mut a = Analyzer::new(48_000.0);
        // WE: 1 kHz -> band 32, 4 kHz -> 47, 250 Hz -> 8 (calibration sweep)
        // (tolerance: bass bands are narrower than an FFT bin, the known gap)
        for (hz, band, tol) in [(1000.0, 32, 1), (4000.0, 47, 1), (250.0, 8, 2)] {
            let tone = sine(hz, 0.5, WINDOW);
            // the reference needs ~1 s to settle: before that everything
            // above it saturates at 1 (WE does the same after silence)
            for _ in 0..60 {
                a.analyze(&tone);
            }
            let f = a.analyze(&tone);
            let p = peak(&f.left);
            assert!(p.abs_diff(band) <= tol, "{hz} Hz -> band {p}, WE {band}");
            assert!(f.left[p] > 0.9, "{hz} Hz peak {}", f.left[p]);
        }
        // BAND_SMOOTH: a new sound settles within ~20 analysis steps
        let tone = sine(1000.0, 0.5, WINDOW);
        let mut f = a.analyze(&tone);
        for _ in 0..30 {
            f = a.analyze(&tone);
        }
        let p = peak(&f.left);
        assert!(
            f.left[p + 1] > 0.2 && f.left[p + 5] < 0.05,
            "spread to neighbours only"
        );
        // ...and silence fades out (0.7 per step), dark after ~1/3 s
        let mut silent = a.analyze(&vec![0.0; WINDOW * 2]);
        assert!(silent.left[p] > 0.1, "not dark at once");
        for _ in 0..20 {
            silent = a.analyze(&vec![0.0; WINDOW * 2]);
        }
        assert!(silent.left.iter().all(|&v| v < 0.01));
        assert_eq!(silent.bands(false, 16).len(), 16);
    }

    #[test]
    fn levels_are_linear_relative_to_the_loudest_recent_sound() {
        let mut a = Analyzer::new(48_000.0);
        let loud = sine(1000.0, 1.0, WINDOW);
        for _ in 0..120 {
            a.analyze(&loud); // 2 s: the reference settles on the loud tone
        }
        let quiet = sine(1000.0, 0.5, WINDOW);
        let mut half = a.analyze(&quiet);
        for _ in 0..30 {
            half = a.analyze(&quiet); // let BAND_SMOOTH settle
        }
        let v = half.left[peak(&half.left)];
        // WE: 0.47 for half the amplitude after a full-scale tone
        assert!((0.4..0.6).contains(&v), "{v}");
        assert!(half.response(3, 8, 8, (0.0, 1.0), 1.0) > 0.3, "16-band max");
    }

    #[test]
    fn a_track_is_silent_before_its_tone_and_lit_during_it() {
        let rate = 48_000.0;
        // 1 s of silence, then 1 s of 1 kHz
        let samples: Vec<f32> = (0..(2.0 * rate) as usize)
            .flat_map(|i| {
                let v = if i < rate as usize {
                    0.0
                } else {
                    (std::f32::consts::TAU * 1000.0 * i as f32 / rate).sin() * 0.5
                };
                [v, v]
            })
            .collect();
        let mut t = AudioTrack::new(samples, rate);
        let quiet = t.frame_at(0.5).left.iter().fold(0.0f32, |a, &b| a.max(b));
        let loud = t.frame_at(1.5).left.iter().fold(0.0f32, |a, &b| a.max(b));
        assert!(quiet < 0.05 && loud > 0.7, "{quiet} {loud}");
        // jumping back restarts the analysis instead of keeping the tone
        let again = t.frame_at(0.5).left.iter().fold(0.0f32, |a, &b| a.max(b));
        assert!(again < 0.05, "{again}");
        // past the end: silence, decaying
        assert!(t.frame_at(10.0).left.iter().all(|&v| v < 0.05));
    }
}
