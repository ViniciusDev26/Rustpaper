//! Lock adapters: everything about the lock screen that depends on the
//! distribution or desktop.
//!
//! The lock itself (lock/screen.rs: `ext-session-lock-v1`, PAM, the live
//! wallpaper) and its daemon (lock/daemon.rs: logind's Lock signal, locking
//! before suspend, idle) are the same everywhere. An adapter says how this
//! desktop should look and authenticate, and how its own lock triggers (a
//! keybinding, a menu entry, its idle and sleep handling) are routed to
//! `rustpaper lock`. Supporting another desktop means one new adapter.
//!
//! | Desktop | Adapter | Triggers it routes |
//! |---|---|---|
//! | Omarchy | [`super::omarchy`] | Super+Ctrl+L, the lid, the System menu, idle lock, lock before suspend |
//! | anything else with `ext-session-lock-v1` | [`super::generic`] | `loginctl lock-session` and lock before suspend (logind); the user's idle daemon calls `rustpaper lock` or `loginctl lock-session` |

use std::path::{Path, PathBuf};

/// Colors (RGB) and font of the lock's panel.
#[derive(Debug, Clone, PartialEq)]
pub struct Theme {
    pub foreground: [u8; 3],
    /// Panel and field background.
    pub background: [u8; 3],
    /// Field border while typing.
    pub accent: [u8; 3],
    /// Wrong password.
    pub error: [u8; 3],
    /// Font family (fontconfig name), None for the system's sans-serif.
    pub font: Option<String>,
}

impl Default for Theme {
    fn default() -> Self {
        Theme {
            foreground: [0xe6, 0xe6, 0xe6],
            background: [0x16, 0x16, 0x1a],
            accent: [0x89, 0xb4, 0xfa],
            error: [0xf3, 0x8b, 0xa8],
            font: None,
        }
    }
}

pub trait LockAdapter {
    /// For logs and the selector.
    fn name(&self) -> &'static str;

    /// The PAM service the password is checked with.
    fn pam_service(&self) -> String;

    /// How the panel looks.
    fn theme(&self) -> Theme {
        Theme::default()
    }

    /// A still picture for monitors without live frames (no engine, or the
    /// desktop stage), after the wallpaper's own preview.
    fn fallback_background(&self) -> Option<PathBuf> {
        None
    }

    /// Runs once the session is locked: what the desktop's own lock command
    /// also did (e.g. reset the keyboard layout, lock a password manager).
    fn on_locked(&self) {}

    /// Seconds of inactivity after which the daemon locks; None when the
    /// desktop's idle daemon calls `rustpaper lock` (or `loginctl
    /// lock-session`) itself.
    fn idle_lock_seconds(&self) -> Option<u32> {
        None
    }

    /// Rustpaper's screensaver is on (`ours`): the desktop's own screensaver
    /// steps aside, or comes back when it is turned off. Idempotent; the
    /// engine calls it at start and when the setting changes.
    fn hand_over_screensaver(&self, _ours: bool) {}

    /// Routes the desktop's lock triggers to `exe lock` (and starts the
    /// daemon). Idempotent. Returns notes for the user (what they must do
    /// by hand, if anything).
    fn enable(&self, exe: &Path) -> Result<Vec<String>, String>;

    /// Gives the triggers back to the desktop's own lock screen.
    fn disable(&self) -> Result<(), String>;

    /// The triggers point at Rustpaper.
    fn enabled(&self) -> bool;
}

/// The adapter for this session; `RUSTPAPER_LOCK_ADAPTER` (`omarchy`,
/// `generic`) overrides the detection (tests on a nested compositor use
/// `generic`, so nothing touches the real session).
pub fn detect() -> Box<dyn LockAdapter> {
    match std::env::var("RUSTPAPER_LOCK_ADAPTER").as_deref() {
        Ok("generic") => return Box::new(super::generic::Generic),
        Ok("omarchy") => return Box::new(super::omarchy::Omarchy),
        _ => {}
    }
    if super::omarchy::Omarchy::present() {
        return Box::new(super::omarchy::Omarchy);
    }
    Box::new(super::generic::Generic)
}

// ---- shared by the adapters ------------------------------------------------

pub(super) fn config_home() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
}

/// The daemon's systemd user unit (every adapter uses it for logind).
pub(super) const DAEMON_UNIT: &str = "rustpaper-lock.service";

