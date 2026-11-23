//! The Omarchy lock adapter.
//!
//! Omarchy's lock screen is a first-party plugin of its Quickshell shell
//! (`omarchy.lock`), and the shell grants the `authentication` capability
//! to first-party plugins only, so the live wallpaper cannot be added to it
//! from outside. Instead every trigger that ends in `omarchy-shell lock
//! lock` is routed to `rustpaper lock`, each through Omarchy's own user
//! customization point, and each reverted by `disable`:
//!
//! | Trigger | Omarchy runs | Routed through |
//! |---|---|---|
//! | Super+Ctrl+L, lid closed | `omarchy-system-lock`, `omarchy-system-lid-close` | `o.rebind` in `~/.config/hypr/bindings.lua` (our marked block) |
//! | System menu > Lock | `omarchy-system-lock` | the `system.lock` id in `~/.config/omarchy/extensions/omarchy-menu.jsonc` (our marked block) |
//! | idle | the shell's idle service (`idle.lock` seconds in shell.json) | the daemon adopts the timeout and sets `idle.lock` to 0 |
//! | suspend | `omarchy-sleep-lock.service` | masked; the daemon locks before suspend |
//!
//! Authentication uses Omarchy's own PAM service (`omarchy-lock-password`,
//! with its faillock policy); the panel takes the theme's colors and font.

use std::path::{Path, PathBuf};

use serde_json::Value;

use super::adapter::{self, LockAdapter, Theme, config_home, has_block, set_block};

pub struct Omarchy;

const START: &str = "-- rustpaper-lock-start";
const END: &str = "-- rustpaper-lock-end";
const MENU_START: &str = "// rustpaper-lock-start";
const MENU_END: &str = "// rustpaper-lock-end";
const SLEEP_UNIT: &str = "omarchy-sleep-lock.service";
/// Omarchy's idle lock when shell.json does not set one (its idle service).
const OMARCHY_DEFAULT_IDLE_LOCK: u64 = 300;

fn bindings_file() -> Option<PathBuf> {
    Some(config_home()?.join("hypr/bindings.lua"))
}

fn menu_file() -> Option<PathBuf> {
    Some(config_home()?.join("omarchy/extensions/omarchy-menu.jsonc"))
}

fn shell_json() -> Option<PathBuf> {
    Some(config_home()?.join("omarchy/shell.json"))
}

/// Rustpaper's own record of the idle timeout it took over.
fn saved_file() -> Option<PathBuf> {
    Some(config_home()?.join("rustpaper/lock.json"))
}

fn state_home() -> Option<PathBuf> {
    std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))
}

impl Omarchy {
    /// Omarchy is installed (its environment, or its system folder).
    pub fn present() -> bool {
        std::env::var_os("OMARCHY_PATH").is_some() || Path::new("/usr/share/omarchy/bin").is_dir()
    }
}

/// The bindings.lua block: the lock key and the lid switches run our lock.
/// The lid command keeps what `omarchy-system-lid-close` does (lock only
/// when the lid closes without an external monitor, then reconcile the
/// displays), with our lock in place of Omarchy's.
fn bindings_block(exe: &Path) -> String {
    let exe = exe.display();
    // (inside `bash -c '...'`: a quote in the path becomes '\'')
    let quoted = exe.to_string().replace('\'', "'\\''");
    let lid = format!(
        "if omarchy-hw-laptop-closed && ! omarchy-hw-external-monitors; then \"{quoted}\" lock; fi; omarchy-hyprland-monitor-clamshell"
    );
    format!(
        "-- Managed by Rustpaper (animated lock screen); the selector's lock screen\n\
         -- setting or `rustpaper lock-setup disable` removes this block.\n\
         o.rebind(\"SUPER + CTRL + L\", \"Lock system\", [[\"{exe}\" lock]])\n\
         o.rebind(\"switch:on:Lid Switch\", nil, [[bash -c '{lid}']], {{ locked = true }})\n\
         o.rebind(\"switch:on:Apple SMC power/lid events\", nil, [[bash -c '{lid}']], {{ locked = true }})\n"
    )
}

/// The menu extension with our `system.lock` override (inside the object:
/// right after its opening brace, like other tools' blocks). Omarchy keeps
/// the fields an override leaves out (icon, label).
fn menu_with_lock(text: &str, exe: Option<&Path>) -> String {
    let without = set_block(text, MENU_START, MENU_END, None);
    let Some(exe) = exe else {
        return without;
    };
    let action = serde_json::to_string(&format!("\"{}\" lock", exe.display())).unwrap_or_default();
    let entry = format!("  \"system.lock\": {{\"action\":{action}}},\n");
    let block = format!("{MENU_START}\n{entry}{MENU_END}\n");
    match without.find('{') {
        Some(i) => {
            // after the line holding the opening brace
            let at = without[i..].find('\n').map_or(without.len(), |n| i + n + 1);
            let mut out = without[..at].to_string();
            if !out.ends_with('\n') {
                out.push('\n');
            }
            out.push_str(&block);
            out.push_str(&without[at..]);
            out
        }
        None => format!("{{\n{block}}}\n"),
    }
}

