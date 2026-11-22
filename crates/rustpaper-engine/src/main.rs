//! rustpaper: Wallpaper Engine wallpapers as the Wayland desktop background.
//!
//! The wallpaper is rendered by the REAL Wallpaper Engine (WE), running under
//! Proton through Steam, so it looks exactly like it does on Windows. For each
//! monitor there is ONE long-lived WE window, shared by every engine instance:
//!
//! 1. a headless Hyprland output `RP-<output>` of the monitor's pixel size,
//!    its default workspace `rustpaper-<output>` pinned to it: out of the
//!    user's sight, but visible to the compositor, which only renders (and
//!    lets us capture) visible windows;
//! 2. WE plays the wallpaper in window `rustpaper-<output>`
//!    (`-control openWallpaper -playInWindow`), which a window rule sends
//!    fullscreen onto that workspace. Asking again with the same window name
//!    loads the new wallpaper INTO that window: switching never opens or
//!    closes windows. (Opening and closing them on every switch degraded WE
//!    within minutes: closes hit the wrong window, closeWallpaper did nothing,
//!    new windows came up black or pixelated at ~3 fps.)
//! 3. `mirror` captures the window and shows it on a layer-shell surface on
//!    the monitor.
//!
//! A watchdog keeps that arrangement (workspace shown, window fullscreen,
//! window reopened if WE dropped it, WE restarted if it keeps failing) and
//! pauses where a fullscreen window covers the monitor. The selector's
//! handover starts the new engine before stopping the old one, so for a
//! moment both mirror the same window; the last engine to exit stops WE's
//! wallpapers (`-control stop`).

mod gbm;
mod hyprland;
mod mirror;
mod power;
mod we;

use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use mirror::Mirrors;
use rustpaper_core::performance;
use rustpaper_core::state::{self, Action, Playback};
use serde_json::{Map, Value};

/// WE's window class under Proton (`steam_app_<AppID>`).
const WE_CLASS: &str = "steam_app_431960";
/// Hidden workspace for WE's own windows (its UI, dialogs).
const WE_UI_WORKSPACE: &str = "special:rustpaper-we";
/// Watchdog period.
const TICK: Duration = Duration::from_secs(2);
/// Time WE gets to open a window before it is asked again.
const OPEN_TIMEOUT: Duration = Duration::from_secs(40);
/// Unanswered window requests before WE is restarted.
const MAX_ATTEMPTS: u32 = 2;
/// Wait after asking for a wallpaper before sending its property values: the
/// window may already exist (showing the previous wallpaper) while WE is
/// still loading the new one, which starts from its defaults.
const PROPS_DELAY: Duration = Duration::from_secs(3);

static QUIT: AtomicBool = AtomicBool::new(false);
/// SIGHUP (`systemctl --user reload rustpaper`): re-read the selector's state.
static RELOAD: AtomicBool = AtomicBool::new(false);

extern "C" fn on_quit(_: libc::c_int) {
    QUIT.store(true, Ordering::SeqCst);
}

extern "C" fn on_reload(_: libc::c_int) {
    RELOAD.store(true, Ordering::SeqCst);
}

/// What to show and how.
#[derive(Clone, PartialEq)]
struct Config {
    /// The wallpaper's project.json (absolute).
    project: PathBuf,
    /// User property values `{ "name": value }`.
    properties: Map<String, Value>,
    mute: bool,
    playback: Playback,
    /// WE quality settings (sanitized); empty = leave WE's as they are.
    performance: Map<String, Value>,
}

/// Where the Config comes from.
enum Source {
    /// `rustpaper <dir> [...]`: fixed.
    Args(Config),
    /// `rustpaper --service`: the selector's state.json, re-read on SIGHUP.
    Service,
}

/// One monitor showing the wallpaper.
struct Target {
    output: String,
    size: (u32, u32),
    headless: String,
    /// Workspace on the headless output AND title of WE's window.
    name: String,
    /// When WE was last asked for the window.
    opened: Instant,
    /// Requests since the window was last seen.
    attempts: u32,
    /// Properties/mute were sent for the current request.
    configured: bool,
}

fn usage() -> ! {
    eprintln!(
        "usage: rustpaper <wallpaper folder> [--props properties.json] [--mute] [--volume 0..100] [--output NAME]\n       rustpaper --service [--output NAME]   (the selector's choice; SIGHUP re-reads it)"
    );
    std::process::exit(2);
}

