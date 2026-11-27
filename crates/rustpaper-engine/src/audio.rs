//! Wallpaper Engine's volume and mute, set on its audio streams in the
//! user's sound server (PipeWire or PulseAudio, through `pactl`).
//!
//! WE's `-control mute|unmute` (and `pause|play`) do nothing to the windows
//! we open with `-playInWindow`: measured with a scene playing music, the
//! stream's peak stayed ~3400 after each of them, with and without
//! `-location <window>` or `-monitor 0`, on Wine's X11 and Wayland drivers
//! alike (2026-11-23). And WE has no volume command at all. Its streams
//! (`application.name` "Wallpaper Engine") take a volume and a mute like
//! any other, so the watchdog keeps them at the user's choice. WE opens a
//! new stream per wallpaper, and the user can change one (pavucontrol), so
//! one `pactl subscribe` watches the sink inputs: the streams are listed
//! again only after it reports one new or changed, or when the volume or
//! mute asked for changed. Listing them every tick instead (a `pactl`
//! connection each 2 s) came to ~187k connections to pipewire-pulse in 4.6
//! days (~2 kB each, 2026-11-27).
//!
//! Through Wine's ALSA driver (launch.rs: never winepulse) WE's stream is
//! named "ALSA plug-in [wine64-preloader]" instead; its
//! `application.process.id` is WE's pid, the host's (pressure-vessel keeps
//! the pid namespace), which identifies it.

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

const APP: &str = "Wallpaper Engine";

/// Keeps WE's streams at a volume, or muted.
#[derive(Default)]
pub struct WeAudio {
    /// `pactl` failed to run once: not tried again (logged).
    broken: bool,
    /// The running `pactl subscribe` (None until the first apply, or after
    /// it exited: sound server restarted).
    watch: Option<Watch>,
    /// What the streams were last brought to.
    applied: Option<(u8, bool)>,
}

/// `pactl subscribe` and the flag its reader thread raises.
struct Watch {
    child: Child,
    /// A sink input appeared or changed since the last listing.
    dirty: Arc<AtomicBool>,
    /// The reader saw `pactl` exit.
    done: Arc<AtomicBool>,
}