/// Writes and starts `rustpaper-lock.service` (`exe lock-daemon`).
pub(super) fn install_daemon(exe: &Path) -> Result<(), String> {
    let dir = config_home()
        .ok_or("no config directory")?
        .join("systemd/user");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    // KillMode=process: a lock started by the daemon must outlive a daemon
    // restart. Killing it would leave the session locked with no way to type
    // the password (the compositor's "lock screen crashed" screen).
    let unit = format!(
        "[Unit]\n\
         Description=Rustpaper lock screen (lock before suspend, loginctl lock-session)\n\
         PartOf=graphical-session.target\n\
         After=graphical-session.target\n\
         \n\
         [Service]\n\
         ExecStart=\"{}\" lock-daemon\n\
         KillMode=process\n\
         Restart=on-failure\n\
         RestartSec=2\n\
         \n\
         [Install]\n\
         WantedBy=graphical-session.target\n",
        exe.display()
    );
    std::fs::write(dir.join(DAEMON_UNIT), unit).map_err(|e| e.to_string())?;
    systemctl(&["daemon-reload"])?;
    systemctl(&["enable", DAEMON_UNIT])?;
    systemctl(&["restart", DAEMON_UNIT])
}

pub(super) fn remove_daemon() -> Result<(), String> {
    let _ = systemctl(&["disable", "--now", DAEMON_UNIT]);
    if let Some(dir) = config_home() {
        let _ = std::fs::remove_file(dir.join("systemd/user").join(DAEMON_UNIT));
    }
    systemctl(&["daemon-reload"])
}

pub(super) fn daemon_enabled() -> bool {
    std::process::Command::new("systemctl")
        .args(["--user", "is-enabled", "--quiet", DAEMON_UNIT])
        .status()
        .is_ok_and(|s| s.success())
}

pub(super) fn systemctl(args: &[&str]) -> Result<(), String> {
    let out = std::process::Command::new("systemctl")
        .arg("--user")
        .args(args)
        .output()
        .map_err(|e| format!("systemctl: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "systemctl --user {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

/// Replaces (or appends, or with `body` None removes) the block between
/// `start` and `end` marker lines in `text`. How the adapters edit a user's
/// config file: only their own block, everything else byte for byte.
pub(super) fn set_block(text: &str, start: &str, end: &str, body: Option<&str>) -> String {
    let mut out = String::new();
    let mut lines = text.split_inclusive('\n');
    let mut replaced = false;
    for line in lines.by_ref() {
        if line.trim_end() == start {
            // skip to the end marker (a missing end drops the rest: never
            // happens with our own blocks, and keeping a half block is worse)
            for l in lines.by_ref() {
                if l.trim_end() == end {
                    break;
                }
            }
            if let Some(b) = body {
                out.push_str(&block(start, end, b));
            }
            replaced = true;
            break;
        }
        out.push_str(line);
    }
    out.extend(lines);
    if !replaced && let Some(b) = body {
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(&block(start, end, b));
    }
    out
}

fn block(start: &str, end: &str, body: &str) -> String {
    let mut s = format!("{start}\n{body}");
    if !s.ends_with('\n') {
        s.push('\n');
    }
    s.push_str(end);
    s.push('\n');
    s
}

/// `text` has the block that starts with `start`.
pub(super) fn has_block(text: &str, start: &str) -> bool {
    text.lines().any(|l| l.trim_end() == start)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_are_added_replaced_and_removed() {
        let user = "-- mine\no.bind(\"SUPER + A\", \"x\", \"y\")";
        let with = set_block(user, "-- s", "-- e", Some("one"));
        assert_eq!(
            with,
            "-- mine\no.bind(\"SUPER + A\", \"x\", \"y\")\n-- s\none\n-- e\n"
        );
        assert!(has_block(&with, "-- s"));
        let again = set_block(&with, "-- s", "-- e", Some("two\n"));
        assert_eq!(
            again,
            "-- mine\no.bind(\"SUPER + A\", \"x\", \"y\")\n-- s\ntwo\n-- e\n"
        );
        let gone = set_block(&again, "-- s", "-- e", None);
        assert_eq!(gone, "-- mine\no.bind(\"SUPER + A\", \"x\", \"y\")\n");
        // text after the block survives
        let mid = set_block("a\n-- s\nold\n-- e\nb\n", "-- s", "-- e", Some("new"));
        assert_eq!(mid, "a\n-- s\nnew\n-- e\nb\n");
    }
}