fn parse_args() -> (Source, Option<String>) {
    let mut args = std::env::args().skip(1);
    let mut dir = None;
    let mut props_file: Option<PathBuf> = None;
    let mut mute = false;
    let mut output = None;
    let mut service = false;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--service" => service = true,
            "--props" => props_file = args.next().map(PathBuf::from),
            "--mute" | "--no-audio" => mute = true,
            // WE has no volume command: anything above 0 plays at WE's own
            // volume (set in WE's settings)
            "--volume" => mute = args.next().and_then(|v| v.parse::<u32>().ok()) == Some(0),
            "--output" => output = args.next(),
            // options of the old native renderer, accepted so existing
            // autostart entries keep working
            "--fps" => {
                args.next();
            }
            "--no-scripts" => {}
            "-h" | "--help" => usage(),
            _ if a.starts_with('-') => usage(),
            _ => dir = Some(PathBuf::from(a)),
        }
    }
    if service {
        return (Source::Service, output);
    }
    let Some(dir) = dir else { usage() };
    let properties = props_file
        .and_then(|f| std::fs::read_to_string(f).ok())
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();
    let project = project_file(&dir).unwrap_or_else(|e| {
        eprintln!("error: {e}");
        std::process::exit(1);
    });
    let config = Config {
        project,
        properties,
        mute,
        playback: Playback::default(),
        performance: Map::new(),
    };
    (Source::Args(config), output)
}

fn project_file(dir: &Path) -> Result<PathBuf, String> {
    let project = dir.join("project.json");
    if !project.is_file() {
        return Err(format!("{} not found", project.display()));
    }
    Ok(std::fs::canonicalize(&project).unwrap_or(project))
}

/// The selector's choice (`~/.config/rustpaper/state.json`).
fn service_config() -> Result<Config, String> {
    let st = state::load();
    let id = st
        .last_id
        .clone()
        .ok_or("no wallpaper chosen yet (pick one in rustpaper-selector)")?;
    let dir =
        rustpaper_core::paths::find_wallpaper(&id).ok_or(format!("wallpaper {id} not found"))?;
    Ok(Config {
        project: project_file(&dir)?,
        properties: state::load_properties(&id),
        mute: st.volume == 0,
        playback: st.playback,
        performance: performance::sanitize(&st.performance),
    })
}

fn main() {
    let (source, output) = parse_args();
    let mut cfg = match &source {
        Source::Args(c) => c.clone(),
        Source::Service => service_config().unwrap_or_else(|e| {
            // nothing to show is not a failure (the service must not loop)
            eprintln!("rustpaper: {e}");
            std::process::exit(0);
        }),
    };
    for sig in [libc::SIGTERM, libc::SIGINT] {
        // SAFETY: the handler only stores to an atomic.
        unsafe { libc::signal(sig, on_quit as *const () as libc::sighandler_t) };
    }
    // SAFETY: same.
    unsafe { libc::signal(libc::SIGHUP, on_reload as *const () as libc::sighandler_t) };
    if hyprland::json("j/version").is_none() {
        eprintln!("error: rustpaper needs Hyprland (0.56 or newer)");
        std::process::exit(1);
    }
    // before any layer surface exists: no compositor fade when switching
    hyprland::disable_layer_animations();

    let targets = plan_targets(output.as_deref());
    if targets.is_empty() {
        eprintln!("error: no monitor to show the wallpaper on");
        std::process::exit(1);
    }
    if !we::ensure_running(&cfg.performance) {
        eprintln!("error: Wallpaper Engine is not running (start it from Steam)");
        std::process::exit(1);
    }
    hide_we_ui();
    // a previous instance may have paused WE (all monitors covered)
    we::control(&["-control", "play"]);

    let (mut mirrors, mut queue, conn) = Mirrors::connect().unwrap_or_else(|e| {
        eprintln!("error: {e}");
        std::process::exit(1);
    });
    let qh = queue.handle();
    let mut targets: Vec<Target> = targets
        .into_iter()
        .filter_map(|mut t| {
            prepare_target(&t);
            open_window(&cfg.project, &mut t);
            mirrors
                .add(&qh, &t.output, &t.name)
                .map_err(|e| eprintln!("{}: {e}", t.output))
                .ok()?;
            Some(t)
        })
        .collect();

    let mut we_paused = false;
    let mut last_tick = Instant::now() - TICK;
    let mut stats = Instant::now();
    let fd = conn.backend().poll_fd().as_raw_fd();
    while !QUIT.load(Ordering::SeqCst) {
        if let Err(e) = dispatch(&mut queue, &mut mirrors, fd) {
            eprintln!("error: Wayland connection lost: {e}");
            break;
        }
        mirrors.pump(&qh);
        if RELOAD.swap(false, Ordering::SeqCst) && matches!(source, Source::Service) {
            match service_config() {
                Ok(new) => {
                    reload(&cfg, &new, &mut targets);
                    cfg = new;
                }
                Err(e) => eprintln!("reload: {e}"),
            }
            last_tick = Instant::now() - TICK;
        }
        if last_tick.elapsed() >= TICK {
            last_tick = Instant::now();
            watchdog(&cfg, &mut targets, &mut mirrors, &mut we_paused);
        }
        if stats.elapsed() >= Duration::from_secs(60) {
            let dt = stats.elapsed().as_secs_f64();
            for m in &mut mirrors.list {
                eprintln!(
                    "{}: {:.1} fps{}",
                    m.output,
                    m.frames as f64 / dt,
                    if m.paused { " (paused)" } else { "" }
                );
                m.frames = 0;
            }
            stats = Instant::now();
        }
    }
    // The windows and headless outputs stay for the next engine (closing
    // WE's windows is unreliable, see the top). The last engine out stops
    // WE's wallpapers so they cost nothing while no wallpaper is shown.
    if !other_engine_running() {
        we::control(&["-control", "stop"]);
    }
}

