// Selector: Tauri desktop app to browse and apply wallpapers. The Rust backend
// scans the Workshop folders with rustpaper-core and exposes commands to the
// static web frontend in `ui/`:
//   list_wallpapers      -> [{id, title, kind, preview, supported}]
//   apply(id)            -> (re)starts the engine on that wallpaper
//   get_properties(id)   -> the wallpaper's customizable properties + saved values
//   save_properties(id, values) -> persists them and hot-swaps the running engine
//   get_settings / set_autostart / set_scripts / set_volume
//   workshop_query / workshop_subscribe / workshop_unsubscribe / workshop_status /
//   workshop_subscriptions
//   get_theme            -> the active Omarchy theme (omarchy.rs), + `theme-changed` events
//                        -> the Steam Workshop store (workshop.rs)

mod omarchy;
mod settings;
mod workshop;

use std::path::{Path, PathBuf};
use std::process::Child;
use std::sync::Mutex;
use std::time::Duration;

use rustpaper_core::project::{Project, WallpaperKind};
use serde::Serialize;
use serde_json::{Map, Value};
use tauri::Manager;

/// The running engine and the wallpaper it shows (killed when switching).
struct EngineState(Mutex<Option<(String, Child)>>);

#[derive(Serialize)]
struct WallpaperEntry {
    id: String, // Workshop id (folder name)
    title: String,
    kind: String,            // "video" | "scene" | "web" | "unknown"
    preview: Option<String>, // absolute path of the preview image
    supported: bool,         // can the engine play it?
    /// In Steam's Workshop folder (can be unsubscribed), not the test corpus.
    workshop: bool,
    /// When the folder appeared (Unix seconds): the download right after
    /// subscribing. Fallback for "recently subscribed" when Steam is not
    /// running (Steam keeps no subscription time locally: appworkshop's
    /// `timetouched` changes on every update check).
    added: u64,
}

/// Creation time of a folder (statx birth time; mtime where unsupported).
fn folder_added(dir: &Path) -> u64 {
    std::fs::metadata(dir)
        .and_then(|m| m.created().or_else(|_| m.modified()))
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs())
}

/// Preview file of a wallpaper folder (static images first, gif last).
fn find_preview(dir: &Path) -> Option<PathBuf> {
    ["preview.jpg", "preview.png", "preview.jpeg", "preview.gif"]
        .iter()
        .map(|n| dir.join(n))
        .find(|p| p.exists())
}

fn wallpaper_dir(id: &str) -> Result<PathBuf, String> {
    // ids are folder names: never let one escape the Workshop folders
    if id.is_empty() || id.contains(['/', '\\']) || id.starts_with('.') {
        return Err(format!("invalid wallpaper id {id:?}"));
    }
    rustpaper_core::paths::find_wallpaper(id)
        .filter(|d| d.join("project.json").exists())
        .ok_or_else(|| format!("project.json not found for {id}"))
}

#[tauri::command]
fn list_wallpapers() -> Vec<WallpaperEntry> {
    let mut out: Vec<WallpaperEntry> = Vec::new();
    let workshop = rustpaper_core::paths::workshop_dir();
    for base in rustpaper_core::paths::wallpaper_dirs() {
        let in_workshop = workshop.as_ref() == Some(&base);
        let Ok(entries) = std::fs::read_dir(&base) else {
            continue;
        };
        for e in entries.flatten() {
            let dir = e.path();
            let id = e.file_name().to_string_lossy().into_owned();
            if !dir.is_dir() || out.iter().any(|w| w.id == id) {
                continue;
            }
            let Ok(project) = Project::load(&dir) else {
                continue;
            };
            let (kind, supported) = match project.kind {
                WallpaperKind::Video => ("video", true),
                WallpaperKind::Scene => ("scene", true),
                WallpaperKind::Web => ("web", false),
                WallpaperKind::Unknown(_) => ("unknown", false),
            };
            out.push(WallpaperEntry {
                id,
                title: project.title,
                kind: kind.to_string(),
                preview: find_preview(&dir).map(|p| p.to_string_lossy().into_owned()),
                supported,
                workshop: in_workshop,
                added: folder_added(&dir),
            });
        }
    }
    out.sort_by_key(|a| a.title.to_lowercase());
    out
}

/// Engine binary: next to the selector (target/{debug,release}/ or the
/// install prefix), else `rustpaper` from PATH.
fn engine_path() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| Some(exe.parent()?.join("rustpaper")))
        .filter(|p| p.exists())
        .unwrap_or_else(|| PathBuf::from("rustpaper"))
}

