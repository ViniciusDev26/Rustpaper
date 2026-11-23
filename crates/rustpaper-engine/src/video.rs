//! Video wallpapers (project.json `"type": "video"`) played by mpv with
//! hardware decoding, instead of Wallpaper Engine.
//!
//! A deliberate exception to "fidelity comes from WE": a video wallpaper is
//! a looping video file, so mpv shows the same picture, and WE under Proton
//! decodes it on the CPU (Proton's Media Foundation goes through
//! GStreamer/FFmpeg in software; NVDEC stayed at 0%). The same 1080p24 H.264
//! wallpaper on an RTX 4060 Ti (2026-11-23): WE 22.7% of a core; `mpv
//! --hwdec=auto` 1.9% (NVDEC 3%); `mpv --hwdec=no` 14.3%. Scenes and web
//! wallpapers stay on WE.
//!
//! mpv is a Wayland client of the private stage (stage.rs): it connects to
//! the stage's own socket, its window is titled [`window_title`] (never
//! WE's `rustpaper-<output>`, so a leftover WE window and mpv can't be
//! mistaken for each other), and the stage makes it fullscreen at its
//! monitor's size and forwards its dmabufs like WE's. The engine tells the
//! stage which of the two a monitor shows (`PrivateStage::set_feed`).
//!
//! Each player has an IPC socket (`--input-ipc-server`): pause/play for the
//! Playback rules, mute and volume on reload, and `loadfile` to switch
//! videos without a new window.

use std::io::Write as _;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use rustpaper_core::project::{Project, WallpaperKind};

/// Least time between two starts of a monitor's mpv (one that exits right
/// away, e.g. a broken file, is not restarted in a tight loop).
const RESTART_INTERVAL: Duration = Duration::from_secs(10);

/// Title of the mpv window that shows the video of monitor `output`.
pub fn window_title(output: &str) -> String {
    format!("rustpaper-mpv-{output}")
}

/// The video file of the wallpaper whose project.json is `project`, when it
/// is a video wallpaper and the file exists.
pub fn file_of(project: &Path) -> Option<PathBuf> {
    let dir = project.parent()?;
    let p = Project::load(dir).ok()?;
    if p.kind != WallpaperKind::Video || p.file.is_empty() {
        return None;
    }
    let file = p.file_path(dir);
    file.is_file().then_some(file)
}

/// mpv is on PATH.
pub fn mpv_installed() -> bool {
    rustpaper_core::paths::program("mpv").is_some()
}

/// How mpv shows a video on a monitor.
#[derive(Clone, Debug, PartialEq)]
pub struct Playback {
    pub file: PathBuf,
    pub mute: bool,
    pub volume: u8,
}

/// The mpv command line (without the program) for monitor `output`.
fn args(output: &str, p: &Playback, ipc: &Path) -> Vec<String> {
    let mut a: Vec<String> = [
        // the user's mpv.conf and scripts stay out (shaders, upscalers,
        // keybindings, resume-playback would change cost and behaviour)
        "--no-config",
        "--really-quiet",
        "--vo=gpu",
        // NVDEC/VA-API/Vulkan when they decode the format, else software;
        // `auto-safe` skips the methods mpv considers unreliable
        "--hwdec=auto-safe",
        "--loop-file=inf",
        "--force-window=yes",
        "--idle=no",
        // a wallpaper: no OSD, OSC, input or cursor
        "--osc=no",
        "--osd-level=0",
        "--input-default-bindings=no",
        "--input-vo-keyboard=no",
        "--input-cursor=no",
        "--cursor-autohide=always",
        "--sub-auto=no",
        "--border=no",
        // cover the monitor (scale to fill, crop the overflow), like WE's
        // default for videos; the stage gives the window the monitor's size
        "--keepaspect-window=no",
        "--panscan=1.0",
        "--audio-client-name=rustpaper",
    ]
    .map(String::from)
    .to_vec();
    a.push(format!("--title={}", window_title(output)));
    a.push(format!("--mute={}", if p.mute { "yes" } else { "no" }));
    a.push(format!("--volume={}", p.volume.min(100)));
    a.push(format!("--input-ipc-server={}", ipc.display()));
    a.push("--".into());
    a.push(p.file.display().to_string());
    a
}

/// One monitor's mpv.
pub struct Player {
    output: String,
    child: Child,
    ipc: PathBuf,
    /// The stage's Wayland socket name (to start mpv again).
    socket: String,
    pub playback: Playback,
    started: Instant,
    paused: bool,
}

impl Player {
    /// Starts mpv for `output` on the stage's Wayland socket `socket`.
    pub fn spawn(output: &str, socket: &str, playback: Playback) -> Result<Player, String> {
        let ipc = ipc_path(output);
        let child = start(output, socket, &playback, &ipc)?;
        eprintln!(
            "{output}: mpv (pid {}) plays {}",
            child.id(),
            playback.file.display()
        );
        Ok(Player {
            output: output.to_string(),
            child,
            ipc,
            socket: socket.to_string(),
            playback,
            started: Instant::now(),
            paused: false,
        })
    }

