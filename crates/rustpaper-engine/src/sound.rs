// Plays a scene's `sound` objects (music, ambience) like WE does.
//
// Each sound object gets a thread that plays its files one at a time with
// `ffplay` (part of ffmpeg, already a runtime dependency) through the default
// output. That output is also what `audio_capture.rs` listens to, so
// audio-reactive layers react to the wallpaper's own music, as in WE.
// Playback stops while the wallpaper is paused (every output covered by a
// fullscreen window) and the current track restarts when it resumes.
// Without ffplay the wallpaper is simply silent.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use rustpaper_gl::scene::SceneSound;

pub struct SoundPlayer {
    paused: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    dir: PathBuf,
}

impl SoundPlayer {
    /// Starts every playable sound; None when there is nothing to play.
    pub fn start(sounds: &[SceneSound]) -> Option<SoundPlayer> {
        let playable: Vec<&SceneSound> = sounds
            .iter()
            .filter(|s| !s.start_silent && s.volume > 0.0 && !s.files.is_empty())
            .collect();
        if playable.is_empty() {
            return None;
        }
        // ffplay needs files: write the tracks to a private runtime folder
        let base = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let dir = base.join(format!("rustpaper-sounds-{}", std::process::id()));
        std::fs::create_dir_all(&dir).ok()?;
        let paused = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        for (i, s) in playable.into_iter().enumerate() {
            let files: Vec<PathBuf> = s
                .files
                .iter()
                .enumerate()
                .filter_map(|(j, (name, bytes))| {
                    let ext = name.rsplit('.').next().unwrap_or("mp3");
                    let path = dir.join(format!("{i}-{j}.{ext}"));
                    std::fs::write(&path, bytes).ok().map(|_| path)
                })
                .collect();
            let (mode, volume) = (s.mode.clone(), s.volume);
            let (min_t, max_t) = (s.min_time, s.max_time.max(s.min_time));
            let (paused, stop) = (paused.clone(), stop.clone());
            println!(
                "sound {:?}: {} file(s), {mode}, volume {volume:.2}",
                s.name,
                files.len()
            );
            std::thread::spawn(move || {
                play_loop(&files, &mode, volume, (min_t, max_t), &paused, &stop)
            });
        }
        Some(SoundPlayer { paused, stop, dir })
    }

    pub fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::Relaxed);
    }
}

impl Drop for SoundPlayer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // the threads kill their ffplay within ~100 ms
        std::thread::sleep(Duration::from_millis(150));
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn play_loop(
    files: &[PathBuf],
    mode: &str,
    volume: f32,
    gap: (f32, f32),
    paused: &AtomicBool,
    stop: &AtomicBool,
) {
    let mut seed = std::process::id() as u64 ^ 0x9E37_79B9_7F4A_7C15;
    let mut rand = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed >> 11) as f32 / (1u64 << 53) as f32
    };
    let mut next = 0usize;
    loop {
        let file = match mode {
            "random" => &files[(rand() * files.len() as f32) as usize % files.len()],
            _ => &files[next % files.len()],
        };
        next += 1;
        if !play(file, volume, paused, stop) {
            return; // stopping
        }
        match mode {
            "single" if next >= files.len() => return,
            "random" => {
                let wait = gap.0 + (gap.1 - gap.0) * rand();
                if !sleep_unless_stopped(wait, stop) {
                    return;
                }
            }
            _ => {}
        }
    }
}

/// Plays one file to the end; false when asked to stop. A pause kills the
/// player and the file starts again once resumed.
fn play(file: &PathBuf, volume: f32, paused: &AtomicBool, stop: &AtomicBool) -> bool {
    loop {
        while paused.load(Ordering::Relaxed) {
            if !sleep_unless_stopped(0.1, stop) {
                return false;
            }
        }
        let Some(mut child) = spawn(file, volume) else {
            // no ffplay: stay silent but keep honoring stop
            return sleep_unless_stopped(f32::MAX, stop);
        };
        loop {
            if stop.load(Ordering::Relaxed) {
                kill(&mut child);
                return false;
            }
            if paused.load(Ordering::Relaxed) {
                kill(&mut child);
                break; // replay once resumed
            }
            match child.try_wait() {
                Ok(Some(_)) | Err(_) => return true,
                Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            }
        }
    }
}

fn spawn(file: &PathBuf, volume: f32) -> Option<Child> {
    Command::new("ffplay")
        .args(["-nodisp", "-autoexit", "-loglevel", "quiet", "-volume"])
        .arg(((volume * 100.0).round() as u32).to_string())
        .arg(file)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()
}

fn kill(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn sleep_unless_stopped(secs: f32, stop: &AtomicBool) -> bool {
    let mut left = secs;
    while left > 0.0 {
        if stop.load(Ordering::Relaxed) {
            return false;
        }
        let step = left.min(0.1);
        std::thread::sleep(Duration::from_secs_f32(step));
        left -= step;
    }
    true
}