/// shell.json's idle lock (seconds; Omarchy's default when unset), and the
/// text with it set to `new` when given.
fn shell_idle(text: &str, new: Option<u64>) -> Option<(u64, String)> {
    let mut v: Value = serde_json::from_str(text).ok()?;
    let current = v
        .get("idle")
        .and_then(|i| i.get("lock"))
        .and_then(Value::as_u64)
        .unwrap_or(OMARCHY_DEFAULT_IDLE_LOCK);
    if let Some(n) = new {
        let obj = v.as_object_mut()?;
        let idle = obj
            .entry("idle")
            .or_insert_with(|| Value::Object(Default::default()));
        idle.as_object_mut()?.insert("lock".into(), Value::from(n));
    }
    let mut out = serde_json::to_string_pretty(&v).ok()?;
    out.push('\n');
    Some((current, out))
}

fn read_saved_idle() -> Option<u64> {
    let text = std::fs::read_to_string(saved_file()?).ok()?;
    serde_json::from_str::<Value>(&text).ok()?["omarchy_idle_lock"].as_u64()
}

fn write_saved_idle(n: Option<u64>) -> Result<(), String> {
    let path = saved_file().ok_or("no config directory")?;
    match n {
        Some(n) => {
            std::fs::create_dir_all(path.parent().unwrap_or(Path::new(".")))
                .map_err(|e| e.to_string())?;
            std::fs::write(
                &path,
                serde_json::json!({ "omarchy_idle_lock": n }).to_string(),
            )
            .map_err(|e| e.to_string())
        }
        None => {
            let _ = std::fs::remove_file(&path);
            Ok(())
        }
    }
}

/// Takes Omarchy's idle lock over: when shell.json has a timeout (the user
/// set one in Omarchy, or never touched it), it becomes ours and Omarchy's
/// is set to 0. Returns our timeout.
fn adopt_idle() -> Option<u64> {
    let path = shell_json()?;
    if let Ok(text) = std::fs::read_to_string(&path)
        && let Some((current, _)) = shell_idle(&text, None)
        && current > 0
        && let Some((_, zeroed)) = shell_idle(&text, Some(0))
        && write_saved_idle(Some(current)).is_ok()
        && std::fs::write(&path, zeroed).is_ok()
    {
        eprintln!("lock: took over Omarchy's idle lock ({current} s)");
    }
    read_saved_idle().filter(|&n| n > 0)
}

fn restore_idle() {
    let (Some(saved), Some(path)) = (read_saved_idle(), shell_json()) else {
        return;
    };
    if let Ok(text) = std::fs::read_to_string(&path)
        && let Some((_, restored)) = shell_idle(&text, Some(saved))
    {
        let _ = std::fs::write(&path, restored);
    }
    let _ = write_saved_idle(None);
}

fn edit(path: &Path, f: impl FnOnce(&str) -> String) -> Result<(), String> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let new = f(&text);
    if new != text {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        std::fs::write(path, new).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    Ok(())
}

