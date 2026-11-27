//! Driving the real Wallpaper Engine (WE), which runs under Proton.
//!
//! WE is started through Steam like any game. To send it commands
//! (`-control openWallpaper ...`) a second `wallpaper64.exe` is started in the
//! SAME Steam Runtime container and wineserver as the running one: we enter
//! its user + mount namespaces (`nsenter`) and reuse its environment. That
//! instance hands the command to the running WE and exits.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Map, Value};

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

/// The X display the running WE renders on (its `DISPLAY`).
pub fn display() -> Option<String> {
    env_var("DISPLAY")
}

/// Variable `name` of the running WE's environment.
pub fn env_var(name: &str) -> Option<String> {
    let environ = std::fs::read(format!("/proc/{}/environ", pid()?)).ok()?;
    let prefix = format!("{name}=");
    environ
        .split(|&b| b == 0)
        .find_map(|e| e.strip_prefix(prefix.as_bytes()))
        .map(|d| String::from_utf8_lossy(d).into_owned())
}

/// The audio driver of the running WE (launch.rs); None when WE is not
/// running.
pub fn audio() -> Option<crate::launch::WeAudio> {
    pid()?;
    Some(crate::launch::WeAudio::of(
        env_var("WINEDLLOVERRIDES").as_deref(),
    ))
}

/// Linux path -> the same file as Wine sees it (`Z:` is the host root; the
/// Steam library drive letter is not mapped in every Proton session).
pub fn wine_path(p: &Path) -> String {
    format!("Z:{}", p.to_string_lossy().replace('/', "\\"))
}

/// Sends `args` to the running WE. False when WE is not running or the
/// control instance failed.
pub fn control(args: &[&str]) -> bool {
    control_within(args, CONTROL_TIMEOUT)
}

/// How long a `-control` command may take. It is delivered in ~1 s, but a
/// busy WE can hold it much longer.
const CONTROL_TIMEOUT: Duration = Duration::from_secs(20);

/// [`control`], giving up after `limit`. The engine's exit uses a short
/// one: systemd stops the service after its TimeoutStopSec (20 s), and a
/// `-control stop` that took that long once got the engine SIGKILLed and
/// the service marked failed (2026-11-22).
pub fn control_within(args: &[&str], limit: Duration) -> bool {
    let Some(dir) = rustpaper_core::paths::we_dir() else {
        return false;
    };
    let mut argv = vec![wine_path(&dir.join("wallpaper64.exe"))];
    argv.extend(args.iter().map(|a| a.to_string()));
    run_in_we(&argv, limit)
}

