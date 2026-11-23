// Module `state`: what the selector decided, shared with the engine.
//
// `~/.config/rustpaper/state.json` is written by the selector and read by the
// engine (`rustpaper --service`, the systemd user service), which re-reads it
// on SIGHUP (`systemctl --user reload rustpaper`). Per-wallpaper property
// values live next to it in `properties/<id>.json`.
//
// Unknown fields are ignored and missing ones take their defaults, so older
// state files (with `autostart`, `no_scripts`) still load.
//
// Wallpapers by role: the desktop (one for every monitor, `last_id`, plus
// per-monitor overrides), the lock screen and the screensaver (one for
// every monitor plus overrides). Each falls back to the next: a monitor's
// screensaver -> the screensaver for all -> that monitor's desktop -> the
// desktop for all; the lock screen -> the desktop.

use std::collections::BTreeMap;
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

/// Where Wallpaper Engine's windows live (see rustpaper-engine docs/ENGINE.md).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum StageChoice {
    /// The private stage when WE's Steam launch option is set up, else the
    /// desktop adapter's stage.
    #[default]
    Auto,
    /// Inside the user's compositor (desktop adapter: headless outputs...).
    Desktop,
    /// Rustpaper's own compositor and XWayland.
    Private,
}

/// What plays `video` wallpapers (project.json `"type": "video"`).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum VideoPlayer {
    /// mpv with hardware decoding, on the private stage (when mpv is
    /// installed; otherwise Wallpaper Engine). Proton decodes WE's videos on
    /// the CPU: the same 1080p24 video took 22.7% of a core in WE and 1.9%
    /// in mpv (docs/ENGINE.md).
    #[default]
    Mpv,
    /// Wallpaper Engine, like every other wallpaper.
    We,
}

/// The screensaver: after `seconds` without input, every monitor shows its
/// screensaver wallpaper over the windows until the next input.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Screensaver {
    pub enabled: bool,
    pub seconds: u32,
    /// For every monitor without its own (None: the desktop's wallpaper).
    pub all: Option<String>,
    /// Output name -> wallpaper id.
    pub monitors: BTreeMap<String, String>,
}

impl Default for Screensaver {
    fn default() -> Self {
        // 150 s: Omarchy's screensaver default
        Screensaver {
            enabled: false,
            seconds: 150,
            all: None,
            monitors: BTreeMap::new(),
        }
    }
}

/// Where a wallpaper shows (the selector's "Use for").
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Desktop,
    Lock,
    Screensaver,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct State {
    /// Workshop id of the desktop wallpaper of every monitor without its
    /// own (`monitors`).
    pub last_id: Option<String>,
    /// Desktop wallpaper per monitor: output name -> wallpaper id.
    pub monitors: BTreeMap<String, String>,
    /// The lock screen's wallpaper (None: the desktop's).
    pub lock_id: Option<String>,
    pub screensaver: Screensaver,
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
    pub stage: StageChoice,
    pub video_player: VideoPlayer,
}

impl Default for State {
    fn default() -> Self {
        State {
            last_id: None,
            monitors: BTreeMap::new(),
            lock_id: None,
            screensaver: Screensaver::default(),
            volume: 100,
            wallpaper_theme: false,
            performance: Map::new(),
            playback: Playback::default(),
            stage: StageChoice::Auto,
            video_player: VideoPlayer::Mpv,
        }
    }
}

impl State {
    /// The desktop wallpaper of monitor `output`.
    pub fn desktop_for(&self, output: &str) -> Option<&str> {
        self.monitors
            .get(output)
            .or(self.last_id.as_ref())
            .map(String::as_str)
    }

    /// The lock screen's wallpaper on monitor `output`.
    pub fn lock_for(&self, output: &str) -> Option<&str> {
        self.lock_id.as_deref().or_else(|| self.desktop_for(output))
    }

    /// The screensaver's wallpaper on monitor `output`.
    pub fn screensaver_for(&self, output: &str) -> Option<&str> {
        self.screensaver
            .monitors
            .get(output)
            .or(self.screensaver.all.as_ref())
            .map(String::as_str)
            .or_else(|| self.desktop_for(output))
    }

    /// Wallpaper `id` shows somewhere (its property changes need a reload).
    pub fn uses(&self, id: &str) -> bool {
        let id = Some(id);
        self.last_id.as_deref() == id
            || self.lock_id.as_deref() == id
            || self.screensaver.all.as_deref() == id
            || self.monitors.values().any(|v| Some(v.as_str()) == id)
            || self
                .screensaver
                .monitors
                .values()
                .any(|v| Some(v.as_str()) == id)
    }

