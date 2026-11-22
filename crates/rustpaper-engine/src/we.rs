//! Driving the real Wallpaper Engine (WE), which runs under Proton.
//!
//! WE is started through Steam like any game. To send it commands
//! (`-control openWallpaper ...`) a second `wallpaper64.exe` is started in the
//! SAME Steam Runtime container and wineserver as the running one: we enter
//! its user + mount namespaces (`nsenter`) and reuse its environment. That
//! instance hands the command to the running WE and exits.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Process name of WE's renderer (exactly 15 chars, so `comm` holds it whole).
const WE_COMM: &str = "wallpaper64.exe";

/// Pids of processes of ours whose `comm` is `name`.
fn pids_named(name: &str) -> Vec<u32> {
    let Ok(rd) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    rd.flatten()
        .filter_map(|e| e.file_name().to_str()?.parse::<u32>().ok())
        .filter(|pid| {
            std::fs::read_to_string(format!("/proc/{pid}/comm")).is_ok_and(|c| c.trim_end() == name)
        })
        .collect()
}

fn cmdline(pid: u32) -> String {
    std::fs::read(format!("/proc/{pid}/cmdline"))
        .map(|b| String::from_utf8_lossy(&b).replace('\0', " "))
        .unwrap_or_default()
}

/// The running WE (not one of our short-lived `-control` instances).
pub fn pid() -> Option<u32> {
    pids_named(WE_COMM)
        .into_iter()
        .filter(|&p| !cmdline(p).contains("-control"))
        .min()
}

/// Linux path -> the same file as Wine sees it (`Z:` is the host root; the
/// Steam library drive letter is not mapped in every Proton session).
pub fn wine_path(p: &Path) -> String {
    format!("Z:{}", p.to_string_lossy().replace('/', "\\"))
}

/// Sends `args` to the running WE. False when WE is not running or the
/// control instance failed.
pub fn control(args: &[&str]) -> bool {
    let Some(dir) = rustpaper_core::paths::we_dir() else {
        return false;
    };
    let mut argv = vec![wine_path(&dir.join("wallpaper64.exe"))];
    argv.extend(args.iter().map(|a| a.to_string()));
    run_in_we(&argv)
}

/// Runs `wine <argv...>` inside the running WE's container and wineserver.
fn run_in_we(argv: &[String]) -> bool {
    let Some(pid) = pid() else {
        return false;
    };
    let args = argv;
    let Ok(environ) = std::fs::read(format!("/proc/{pid}/environ")) else {
        return false;
    };
    // the wineserver socket / preload settings belong to that process only
    let env: Vec<String> = environ
        .split(|&b| b == 0)
        .filter(|e| !e.is_empty())
        .map(|e| String::from_utf8_lossy(e).into_owned())
        .filter(|e| {
            ![
                "WINESERVERSOCKET=",
                "WINELOADERNOEXEC=",
                "WINEPRELOADRESERVE=",
            ]
            .iter()
            .any(|k| e.starts_with(k))
        })
        .collect();
    let child = Command::new("nsenter")
        .args([
            "-t",
            &pid.to_string(),
            "-U",
            "-m",
            "--preserve-credentials",
            "env",
            "-i",
        ])
        .args(&env)
        .arg("wine")
        .args(argv)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    let Ok(mut child) = child else {
        eprintln!("we: could not run nsenter (util-linux)");
        return false;
    };
    // it exits once the command is delivered (~1 s); never hang on it
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                eprintln!("we: control command timed out: {args:?}");
                return false;
            }
        }
    }
}

fn config_path() -> Option<PathBuf> {
    Some(rustpaper_core::paths::we_dir()?.join("config.json"))
}

/// Adjusts WE's `config.json` for being driven from here. Returns true when
/// it changed something (WE only reads it at start):
/// - `safemode: false`: WE sets it while running and clears it on a clean
///   exit; after a crash or kill the next start shows a "Safe Start" dialog
///   that blocks every `-control` command.
/// - `playbackfullscreen` / `playbackmaximized: run`: WE pauses a wallpaper
///   when another app is fullscreen or maximized, and every wallpaper window
///   of ours IS a fullscreen app: with two monitors each paused the other
///   (DP-1 went 60 -> 29 -> 0 fps, 2026-11-22). Pausing is decided here
///   instead, from what Hyprland shows on the real monitors.
pub fn prepare_config() -> bool {
    let Some(path) = config_path() else {
        return false;
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return false;
    };
    let Ok(mut j) = serde_json::from_str::<serde_json::Value>(text.trim_start_matches('\u{feff}'))
    else {
        return false;
    };
    let mut changed = false;
    if let Some(users) = j.as_object_mut() {
        for user in users.values_mut() {
            let Some(g) = user
                .pointer_mut("/general/user")
                .and_then(|g| g.as_object_mut())
            else {
                continue;
            };
            let mut set = |k: &str, v: serde_json::Value| {
                if g.get(k) != Some(&v) {
                    g.insert(k.into(), v);
                    changed = true;
                }
            };
            set("safemode", false.into());
            set("playbackfullscreen", "run".into());
            set("playbackmaximized", "run".into());
        }
    }
    if changed {
        let backup = path.with_extension("json.rustpaper-backup");
        if !backup.exists() {
            let _ = std::fs::copy(&path, &backup);
        }
        if let Ok(out) = serde_json::to_string_pretty(&j) {
            let _ = std::fs::write(&path, out);
        }
    }
    changed
}

