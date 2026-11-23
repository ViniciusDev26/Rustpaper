//! Wallpaper Engine's volume and mute, set on its audio streams in the
//! user's sound server (PipeWire or PulseAudio, through `pactl`).
//!
//! WE's `-control mute|unmute` (and `pause|play`) do nothing to the windows
//! we open with `-playInWindow`: measured with a scene playing music, the
//! stream's peak stayed ~3400 after each of them, with and without
//! `-location <window>` or `-monitor 0`, on Wine's X11 and Wayland drivers
//! alike (2026-11-23). And WE has no volume command at all. Its streams
//! (`application.name` "Wallpaper Engine") take a volume and a mute like
//! any other, so the watchdog keeps them at the user's choice; WE opens a
//! new stream per wallpaper, hence the check on every tick rather than once.

use std::process::{Command, Stdio};

const APP: &str = "Wallpaper Engine";

/// Keeps WE's streams at a volume, or muted.
#[derive(Default)]
pub struct WeAudio {
    /// `pactl` failed to run once: not tried again (logged).
    broken: bool,
}

impl WeAudio {
    /// Sets every WE stream to `volume` (0..100), muted when `mute`.
    pub fn apply(&mut self, volume: u8, mute: bool) {
        if self.broken {
            return;
        }
        let out = Command::new("pactl")
            .args(["-f", "json", "list", "sink-inputs"])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output();
        let out = match out {
            Ok(o) if o.status.success() => o.stdout,
            Ok(_) => return, // no sound server right now: next tick
            Err(e) => {
                eprintln!("audio: pactl: {e}; Wallpaper Engine's volume is not managed");
                self.broken = true;
                return;
            }
        };
        let Ok(streams) = serde_json::from_slice::<serde_json::Value>(&out) else {
            return;
        };
        for args in changes(&streams, volume, mute) {
            let _ = Command::new("pactl")
                .args(&args)
                .stdin(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    }
}

/// The `pactl` commands that bring WE's streams in `streams` (pactl's JSON)
/// to `volume` / `mute`.
fn changes(streams: &serde_json::Value, volume: u8, mute: bool) -> Vec<[String; 3]> {
    let Some(list) = streams.as_array() else {
        return Vec::new();
    };
    let want = format!("{}%", volume.min(100));
    let mut cmds = Vec::new();
    for s in list {
        if s["properties"]["application.name"].as_str() != Some(APP) {
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
             "properties": {"application.name": "rustpaper"}}
        ]);
        let s = |a: &[&str; 3]| a.map(String::from);
        assert_eq!(
            changes(&streams, 40, true),
            vec![
                s(&["set-sink-input-mute", "7", "1"]),
                s(&["set-sink-input-volume", "7", "40%"]),
            ]
        );
        assert_eq!(
            changes(&streams, 100, false),
            vec![
                s(&["set-sink-input-mute", "8", "0"]),
                s(&["set-sink-input-volume", "8", "100%"]),
            ]
        );
        assert!(changes(&serde_json::json!([]), 0, true).is_empty());
    }
}