/// Engine command line for a wallpaper, with its saved property values.
fn engine_args(id: &str, dir: &Path) -> Vec<String> {
    let mut args = vec![dir.to_string_lossy().into_owned()];
    if let Some(p) = settings::properties_file(id).filter(|p| p.exists()) {
        args.push("--props".into());
        args.push(p.to_string_lossy().into_owned());
    }
    let s = settings::load();
    if s.no_scripts {
        args.push("--no-scripts".into());
    }
    match s.volume {
        0 => args.push("--mute".into()),
        100.. => {}
        v => {
            args.push("--volume".into());
            args.push(v.to_string());
        }
    }
    args
}

/// Starts the engine on `id`. The previous engine is killed only after the
/// new one had time to load and draw its first frame, so switching (or
/// tweaking a property) never flashes the desktop background in between.
fn start_engine(id: &str, state: &EngineState) -> Result<(), String> {
    let dir = wallpaper_dir(id)?;
    let child = std::process::Command::new(engine_path())
        .args(engine_args(id, &dir))
        .spawn()
        .map_err(|e| format!("failed to start the engine: {e}"))?;
    let previous = state.0.lock().unwrap().replace((id.to_string(), child));
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(2500));
        if let Some((_, mut old)) = previous {
            let _ = old.kill();
            let _ = old.wait();
        }
        kill_stray_engines();
    });
    Ok(())
}

/// Kills the `rustpaper` engines of this user that this selector did not
/// start: ones left by a previous selector session (closing the selector
/// leaves its wallpaper running) or started by hand / autostart. Two engines
/// on the same layer flicker. Our own children are left alone: the handover
/// above stops them one by one, and with quick successive applies a child
/// may be the engine that must stay (keeping "the newest pid" instead raced:
/// an older handover killed the newer engine).
fn kill_stray_engines() {
    // SAFETY: getuid has no preconditions.
    let uid = unsafe { libc::getuid() };
    let Ok(procs) = std::fs::read_dir("/proc") else {
        return;
    };
    for p in procs.flatten() {
        let Some(pid) = p.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else {
            continue;
        };
        if !is_engine(&p.path(), uid) {
            continue;
        }
        eprintln!("stopping engine {pid} (not started by this selector)");
        // SAFETY: plain kill(2) on a pid that was just read from /proc.
        unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
    }
}

/// `/proc/<pid>` is a `rustpaper` process (exact name, so not the selector
/// or the script worker) owned by `uid` and not a child of this selector.
fn is_engine(proc_dir: &Path, uid: libc::uid_t) -> bool {
    let named =
        std::fs::read_to_string(proc_dir.join("comm")).is_ok_and(|c| c.trim_end() == "rustpaper");
    let Some(status) = named
        .then(|| std::fs::read_to_string(proc_dir.join("status")).ok())
        .flatten()
    else {
        return false;
    };
    let field = |name: &str| {
        status
            .lines()
            .find_map(|l| l.strip_prefix(name))
            .and_then(|l| l.split_whitespace().next())
            .and_then(|v| v.parse::<u32>().ok())
    };
    field("Uid:") == Some(uid) && field("PPid:") != Some(std::process::id())
}

#[tauri::command]
fn apply(id: String, state: tauri::State<EngineState>) -> Result<(), String> {
    start_engine(&id, &state)?;
    // Remember the wallpaper; with autostart on, point the .desktop at it.
    let mut s = settings::load();
    s.last_id = Some(id.clone());
    if s.autostart {
        let _ = settings::write_autostart(&engine_path(), &engine_args(&id, &wallpaper_dir(&id)?));
    }
    settings::save(&s);
    Ok(())
}

/// What the customization panel needs: the property definitions of
/// project.json (`general.properties`, in WE's display order) and the values
/// the user saved for this wallpaper.
#[derive(Serialize)]
struct PropertiesView {
    properties: Vec<Value>,
    saved: Map<String, Value>,
    applied: bool,
}

#[tauri::command]
fn get_properties(id: String, state: tauri::State<EngineState>) -> Result<PropertiesView, String> {
    let dir = wallpaper_dir(&id)?;
    let text = std::fs::read_to_string(dir.join("project.json")).map_err(|e| e.to_string())?;
    let project: Value =
        serde_json::from_str(text.trim_start_matches('\u{feff}')).map_err(|e| e.to_string())?;
    let mut properties: Vec<Value> = project
        .pointer("/general/properties")
        .and_then(|p| p.as_object())
        .into_iter()
        .flatten()
        .map(|(name, p)| {
            let mut p = p.clone();
            if let Value::Object(m) = &mut p {
                m.insert("name".into(), Value::String(name.clone()));
            }
            p
        })
        .collect();
    properties.sort_by_key(|p| p.get("order").and_then(|o| o.as_i64()).unwrap_or(0));
    let applied = state
        .0
        .lock()
        .unwrap()
        .as_ref()
        .is_some_and(|(cur, _)| *cur == id);
    Ok(PropertiesView {
        properties,
        saved: settings::load_properties(&id),
        applied,
    })
}

