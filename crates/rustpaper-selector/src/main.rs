// Selector: Tauri desktop app to browse and apply wallpapers. The Rust backend
// scans the Workshop folders with rustpaper-core and exposes commands to the
// web frontend in `ui/`:
//   list_wallpapers      -> [{id, title, kind, preview, workshop, added}]
//   apply(id)            -> shows that wallpaper (engine service, service.rs)
//   get_properties(id)   -> the wallpaper's customizable properties + saved values
//   save_properties(id, values) -> persists them; the engine reloads them
//   wallpaper_colors(id) -> dominant colors of its preview (color suggestions)
//   get_settings / set_autostart / set_volume
//   get_performance / set_performance / set_playback / set_video_player
//                        -> the Performance tab
//   workshop_query / workshop_subscribe / workshop_unsubscribe / workshop_status /
//   workshop_subscriptions -> the Steam Workshop store (workshop.rs)
//   get_theme            -> the active Omarchy theme (omarchy.rs), + `theme-changed` events
//   theme_from_wallpaper(id) / set_wallpaper_theme(enabled) / desktop_info
//                        -> an Omarchy theme from the wallpaper's colors (palette.rs)
//
// What is shown, and how, lives in rustpaper-core's state.json; the engine
// (`rustpaper --service`) reads it and re-reads it on reload.

mod omarchy;
mod palette;
mod service;
mod workshop;

use std::path::{Path, PathBuf};
use std::process::Child;
use std::sync::Mutex;
use std::time::Duration;

use rustpaper_core::performance;
use rustpaper_core::project::{Project, WallpaperKind};
use rustpaper_core::state::{self, Playback, Role, VideoPlayer};
use serde::Serialize;
use serde_json::{Map, Value};
use tauri::{Emitter, Manager};

/// The engine started as our own child, only when there is no systemd user
/// manager (otherwise systemd runs it: service.rs).
struct EngineState(Mutex<Option<Child>>);

#[derive(Serialize)]
struct WallpaperEntry {
    id: String, // Workshop id (folder name)
    title: String,
    kind: String,            // "video" | "scene" | "web" | "unknown"
    preview: Option<String>, // absolute path of the preview image
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
            // all of them play: the real Wallpaper Engine renders them
            let kind = match project.kind {
                WallpaperKind::Video => "video",
                WallpaperKind::Scene => "scene",
                WallpaperKind::Web => "web",
                WallpaperKind::Unknown(_) => "unknown",
            };
            out.push(WallpaperEntry {
                id,
                title: project.title,
                kind: kind.to_string(),
                preview: find_preview(&dir).map(|p| p.to_string_lossy().into_owned()),
                workshop: in_workshop,
                added: folder_added(&dir),
            });
        }
    }
    out.sort_by_key(|a| a.title.to_lowercase());
    out
}

/// Engine binary: next to the selector (target/{debug,release}/ or the
/// install prefix), else `rustpaper` from PATH. Absolute: it goes into the
/// systemd unit.
fn engine_path() -> PathBuf {
    let beside = std::env::current_exe()
        .ok()
        .and_then(|exe| Some(exe.parent()?.join("rustpaper")))
        .filter(|p| p.exists());
    beside
        .or_else(|| {
            std::env::split_paths(&std::env::var_os("PATH")?)
                .map(|d| d.join("rustpaper"))
                .find(|p| p.is_file())
        })
        .unwrap_or_else(|| PathBuf::from("rustpaper"))
}

/// Makes the engine show what state.json says (a new wallpaper, or changed
/// properties/settings). Through systemd: the running engine reloads in
/// place, or the service starts. Without systemd: a new child engine, the
/// previous one stopped 2.5 s later, after the new one drew its first frame
/// (so the desktop never flashes in between).
fn refresh_engine(state: &EngineState) -> Result<(), String> {
    let engine = engine_path();
    if service::available() {
        service::install(&engine)?;
        service::apply()?;
        // engines systemd does not run (older selectors, started by hand)
        std::thread::spawn(|| {
            std::thread::sleep(Duration::from_millis(2500));
            kill_stray_engines(service::main_pid());
        });
        return Ok(());
    }
    let child = std::process::Command::new(&engine)
        .arg("--service")
        .spawn()
        .map_err(|e| format!("failed to start the engine ({}): {e}", engine.display()))?;
    let previous = state.0.lock().unwrap().replace(child);
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(2500));
        if let Some(old) = previous {
            stop_engine(old);
        }
        kill_stray_engines(None);
    });
    Ok(())
}

