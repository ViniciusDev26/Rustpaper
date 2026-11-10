// System audio capture for audio-reactive wallpapers.
//
// Records the monitor of the default output through PipeWire (`pw-record`
// with stream.capture.sink, raw f32 stereo on stdout) in a background thread
// and keeps the latest spectrum (rustpaper_gl::audio::Analyzer) for the render
// loop. If pw-record is missing or fails, the spectrum simply stays silent.

use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};

use rustpaper_gl::audio::{Analyzer, AudioFrame, Publisher, WINDOW};

const RATE: u32 = 48_000;

pub struct AudioCapture {
    child: Child,
    latest: Arc<Mutex<AudioFrame>>,
}

impl AudioCapture {
    pub fn start() -> Option<AudioCapture> {
        let mut child = Command::new("pw-record")
            .args([
                "-P",
                "{ stream.capture.sink=true node.name=rustpaper-audio }",
            ])
            .args([
                "--format",
                "f32",
                "--channels",
                "2",
                "--rate",
                &RATE.to_string(),
                "-",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let mut out = child.stdout.take()?;
        let latest = Arc::new(Mutex::new(AudioFrame::default()));
        let shared = latest.clone();
        std::thread::spawn(move || {
            let mut analyzer = Analyzer::new(RATE as f32);
            let mut publisher = Publisher::default();
            let mut ring: Vec<f32> = Vec::with_capacity(WINDOW * 4);
            // ~60 analyses per second: 800 stereo frames per read
            let mut buf = vec![0u8; 800 * 2 * 4];
            while out.read_exact(&mut buf).is_ok() {
                ring.extend(
                    buf.as_chunks::<4>()
                        .0
                        .iter()
                        .map(|b| f32::from_le_bytes(*b)),
                );
                if ring.len() > WINDOW * 2 {
                    let excess = ring.len() - WINDOW * 2;
                    ring.drain(..excess);
                }
                let frame = publisher.step(analyzer.analyze(&ring), 800.0 / RATE as f32);
                if let Ok(mut l) = shared.lock() {
                    *l = frame;
                }
            }
        });
        Some(AudioCapture { child, latest })
    }

    pub fn frame(&self) -> AudioFrame {
        self.latest.lock().map(|f| *f).unwrap_or_default()
    }
}

impl Drop for AudioCapture {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
