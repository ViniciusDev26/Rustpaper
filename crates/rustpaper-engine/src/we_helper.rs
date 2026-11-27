//! Keeps `rustpaper-we-helper.exe` (crates/rustpaper-we-helper) running in
//! WE's Proton prefix while WE runs. It marks our WE windows as the desktop
//! for WE's web wallpapers, which otherwise ignore every click under Wine
//! (that crate's header has the details).
//!
//! The .exe is installed next to the engine (`/usr/lib/rustpaper`, or
//! `target/x86_64-pc-windows-msvc/<profile>` while developing), places WE's
//! Steam Runtime container does not see: it is copied to the cache in the
//! home, which the container shares, and started from there.

use std::path::PathBuf;
use std::process::Child;
use std::time::{Duration, Instant};

use crate::we;

const EXE: &str = "rustpaper-we-helper.exe";

/// Wait before starting the helper again after it exited (it exits by
/// itself when WE's windows are gone).
const RETRY: Duration = Duration::from_secs(10);

#[derive(Default)]
pub struct WeHelper {
    child: Option<Child>,
    next_try: Option<Instant>,
    /// Logged that the .exe is missing (once).
    missing: bool,
}

impl WeHelper {
    /// Starts the helper when WE runs and it does not (called every tick
    /// while WE is in use).
    pub fn tend(&mut self) {
        if let Some(c) = &mut self.child {
            match c.try_wait() {
                Ok(None) => return,
                _ => self.child = None,
            }
        }
        if self.next_try.is_some_and(|t| Instant::now() < t) {
            return;
        }
        self.next_try = Some(Instant::now() + RETRY);
        let Some(exe) = self.installed() else {
            return;
        };
        if we::pid().is_none() {
            return;
        }
        self.child = we::spawn_in_we(&[we::wine_path(&exe)]);
    }

    /// Stops the helper (WE no longer used, or the engine exits).
    pub fn stop(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }

    /// The helper copied where WE's container sees it, refreshed when the
    /// installed one differs. None (logged once) when it is not installed.
    fn installed(&mut self) -> Option<PathBuf> {
        let Some(src) = installed_file(EXE) else {
            if !self.missing {
                self.missing = true;
                eprintln!("we: {EXE} not found; web wallpapers will not take clicks");
            }
            return None;
        };
        let dir = we::cache_dir()?;
        let dst = dir.join(EXE);
        let bytes = std::fs::read(&src).ok()?;
        if std::fs::read(&dst).ok().as_deref() != Some(bytes.as_slice()) {
            std::fs::create_dir_all(&dir).ok()?;
            if let Err(e) = std::fs::write(&dst, &bytes) {
                eprintln!("we: copying {EXE} to {}: {e}", dir.display());
                return None;
            }
        }
        Some(dst)
    }
}

impl Drop for WeHelper {
    fn drop(&mut self) {
        self.stop();
    }
}

/// A Windows file of rustpaper-we-helper (`EXE`, or media.rs's DLL) as
/// installed: next to the engine, in `../lib/rustpaper` (/usr/bin ->
/// /usr/lib/rustpaper), or, in a cargo build, under the Windows target's
/// directory of the same profile (target/release ->
/// target/x86_64-pc-windows-msvc/release).
pub fn installed_file(name: &str) -> Option<PathBuf> {
    let me = std::env::current_exe().ok()?;
    let dir = me.parent()?;
    let mut candidates = vec![dir.join(name), dir.join("../lib/rustpaper").join(name)];
    if let (Some(profile), Some(target)) = (dir.file_name(), dir.parent()) {
        candidates.push(
            target
                .join("x86_64-pc-windows-msvc")
                .join(profile)
                .join(name),
        );
    }
    candidates.into_iter().find(|p| p.is_file())
}
