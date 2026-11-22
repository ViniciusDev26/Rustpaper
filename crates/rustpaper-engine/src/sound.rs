// Plays a scene's `sound` objects (music, ambience) like WE does.
//
// Each sound object gets a thread that plays its files one at a time with
// `ffplay` (part of ffmpeg, already a runtime dependency) through the default
// output. That output is also what `audio_capture.rs` listens to, so
// audio-reactive layers react to the wallpaper's own music, as in WE.
// Playback stops while the wallpaper is paused (every output covered by a
// fullscreen window) and the current track restarts when it resumes.
// Without ffplay the wallpaper is simply silent.
//
// The selector stops an engine with SIGKILL, so `Drop` never runs: each
// ffplay asks the kernel to SIGKILL it when its parent dies
// (PR_SET_PDEATHSIG), otherwise it kept playing the track to its end after
// the wallpaper was switched or removed.

use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;

use rustpaper_gl::scene::SceneSound;

pub struct SoundPlayer {
    /// Volume (f32 bits) of each playing sound object, by object id: a
    /// SceneScript can change it; the next file played uses it.
    volumes: Vec<(i64, Arc<AtomicU32>)>,
    /// Held (not playing) per sound object: `startsilent`, or a SceneScript
    /// called stop() on it.
    held: Vec<(i64, Arc<AtomicBool>)>,
    paused: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    dir: PathBuf,
    /// The user's master volume (0..1, `--volume`): scales every sound,
    /// including the volumes a SceneScript sets.
    master: f32,
}

impl SoundPlayer {
    /// Starts every playable sound; None when there is nothing to play.
    pub fn start(sounds: &[SceneSound], master: f32) -> Option<SoundPlayer> {
        let playable: Vec<&SceneSound> = sounds
            .iter()
            // `startsilent` sounds are loaded too, held until a SceneScript
            // calls play() on them
            .filter(|s| s.volume > 0.0 && !s.files.is_empty())
            .collect();
        if playable.is_empty() {
            return None;
        }
        // ffplay needs files: write the tracks to a private runtime folder
        let base = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        remove_stale_dirs(&base);
        let dir = base.join(format!("rustpaper-sounds-{}", std::process::id()));
        std::fs::create_dir_all(&dir).ok()?;
        let paused = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let mut volumes = Vec::new();
        let mut held = Vec::new();
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
            let vol = Arc::new(AtomicU32::new((volume * master).to_bits()));
            volumes.push((s.id, vol.clone()));
            let hold = Arc::new(AtomicBool::new(s.start_silent));
            held.push((s.id, hold.clone()));
            println!(
                "sound {:?}: {} file(s), {mode}, volume {volume:.2} (master {master:.2})",
                s.name,
                files.len()
            );
            std::thread::spawn(move || {
                play_loop(&files, &mode, &vol, (min_t, max_t), &paused, &hold, &stop)
            });
        }
        Some(SoundPlayer {
            volumes,
            held,
            paused,
            stop,
            dir,
            master,
        })
    }

    /// A SceneScript called play() (true) or stop() (false) on a sound.
    pub fn set_playing(&self, id: i64, playing: bool) {
        for (_, h) in self.held.iter().filter(|(i, _)| *i == id) {
            h.store(!playing, Ordering::Relaxed);
        }
    }

    /// A sound object's volume (0..1) changed (SceneScript `volume`).
    pub fn set_volume(&self, id: i64, volume: f32) {
        for (_, v) in self.volumes.iter().filter(|(i, _)| *i == id) {
            let volume = volume.clamp(0.0, 1.0) * self.master;
            v.store(volume.to_bits(), Ordering::Relaxed);
        }
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

/// Deletes the track folders of engines that were killed (SIGKILL skips
/// `Drop`): `rustpaper-sounds-<pid>` whose process is gone.
fn remove_stale_dirs(base: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(base) else {
        return;
    };
    for e in entries.flatten() {
        let name = e.file_name();
        let Some(pid) = name
            .to_str()
            .and_then(|n| n.strip_prefix("rustpaper-sounds-"))
            .and_then(|p| p.parse::<u32>().ok())
        else {
            continue;
        };
        if !std::path::Path::new(&format!("/proc/{pid}")).exists() {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

fn play_loop(
    files: &[PathBuf],
    mode: &str,
    volume: &AtomicU32,
    gap: (f32, f32),
    paused: &AtomicBool,
    held: &AtomicBool,
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
        let volume = f32::from_bits(volume.load(Ordering::Relaxed));
        if !play(file, volume, paused, held, stop) {
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
fn play(
    file: &PathBuf,
    volume: f32,
    paused: &AtomicBool,
    held: &AtomicBool,
    stop: &AtomicBool,
) -> bool {
    let waiting = || paused.load(Ordering::Relaxed) || held.load(Ordering::Relaxed);
    loop {
        while waiting() {
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
            if waiting() {
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
    let parent = std::process::id() as libc::pid_t;
    let mut cmd = Command::new("ffplay");
    cmd.args(["-nodisp", "-autoexit", "-loglevel", "quiet", "-volume"])
        .arg(((volume * 100.0).round() as u32).to_string())
        .arg(file)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // SAFETY: runs in the forked child before exec and only makes
    // async-signal-safe syscalls (prctl, getppid, _exit).
    unsafe {
        cmd.pre_exec(move || {
            // The death signal follows the *thread* that forked: this sound
            // thread lives until its ffplay is reaped, so that is fine.
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            // the engine may have died before prctl took effect
            if libc::getppid() != parent {
                libc::_exit(0);
            }
            Ok(())
        });
    }
    cmd.spawn().ok()
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