impl Drop for Watch {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Watch {
    fn start() -> std::io::Result<Self> {
        let mut child = Command::new("pactl")
            .arg("subscribe")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let out = child.stdout.take().expect("piped stdout");
        let dirty = Arc::new(AtomicBool::new(false));
        let done = Arc::new(AtomicBool::new(false));
        let (d, e) = (dirty.clone(), done.clone());
        std::thread::spawn(move || {
            for line in BufReader::new(out).lines() {
                let Ok(line) = line else { break };
                if stream_event(&line) {
                    d.store(true, Ordering::SeqCst);
                }
            }
            e.store(true, Ordering::SeqCst);
        });
        Ok(Self { child, dirty, done })
    }
}

/// A `pactl subscribe` line about a new or changed sink input.
fn stream_event(line: &str) -> bool {
    line.contains(" on sink-input ")
        && (line.starts_with("Event 'new'") || line.starts_with("Event 'change'"))
}

impl WeAudio {
    /// Sets every WE stream to `volume` (0..100), muted when `mute`.
    pub fn apply(&mut self, volume: u8, mute: bool) {
        if self.broken {
            return;
        }
        if self
            .watch
            .as_ref()
            .is_some_and(|w| w.done.load(Ordering::SeqCst))
        {
            self.watch = None;
        }
        if self.watch.is_none() {
            match Watch::start() {
                Ok(w) => self.watch = Some(w),
                Err(e) => {
                    eprintln!("audio: pactl: {e}; Wallpaper Engine's volume is not managed");
                    self.broken = true;
                    return;
                }
            }
            // events before it started were missed
            self.applied = None;
        }
        let dirty = self
            .watch
            .as_ref()
            .is_some_and(|w| w.dirty.swap(false, Ordering::SeqCst));
        if !dirty && self.applied == Some((volume, mute)) {
            return;
        }
        let out = Command::new("pactl")
            .args(["-f", "json", "list", "sink-inputs"])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output();
        let Ok(out) = out.map(|o| o.stdout).and_then(|o| {
            serde_json::from_slice::<serde_json::Value>(&o).map_err(std::io::Error::other)
        }) else {
            // no sound server right now: next tick
            self.applied = None;
            return;
        };
        // our own changes come back as 'change' events: one more listing,
        // which then finds nothing to change
        for args in changes(&out, volume, mute, crate::we::pid()) {
            let _ = Command::new("pactl")
                .args(&args)
                .stdin(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
        self.applied = Some((volume, mute));
    }
}

/// The `pactl` commands that bring WE's streams in `streams` (pactl's JSON)
/// to `volume` / `mute`. WE's streams carry its name, or come from its
/// process `we_pid`.
fn changes(
    streams: &serde_json::Value,
    volume: u8,
    mute: bool,
    we_pid: Option<u32>,
) -> Vec<[String; 3]> {
    let Some(list) = streams.as_array() else {
        return Vec::new();
    };
    let want = format!("{}%", volume.min(100));
    let mut cmds = Vec::new();
    for s in list {
        let props = &s["properties"];
        let from_we = we_pid.is_some_and(|p| {
            props["application.process.id"].as_str() == Some(p.to_string().as_str())
        });
        if props["application.name"].as_str() != Some(APP) && !from_we {
            continue;
        }
        let Some(index) = s["index"].as_u64().map(|i| i.to_string()) else {
            continue;
        };
        if s["mute"].as_bool() != Some(mute) {
            let arg = if mute { "1" } else { "0" };
            cmds.push(["set-sink-input-mute".into(), index.clone(), arg.into()]);
        }
        let level_ok = s["volume"].as_object().is_some_and(|c| {
            !c.is_empty()
                && c.values()
                    .all(|v| v["value_percent"].as_str().map(str::trim) == Some(want.as_str()))
        });
        if !level_ok {
            cmds.push(["set-sink-input-volume".into(), index, want.clone()]);
        }
    }
    cmds
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_we_streams_off_target_change() {
        let streams = serde_json::json!([
            {"index": 7, "mute": false,
             "volume": {"front-left": {"value_percent": "100%"}, "front-right": {"value_percent": "100%"}},
             "properties": {"application.name": "Wallpaper Engine"}},
            {"index": 8, "mute": true,
             "volume": {"front-left": {"value_percent": "40%"}},
             "properties": {"application.name": "Wallpaper Engine"}},
            {"index": 9, "mute": false,
             "volume": {"front-left": {"value_percent": "100%"}},
             "properties": {"application.name": "rustpaper"}},
            {"index": 10, "mute": false,
             "volume": {"front-left": {"value_percent": "100%"}},
             "properties": {"application.name": "ALSA plug-in [wine64-preloader]",
                            "application.process.id": "4242"}}
        ]);
        let s = |a: &[&str; 3]| a.map(String::from);
        assert_eq!(
            changes(&streams, 40, true, None),
            vec![
                s(&["set-sink-input-mute", "7", "1"]),
                s(&["set-sink-input-volume", "7", "40%"]),
            ]
        );
        assert_eq!(
            changes(&streams, 100, false, None),
            vec![
                s(&["set-sink-input-mute", "8", "0"]),
                s(&["set-sink-input-volume", "8", "100%"]),
            ]
        );
        // WE on Wine's ALSA driver: its stream, found by its pid
        assert_eq!(
            changes(&streams, 100, false, Some(4242)),
            vec![
                s(&["set-sink-input-mute", "8", "0"]),
                s(&["set-sink-input-volume", "8", "100%"]),
            ]
        );
        assert_eq!(
            changes(&streams, 100, true, Some(4242)),
            vec![
                s(&["set-sink-input-mute", "7", "1"]),
                s(&["set-sink-input-volume", "8", "100%"]),
                s(&["set-sink-input-mute", "10", "1"]),
            ]
        );
        assert!(changes(&serde_json::json!([]), 0, true, None).is_empty());
    }

    #[test]
    fn only_new_or_changed_sink_inputs_wake_it() {
        assert!(stream_event("Event 'new' on sink-input #191752"));
        assert!(stream_event("Event 'change' on sink-input #191752"));
        assert!(!stream_event("Event 'remove' on sink-input #191752"));
        assert!(!stream_event("Event 'change' on sink #56"));
        assert!(!stream_event("Event 'new' on client #77"));
    }
}
