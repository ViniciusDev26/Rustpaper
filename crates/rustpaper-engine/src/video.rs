// `video` module: decodes a video using ffmpeg as a subprocess. A
// thread reads raw RGBA frames from ffmpeg's stdout and always keeps the most recent one
// in a shared slot. The renderer reads that slot when it wants to update the texture.

use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;

// The most recent frame + a "generation" counter (incremented on every new frame),
// so the renderer knows whether it has already seen this frame.
struct Frame {
    data: Vec<u8>,
    generation: u64,
}

pub struct Video {
    pub width: u32,
    pub height: u32,
    slot: Arc<Mutex<Frame>>,
    child: Mutex<Option<Child>>, // kept only to kill ffmpeg on Drop
}

impl Video {
    pub fn open(path: &str) -> Self {
        let (width, height) = probe(path);
        let frame_size = (width * height * 4) as usize; // RGBA = 4 bytes/pixel

        let slot = Arc::new(Mutex::new(Frame {
            data: vec![0u8; frame_size], // starts black until the 1st frame arrives
            generation: 0,
        }));

        // ffmpeg: -stream_loop -1 = repeat forever; -re = real-time pace (paces
        // at the video's fps); output = rawvideo RGBA on stdout.
        let mut child = Command::new("ffmpeg")
            .args([
                "-loglevel",
                "error",
                "-stream_loop",
                "-1",
                "-re",
                "-i",
                path,
                "-f",
                "rawvideo",
                "-pix_fmt",
                "rgba",
                "-",
            ])
            .stdout(Stdio::piped())
            .spawn()
            .expect("failed to start ffmpeg (is it installed?)");

        let mut stdout = child.stdout.take().unwrap();
        let slot_thread = Arc::clone(&slot);

        // Decoding thread: reads one whole frame at a time and updates the slot.
        thread::spawn(move || {
            loop {
                let mut frame = vec![0u8; frame_size];
                // read_exact fills the whole buffer (blocks until a complete frame is available).
                if stdout.read_exact(&mut frame).is_err() {
                    break; // ffmpeg finished or the pipe broke
                }
                let mut s = slot_thread.lock().unwrap();
                s.data = frame;
                s.generation += 1;
            }
        });

        Video {
            width,
            height,
            slot,
            child: Mutex::new(Some(child)),
        }
    }

    // If there is a frame newer than `last_seen`, calls `f` with its bytes
    // (holding the lock - no extra copy) and returns the new generation.
    pub fn upload_if_newer<F: FnOnce(&[u8])>(&self, last_seen: u64, f: F) -> Option<u64> {
        let s = self.slot.lock().unwrap();
        if s.generation > last_seen {
            f(&s.data);
            Some(s.generation)
        } else {
            None
        }
    }
}

impl Drop for Video {
    fn drop(&mut self) {
        if let Some(mut c) = self.child.lock().unwrap().take() {
            let _ = c.kill();
        }
    }
}

// Discovers the video's width/height by running ffprobe.
fn probe(path: &str) -> (u32, u32) {
    let out = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height",
            "-of",
            "csv=p=0",
            path,
        ])
        .output()
        .expect("failed to run ffprobe");
    let s = String::from_utf8_lossy(&out.stdout);
    parse_dimensions(&s).expect("invalid ffprobe output")
}

// PURE parse of ffprobe's output ("1920,1080"). Separate from the subprocess so it is
// testable. Returns None if the input doesn't have two valid numbers.
fn parse_dimensions(s: &str) -> Option<(u32, u32)> {
    let s = s.trim();
    let mut it = s.split(',');
    let w = it.next()?.trim().parse().ok()?;
    let h = it.next()?.trim().parse().ok()?;
    Some((w, h))
}

#[cfg(test)]
mod tests {
    use super::parse_dimensions;

    #[test]
    fn parses_valid() {
        assert_eq!(parse_dimensions("1920,1080"), Some((1920, 1080)));
    }

    #[test]
    fn parses_with_whitespace() {
        // ffprobe sometimes appends \n/whitespace.
        assert_eq!(parse_dimensions("  1920,1080\n"), Some((1920, 1080)));
    }

    #[test]
    fn rejects_garbage() {
        assert_eq!(parse_dimensions("abc"), None);
        assert_eq!(parse_dimensions("1920"), None); // the height is missing
        assert_eq!(parse_dimensions(""), None);
    }
}
