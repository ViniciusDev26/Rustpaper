//! rustpaper: Wallpaper Engine wallpapers as the Wayland desktop background.
//!
//! The wallpaper is rendered by the REAL Wallpaper Engine (WE), running under
//! Proton through Steam, so it looks exactly like it does on Windows. This
//! file is the core, the same on every desktop:
//!
//! - **WE** (`we.rs`): started through Steam, driven with its own `-control`
//!   command line. For each monitor it plays the wallpaper in ONE long-lived
//!   window, `rustpaper-<output>` (`-playInWindow`). Asking again with the
//!   same name loads the new wallpaper INTO that window, so switching never
//!   opens or closes windows. (Opening and closing them on every switch
//!   degraded WE within minutes: closes hit the wrong window, closeWallpaper
//!   did nothing, new windows came up black or pixelated at ~3 fps.)
//! - **Desktop adapter** (`desktop/`): which monitors exist, what covers them
//!   (for the Playback rules), and a Stage where WE's windows live out of
//!   sight but rendered. Hyprland and Sway have one; a new desktop is a new
//!   adapter.
//! - **Mirror** (`mirror.rs`): captures each window (`ext-image-copy-capture`)
//!   and shows it on a layer-shell surface on its monitor.
//! - **Watchdog**: every 2 s the Stage repairs the arrangement; WE is asked
//!   again for missing windows and restarted when it stops answering; the
//!   Playback rules pause where the wallpaper is hidden.
//! - **Service mode** (`config.rs`): `--service` shows the selector's choice
//!   and reloads it in place on SIGHUP.

mod config;
mod desktop;
mod gbm;
mod launch;
mod mirror;
mod power;
mod stage;
mod we;

use std::os::fd::AsRawFd;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use config::{Config, Source};
use desktop::{Coverage, Desktop, Output, Stage};
use mirror::Mirrors;
use rustpaper_core::state::{Action, Playback, StageChoice};
use stage::{PrivateStage, StageEvent};

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

/// One monitor showing the wallpaper.
struct Target {
    output: Output,
    /// Title of WE's window for this monitor.
    title: String,
    /// When WE was last asked for the window.
    opened: Instant,
    /// Requests since the window was last seen.
    attempts: u32,
    /// Properties/mute were sent for the current request.
    configured: bool,
}

fn fail(msg: impl std::fmt::Display) -> ! {
    eprintln!("error: {msg}");
    std::process::exit(1);
}