fn kill_named(name: &str) {
    for pid in pids_named(name) {
        // SAFETY: plain kill(2) on a pid just read from /proc.
        unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
    }
}

/// Makes sure WE runs with our config. Starts it through Steam when needed
/// (restarting it when the config had to change). Blocks until WE accepts
/// commands; false when it never came up.
pub fn ensure_running() -> bool {
    let changed = prepare_config();
    if pid().is_some() && !changed {
        return true;
    }
    if pid().is_some() {
        eprintln!("we: restarting Wallpaper Engine (config changed)");
        return restart();
    }
    start()
}

/// Stops WE (UI, launcher and renderer) and starts it again. Also the cure
/// for a degraded WE: after dozens of wallpaper windows opened and closed it
/// rendered new ones black or pixelated, at ~3 fps, until restarted.
pub fn restart() -> bool {
    for n in ["launcher.exe", "wallpaperui.exe", WE_COMM] {
        kill_named(n);
    }
    let t = Instant::now();
    while pid().is_some() && t.elapsed() < Duration::from_secs(30) {
        std::thread::sleep(Duration::from_millis(250));
    }
    prepare_config();
    start()
}

fn start() -> bool {
    eprintln!("we: starting Wallpaper Engine through Steam");
    let started = Command::new("steam")
        .args(["-applaunch", rustpaper_core::paths::WE_APP_ID])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    if started.is_err() {
        eprintln!("we: `steam` not found: start Wallpaper Engine from Steam");
    }
    let t = Instant::now();
    while pid().is_none() {
        if t.elapsed() > Duration::from_secs(120) {
            eprintln!("we: Wallpaper Engine did not start");
            return false;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    // it accepts -control a few seconds after the process appears
    std::thread::sleep(Duration::from_secs(10));
    true
}

/// `-control applyProperties` for the wallpaper in window `window`: user
/// property values `{ "name": value }`.
///
/// WE parses the JSON out of its raw Windows command line, between `RAW~(`
/// and `)~END`. Passed as an argument, Wine rebuilds that command line with
/// the JSON's quotes escaped (`\"`) and WE rejected it (exit code 4, no
/// change). So the command goes into a .bat run by `cmd.exe`, which keeps the
/// line as written: the clock of Lonely Cat 3299228616 turned red with
/// `{"clockcolor":"1 0 0"}` that way (2026-11-22). Without `-location`, WE
/// applies properties to its desktop wallpapers only.
pub fn apply_properties(props: &serde_json::Map<String, serde_json::Value>, window: &str) -> bool {
    let Some(json) = batch_safe_json(props) else {
        return true;
    };
    let (Some(dir), Some(cache)) = (rustpaper_core::paths::we_dir(), cache_dir()) else {
        return false;
    };
    let bat = cache.join(format!("properties-{}.bat", std::process::id()));
    let line = format!(
        "@\"{}\" -control applyProperties -location {window} -properties RAW~({json})~END\r\n",
        wine_path(&dir.join("wallpaper64.exe"))
    );
    if std::fs::create_dir_all(&cache).is_err() || std::fs::write(&bat, line).is_err() {
        return false;
    }
    let ok = run_in_we(&["cmd".into(), "/c".into(), wine_path(&bat)]);
    let _ = std::fs::remove_file(&bat);
    ok
}

/// Somewhere both we and WE's container can read (the home is shared with
/// the Steam Runtime container).
fn cache_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))?;
    Some(base.join("rustpaper"))
}

/// The properties as JSON that survives a .bat line: `%` is doubled (cmd
/// expands `%VAR%` even inside quotes), and string values that could break
/// cmd's quoting or the code page (quotes, control characters, non-ASCII)
/// are left out. None when nothing is left.
fn batch_safe_json(props: &serde_json::Map<String, serde_json::Value>) -> Option<String> {
    let safe_str = |t: &str| {
        t.chars()
            .all(|c| c.is_ascii() && !c.is_ascii_control() && c != '"')
    };
    let kept: serde_json::Map<String, serde_json::Value> = props
        .iter()
        .filter(|(k, v)| {
            let ok = safe_str(k) && v.as_str().is_none_or(safe_str);
            if !ok {
                eprintln!("we: property {k} left out (its text cannot go through cmd.exe)");
            }
            ok
        })
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    if kept.is_empty() {
        return None;
    }
    Some(
        serde_json::Value::Object(kept)
            .to_string()
            .replace('%', "%%"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn property_json_survives_a_batch_line() {
        let props = serde_json::json!({
            "schemecolor": "1 0 0",
            "rate": 10,
            "label": "50% off",
            "quote": "say \"hi\"",
            "accent": "caf\u{e9}",
        });
        let json = batch_safe_json(props.as_object().unwrap()).unwrap();
        assert_eq!(
            json,
            r#"{"schemecolor":"1 0 0","rate":10,"label":"50%% off"}"#
        );
        assert!(batch_safe_json(&serde_json::Map::new()).is_none());
    }

    #[test]
    fn wine_paths_use_the_host_root_drive() {
        assert_eq!(
            wine_path(Path::new("/home/u/x/project.json")),
            "Z:\\home\\u\\x\\project.json"
        );
    }
}