impl LockAdapter for Omarchy {
    fn name(&self) -> &'static str {
        "omarchy"
    }

    fn pam_service(&self) -> String {
        if Path::new("/etc/pam.d/omarchy-lock-password").exists() {
            "omarchy-lock-password".into()
        } else {
            super::generic::pam_service()
        }
    }

    fn theme(&self) -> Theme {
        let mut t = Theme::default();
        if let Some(o) = rustpaper_core::omarchy::theme() {
            let pick = |keys: &[&str], d: [u8; 3]| keys.iter().find_map(|k| o.rgb(k)).unwrap_or(d);
            t.foreground = pick(&["foreground"], t.foreground);
            t.background = pick(&["dark_background", "background"], t.background);
            t.accent = pick(&["accent", "blue"], t.accent);
            t.error = pick(&["red"], t.error);
        }
        t.font = std::process::Command::new("omarchy-font-current")
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|f| !f.is_empty());
        t
    }

    fn fallback_background(&self) -> Option<PathBuf> {
        // the theme's desktop background (Rustpaper's wallpaper theme sets it
        // to the wallpaper's preview)
        Some(state_home()?.join("omarchy/current/background")).filter(|p| p.exists())
    }

    fn on_locked(&self) {
        // what omarchy-system-lock does after locking: first keyboard layout,
        // lock 1Password, stop the screensaver
        let script = r#"
hyprctl switchxkblayout all 0 >/dev/null 2>&1
if pgrep -x 1password >/dev/null && command -v 1password >/dev/null; then
  timeout --kill-after=1s 3s 1password --lock >/dev/null 2>&1 &
fi
pkill -x ttfx 2>/dev/null
pkill -f '[o]rg.omarchy.screensaver' 2>/dev/null
true
"#;
        let _ = std::process::Command::new("bash")
            .args(["-c", script])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    }

    fn idle_lock_seconds(&self) -> Option<u32> {
        adopt_idle().map(|n| n.min(u32::MAX as u64) as u32)
    }

    fn enable(&self, exe: &Path) -> Result<Vec<String>, String> {
        let bindings = bindings_file().ok_or("no config directory")?;
        edit(&bindings, |t| {
            set_block(t, START, END, Some(&bindings_block(exe)))
        })?;
        let menu = menu_file().ok_or("no config directory")?;
        edit(&menu, |t| menu_with_lock(t, Some(exe)))?;
        adopt_idle();
        // our daemon locks before suspend from now on
        let _ = adapter::systemctl(&["mask", "--now", SLEEP_UNIT]);
        adapter::install_daemon(exe)?;
        Ok(vec![
            "Super+Ctrl+L, the lid, the System menu, idle and suspend now lock with Rustpaper's animated lock screen.".into(),
        ])
    }

    fn disable(&self) -> Result<(), String> {
        if let Some(b) = bindings_file() {
            edit(&b, |t| set_block(t, START, END, None))?;
        }
        if let Some(m) = menu_file()
            && m.exists()
        {
            edit(&m, |t| menu_with_lock(t, None))?;
        }
        restore_idle();
        let _ = adapter::systemctl(&["unmask", SLEEP_UNIT]);
        let _ = adapter::systemctl(&["start", SLEEP_UNIT]);
        adapter::remove_daemon()
    }

    fn enabled(&self) -> bool {
        bindings_file()
            .and_then(|b| std::fs::read_to_string(b).ok())
            .is_some_and(|t| has_block(&t, START))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_block_rebinds_the_lock_key_and_the_lid() {
        let b = bindings_block(Path::new("/usr/bin/rustpaper"));
        assert!(b.contains(
            r#"o.rebind("SUPER + CTRL + L", "Lock system", [["/usr/bin/rustpaper" lock]])"#
        ));
        assert!(b.contains("\"switch:on:Lid Switch\""));
        assert!(b.contains("omarchy-hyprland-monitor-clamshell"));
        assert!(!b.contains("]]]"), "no long-bracket clash");
    }

    /// The block parses as Lua (checked with `luac -p` where installed).
    #[test]
    fn the_block_is_valid_lua() {
        let Ok(luac) = which("luac") else { return };
        let dir = std::env::temp_dir().join(format!("rp-lua-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("bindings.lua");
        let user = "local o = require('x')\no.bind(\"SUPER + A\", \"x\", \"y\")\n";
        let text = set_block(
            user,
            START,
            END,
            Some(&bindings_block(Path::new("/home/a b/bin/rust'paper"))),
        );
        std::fs::write(&file, text).unwrap();
        let out = std::process::Command::new(luac)
            .arg("-p")
            .arg(&file)
            .output()
            .unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn which(cmd: &str) -> Result<PathBuf, ()> {
        std::env::split_paths(&std::env::var_os("PATH").ok_or(())?)
            .map(|d| d.join(cmd))
            .find(|p| p.is_file())
            .ok_or(())
    }

    #[test]
    fn menu_override_goes_inside_the_object() {
        let user = "{\n// thpm-menu-start\n  \"a\": {\"action\":\"x\"},\n// thpm-menu-end\n}\n";
        let with = menu_with_lock(user, Some(Path::new("/usr/bin/rustpaper")));
        assert_eq!(
            with,
            "{\n// rustpaper-lock-start\n  \"system.lock\": {\"action\":\"\\\"/usr/bin/rustpaper\\\" lock\"},\n// rustpaper-lock-end\n// thpm-menu-start\n  \"a\": {\"action\":\"x\"},\n// thpm-menu-end\n}\n"
        );
        assert_eq!(
            menu_with_lock(&with, Some(Path::new("/usr/bin/rustpaper"))),
            with,
            "idempotent"
        );
        assert_eq!(menu_with_lock(&with, None), user);
        assert!(
            menu_with_lock("", Some(Path::new("/x"))).starts_with("{\n// rustpaper-lock-start\n")
        );
    }

    #[test]
    fn idle_lock_is_read_and_set() {
        let (n, text) = shell_idle(
            "{\"version\":1,\"idle\":{\"screensaver\":150,\"lock\":300}}",
            Some(0),
        )
        .unwrap();
        assert_eq!(n, 300);
        assert!(text.contains("\"lock\": 0"));
        assert!(
            text.find("\"version\"") < text.find("\"idle\""),
            "key order kept"
        );
        assert_eq!(
            shell_idle("{\"version\":1}", None).unwrap().0,
            OMARCHY_DEFAULT_IDLE_LOCK
        );
        assert!(
            shell_idle("{\"version\":1}", Some(0))
                .unwrap()
                .1
                .contains("\"lock\": 0")
        );
    }
}