/// Saves the user's values (an empty map = back to the defaults) and, when
/// the wallpaper is on screen, restarts the engine with them.
#[tauri::command]
fn save_properties(
    id: String,
    values: Map<String, Value>,
    state: tauri::State<EngineState>,
) -> Result<(), String> {
    wallpaper_dir(&id)?;
    settings::save_properties(&id, &values).map_err(|e| e.to_string())?;
    let running = state
        .0
        .lock()
        .unwrap()
        .as_ref()
        .is_some_and(|(cur, _)| *cur == id);
    if running {
        start_engine(&id, &state)?;
    }
    Ok(())
}

#[tauri::command]
fn get_settings() -> settings::Settings {
    settings::load()
}

#[tauri::command]
fn set_autostart(enabled: bool) -> Result<(), String> {
    let mut s = settings::load();
    s.autostart = enabled;
    settings::save(&s);
    if !enabled {
        settings::remove_autostart();
        return Ok(());
    }
    // A wallpaper must have been applied to know what to restore.
    let id = s
        .last_id
        .ok_or("apply a wallpaper before enabling autostart")?;
    let args = engine_args(&id, &wallpaper_dir(&id)?);
    settings::write_autostart(&engine_path(), &args).map_err(|e| e.to_string())
}

/// "Run scripts" checkbox: saves the preference, restarts the running
/// wallpaper with it and refreshes the autostart entry.
#[tauri::command]
fn set_scripts(enabled: bool, state: tauri::State<EngineState>) -> Result<(), String> {
    let mut s = settings::load();
    s.no_scripts = !enabled;
    settings::save(&s);
    restart_with_settings(&s, &state)
}

/// Volume slider (0..100, 0 = mute): same as `set_scripts`.
#[tauri::command]
fn set_volume(volume: u8, state: tauri::State<EngineState>) -> Result<(), String> {
    let mut s = settings::load();
    s.volume = volume.min(100);
    settings::save(&s);
    restart_with_settings(&s, &state)
}

/// After an engine-wide setting changed: restarts the running wallpaper
/// (its command line carries the setting) and refreshes the autostart entry.
fn restart_with_settings(s: &settings::Settings, state: &EngineState) -> Result<(), String> {
    let running = state.0.lock().unwrap().as_ref().map(|(id, _)| id.clone());
    if let Some(id) = running {
        start_engine(&id, state)?;
    }
    if s.autostart
        && let Some(id) = &s.last_id
    {
        let args = engine_args(id, &wallpaper_dir(id)?);
        settings::write_autostart(&engine_path(), &args).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// The active Omarchy theme's palette, None outside Omarchy (the UI keeps its
/// own colors).
#[tauri::command]
fn get_theme() -> Option<omarchy::Theme> {
    omarchy::theme()
}

/// Runs a Workshop request on a blocking thread (the helper answers in
/// seconds; Tauri commands must not block the UI).
async fn workshop_call(app: tauri::AppHandle, cmd: Value) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || app.state::<workshop::Workshop>().request(cmd))
        .await
        .map_err(|e| e.to_string())?
}

/// One page of the Workshop: `{sort, kind, search, page, mature}` ->
/// `{total, page, items: [...]}`.
#[tauri::command]
async fn workshop_query(app: tauri::AppHandle, query: Value) -> Result<Value, String> {
    let mut cmd = match query {
        Value::Object(m) => Value::Object(m),
        _ => return Err("query must be an object".into()),
    };
    cmd["cmd"] = Value::from("query");
    workshop_call(app, cmd).await
}

#[tauri::command]
async fn workshop_subscribe(app: tauri::AppHandle, id: String) -> Result<(), String> {
    workshop_call(app, serde_json::json!({ "cmd": "subscribe", "item": id }))
        .await
        .map(|_| ())
}