fn main() {
    // the Steam launch option for WE (launch.rs): exec, never returns
    let argv: Vec<String> = std::env::args().collect();
    if argv.get(1).map(String::as_str) == Some("we-launch") {
        launch::run(&argv[2..]);
    }
    if argv.get(1).map(String::as_str) == Some("setup-steam") {
        match launch::setup_steam() {
            Ok(()) => std::process::exit(0),
            Err(e) => fail(e),
        }
    }
    let (source, only) = config::parse_args();
    let mut cfg = match &source {
        Source::Args(c) => c.clone(),
        Source::Service(stage) => {
            let mut c = config::service_config().unwrap_or_else(|e| {
                // nothing to show is not a failure (the service must not loop)
                eprintln!("rustpaper: {e}");
                std::process::exit(0);
            });
            if let Some(s) = stage {
                c.stage = *s;
            }
            c
        }
    };
    for sig in [libc::SIGTERM, libc::SIGINT] {
        // SAFETY: the handler only stores to an atomic.
        unsafe { libc::signal(sig, on_quit as *const () as libc::sighandler_t) };
    }
    // SAFETY: same.
    unsafe { libc::signal(libc::SIGHUP, on_reload as *const () as libc::sighandler_t) };

    // the user's compositor first: the generic adapter reads its outputs
    let (mut mirrors, mut queue, conn) = Mirrors::connect().unwrap_or_else(|e| fail(e));
    let qh = queue.handle();
    let desktop = desktop::detect(mirrors.wl_outputs()).unwrap_or_else(|e| fail(e));
    eprintln!("desktop: {}", desktop.name());
    // before any layer surface exists: no compositor fade when switching
    desktop.prepare_layers();
    let outputs: Vec<Output> = desktop
        .outputs()
        .into_iter()
        .filter(|o| only.as_deref().is_none_or(|n| n == o.name))
        .collect();
    if outputs.is_empty() {
        fail("no monitor to show the wallpaper on");
    }

    // RUSTPAPER_NO_WE=1 (debugging): only the stage and the mirror run, for
    // testing them with any X client (e.g. a window titled rustpaper-DP-1).
    let we_on = std::env::var_os("RUSTPAPER_NO_WE").is_none();
    // Where WE's windows live: Rustpaper's own compositor (private stage),
    // or the desktop adapter's stage inside the user's compositor.
    let private = match cfg.stage {
        StageChoice::Private => true,
        StageChoice::Desktop => false,
        StageChoice::Auto => launch::configured() || desktop.stage().is_none(),
    };
    let private_stage = if private {
        if we_on && !launch::configured() {
            fail(
                "the private stage needs Wallpaper Engine's Steam launch option: run `rustpaper setup-steam` once",
            );
        }
        Some(PrivateStage::start(&outputs, mirrors.host_dmabuf.clone()).unwrap_or_else(|e| fail(e)))
    } else {
        None
    };
    let stage: &dyn Stage = match &private_stage {
        Some(p) => p,
        None => desktop.stage().unwrap_or_else(|| {
            fail(format!(
                "{} cannot host Wallpaper Engine's windows: use the private stage (`rustpaper setup-steam`)",
                desktop.name()
            ))
        }),
    };

    if we_on && !we::ensure_running(&cfg.performance) {
        fail("Wallpaper Engine is not running (start it from Steam)");
    }
    if we_on
        && let Some(p) = &private_stage
        && we::display().as_deref() != Some(p.display())
    {
        // started before this stage (or without it): Steam starts it again
        // through the launch option, which points it at our display
        eprintln!("stage: moving Wallpaper Engine onto the private stage");
        we::restart(&cfg.performance);
    }
    if we_on {
        stage.hide_we_ui();
        // a previous instance may have paused WE (all monitors covered)
        we::control(&["-control", "play"]);
    }

    let mut targets: Vec<Target> = outputs
        .into_iter()
        .filter_map(|output| {
            stage.prepare(&output);
            let mut t = Target {
                title: desktop::window_title(&output.name),
                output,
                opened: Instant::now(),
                attempts: 0,
                configured: false,
            };
            if we_on {
                open_window(&cfg.project, &mut t);
            }
            mirrors
                .add(&qh, &t.output.name, &t.title, private_stage.is_none())
                .map_err(|e| eprintln!("{}: {e}", t.output.name))
                .ok()?;
            Some(t)
        })
        .collect();

    let mut we_paused = false;
    let mut last_tick = Instant::now() - TICK;
    let mut stats = Instant::now();
    let fd = conn.backend().poll_fd().as_raw_fd();
    while !QUIT.load(Ordering::SeqCst) {
        let stage_fd = private_stage.as_ref().map(|p| p.notify_fd());
        if let Err(e) = dispatch(&mut queue, &mut mirrors, fd, stage_fd) {
            eprintln!("error: Wayland connection lost: {e}");
            break;
        }
        if let Some(p) = &private_stage {
            // WE's frames to the monitors; released buffers back to WE
            for event in p.drain() {
                match event {
                    StageEvent::Frame(frame) => mirrors.show(&qh, frame),
                    StageEvent::BufferGone(key) => mirrors.forget(key),
                }
            }
            for key in mirrors.take_released() {
                p.release(key);
            }
        }
        mirrors.pump(&qh);
        if RELOAD.swap(false, Ordering::SeqCst) && matches!(source, Source::Service(_)) {
            match config::service_config() {
                Ok(new) => {
                    reload(stage, &cfg, &new, &mut targets);
                    cfg = new;
                }
                Err(e) => eprintln!("reload: {e}"),
            }
            last_tick = Instant::now() - TICK;
        }
        if we_on && last_tick.elapsed() >= TICK {
            last_tick = Instant::now();
            watchdog(
                desktop.as_ref(),
                stage,
                &cfg,
                &mut targets,
                &mut mirrors,
                &mut we_paused,
            );
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
    // The windows and hidden outputs stay for the next engine (closing WE's
    // windows is unreliable, see the top). The last engine out stops WE's
    // wallpapers so they cost nothing while no wallpaper is shown.
    if we_on && !other_engine_running() {
        we::control(&["-control", "stop"]);
    }
}

/// Applies a new Config without restarting the engine (so the desktop never
/// shows the plain background in between): new quality settings restart WE,
/// a new wallpaper or new property values load into the same windows.
fn reload(stage: &dyn Stage, old: &Config, new: &Config, targets: &mut [Target]) {
    if !new.performance.is_empty() && we::needs_restart(&new.performance) {
        eprintln!("reload: new quality settings, restarting Wallpaper Engine");
        if we::restart(&new.performance) {
            stage.hide_we_ui();
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
    stage_fd: Option<i32>,
) -> Result<(), String> {
    queue.dispatch_pending(mirrors).map_err(|e| e.to_string())?;
    queue.flush().map_err(|e| e.to_string())?;
    if let Some(guard) = queue.prepare_read() {
        // also wake up for the private stage's frames
        let mut pfds = [
            libc::pollfd {
                fd,
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: stage_fd.unwrap_or(-1),
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        // SAFETY: two valid pollfds (a negative fd is ignored by poll).
        let n = unsafe { libc::poll(pfds.as_mut_ptr(), 2, 250) };
        if n > 0
            && pfds[0].revents != 0
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

/// Asks WE to play the wallpaper in `t`'s window (opened, or reused).
fn open_window(project: &Path, t: &mut Target) {
    let (w, h) = (t.output.size.0.to_string(), t.output.size.1.to_string());
    let file = we::wine_path(project);
    eprintln!(
        "{}: asking Wallpaper Engine for window {}",
        t.output.name, t.title
    );
    t.opened = Instant::now();
    t.attempts += 1;
    t.configured = false;
    we::control(&[
        "-control",
        "openWallpaper",
        "-file",
        &file,
        "-playInWindow",
        &t.title,
        "-width",
        &w,
        "-height",
        &h,
    ]);
}

/// Keeps every target alive and decides pausing.
fn watchdog(
    desktop: &dyn Desktop,
    stage: &dyn Stage,
    cfg: &Config,
    targets: &mut [Target],
    mirrors: &mut Mirrors,
    we_paused: &mut bool,
) {
    for t in targets.iter_mut() {
        if !stage.tend(&t.output) {
            if t.opened.elapsed() > OPEN_TIMEOUT {
                // WE dropped the window (crash, restart) or ignored the request
                if t.attempts >= MAX_ATTEMPTS || we::pid().is_none() {
                    eprintln!(
                        "{}: Wallpaper Engine is not answering, restarting it",
                        t.output.name
                    );
                    if we::restart(&cfg.performance) {
                        stage.hide_we_ui();
                    }
                    t.attempts = 0;
                } else {
                    eprintln!(
                        "{}: no window from Wallpaper Engine, asking again",
                        t.output.name
                    );
                }
                open_window(&cfg.project, t);
            }
            continue;
        }
        t.attempts = 0;
        if !t.configured && t.opened.elapsed() > PROPS_DELAY {
            t.configured = true;
            if !we::apply_properties(&cfg.properties, &t.title) {
                eprintln!(
                    "{}: Wallpaper Engine refused the property values",
                    t.output.name
                );
            }
            we::control(&["-control", if cfg.mute { "mute" } else { "unmute" }]);
        }
    }
    // the user's Playback settings, per monitor; WE itself pauses when
    // every monitor is paused
    let battery = power::on_battery();
    if let Some(cov) = desktop.coverage() {
        for m in &mut mirrors.list {
            m.paused = cov
                .iter()
                .find(|c| c.name == m.output)
                .is_some_and(|c| should_pause(&cfg.playback, c, battery));
        }
    }
    let all = !mirrors.list.is_empty() && mirrors.list.iter().all(|m| m.paused);
    if all != *we_paused {
        *we_paused = all;
        we::control(&["-control", if all { "pause" } else { "play" }]);
    }
    for (i, t) in targets.iter().enumerate() {
        if !mirrors.capturing(i) && t.opened.elapsed() > OPEN_TIMEOUT {
            eprintln!(
                "{}: window {} is not being captured",
                t.output.name, t.title
            );
        }
    }
}

/// Whether the Playback settings pause monitor `c`.
fn should_pause(p: &Playback, c: &Coverage, on_battery: bool) -> bool {
    let pause = |a: Action, cond: bool| cond && a == Action::Pause;
    pause(p.fullscreen, c.fullscreen)
        || pause(p.maximized, c.maximized)
        || pause(p.focused, c.focused)
        || pause(p.sleep, c.asleep)
        || pause(p.battery, on_battery)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playback_settings_decide_pausing() {
        let p = Playback::default();
        let tiled = Coverage {
            maximized: true,
            ..Default::default()
        };
        let full = Coverage {
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
        assert!(should_pause(&eco, &Coverage::default(), true), "on battery");
    }
}
