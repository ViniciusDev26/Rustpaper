// Selector persistence (XDG): the last applied wallpaper, the autostart
// preference and its .desktop file, and the per-wallpaper property values the
// user picked in the customization panel.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Serialize, Deserialize, Clone)]
pub struct Settings {
    pub last_id: Option<String>,
    pub autostart: bool,
    /// Don't run SceneScripts (`rustpaper --no-scripts`). They run sandboxed
    /// by default; this is for users who want no Workshop code at all.
    #[serde(default)]
    pub no_scripts: bool,
    /// Master volume (0..100) of the wallpapers' own sounds
    /// (`rustpaper --volume`); 0 mutes them.
    #[serde(default = "full_volume")]
    pub volume: u8,
    /// Generate and apply an Omarchy theme from each applied wallpaper's
    /// colors (like Aether; omarchy.rs). Off by default: it changes the
    /// whole desktop's theme.
    #[serde(default)]
    pub wallpaper_theme: bool,
}

fn full_volume() -> u8 {
    100
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            last_id: None,
            autostart: false,
            no_scripts: false,
            volume: full_volume(),
            wallpaper_theme: false,
        }
    }
}

fn config_home() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let home = std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_default();
            home.join(".config")
        })
}

fn state_path() -> PathBuf {
    config_home().join("rustpaper").join("state.json")
}

fn autostart_file() -> PathBuf {
    config_home().join("autostart").join("rustpaper.desktop")
}

pub fn load() -> Settings {
    std::fs::read_to_string(state_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save(s: &Settings) {
    if let Some(dir) = state_path().parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(json) = serde_json::to_string_pretty(s) {
        let _ = std::fs::write(state_path(), json);
    }
}

/// `~/.config/rustpaper/properties/<id>.json`: `{ "name": value, ... }`, the
/// format `rustpaper --props` reads.
pub fn properties_file(id: &str) -> Option<PathBuf> {
    let safe = !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    safe.then(|| {
        config_home()
            .join("rustpaper")
            .join("properties")
            .join(format!("{id}.json"))
    })
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

/// Quotes one argument for a .desktop `Exec=` line.
fn desktop_quote(arg: &str) -> String {
    let escaped: String = arg
        .chars()
        .flat_map(|c| match c {
            '"' | '`' | '$' | '\\' => vec!['\\', c],
            _ => vec![c],
        })
        .collect();
    format!("\"{}\"", escaped.replace('%', "%%"))
}

/// Autostart .desktop running the engine with `args` (wallpaper folder and
/// options) at the next login.
pub fn write_autostart(engine: &Path, args: &[String]) -> std::io::Result<()> {
    if let Some(dir) = autostart_file().parent() {
        std::fs::create_dir_all(dir)?;
    }
    let exec: Vec<String> = std::iter::once(engine.to_string_lossy().into_owned())
        .chain(args.iter().cloned())
        .map(|a| desktop_quote(&a))
        .collect();
    let content = format!(
        "[Desktop Entry]\nType=Application\nName=Rustpaper\nExec={}\nX-GNOME-Autostart-enabled=true\nNoDisplay=true\n",
        exec.join(" ")
    );
    std::fs::write(autostart_file(), content)
}

pub fn remove_autostart() {
    let _ = std::fs::remove_file(autostart_file());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn property_files_only_for_plain_ids() {
        assert!(properties_file("2068329709").is_some());
        assert!(properties_file("../etc").is_none());
        assert!(properties_file("").is_none());
    }

    #[test]
    fn old_state_files_keep_scripts_on_and_full_volume() {
        let s: Settings = serde_json::from_str(r#"{"last_id":"1","autostart":true}"#).unwrap();
        assert!(!s.no_scripts);
        assert_eq!(s.volume, 100);
        assert_eq!(Settings::default().volume, 100);
    }

    #[test]
    fn desktop_exec_quoting() {
        assert_eq!(desktop_quote("/a b/c"), "\"/a b/c\"");
        assert_eq!(desktop_quote("x\"$y%"), "\"x\\\"\\$y%%\"");
    }
}
