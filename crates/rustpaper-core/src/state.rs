// Module `state`: what the selector decided, shared with the engine.
//
// `~/.config/rustpaper/state.json` is written by the selector and read by the
// engine (`rustpaper --service`, the systemd user service), which re-reads it
// on SIGHUP (`systemctl --user reload rustpaper`). Per-wallpaper property
// values live next to it in `properties/<id>.json`.
//
// Unknown fields are ignored and missing ones take their defaults, so older
// state files (with `autostart`, `no_scripts`) still load.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// What the engine does under a playback condition (WE's "Playback" settings;
/// WE's own copies are forced to "run" because every wallpaper window is a
/// fullscreen app to WE, see rustpaper-engine/src/we.rs).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    /// Keep rendering.
    Run,
    /// Stop capturing on that monitor; WE itself pauses when every monitor
    /// is paused.
    Pause,
}

/// When to pause, per condition (the same conditions as WE's Playback tab).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(default)]
pub struct Playback {
    /// A fullscreen window covers the monitor.
    pub fullscreen: Action,
    /// A maximized window, or tiled windows, cover the monitor (Hyprland
    /// tiles by default: the wallpaper only shows in the gaps).
    pub maximized: Action,
    /// Any window on the monitor has the keyboard focus.
    pub focused: Action,
    /// The monitor is powered off (DPMS).
    pub sleep: Action,
    /// The computer runs on battery.
    pub battery: Action,
}

impl Default for Playback {
    fn default() -> Self {
        // WE's defaults for fullscreen / display asleep; keep running otherwise
        Playback {
            fullscreen: Action::Pause,
            maximized: Action::Run,
            focused: Action::Run,
            sleep: Action::Pause,
            battery: Action::Run,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct State {
    /// Workshop id of the wallpaper on screen.
    pub last_id: Option<String>,
    /// Master volume 0..100; 0 mutes (Wallpaper Engine has no volume command,
    /// so anything above 0 plays at WE's own volume).
    pub volume: u8,
    /// Generate and apply an Omarchy theme from each applied wallpaper.
    pub wallpaper_theme: bool,
    /// WE quality settings (`config.json` keys: fps, msaa, resolution,
    /// postprocessing, shadows, volumetrics, reflection, preset). The engine
    /// writes them into WE's config while WE is stopped: written while it
    /// runs, WE would overwrite them on exit.
    pub performance: Map<String, Value>,
    pub playback: Playback,
}

impl Default for State {
    fn default() -> Self {
        State {
            last_id: None,
            volume: 100,
            wallpaper_theme: false,
            performance: Map::new(),
            playback: Playback::default(),
        }
    }
}

/// `$XDG_CONFIG_HOME/rustpaper` (or ~/.config/rustpaper).
pub fn config_dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_default()
        .join("rustpaper")
}

pub fn state_file() -> PathBuf {
    config_dir().join("state.json")
}

/// `properties/<id>.json` (`{ "name": value }`); None for ids that are not
/// plain names (never a path outside the folder).
pub fn properties_file(id: &str) -> Option<PathBuf> {
    let safe = !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    safe.then(|| config_dir().join("properties").join(format!("{id}.json")))
}

pub fn load() -> State {
    std::fs::read_to_string(state_file())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save(s: &State) -> std::io::Result<()> {
    std::fs::create_dir_all(config_dir())?;
    std::fs::write(state_file(), serde_json::to_string_pretty(s)?)
}

pub fn load_properties(id: &str) -> Map<String, Value> {
    properties_file(id)
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Writes the values; an empty map removes the file (wallpaper defaults).
pub fn save_properties(id: &str, values: &Map<String, Value>) -> std::io::Result<()> {
    let path = properties_file(id).ok_or_else(|| std::io::Error::other("invalid wallpaper id"))?;
    if values.is_empty() {
        return match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        };
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, serde_json::to_string_pretty(values)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_state_files_still_load() {
        let s: State =
            serde_json::from_str(r#"{"last_id":"1","autostart":true,"no_scripts":true}"#).unwrap();
        assert_eq!(s.last_id.as_deref(), Some("1"));
        assert_eq!(s.volume, 100);
        assert_eq!(s.playback, Playback::default());
        assert!(s.performance.is_empty());
    }

    #[test]
    fn playback_actions_are_lowercase() {
        let p: Playback = serde_json::from_str(r#"{"focused":"pause"}"#).unwrap();
        assert_eq!(p.focused, Action::Pause);
        assert_eq!(
            p.fullscreen,
            Action::Pause,
            "missing keys keep their defaults"
        );
        assert!(
            serde_json::to_string(&p)
                .unwrap()
                .contains(r#""maximized":"run""#)
        );
    }

    #[test]
    fn property_files_only_for_plain_ids() {
        assert!(properties_file("2068329709").is_some());
        assert!(properties_file("../etc").is_none());
        assert!(properties_file("").is_none());
    }
}