/// An engine is showing a wallpaper (the service, or our child).
fn engine_running(state: &EngineState) -> bool {
    service::is_active()
        || state
            .0
            .lock()
            .unwrap()
            .as_mut()
            .is_some_and(|c| matches!(c.try_wait(), Ok(None)))
}

/// The desktop wallpaper on screen (every monitor's without its own), if
/// an engine runs.
fn desktop_showing(state: &EngineState) -> Option<String> {
    state::load().last_id.filter(|_| engine_running(state))
}

/// Wallpaper `id` is on screen somewhere (a monitor's desktop, the lock
/// screen or the screensaver) and an engine runs.
fn in_use(state: &EngineState, id: &str) -> bool {
    state::load().uses(id) && engine_running(state)
}

/// Refreshes the engine when one is running (settings changed).
fn refresh_if_running(state: &EngineState) -> Result<(), String> {
    if engine_running(state) {
        refresh_engine(state)?;
    }
    Ok(())
}

/// Stops an engine we started: SIGTERM first, so it exits cleanly (the last
/// engine out stops Wallpaper Engine's wallpapers); SIGKILL if it hangs.
fn stop_engine(mut child: std::process::Child) {
    // SAFETY: plain kill(2) on our own child's pid.
    unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGTERM) };
    for _ in 0..50 {
        if matches!(child.try_wait(), Ok(Some(_))) {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Stops the `rustpaper` engines of this user other than `keep` (the
/// service's) that this selector did not start: ones left by a previous selector session (closing the selector
/// leaves its wallpaper running) or started by hand / autostart. Two engines
/// on the same layer flicker. Our own children are left alone: the handover
/// above stops them one by one, and with quick successive applies a child
/// may be the engine that must stay (keeping "the newest pid" instead raced:
/// an older handover killed the newer engine).
fn kill_stray_engines(keep: Option<u32>) {
    // SAFETY: getuid has no preconditions.
    let uid = unsafe { libc::getuid() };
    let Ok(procs) = std::fs::read_dir("/proc") else {
        return;
    };
    for p in procs.flatten() {
        let Some(pid) = p.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else {
            continue;
        };
        if Some(pid) == keep || !is_engine(&p.path(), uid) {
            continue;
        }
        eprintln!("stopping engine {pid} (not started by this selector)");
        // SIGTERM: a clean exit (see stop_engine).
        // SAFETY: plain kill(2) on a pid that was just read from /proc.
        unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
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

/// A monitor name from the UI: what `rustpaper outputs` reported, nothing
/// else (it ends up in state.json).
fn check_output(output: &Option<String>) -> Result<(), String> {
    match output {
        Some(o) if !outputs().iter().any(|v| &v.name == o) => Err(format!("no monitor named {o}")),
        _ => Ok(()),
    }
}

/// Shows wallpaper `id` as `role` (default: the desktop) on `output` (None:
/// every monitor). The desktop wallpaper also themes the desktop when
/// "Theme from wallpaper" is on.
#[tauri::command]
fn apply(
    app: tauri::AppHandle,
    id: String,
    role: Option<Role>,
    output: Option<String>,
    state: tauri::State<EngineState>,
) -> Result<(), String> {
    wallpaper_dir(&id)?;
    check_output(&output)?;
    let role = role.unwrap_or(Role::Desktop);
    let mut st = state::load();
    if role != Role::Desktop && st.last_id.is_none() {
        // everything falls back to the desktop: it must exist first
        st.last_id = Some(id.clone());
    }
    st.assign(role, output.as_deref(), &id);
    state::save(&st).map_err(|e| e.to_string())?;
    refresh_engine(&state)?;
    if role == Role::Desktop && st.wallpaper_theme && omarchy::available() {
        // seconds of work (omarchy-theme-set retints every app): off the
        // command, reported through `wallpaper-theme` events
        std::thread::spawn(move || {
            let res = wallpaper_theme(&id);
            let _ = app.emit("wallpaper-theme", res.err());
        });
    }
    Ok(())
}

/// Generates and applies the Omarchy theme of wallpaper `id` (its preview's
/// colors). Blocking.
fn wallpaper_theme(id: &str) -> Result<(), String> {
    let dir = wallpaper_dir(id)?;
    let preview = find_preview(&dir).ok_or("this wallpaper has no preview image")?;
    let title = Project::load(&dir).map(|p| p.title).unwrap_or_default();
    omarchy::apply_wallpaper_theme(id, &title, &preview)
}

/// "Use as theme" in the panel: the Omarchy theme from this wallpaper, now.
#[tauri::command]
async fn theme_from_wallpaper(id: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || wallpaper_theme(&id))
        .await
        .map_err(|e| e.to_string())?
}

/// "Theme from wallpaper" in the header: saved; turning it on themes the
/// desktop from the wallpaper on screen right away (if any).
#[tauri::command]
async fn set_wallpaper_theme(
    enabled: bool,
    state: tauri::State<'_, EngineState>,
) -> Result<(), String> {
    let mut st = state::load();
    st.wallpaper_theme = enabled;
    state::save(&st).map_err(|e| e.to_string())?;
    match desktop_showing(&state) {
        Some(id) if enabled => tauri::async_runtime::spawn_blocking(move || wallpaper_theme(&id))
            .await
            .map_err(|e| e.to_string())?,
        _ => Ok(()),
    }
}

/// What the desktop offers: Hyprland (undecorated window), Omarchy (themes).
#[derive(Serialize)]
struct DesktopInfo {
    hyprland: bool,
    omarchy: bool,
    /// Tab to open on (`RUSTPAPER_SELECTOR_TAB`: library, store or
    /// performance), for screenshots and launcher shortcuts.
    start_tab: Option<String>,
}

#[tauri::command]
fn desktop_info() -> DesktopInfo {
    DesktopInfo {
        hyprland: omarchy::is_hyprland(),
        omarchy: omarchy::available(),
        start_tab: std::env::var("RUSTPAPER_SELECTOR_TAB").ok(),
    }
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
    let applied = in_use(&state, &id);
    Ok(PropertiesView {
        properties,
        saved: state::load_properties(&id),
        applied,
    })
}

/// Saves the user's values (an empty map = back to the defaults); the
/// engine reloads them when the wallpaper is on screen.
#[tauri::command]
fn save_properties(
    id: String,
    values: Map<String, Value>,
    state: tauri::State<EngineState>,
) -> Result<(), String> {
    wallpaper_dir(&id)?;
    state::save_properties(&id, &values).map_err(|e| e.to_string())?;
    if in_use(&state, &id) {
        refresh_engine(&state)?;
    }
    Ok(())
}

/// Dominant colors of a wallpaper's preview, most common first (`#rrggbb`):
/// suggestions for its color properties.
#[tauri::command]
async fn wallpaper_colors(id: String) -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let dir = wallpaper_dir(&id)?;
        let preview = find_preview(&dir).ok_or("this wallpaper has no preview image")?;
        omarchy::image_colors(&preview, 8)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// A monitor as the engine sees it (`rustpaper outputs`).
#[derive(Serialize, serde::Deserialize, Clone)]
struct OutputView {
    name: String,
    width: u32,
    height: u32,
}

/// The monitors, from the engine's desktop adapter (the names per-monitor
/// choices use). Empty when it cannot tell (no Wayland display).
fn outputs() -> Vec<OutputView> {
    std::process::Command::new(engine_path())
        .arg("outputs")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| serde_json::from_slice(&o.stdout).ok())
        .unwrap_or_default()
}

/// Where every wallpaper shows: the "Apply to" menu and the badges.
#[derive(Serialize)]
struct RolesView {
    outputs: Vec<OutputView>,
    /// Desktop for every monitor without its own.
    desktop: Option<String>,
    /// Desktop per monitor.
    monitors: std::collections::BTreeMap<String, String>,
    lock: Option<String>,
    screensaver: state::Screensaver,
}

#[tauri::command]
fn get_roles() -> RolesView {
    let st = state::load();
    RolesView {
        outputs: outputs(),
        desktop: st.last_id,
        monitors: st.monitors,
        lock: st.lock_id,
        screensaver: st.screensaver,
    }
}

/// Removes a role's choice so its fallback shows (the lock screen -> the
/// desktop's; a monitor's -> the one for every monitor).
#[tauri::command]
fn clear_role(
    role: Role,
    output: Option<String>,
    state: tauri::State<EngineState>,
) -> Result<(), String> {
    let mut st = state::load();
    st.unassign(role, output.as_deref());
    state::save(&st).map_err(|e| e.to_string())?;
    refresh_if_running(&state)
}

/// The screensaver switch and its delay.
#[tauri::command]
fn set_screensaver(
    enabled: bool,
    seconds: u32,
    state: tauri::State<EngineState>,
) -> Result<(), String> {
    let mut st = state::load();
    st.screensaver.enabled = enabled;
    st.screensaver.seconds = seconds.clamp(10, 24 * 3600);
    state::save(&st).map_err(|e| e.to_string())?;
    refresh_if_running(&state)
}

/// The header's settings.
#[derive(Serialize)]
struct SettingsView {
    last_id: Option<String>,
    /// The engine service starts with the graphical session.
    autostart: bool,
    volume: u8,
    wallpaper_theme: bool,
    /// The desktop's lock triggers run Rustpaper's animated lock screen.
    animated_lock: bool,
}

#[tauri::command]
fn get_settings() -> SettingsView {
    let st = state::load();
    SettingsView {
        last_id: st.last_id,
        autostart: service::is_enabled(),
        volume: st.volume,
        wallpaper_theme: st.wallpaper_theme,
        animated_lock: lock_status().unwrap_or(false),
    }
}

/// `rustpaper lock-setup status --json`: the engine's lock adapter knows
/// whether the desktop's lock triggers point at it.
fn lock_status() -> Option<bool> {
    let out = std::process::Command::new(engine_path())
        .args(["lock-setup", "status", "--json"])
        .output()
        .ok()?;
    let v: Value = serde_json::from_slice(&out.stdout).ok()?;
    v["enabled"].as_bool()
}

/// "Animated lock screen": `rustpaper lock-setup enable|disable` routes the
/// desktop's lock triggers (keybinding, idle, suspend; per desktop, see the
/// engine's lock adapters) to Rustpaper's lock, or gives them back. Returns
/// what the engine says to tell the user.
#[tauri::command]
async fn set_animated_lock(enabled: bool) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let out = std::process::Command::new(engine_path())
            .args(["lock-setup", if enabled { "enable" } else { "disable" }])
            .output()
            .map_err(|e| format!("rustpaper lock-setup: {e}"))?;
        let text = |b: &[u8]| String::from_utf8_lossy(b).trim().to_string();
        if out.status.success() {
            Ok(text(&out.stdout))
        } else {
            Err(text(&out.stderr))
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

/// "Start on login": enables/disables the engine's systemd user service.
#[tauri::command]
fn set_autostart(enabled: bool) -> Result<(), String> {
    if !service::available() {
        return Err("no systemd user session: start on login needs systemd".into());
    }
    service::install(&engine_path())?;
    service::set_enabled(enabled)
}

/// Volume slider (0..100, 0 = mute): saved, then the engine reloads.
#[tauri::command]
fn set_volume(volume: u8, state: tauri::State<EngineState>) -> Result<(), String> {
    let mut st = state::load();
    st.volume = volume.min(100);
    state::save(&st).map_err(|e| e.to_string())?;
    refresh_if_running(&state)
}

/// The Performance tab: WE's quality settings (with their current values)
/// and presets, the Playback rules, and what plays video wallpapers.
#[derive(Serialize)]
struct PerformanceView {
    settings: Vec<performance::Setting>,
    values: Map<String, Value>,
    presets: Vec<PresetView>,
    preset: &'static str,
    playback: Playback,
    playback_settings: Vec<performance::PlaybackSetting>,
    video_player: VideoPlayer,
    /// mpv is on PATH (without it, video wallpapers play in WE).
    mpv_installed: bool,
    /// WE runs on Wine's Wayland driver (GE-Proton, `rustpaper we-proton`).
    wine_wayland: bool,
}

#[derive(Serialize)]
struct PresetView {
    name: &'static str,
    values: Map<String, Value>,
}

#[tauri::command]
fn get_performance() -> PerformanceView {
    let st = state::load();
    let values = performance::current(&st.performance);
    PerformanceView {
        preset: performance::preset_of(&values),
        settings: performance::settings(),
        presets: performance::presets()
            .into_iter()
            .map(|(name, values)| PresetView { name, values })
            .collect(),
        values,
        playback: st.playback,
        playback_settings: performance::playback_settings(),
        video_player: st.video_player,
        mpv_installed: rustpaper_core::paths::program("mpv").is_some(),
        wine_wayland: st.wine_wayland,
    }
}

/// "Wine's Wayland driver" in the Performance tab: `rustpaper we-proton
/// enable|disable` (may download GE-Proton and restarts Steam; the engine
/// then restarts WE). Returns what the engine printed.
#[tauri::command]
async fn set_wine_wayland(enabled: bool) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let out = std::process::Command::new(engine_path())
            .args(["we-proton", if enabled { "enable" } else { "disable" }])
            .output()
            .map_err(|e| format!("rustpaper we-proton: {e}"))?;
        let text = |b: &[u8]| String::from_utf8_lossy(b).trim().to_string();
        if out.status.success() {
            Ok(text(&out.stdout)
                .lines()
                .last()
                .unwrap_or_default()
                .to_string())
        } else {
            Err(text(&out.stderr))
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Saves WE quality settings; the engine restarts WE with them (it must be
/// stopped while its config is written).
#[tauri::command]
fn set_performance(
    values: Map<String, Value>,
    state: tauri::State<EngineState>,
) -> Result<(), String> {
    let mut st = state::load();
    st.performance = performance::sanitize(&values);
    state::save(&st).map_err(|e| e.to_string())?;
    refresh_if_running(&state)
}

#[tauri::command]
fn set_playback(playback: Playback, state: tauri::State<EngineState>) -> Result<(), String> {
    let mut st = state::load();
    st.playback = playback;
    state::save(&st).map_err(|e| e.to_string())?;
    refresh_if_running(&state)
}

/// What plays video wallpapers: mpv (hardware decoding) or WE.
#[tauri::command]
fn set_video_player(player: VideoPlayer, state: tauri::State<EngineState>) -> Result<(), String> {
    let mut st = state::load();
    st.video_player = player;
    state::save(&st).map_err(|e| e.to_string())?;
    refresh_if_running(&state)
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
    let running = desktop_showing(&app.state::<EngineState>());
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
            wallpaper_colors,
            get_settings,
            set_autostart,
            set_volume,
            get_performance,
            set_performance,
            set_playback,
            set_video_player,
            set_wine_wayland,
            workshop_query,
            workshop_subscribe,
            workshop_unsubscribe,
            workshop_subscriptions,
            workshop_status,
            get_theme,
            theme_from_wallpaper,
            set_wallpaper_theme,
            set_animated_lock,
            get_roles,
            clear_role,
            set_screensaver,
            desktop_info
        ])
        // Closing the selector leaves the wallpaper running: systemd runs the
        // engine (or, without systemd, dropping a `Child` doesn't kill it and
        // the next selector replaces it: `kill_stray_engines`).
        .run(tauri::generate_context!())
        .expect("failed to run the Tauri app");
}

/// WebKitGTK workaround for the NVIDIA driver: WebKit's DMABUF renderer
/// hands the compositor the GPU's dmabufs, and the selector died at once
/// with "Error 71 (Protocol error) dispatching to Wayland display"
/// (Hyprland, NVIDIA open 615.78, webkit2gtk 2.54.1, 2026-11-27). Turning
/// the renderer off (`WEBKIT_DISABLE_DMABUF_RENDERER=1`, what we did
/// before, from the devcontainer days) paints on the CPU: the gallery's ~35
/// animated GIF previews kept the web process at 100% of a core and
/// scrolling lagged (1.7% with no GIFs on screen). With `FORCE_SHM` the GPU
/// still renders and only the hand-over goes through shared memory: no
/// protocol error, and scrolling is smooth. Left alone when the user set
/// either variable.
fn setup_linux_env() {
    if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none()
        && std::env::var_os("WEBKIT_DMABUF_RENDERER_FORCE_SHM").is_none()
    {
        // SAFETY: called first thing in main, before any thread exists.
        unsafe { std::env::set_var("WEBKIT_DMABUF_RENDERER_FORCE_SHM", "1") };
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