/// Applies a new Config without restarting the engine (so the desktop never
/// shows the plain background in between): new quality settings restart WE,
/// a new wallpaper or new property values load into the same windows.
fn reload(old: &Config, new: &Config, targets: &mut [Target]) {
    if !new.performance.is_empty() && we::needs_restart(&new.performance) {
        eprintln!("reload: new quality settings, restarting Wallpaper Engine");
        if we::restart(&new.performance) {
            hide_we_ui();
        }
        for t in targets.iter_mut() {
            t.attempts = 0;
            open_window(&new.project, t);
        }
    } else if new.project != old.project || new.properties != old.properties {
        eprintln!("reload: {}", new.project.display());
        for t in targets.iter_mut() {
            open_window(&new.project, t);
        }
    } else if new.mute != old.mute {
        we::control(&["-control", if new.mute { "mute" } else { "unmute" }]);
    }
}

/// Another `rustpaper` of this user is running (the selector's handover
/// starts the next engine before stopping this one).
fn other_engine_running() -> bool {
    let me = std::process::id();
    let Ok(rd) = std::fs::read_dir("/proc") else {
        return false;
    };
    rd.flatten()
        .filter_map(|e| e.file_name().to_str()?.parse::<u32>().ok())
        .filter(|&pid| pid != me)
        .any(|pid| {
            std::fs::read_to_string(format!("/proc/{pid}/comm"))
                .is_ok_and(|c| c.trim_end() == "rustpaper")
        })
}

/// One round of Wayland events, waiting at most 250 ms for them (the loop
/// also runs the watchdog and notices signals).
fn dispatch(
    queue: &mut wayland_client::EventQueue<Mirrors>,
    mirrors: &mut Mirrors,
    fd: i32,
) -> Result<(), String> {
    queue.dispatch_pending(mirrors).map_err(|e| e.to_string())?;
    queue.flush().map_err(|e| e.to_string())?;
    if let Some(guard) = queue.prepare_read() {
        let mut pfd = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one valid pollfd.
        let n = unsafe { libc::poll(&mut pfd, 1, 250) };
        if n > 0
            && let Err(e) = guard.read()
            && !harmless(&e)
        {
            return Err(e.to_string());
        }
    }
    queue.dispatch_pending(mirrors).map_err(|e| e.to_string())?;
    Ok(())
}

/// EAGAIN / EINTR while reading the socket: try again next round.
fn harmless(e: &wayland_client::backend::WaylandError) -> bool {
    use std::io::ErrorKind::{Interrupted, WouldBlock};
    matches!(e, wayland_client::backend::WaylandError::Io(io) if matches!(io.kind(), WouldBlock | Interrupted))
}

/// Monitors to cover (all real ones, or just `only`), with their pixel size.
fn plan_targets(only: Option<&str>) -> Vec<Target> {
    hyprland::monitors_raw()
        .iter()
        .filter_map(|m| {
            let name = m.get("name")?.as_str()?.to_string();
            let w = m.get("width")?.as_u64()? as u32;
            let h = m.get("height")?.as_u64()? as u32;
            Some((name, w, h))
        })
        .filter(|(name, ..)| !is_headless(name) && only.is_none_or(|o| o == name))
        .map(|(output, w, h)| Target {
            name: format!("rustpaper-{output}"),
            headless: format!("RP-{output}"),
            output,
            size: (w, h),
            opened: Instant::now(),
            attempts: 0,
            configured: false,
        })
        .collect()
}