    /// Shows `playback` from now on: another file through `loadfile` (same
    /// window, no flash), mute/volume as properties.
    pub fn update(&mut self, playback: Playback) {
        if playback.file != self.playback.file {
            eprintln!("{}: mpv plays {}", self.output, playback.file.display());
            self.command(&["loadfile", &playback.file.display().to_string(), "replace"]);
            if self.paused {
                // loadfile keeps `pause`; the next watchdog round decides
                self.command_json(serde_json::json!(["set_property", "pause", false]));
                self.paused = false;
            }
        }
        if playback.mute != self.playback.mute {
            self.command_json(serde_json::json!(["set_property", "mute", playback.mute]));
        }
        if playback.volume != self.playback.volume {
            self.command_json(serde_json::json!([
                "set_property",
                "volume",
                playback.volume
            ]));
        }
        self.playback = playback;
    }

    /// Pauses or resumes (the Playback rules).
    pub fn set_paused(&mut self, paused: bool) {
        if paused != self.paused
            && self.command_json(serde_json::json!(["set_property", "pause", paused]))
        {
            self.paused = paused;
        }
    }

    /// Restarts mpv when it exited (crash, killed); true when it runs.
    pub fn tend(&mut self) -> bool {
        match self.child.try_wait() {
            Ok(None) => return true,
            Ok(Some(status)) => eprintln!("{}: mpv exited ({status})", self.output),
            Err(e) => eprintln!("{}: mpv: {e}", self.output),
        }
        if self.started.elapsed() < RESTART_INTERVAL {
            return false;
        }
        self.started = Instant::now();
        match start(&self.output, &self.socket, &self.playback, &self.ipc) {
            Ok(child) => {
                eprintln!("{}: mpv restarted (pid {})", self.output, child.id());
                self.child = child;
                self.paused = false;
                true
            }
            Err(e) => {
                eprintln!("{}: {e}", self.output);
                false
            }
        }
    }

    fn command(&self, cmd: &[&str]) -> bool {
        self.command_json(serde_json::json!(cmd))
    }

    /// Sends one IPC command; the reply is not read (fire and forget, the
    /// next watchdog round corrects anything that did not take).
    fn command_json(&self, cmd: serde_json::Value) -> bool {
        let line = format!("{}\n", serde_json::json!({ "command": cmd }));
        let sent = UnixStream::connect(&self.ipc).and_then(|mut s| {
            s.set_write_timeout(Some(Duration::from_millis(500)))?;
            s.write_all(line.as_bytes())
        });
        if let Err(e) = &sent {
            eprintln!("{}: mpv IPC: {e}", self.output);
        }
        sent.is_ok()
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.ipc);
    }
}

fn ipc_path(output: &str) -> PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("rustpaper");
    let _ = std::fs::create_dir_all(&dir);
    dir.join(format!("mpv-{}-{output}.sock", std::process::id()))
}

fn start(output: &str, socket: &str, p: &Playback, ipc: &Path) -> Result<Child, String> {
    let mut cmd = Command::new("mpv");
    cmd.args(args(output, p, ipc))
        // only the stage: never the user's compositor or an X server
        .env("WAYLAND_DISPLAY", socket)
        .env_remove("WAYLAND_SOCKET")
        .env_remove("DISPLAY")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    // SAFETY: prctl is async-signal-safe; it only sets a flag on the child.
    unsafe {
        cmd.pre_exec(|| {
            // mpv goes with the engine, even when the engine is killed.
            // SIGKILL: on SIGTERM after its compositor (the stage) vanished,
            // mpv hung in a futex wait and stayed (2026-11-23)
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL);
            Ok(())
        });
    }
    cmd.spawn().map_err(|e| format!("could not start mpv: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_video_wallpapers_with_their_file_play_in_mpv() {
        let dir = std::env::temp_dir().join(format!("rustpaper-video-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let project = dir.join("project.json");
        std::fs::write(&project, r#"{"type":"Video","file":"clip.mp4"}"#).unwrap();
        assert_eq!(file_of(&project), None, "the file is missing");
        std::fs::write(dir.join("clip.mp4"), b"").unwrap();
        assert_eq!(file_of(&project), Some(dir.join("clip.mp4")));
        std::fs::write(&project, r#"{"type":"scene","file":"scene.json"}"#).unwrap();
        assert_eq!(file_of(&project), None, "scenes stay on WE");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mpv_runs_as_a_silent_wallpaper_window() {
        let p = Playback {
            file: PathBuf::from("/w/-odd name.mp4"),
            mute: true,
            volume: 40,
        };
        let a = args("DP-1", &p, Path::new("/run/x.sock"));
        for want in [
            "--title=rustpaper-mpv-DP-1",
            "--hwdec=auto-safe",
            "--loop-file=inf",
            "--mute=yes",
            "--volume=40",
            "--input-ipc-server=/run/x.sock",
            "--no-config",
        ] {
            assert!(a.iter().any(|x| x == want), "{want} in {a:?}");
        }
        // the file comes after `--`: a name starting with '-' is no option
        assert_eq!(&a[a.len() - 2..], ["--", "/w/-odd name.mp4"]);
        assert_ne!(window_title("DP-1"), crate::desktop::window_title("DP-1"));
    }
}
