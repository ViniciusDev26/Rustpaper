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
mod we;

use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use mirror::Mirrors;

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

extern "C" fn on_signal(_: libc::c_int) {
    QUIT.store(true, Ordering::SeqCst);
}

struct Options {
    dir: PathBuf,
    properties: serde_json::Map<String, serde_json::Value>,
    mute: bool,
    output: Option<String>,
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
        "usage: rustpaper <wallpaper folder> [--props properties.json] [--mute] [--volume 0..100] [--output NAME]"
    );
    std::process::exit(2);
}

fn parse_args() -> Options {
    let mut args = std::env::args().skip(1);
    let mut dir = None;
    let mut props_file: Option<PathBuf> = None;
    let mut mute = false;
    let mut output = None;
    while let Some(a) = args.next() {
        match a.as_str() {
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
    let Some(dir) = dir else { usage() };
    // User property values chosen in the selector: { "name": value, ... }
    let properties = props_file
        .and_then(|f| std::fs::read_to_string(f).ok())
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();
    Options {
        dir,
        properties,
        mute,
        output,
    }
}

fn main() {
    let opts = parse_args();
    let project = opts.dir.join("project.json");
    if !project.is_file() {
        eprintln!("error: {} not found", project.display());
        std::process::exit(1);
    }
    let project = std::fs::canonicalize(&project).unwrap_or(project);
    for sig in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
        // SAFETY: the handler only stores to an atomic.
        unsafe { libc::signal(sig, on_signal as *const () as libc::sighandler_t) };
    }
    if hyprland::json("j/version").is_none() {
        eprintln!("error: rustpaper needs Hyprland (0.56 or newer)");
        std::process::exit(1);
    }
    // before any layer surface exists: no compositor fade when switching
    hyprland::disable_layer_animations();

    let targets = plan_targets(opts.output.as_deref());
    if targets.is_empty() {
        eprintln!("error: no monitor to show the wallpaper on");
        std::process::exit(1);
    }
    if !we::ensure_running() {
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
            open_window(&project, &mut t);
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
        if last_tick.elapsed() >= TICK {
            last_tick = Instant::now();
            watchdog(&project, &opts, &mut targets, &mut mirrors, &mut we_paused);
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
fn watchdog(
    project: &Path,
    opts: &Options,
    targets: &mut [Target],
    mirrors: &mut Mirrors,
    we_paused: &mut bool,
) {
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
                    if we::restart() {
                        hide_we_ui();
                    }
                    t.attempts = 0;
                } else {
                    eprintln!(
                        "{}: no window from Wallpaper Engine, asking again",
                        t.output
                    );
                }
                open_window(project, t);
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
            if !we::apply_properties(&opts.properties, &t.name) {
                eprintln!("{}: Wallpaper Engine refused the property values", t.output);
            }
            we::control(&["-control", if opts.mute { "mute" } else { "unmute" }]);
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
    // pause capture where a fullscreen window covers the monitor; pause WE
    // itself when that is every monitor
    if let Some(mons) = hyprland::monitors() {
        for m in &mut mirrors.list {
            m.paused = mons.iter().any(|x| x.name == m.output && x.covered);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headless_outputs_are_not_monitors() {
        assert!(is_headless("RP-DP-1"));
        assert!(is_headless("RPREF"));
        assert!(is_headless("HEADLESS-2"));
        assert!(!is_headless("DP-1"));
        assert!(!is_headless("HDMI-A-1"));
    }
}
