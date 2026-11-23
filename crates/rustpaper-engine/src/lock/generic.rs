//! The generic lock adapter: any compositor with `ext-session-lock-v1`
//! (Hyprland, Sway, niri, ...), on any distribution with logind.
//!
//! It routes what every systemd desktop shares: the daemon locks on
//! logind's `Lock` signal (`loginctl lock-session`, which most idle daemons
//! and desktop menus use) and before suspend. Desktop-specific triggers (a
//! keybinding, the idle daemon's own lock command) are the user's: the notes
//! returned by `enable` say what to point at `rustpaper lock`.

use std::path::{Path, PathBuf};

use super::adapter::{self, LockAdapter};

pub struct Generic;

/// The PAM service the packages install (`/etc/pam.d/rustpaper-lock`, which
/// includes `login`). Without it (a build from source), `login` itself.
pub fn pam_service() -> String {
    if Path::new("/etc/pam.d/rustpaper-lock").exists() {
        "rustpaper-lock".into()
    } else {
        "login".into()
    }
}

impl LockAdapter for Generic {
    fn name(&self) -> &'static str {
        "generic"
    }

    fn pam_service(&self) -> String {
        pam_service()
    }

    fn fallback_background(&self) -> Option<PathBuf> {
        None
    }

    /// `RUSTPAPER_LOCK_IDLE=<seconds>` in the daemon's environment (e.g.
    /// `systemctl --user set-environment`): lock after that long idle, for
    /// desktops without an idle daemon of their own.
    fn idle_lock_seconds(&self) -> Option<u32> {
        std::env::var("RUSTPAPER_LOCK_IDLE")
            .ok()?
            .parse()
            .ok()
            .filter(|&n| n > 0)
    }

    fn enable(&self, exe: &Path) -> Result<Vec<String>, String> {
        adapter::install_daemon(exe)?;
        Ok(vec![format!(
            "Rustpaper now locks on `loginctl lock-session` and before suspend. Point your keybinding and your idle daemon's lock command at `{} lock` (or `loginctl lock-session`).",
            exe.display()
        )])
    }

    fn disable(&self) -> Result<(), String> {
        adapter::remove_daemon()
    }

    fn enabled(&self) -> bool {
        adapter::daemon_enabled()
    }
}