    /// Shows `id` as `role` on `output` (None: every monitor, replacing the
    /// per-monitor choices of that role).
    pub fn assign(&mut self, role: Role, output: Option<&str>, id: &str) {
        let id = id.to_string();
        match (role, output) {
            (Role::Desktop, None) => {
                self.last_id = Some(id);
                self.monitors.clear();
            }
            (Role::Desktop, Some(o)) => {
                self.monitors.insert(o.to_string(), id);
            }
            (Role::Lock, _) => self.lock_id = Some(id),
            (Role::Screensaver, None) => {
                self.screensaver.all = Some(id);
                self.screensaver.monitors.clear();
                self.screensaver.enabled = true;
            }
            (Role::Screensaver, Some(o)) => {
                self.screensaver.monitors.insert(o.to_string(), id);
                self.screensaver.enabled = true;
            }
        }
    }

    /// Removes the choice of `role` on `output` (None: the one for every
    /// monitor), so the fallback shows. The desktop for all stays (it is
    /// what everything falls back to).
    pub fn unassign(&mut self, role: Role, output: Option<&str>) {
        match (role, output) {
            (Role::Desktop, None) => {}
            (Role::Desktop, Some(o)) => {
                self.monitors.remove(o);
            }
            (Role::Lock, _) => self.lock_id = None,
            (Role::Screensaver, None) => self.screensaver.all = None,
            (Role::Screensaver, Some(o)) => {
                self.screensaver.monitors.remove(o);
            }
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
        assert_eq!(
            s.video_player,
            VideoPlayer::Mpv,
            "mpv unless chosen otherwise"
        );
    }

    #[test]
    fn video_player_is_lowercase() {
        let s: State = serde_json::from_str(r#"{"video_player":"we"}"#).unwrap();
        assert_eq!(s.video_player, VideoPlayer::We);
        assert!(
            serde_json::to_string(&State::default())
                .unwrap()
                .contains(r#""video_player":"mpv""#)
        );
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

#[cfg(test)]
mod role_tests {
    use super::*;

    #[test]
    fn roles_fall_back_in_order() {
        let mut s = State {
            last_id: Some("all".into()),
            ..Default::default()
        };
        assert_eq!(s.desktop_for("DP-1"), Some("all"));
        assert_eq!(s.lock_for("DP-1"), Some("all"), "lock: the desktop's");
        assert_eq!(s.screensaver_for("DP-1"), Some("all"));
        s.assign(Role::Desktop, Some("DP-1"), "dp");
        assert_eq!(s.desktop_for("DP-1"), Some("dp"));
        assert_eq!(s.desktop_for("HDMI-A-1"), Some("all"));
        assert_eq!(
            s.screensaver_for("DP-1"),
            Some("dp"),
            "screensaver: the monitor's desktop"
        );
        s.assign(Role::Screensaver, None, "saver");
        assert!(s.screensaver.enabled, "assigning turns the screensaver on");
        assert_eq!(s.screensaver_for("DP-1"), Some("saver"));
        s.assign(Role::Screensaver, Some("HDMI-A-1"), "hdmi-saver");
        assert_eq!(s.screensaver_for("HDMI-A-1"), Some("hdmi-saver"));
        s.assign(Role::Lock, None, "lock");
        assert_eq!(s.lock_for("HDMI-A-1"), Some("lock"));
        assert!(s.uses("hdmi-saver") && s.uses("dp") && !s.uses("other"));
        s.unassign(Role::Screensaver, Some("HDMI-A-1"));
        assert_eq!(s.screensaver_for("HDMI-A-1"), Some("saver"));
        s.assign(Role::Desktop, None, "new");
        assert!(
            s.monitors.is_empty(),
            "all monitors replaces the per-monitor choices"
        );
        assert_eq!(s.desktop_for("DP-1"), Some("new"));
    }

    #[test]
    fn old_state_files_still_load() {
        let s: State =
            serde_json::from_str(r#"{"last_id":"1","volume":50,"autostart":true}"#).unwrap();
        assert_eq!(s.last_id.as_deref(), Some("1"));
        assert!(s.monitors.is_empty() && s.lock_id.is_none() && !s.screensaver.enabled);
        assert_eq!(s.screensaver.seconds, 150);
    }
}