/// Unsubscribes and removes the downloaded folder right away (Steam keeps
/// it until a later cleanup, so it would stay in the gallery). Refused for
/// the wallpaper on screen: the engine may still read its files (videos
/// stream from disk).
#[tauri::command]
async fn workshop_unsubscribe(app: tauri::AppHandle, id: String) -> Result<(), String> {
    let running = app
        .state::<EngineState>()
        .0
        .lock()
        .unwrap()
        .as_ref()
        .map(|(r, _)| r.clone());
    if running.as_deref() == Some(id.as_str()) {
        return Err("This wallpaper is on screen: apply another one before unsubscribing.".into());
    }
    workshop_call(app, serde_json::json!({ "cmd": "unsubscribe", "item": id })).await?;
    workshop::remove_unsubscribed(&id)
}

/// When the user subscribed to each item: `{id: unix seconds}` (Steam's
/// subscriptions list, every page).
#[tauri::command]
async fn workshop_subscriptions(app: tauri::AppHandle) -> Result<Value, String> {
    workshop_call(app, serde_json::json!({ "cmd": "subscriptions" })).await
}

/// Local state of items: `{id: {subscribed, installed, downloading,
/// downloaded, total, needs_update}}`.
#[tauri::command]
async fn workshop_status(app: tauri::AppHandle, ids: Vec<String>) -> Result<Value, String> {
    workshop_call(app, serde_json::json!({ "cmd": "status", "items": ids })).await
}

fn main() {
    setup_linux_env();

    tauri::Builder::default()
        .manage(EngineState(Mutex::new(None)))
        .manage(workshop::Workshop::default())
        .setup(|app| {
            // Previews are served by the asset protocol: allow exactly the
            // folders wallpapers come from (they vary per Steam library).
            let scope = app.asset_protocol_scope();
            for dir in rustpaper_core::paths::wallpaper_dirs() {
                scope.allow_directory(&dir, true)?;
            }
            // tauri.conf.json opens the window undecorated: under Hyprland it
            // gets the compositor's border/rounding like Omarchy's own apps,
            // the UI header being the top of the window; elsewhere (GNOME...)
            // the toolkit's title bar and buttons come back.
            if !omarchy::is_hyprland()
                && let Some(w) = app.get_webview_window("main")
            {
                w.set_decorations(true)?;
            }
            omarchy::watch(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_wallpapers,
            apply,
            get_properties,
            save_properties,
            get_settings,
            set_autostart,
            set_scripts,
            set_volume,
            workshop_query,
            workshop_subscribe,
            workshop_unsubscribe,
            workshop_subscriptions,
            workshop_status,
            get_theme
        ])
        // Closing the selector leaves the wallpaper running (dropping a
        // `Child` doesn't kill it); the next selector replaces it on apply
        // (`kill_stray_engines`).
        .run(tauri::generate_context!())
        .expect("failed to run the Tauri app");
}

/// WebKitGTK workarounds. DMABUF rendering breaks on the NVIDIA driver
/// (blank window), so it is off unless the user set the variable already.
fn setup_linux_env() {
    if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
        // SAFETY: called first thing in main, before any thread exists.
        unsafe { std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1") };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engines_are_recognized_by_exact_name_and_owner() {
        // a copy of `sleep` named `rustpaper` looks like an engine to /proc
        let dir = std::env::temp_dir().join(format!("rp-engine-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let fake = dir.join("rustpaper");
        std::fs::copy("/bin/sleep", &fake).unwrap();
        let uid = unsafe { libc::getuid() };
        // our own child: stopped by the handover, never a stray
        let mut own = std::process::Command::new(&fake).arg("5").spawn().unwrap();
        let own_dir = PathBuf::from(format!("/proc/{}", own.id()));
        assert!(!is_engine(&own_dir, uid), "a child of this selector");
        // started by someone else (here: a shell that is its parent)
        let mut sh = std::process::Command::new("sh")
            .arg("-c")
            .arg(format!("'{}' 5; true", fake.display()))
            .spawn()
            .unwrap();
        let children = format!("/proc/{0}/task/{0}/children", sh.id());
        let stray = (0..50)
            .find_map(|_| {
                std::thread::sleep(Duration::from_millis(20));
                let c = std::fs::read_to_string(&children).ok()?;
                c.split_whitespace().next()?.parse::<u32>().ok()
            })
            .expect("the shell's child");
        let stray_dir = PathBuf::from(format!("/proc/{stray}"));
        assert!(is_engine(&stray_dir, uid));
        assert!(!is_engine(&stray_dir, uid + 1), "another user's engine");
        assert!(!is_engine(Path::new("/proc/self"), uid), "the test binary");
        unsafe { libc::kill(stray as libc::pid_t, libc::SIGKILL) };
        for c in [&mut own, &mut sh] {
            let _ = c.kill();
            let _ = c.wait();
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