/// Runs `wine <argv...>` inside the running WE's container and wineserver.
fn run_in_we(argv: &[String], limit: Duration) -> bool {
    // a stopped WE would never take the command (the watchdog pauses it
    // again on its next tick when nothing needs it)
    set_paused(false);
    let args = argv;
    let Some(mut child) = spawn_in_we(argv) else {
        return false;
    };
    // it exits once the command is delivered (~1 s); never hang on it
    let deadline = Instant::now() + limit;
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

/// Starts `wine <argv...>` inside the running WE's container and
/// wineserver, without waiting for it. None when WE is not running.
pub fn spawn_in_we(argv: &[String]) -> Option<Child> {
    let pid = pid()?;
    let environ = std::fs::read(format!("/proc/{pid}/environ")).ok()?;
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
    match child {
        Ok(c) => Some(c),
        Err(_) => {
            eprintln!("we: could not run nsenter (util-linux)");
            None
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
/// - `playbackfullscreen` / `playbackmaximized` / `playbackfocus: run`: WE
///   pauses a wallpaper when another app is fullscreen, maximized or
///   focused, and every wallpaper window of ours IS a fullscreen app: with
///   two monitors each paused the other (DP-1 went 60 -> 29 -> 0 fps,
///   2026-11-22). The engine applies the user's Playback settings itself,
///   from what the desktop adapter reports about the real monitors.
/// - the user's quality settings (`performance`, already sanitized).
pub fn prepare_config(performance: &Map<String, Value>) -> bool {
    edit_config(performance, true)
}

/// Whether WE runs with other quality settings than `performance` (it must
/// then be restarted to pick them up).
pub fn needs_restart(performance: &Map<String, Value>) -> bool {
    edit_config(performance, false)
}

fn edit_config(performance: &Map<String, Value>, write: bool) -> bool {
    let Some(path) = config_path() else {
        return false;
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return false;
    };
    let Ok(mut j) = serde_json::from_str::<Value>(text.trim_start_matches('\u{feff}')) else {
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
            let mut set = |k: &str, v: Value| {
                if g.get(k) != Some(&v) {
                    g.insert(k.into(), v);
                    changed = true;
                }
            };
            set("safemode", false.into());
            set("playbackfullscreen", "run".into());
            set("playbackmaximized", "run".into());
            set("playbackfocus", "run".into());
            for (k, v) in performance {
                set(k, v.clone());
            }
        }
    }
    if changed && write {
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

/// Every process of WE's launch: they carry Steam's `SteamAppId=431960` (or
/// WE's Wine prefix) in their environment. That includes what dies with WE
/// only eventually or not at all: Wine's `wineserver`, `winedevice.exe`,
/// `services.exe`..., and the embedded Chromium's `CrGpuMain`.
fn we_processes() -> Vec<u32> {
    let me = std::process::id();
    let Ok(rd) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    rd.flatten()
        .filter_map(|e| e.file_name().to_str()?.parse::<u32>().ok())
        .filter(|&pid| pid != me)
        .filter(|pid| {
            std::fs::read(format!("/proc/{pid}/environ")).is_ok_and(|env| {
                env.split(|&b| b == 0).any(|e| {
                    e == b"SteamAppId=431960"
                        || (e.starts_with(b"WINEPREFIX=")
                            && e.windows(18).any(|w| w == b"compatdata/431960"))
                })
            })
        })
        .collect()
}

/// WE paused with SIGSTOP by [`set_paused`].
static PAUSED: AtomicBool = AtomicBool::new(false);

/// What renders WE's wallpapers: the renderer itself and, for web
/// wallpapers, its embedded Chromium's subprocesses (`--type=gpu-process`,
/// `--type=renderer`...). Wine's own services stay running.
fn renderers() -> Vec<u32> {
    we_processes()
        .into_iter()
        .filter(|&pid| {
            let line = cmdline(pid);
            let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).unwrap_or_default();
            (comm.trim_end() == WE_COMM && !line.contains("-control")) || line.contains("--type=")
        })
        .collect()
}

/// Pauses WE (SIGSTOP on its renderers) or resumes it. WE's own
/// `-control pause` does nothing to the windows we open with
/// `-playInWindow` (audio.rs has the measurement), so a WE window no monitor
/// showed kept rendering, and so did WE under the Playback rules' Pause.
/// Stopped, WE's frames freeze where they are and it takes no CPU; the card
/// went from ~32 W to 24 W, and after SIGCONT it played on with no restart
/// (2026-11-23). Only with the private stage: nothing there pings its
/// clients, while a desktop's compositor could flag a stopped window as not
/// responding.
pub fn set_paused(paused: bool) {
    if PAUSED.swap(paused, Ordering::SeqCst) == paused {
        return;
    }
    signal_all(
        &renderers(),
        if paused { libc::SIGSTOP } else { libc::SIGCONT },
    );
    eprintln!("we: {}", if paused { "paused" } else { "resumed" });
}

/// Resumes a WE that an engine left stopped (killed while it was paused).
pub fn resume_stale() {
    PAUSED.store(false, Ordering::SeqCst);
    signal_all(&renderers(), libc::SIGCONT);
}

fn signal_all(pids: &[u32], sig: libc::c_int) {
    for &pid in pids {
        // SAFETY: plain kill(2) on pids just read from /proc.
        unsafe { libc::kill(pid as libc::pid_t, sig) };
    }
}

/// Stops WE's whole launch, not just its renderer. Killing only
/// wallpaper64.exe left Wine's helpers and a Chromium GPU process running
/// in its Steam container; Steam then refused every new launch
/// ("WaitingPrevProcess" -> AppError_16 in its console log) until they were
/// gone (2026-11-22). SIGTERM first, SIGKILL after 10 s.
fn stop_all() {
    // a stopped process only acts on SIGTERM once continued
    resume_stale();
    let t = Instant::now();
    signal_all(&we_processes(), libc::SIGTERM);
    while !we_processes().is_empty() {
        if t.elapsed() > Duration::from_secs(10) {
            signal_all(&we_processes(), libc::SIGKILL);
            std::thread::sleep(Duration::from_millis(500));
            break;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// Makes sure WE runs with our config. Starts it through Steam when needed
/// (restarting it when the config had to change). Blocks until WE accepts
/// commands; false when it never came up.
pub fn ensure_running(performance: &Map<String, Value>) -> bool {
    let changed = prepare_config(performance);
    if pid().is_some() && !changed {
        return true;
    }
    if pid().is_some() {
        eprintln!("we: restarting Wallpaper Engine (config changed)");
        return restart(performance);
    }
    start()
}

/// Stops WE (UI, launcher and renderer) and starts it again. Also the cure
/// for a degraded WE: after dozens of wallpaper windows opened and closed it
/// rendered new ones black or pixelated, at ~3 fps, until restarted.
pub fn restart(performance: &Map<String, Value>) -> bool {
    stop_all();
    // written while WE is stopped: a running WE rewrites its config on exit
    prepare_config(performance);
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
    let ok = run_in_we(
        &["cmd".into(), "/c".into(), wine_path(&bat)],
        CONTROL_TIMEOUT,
    );
    let _ = std::fs::remove_file(&bat);
    ok
}

/// Somewhere both we and WE's container can read (the home is shared with
/// the Steam Runtime container).
pub fn cache_dir() -> Option<PathBuf> {
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