/// Outputs that are not real monitors: ours, the old harness/spike ones,
/// Hyprland's default headless names.
fn is_headless(name: &str) -> bool {
    ["RP-", "RPREF", "RPWRAP", "HEADLESS"]
        .iter()
        .any(|p| name.starts_with(p))
}

/// The headless output, its pinned workspace and the window rule for WE's
/// window of `t` (all reused when a previous engine left them).
fn prepare_target(t: &Target) {
    let (w, h) = t.size;
    // the rule first: the new output then opens straight on our workspace
    hyprland::eval(&format!(
        "hl.workspace_rule({{ workspace = 'name:{}', monitor = '{}', default = true, persistent = true }})",
        t.name, t.headless
    ));
    let exists = hyprland::monitors_raw()
        .iter()
        .any(|m| m.get("name").and_then(|n| n.as_str()) == Some(&t.headless));
    if !exists {
        hyprland::create_headless(&t.headless);
    }
    hyprland::eval(&format!(
        "hl.monitor({{ output = '{}', mode = '{w}x{h}@60', position = 'auto', scale = 1 }})",
        t.headless
    ));
    // When the idle daemon powers the monitors off (DPMS) a headless output
    // stops rendering too: keep ours on.
    hyprland::dispatch(&format!(
        "hl.dsp.dpms({{ action = 'on', monitor = '{}' }})",
        t.headless
    ));
    // WE's window: straight onto that workspace, fullscreen, never focused,
    // no animation (the fullscreen grow resized the capture every frame).
    // Fullscreen for the compositor only (`fullscreen_state` internal 2,
    // client 0): told it was fullscreen, the window (Wine) kept leaving it,
    // tiled at 1896x1026 every couple of seconds; not told, it stays.
    hyprland::eval(&format!(
        "hl.window_rule({{ name = '{n}', match = {{ class = '{WE_CLASS}', title = '^{n}$' }}, workspace = 'name:{n} silent', fullscreen_state = '2 0', no_anim = true, no_focus = true, no_initial_focus = true, focus_on_activate = false }})",
        n = t.name
    ));
}

/// Asks WE to play the wallpaper in `t`'s window (opened, or reused).
fn open_window(project: &Path, t: &mut Target) {
    let (w, h) = (t.size.0.to_string(), t.size.1.to_string());
    let file = we::wine_path(project);
    let title = t.name.clone();
    eprintln!("{}: asking Wallpaper Engine for window {title}", t.output);
    t.opened = Instant::now();
    t.attempts += 1;
    t.configured = false;
    we::control(&[
        "-control",
        "openWallpaper",
        "-file",
        &file,
        "-playInWindow",
        &title,
        "-width",
        &w,
        "-height",
        &h,
    ]);
}

/// WE's own windows (UI, dialogs) never take focus and live on a hidden
/// special workspace.
fn hide_we_ui() {
    hyprland::eval(&format!(
        "hl.window_rule({{ name = 'rustpaper-we', match = {{ class = '{WE_CLASS}' }}, no_focus = true, no_initial_focus = true, focus_on_activate = false, no_anim = true }})"
    ));
    for c in hyprland::clients() {
        let ours = c
            .get("title")
            .and_then(|t| t.as_str())
            .is_some_and(|t| t.starts_with("rustpaper-"));
        let we = c.get("class").and_then(|t| t.as_str()) == Some(WE_CLASS);
        let hidden = c.pointer("/workspace/name").and_then(|n| n.as_str()) == Some(WE_UI_WORKSPACE);
        if we
            && !ours
            && !hidden
            && let Some(a) = c.get("address").and_then(|a| a.as_str())
        {
            hyprland::dispatch(&format!(
                "hl.dsp.window.move({{ window = 'address:{a}', workspace = '{WE_UI_WORKSPACE}', follow = false }})"
            ));
        }
    }
}

