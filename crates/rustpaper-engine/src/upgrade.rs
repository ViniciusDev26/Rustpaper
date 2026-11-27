//! Running the new binary after an update.
//!
//! The package manager replaces `/usr/bin/rustpaper` under a running engine
//! (and lock daemon): they kept running the old one until restarted by hand
//! or at the next login, so an update seemed not to work (0.12.0's media
//! integration, 2026-11-27: `omarchy update` installed it, the engine
//! started before it ran on). Replaced, the file the process runs is gone:
//! `/proc/self/exe` then reads `<path> (deleted)`. Once a new file sits at
//! that path for [`SETTLE`] (the rest of the update lands meanwhile: the
//! Windows files in `/usr/lib/rustpaper`), the process leaves its loop the
//! way it does on SIGTERM, cleaning up as usual, and `main` replaces it
//! with the new binary and the same arguments ([`exec`]): the same PID, so
//! systemd sees no restart, and the same works without systemd. A cargo
//! build replacing `target/release/rustpaper` does the same to a dev engine.

use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// How long the new file must be there before switching to it.
const SETTLE: Duration = Duration::from_secs(10);

/// Set once the process should run the new binary when it exits its loop.
static REQUESTED: AtomicBool = AtomicBool::new(false);

/// The path of a replaced executable, from `/proc/self/exe`'s link text.
fn replaced_path(link: &str) -> Option<&str> {
    link.strip_suffix(" (deleted)")
}

/// The new binary at our path, when ours was replaced (None when it was
/// only removed: nothing to run then).
fn replacement() -> Option<PathBuf> {
    let link = std::fs::read_link("/proc/self/exe").ok()?;
    let path = PathBuf::from(replaced_path(link.to_str()?)?);
    path.is_file().then_some(path)
}

/// Notices our binary being replaced (call it now and then).
#[derive(Default)]
pub struct Watch {
    since: Option<Instant>,
}

impl Watch {
    /// True once a new binary has been at our path for [`SETTLE`]; it is
    /// then recorded for [`exec`] (and logged).
    pub fn replaced(&mut self) -> bool {
        let Some(path) = replacement() else {
            self.since = None;
            return false;
        };
        let since = *self.since.get_or_insert_with(Instant::now);
        if since.elapsed() < SETTLE || REQUESTED.load(Ordering::SeqCst) {
            return false;
        }
        eprintln!("{} was updated: restarting into it", path.display());
        REQUESTED.store(true, Ordering::SeqCst);
        true
    }
}

/// After the process's loop ended: when [`Watch::replaced`] said so, runs the
/// new binary with the same arguments in this process (never returns then).
pub fn exec_if_replaced() {
    if !REQUESTED.load(Ordering::SeqCst) {
        return;
    }
    let Some(path) = replacement() else {
        eprintln!("the new binary is gone; exiting");
        std::process::exit(1);
    };
    let err = std::process::Command::new(&path)
        .args(std::env::args_os().skip(1))
        .exec();
    // systemd's Restart=on-failure starts it again
    eprintln!("{}: {err}", path.display());
    std::process::exit(1);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_replaced_binary_shows_as_deleted() {
        assert_eq!(
            replaced_path("/usr/bin/rustpaper (deleted)"),
            Some("/usr/bin/rustpaper")
        );
        assert_eq!(
            replaced_path("/home/a b/rustpaper (deleted)"),
            Some("/home/a b/rustpaper")
        );
        assert_eq!(replaced_path("/usr/bin/rustpaper"), None);
    }

    #[test]
    fn a_running_binary_is_not_replaced() {
        // the test binary itself is in place
        assert!(replacement().is_none());
        assert!(!Watch::default().replaced());
    }
}
