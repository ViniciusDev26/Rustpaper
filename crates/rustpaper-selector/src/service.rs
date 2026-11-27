// The engine as a systemd user service (`rustpaper.service`).
//
// systemd owns the engine's lifetime: the selector writes the unit, starts
// it, and on every change (wallpaper, properties, volume, quality, playback)
// saves state.json and runs `systemctl --user reload-or-restart`. A running
// engine gets SIGHUP and applies the change in place, so the desktop never
// shows the plain background in between. "Start on login" is
// `systemctl --user enable/disable`: the unit is `WantedBy=` the graphical
// session (uwsm starts `graphical-session.target` on Omarchy, with
// WAYLAND_DISPLAY / HYPRLAND_INSTANCE_SIGNATURE in the user manager's
// environment).

use std::path::{Path, PathBuf};
use std::process::Command;

pub const UNIT: &str = "rustpaper.service";

fn config_home() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_default()
}

fn unit_path() -> PathBuf {
    config_home().join("systemd/user").join(UNIT)
}

/// The autostart entry older selectors wrote; replaced by the service.
fn legacy_autostart() -> PathBuf {
    config_home().join("autostart/rustpaper.desktop")
}

/// Quotes one word of an `ExecStart=` line (systemd.service(5): double
/// quotes, backslash escapes, `%` doubled for specifiers).
fn quote(arg: &str) -> String {
    let mut out = String::from("\"");
    for c in arg.chars() {
        match c {
            '"' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            '%' => out.push_str("%%"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

pub fn unit_text(engine: &Path) -> String {
    format!(
        "# Written by rustpaper-selector; rewritten when the engine moves.
[Unit]
Description=Rustpaper live wallpaper (Wallpaper Engine under Proton)
Documentation=https://github.com/sugomi-dev/Rustpaper
PartOf=graphical-session.target
After=graphical-session.target
Requisite=graphical-session.target

[Service]
Type=simple
ExecStart={} --service
# re-read the selector's state (new wallpaper, properties, settings)
ExecReload=/bin/kill -HUP $MAINPID
Restart=on-failure
RestartSec=5
# the last engine out stops Wallpaper Engine's wallpapers
TimeoutStopSec=20

[Install]
WantedBy=graphical-session.target
",
        quote(&engine.to_string_lossy())
    )
}

fn systemctl(args: &[&str]) -> Result<String, String> {
    let out = Command::new("systemctl")
        .arg("--user")
        .args(args)
        .output()
        .map_err(|e| format!("systemctl: {e}"))?;
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if out.status.success() {
        Ok(text)
    } else {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        Err(format!(
            "systemctl --user {}: {}",
            args.join(" "),
            if err.is_empty() { text } else { err }
        ))
    }
}

/// A systemd user manager is reachable.
pub fn available() -> bool {
    systemctl(&["show-environment"]).is_ok()
}

/// Writes the unit when missing or different (the engine moved) and makes
/// systemd read it. Also drops the old autostart .desktop.
pub fn install(engine: &Path) -> Result<(), String> {
    let path = unit_path();
    let text = unit_text(engine);
    if std::fs::read_to_string(&path).ok().as_deref() != Some(text.as_str()) {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))?;
        systemctl(&["daemon-reload"])?;
    }
    let _ = std::fs::remove_file(legacy_autostart());
    Ok(())
}

/// Starts the engine, or makes the running one re-read state.json.
pub fn apply() -> Result<(), String> {
    systemctl(&["reload-or-restart", UNIT]).map(|_| ())
}

pub fn is_active() -> bool {
    systemctl(&["is-active", UNIT]).is_ok_and(|s| s == "active" || s == "reloading")
}

pub fn is_enabled() -> bool {
    systemctl(&["is-enabled", UNIT]).is_ok_and(|s| s == "enabled")
}

pub fn set_enabled(on: bool) -> Result<(), String> {
    systemctl(&[if on { "enable" } else { "disable" }, UNIT]).map(|_| ())
}

/// The engine process systemd runs (0 / None when stopped).
pub fn main_pid() -> Option<u32> {
    systemctl(&["show", "-p", "MainPID", "--value", UNIT])
        .ok()?
        .parse::<u32>()
        .ok()
        .filter(|&p| p != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exec_start_is_quoted() {
        let text = unit_text(Path::new("/home/a b/.local/bin/rust%paper"));
        assert!(text.contains("ExecStart=\"/home/a b/.local/bin/rust%%paper\" --service\n"));
        assert!(text.contains("WantedBy=graphical-session.target"));
        assert_eq!(quote(r#"a"b\c"#), r#""a\"b\\c""#);
    }
}