/// Keeps every target's arrangement and decides pausing.
fn watchdog(cfg: &Config, targets: &mut [Target], mirrors: &mut Mirrors, we_paused: &mut bool) {
    let clients = hyprland::clients();
    let monitors = hyprland::monitors_raw();
    for t in targets.iter_mut() {
        let client = clients
            .iter()
            .find(|c| c.get("title").and_then(|x| x.as_str()) == Some(&t.name));
        let Some(c) = client else {
            if t.opened.elapsed() > OPEN_TIMEOUT {
                // WE dropped the window (crash, restart) or ignored the request
                if t.attempts >= MAX_ATTEMPTS || we::pid().is_none() {
                    eprintln!(
                        "{}: Wallpaper Engine is not answering, restarting it",
                        t.output
                    );
                    if we::restart(&cfg.performance) {
                        hide_we_ui();
                    }
                    t.attempts = 0;
                } else {
                    eprintln!(
                        "{}: no window from Wallpaper Engine, asking again",
                        t.output
                    );
                }
                open_window(&cfg.project, t);
            }
            continue;
        };
        t.attempts = 0;
        let addr = c
            .get("address")
            .and_then(|a| a.as_str())
            .unwrap_or_default();
        if !t.configured && t.opened.elapsed() > PROPS_DELAY {
            t.configured = true;
            if !we::apply_properties(&cfg.properties, &t.name) {
                eprintln!("{}: Wallpaper Engine refused the property values", t.output);
            }
            we::control(&["-control", if cfg.mute { "mute" } else { "unmute" }]);
        }
        let ws = c
            .pointer("/workspace/name")
            .and_then(|n| n.as_str())
            .unwrap_or_default();
        if ws != t.name {
            hyprland::dispatch(&format!(
                "hl.dsp.window.move({{ window = 'address:{addr}', workspace = 'name:{}', follow = false }})",
                t.name
            ));
        }
        // keep it fullscreen (compositor-side only, see prepare_target)
        let fs = (
            c.get("fullscreen").and_then(|f| f.as_i64()),
            c.get("fullscreenClient").and_then(|f| f.as_i64()),
        );
        if fs != (Some(2), Some(0)) {
            hyprland::dispatch(&format!(
                "hl.dsp.window.fullscreen_state({{ internal = 2, client = 0, action = 'set', window = 'address:{addr}' }})"
            ));
        }
        // a window on a hidden workspace is not rendered: the capture stalls
        let shown = monitors
            .iter()
            .find(|m| m.get("name").and_then(|n| n.as_str()) == Some(&t.headless))
            .and_then(|m| {
                m.pointer("/activeWorkspace/name")?
                    .as_str()
                    .map(String::from)
            });
        if shown.as_deref().is_some_and(|s| s != t.name) {
            let me = hyprland::active_window();
            hyprland::dispatch(&format!(
                "hl.dsp.focus({{ workspace = 'name:{}' }})",
                t.name
            ));
            if let Some(me) = me {
                hyprland::dispatch(&format!("hl.dsp.focus({{ window = 'address:{me}' }})"));
            }
        }
    }
    // the user's Playback settings, per monitor; WE itself pauses when
    // every monitor is paused
    let battery = power::on_battery();
    if let Some(mons) = hyprland::monitors() {
        for m in &mut mirrors.list {
            m.paused = mons
                .iter()
                .find(|x| x.name == m.output)
                .is_some_and(|x| should_pause(&cfg.playback, x, battery));
        }
    }
    let all = !mirrors.list.is_empty() && mirrors.list.iter().all(|m| m.paused);
    if all != *we_paused {
        *we_paused = all;
        we::control(&["-control", if all { "pause" } else { "play" }]);
    }
    for (i, t) in targets.iter().enumerate() {
        if !mirrors.capturing(i) && t.opened.elapsed() > OPEN_TIMEOUT {
            eprintln!("{}: window {} is not being captured", t.output, t.name);
        }
    }
}

/// Whether the Playback settings pause monitor `m`.
fn should_pause(p: &Playback, m: &hyprland::Monitor, on_battery: bool) -> bool {
    let pause = |a: Action, cond: bool| cond && a == Action::Pause;
    pause(p.fullscreen, m.fullscreen)
        || pause(p.maximized, m.maximized)
        || pause(p.focused, m.focused)
        || pause(p.sleep, m.asleep)
        || pause(p.battery, on_battery)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playback_settings_decide_pausing() {
        let p = Playback::default();
        let tiled = hyprland::Monitor {
            maximized: true,
            ..Default::default()
        };
        let full = hyprland::Monitor {
            fullscreen: true,
            ..Default::default()
        };
        assert!(
            !should_pause(&p, &tiled, false),
            "tiled windows keep running by default"
        );
        assert!(should_pause(&p, &full, false));
        let eco = Playback {
            maximized: Action::Pause,
            battery: Action::Pause,
            ..p
        };
        assert!(should_pause(&eco, &tiled, false));
        assert!(
            should_pause(&eco, &hyprland::Monitor::default(), true),
            "on battery"
        );
    }

    #[test]
    fn headless_outputs_are_not_monitors() {
        assert!(is_headless("RP-DP-1"));
        assert!(is_headless("RPREF"));
        assert!(is_headless("HEADLESS-2"));
        assert!(!is_headless("DP-1"));
        assert!(!is_headless("HDMI-A-1"));
    }
}
