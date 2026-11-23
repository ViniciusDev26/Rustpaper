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
//! - **Lock screen** (`lock/`): `rustpaper lock` and its daemon; the engine
//!   feeds it the private stage's frames (`lock/feed.rs`).
//! - **Video wallpapers** (`video.rs`): on the private stage, mpv with
//!   hardware decoding plays them instead of WE (Proton decodes video on the
//!   CPU). WE is started only when some monitor needs it, and paused when
//!   none does.

mod audio;
mod config;
mod desktop;
mod gbm;
mod launch;
mod lock;
mod mirror;
mod power;
mod proton;
mod stage;
mod video;
mod we;

use std::collections::HashMap;
use std::os::fd::AsRawFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use config::{Choice, Config, Mode, Source};
use desktop::{Coverage, Desktop, Output, Stage};
use mirror::Mirrors;
use rustpaper_core::state::{Action, Playback, StageChoice, VideoPlayer};
use stage::{Feed, PrivateStage, StageEvent};

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
    /// The wallpaper this monitor should show now (its mode's choice).
    want: Choice,
    /// The monitor whose WE window feeds this one (shared rendering), or
    /// None when it shows its own window.
    source: Option<String>,
    /// mpv playing this monitor's video wallpaper (video.rs) instead of WE.
    player: Option<video::Player>,
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
    if matches!(
        argv.get(1).map(String::as_str),
        Some("lock" | "lock-daemon" | "lock-setup")
    ) {
        lock::main(&argv);
    }
    if argv.get(1).map(String::as_str) == Some("outputs") {
        // the monitors as the engine sees them (the selector's per-monitor
        // choices use these names)
        let (mirrors, _, _) = Mirrors::connect().unwrap_or_else(|e| fail(e));
        let desktop = desktop::detect(mirrors.wl_outputs()).unwrap_or_else(|e| fail(e));
        let list: Vec<serde_json::Value> = desktop
            .outputs()
            .iter()
            .map(|o| serde_json::json!({ "name": o.name, "width": o.size.0, "height": o.size.1 }))
            .collect();
        println!("{}", serde_json::Value::Array(list));
        std::process::exit(0);
    }
    if argv.get(1).map(String::as_str) == Some("we-proton") {
        let rest: Vec<&str> = argv.iter().skip(2).map(String::as_str).collect();
        std::process::exit(proton::main(&rest));
    }
    if argv.get(1).map(String::as_str) == Some("setup-steam") {
        match launch::setup_steam() {
            Ok(()) => std::process::exit(0),
            Err(e) => fail(e),
        }
    }
    let (source, only) = config::parse_args();
    let mut cfg = match &source {
        Source::Args(c) => (**c).clone(),
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
    // mpv still plays video wallpapers.
    let we_allowed = std::env::var_os("RUSTPAPER_NO_WE").is_none();
    let we_on = we_allowed;
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

    // video wallpapers in mpv: needs the private stage (mpv is a client of
    // its Wayland socket) and mpv itself
    let mpv = video::mpv_installed();
    if cfg.video_player == VideoPlayer::Mpv {
        if !mpv {
            eprintln!("video: mpv is not installed; video wallpapers play in Wallpaper Engine");
        } else if private_stage.is_none() {
            eprintln!(
                "video: mpv needs the private stage; video wallpapers play in Wallpaper Engine"
            );
        }
    }
    let videos = |c: &Config| private_stage.is_some() && mpv && c.video_player == VideoPlayer::Mpv;
    let names: Vec<String> = outputs.iter().map(|o| o.name.clone()).collect();
    // WE only when some monitor shows something else than a video in mpv
    let mut we_on = we_on && needs_we(&cfg, &names, videos(&cfg), is_video);
    if we_allowed && !we_on {
        eprintln!("Wallpaper Engine is not needed: every wallpaper is a video (mpv)");
    }
    if we_on && !start_we(&cfg, private_stage.as_ref(), stage) {
        fail("Wallpaper Engine is not running (start it from Steam)");
    }

    let mut targets: Vec<Target> = outputs
        .into_iter()
        .filter_map(|output| {
            stage.prepare(&output);
            let t = Target {
                title: desktop::window_title(&output.name),
                want: cfg.choice(&output.name, Mode::Desktop).clone(),
                output,
                opened: Instant::now(),
                attempts: 0,
                configured: false,
                source: None,
                player: None,
            };
            mirrors
                .add(&qh, &t.output.name, &t.title, private_stage.is_none())
                .map_err(|e| eprintln!("{}: {e}", t.output.name))
                .ok()?;
            Some(t)
        })
        .collect();
    // one WE window per distinct wallpaper (the private stage can show a
    // window's buffers on several monitors; desktop stages capture each
    // window for its own monitor)
    let share = private_stage.is_some();
    let ctx = |c: &Config, we_on: bool| Ctx {
        share,
        we_on,
        private: private_stage.as_ref(),
        videos: videos(c),
    };
    assign(&cfg, Mode::Desktop, &mut targets, &ctx(&cfg, we_on), true);

    // the lock screen shows the private stage's frames too (lock/feed.rs)
    let mut feed = private_stage
        .as_ref()
        .and_then(|_| lock::feed::FeedServer::bind());
    // who still uses each of WE's buffers (lock::feed::holders); a buffer
    // goes back to WE when neither the mirror nor the lock does
    let mut holders: HashMap<u64, u64> = HashMap::new();

    // the screensaver: idle for its timeout (service mode hands Omarchy's
    // screensaver over to it, lock/adapter.rs)
    let service = matches!(source, Source::Service(_));
    let saver_timeout = |c: &Config| c.screensaver.enabled.then_some(c.screensaver.seconds);
    mirrors.set_idle_timeout(&qh, saver_timeout(&cfg));
    if service {
        lock::adapter::detect().hand_over_screensaver(cfg.screensaver.enabled);
    }
    // the desktop's "stay awake" switch holds the screensaver off
    let session = lock::adapter::detect();
    let mut mode = Mode::Desktop;

    // a previous engine killed while WE was paused left it stopped
    we::resume_stale();
    let mut we_audio = audio::WeAudio::default();
    let mut last_tick = Instant::now() - TICK;
    let mut stats = Instant::now();
    let fd = conn.backend().poll_fd().as_raw_fd();
    while !QUIT.load(Ordering::SeqCst) {
        let mut extra = vec![private_stage.as_ref().map_or(-1, |p| p.notify_fd())];
        extra.extend(feed.as_ref().map_or([-1, -1], |f| f.poll_fds()));
        if let Err(e) = dispatch(&mut queue, &mut mirrors, fd, &extra) {
            eprintln!("error: Wayland connection lost: {e}");
            break;
        }
        if let Some(p) = &private_stage {
            use lock::feed::holders;
            // WE's frames to the monitors (and the lock screen); released
            // buffers back to WE
            for event in p.drain() {
                match event {
                    StageEvent::Frame(frame) => {
                        let dmabuf = matches!(frame.pixels, stage::FramePixels::Dmabuf { .. });
                        // the monitors this window feeds: its own, and those
                        // sharing it
                        let dests: Vec<usize> = targets
                            .iter()
                            .filter(|t| match &t.source {
                                None => t.output.name == frame.output,
                                Some(src) => *src == frame.output,
                            })
                            .filter_map(|t| mirrors.index_of(&t.output.name))
                            .collect();
                        if dests.is_empty() {
                            // a window no monitor shows now (one a shared
                            // monitor used before): straight back to WE
                            if dmabuf {
                                p.release(frame.key);
                            }
                            continue;
                        }
                        for i in dests {
                            let name = mirrors.list[i].output.clone();
                            let Some(f) = frame.for_output(&name) else {
                                continue;
                            };
                            if dmabuf {
                                let mut who = holders::mirror(i);
                                if feed.as_mut().is_some_and(|fd| fd.send(&f)) {
                                    who |= holders::lock(i);
                                }
                                *holders.entry(frame.key).or_default() |= who;
                            }
                            mirrors.show(&qh, f);
                        }
                    }
                    StageEvent::BufferGone(key) => {
                        mirrors.forget(key);
                        if let Some(f) = &mut feed {
                            f.gone(key);
                        }
                    }
                }
            }
            // the user's pointer over a wallpaper, to the WE window it shows
            // (a monitor sharing another's window points into that one)
            for (output, input) in mirrors.take_pointer() {
                let owner = targets
                    .iter()
                    .find(|t| t.output.name == output)
                    .and_then(|t| t.source.clone())
                    .unwrap_or(output);
                p.pointer(&owner, input);
            }
            let mut freed: Vec<(u64, u64)> = mirrors
                .take_released()
                .into_iter()
                .map(|(i, k)| (k, holders::mirror(i)))
                .collect();
            if let Some(f) = &mut feed {
                let (keys, changed) = f.poll();
                freed.extend(
                    keys.into_iter()
                        .filter_map(|(out, k)| Some((k, holders::lock(mirrors.index_of(&out)?)))),
                );
                if changed {
                    mirrors.set_locked(&qh, f.active());
                }
            }
            for (key, who) in freed {
                match holders.get_mut(&key) {
                    Some(h) => {
                        *h &= !who;
                        if *h == 0 {
                            holders.remove(&key);
                            p.release(key);
                        }
                    }
                    None => p.release(key),
                }
            }
        }
        mirrors.pump(&qh);
        if RELOAD.swap(false, Ordering::SeqCst) && service {
            match config::service_config() {
                Ok(new) => {
                    if new.screensaver.enabled != cfg.screensaver.enabled {
                        lock::adapter::detect().hand_over_screensaver(new.screensaver.enabled);
                    }
                    mirrors.set_idle_timeout(&qh, saver_timeout(&new));
                    let mut force = false;
                    if we_allowed && !we_on && needs_we(&new, &names, videos(&new), is_video) {
                        eprintln!("reload: a wallpaper needs Wallpaper Engine, starting it");
                        we_on = start_we(&new, private_stage.as_ref(), stage);
                        force = we_on;
                    }
                    reload(
                        stage,
                        &cfg,
                        &new,
                        mode,
                        &mut targets,
                        &ctx(&new, we_on),
                        force,
                    );
                    cfg = new;
                }
                Err(e) => eprintln!("reload: {e}"),
            }
            last_tick = Instant::now() - TICK;
        }
        // what the monitors show: the lock's wallpaper while the lock shows
        // the frames, the screensaver's while idle, else the desktop's
        let locked = feed.as_ref().is_some_and(|f| f.active());
        let now_mode = if locked {
            Mode::Lock
        } else if cfg.screensaver.enabled && mirrors.idle && !session.idle_inhibited() {
            Mode::Screensaver
        } else {
            Mode::Desktop
        };
        if now_mode != mode {
            eprintln!("mode: {now_mode:?}");
            mode = now_mode;
            mirrors.set_screensaver(&qh, mode == Mode::Screensaver);
            assign(&cfg, mode, &mut targets, &ctx(&cfg, we_on), false);
            // re-decide pausing now (WE must play while locked or idle)
            last_tick = Instant::now() - TICK;
        }
        if last_tick.elapsed() >= TICK {
            last_tick = Instant::now();
            watchdog(
                desktop.as_ref(),
                stage,
                &cfg,
                &mut targets,
                &mut mirrors,
                private_stage.is_some(),
                &mut we_audio,
                mode,
                we_on,
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
    for t in &mut targets {
        // mpv goes first (a quick kill; the WE stop below may take seconds)
        t.player = None;
    }
    if we_on && !other_engine_running() {
        // within systemd's stop timeout (we.rs control_within)
        we::control_within(&["-control", "stop"], Duration::from_secs(8));
    }
    // never leave WE stopped behind (the next engine reuses it)
    we::set_paused(false);
}

/// Applies a new Config without restarting the engine (so the desktop never
/// shows the plain background in between): new quality settings restart WE,
/// a new wallpaper or new property values load into the same windows.
/// `force`: load every WE window (WE was just started).
fn reload(
    stage: &dyn Stage,
    old: &Config,
    new: &Config,
    mode: Mode,
    targets: &mut [Target],
    ctx: &Ctx,
    force: bool,
) {
    let new_driver = new.wine_wayland != old.wine_wayland;
    if ctx.we_on
        && (new_driver || (!new.performance.is_empty() && we::needs_restart(&new.performance)))
    {
        // a new Wine driver only applies to a WE started again (through
        // Steam: the launch option sets its environment)
        eprintln!(
            "reload: {}, restarting Wallpaper Engine",
            if new_driver {
                "new Wine driver"
            } else {
                "new quality settings"
            }
        );
        if we::restart(&new.performance) {
            stage.hide_we_ui();
        }
        for t in targets.iter_mut() {
            t.attempts = 0;
        }
        assign(new, mode, targets, ctx, true);
        return;
    }
    // mpv takes the new mute/volume here too (Player::update); WE's streams
    // on the next tick (audio.rs)
    assign(new, mode, targets, ctx, force);
}

/// What [`assign`] may use.
struct Ctx<'a> {
    /// Monitors may show another monitor's window (the private stage).
    share: bool,
    /// WE runs (else nothing is asked of it).
    we_on: bool,
    private: Option<&'a PrivateStage>,
    /// Video wallpapers play in mpv (needs `private`).
    videos: bool,
}

/// Starts WE (through Steam, when it is not running), on the private stage
/// when there is one; false when it could not be started.
fn start_we(cfg: &Config, private: Option<&PrivateStage>, stage: &dyn Stage) -> bool {
    if !we::ensure_running(&cfg.performance) {
        eprintln!("error: Wallpaper Engine is not running (start it from Steam)");
        return false;
    }
    // on Wine's Wayland driver WE talks to the stage's Wayland socket, on
    // its X11 driver to the stage's XWayland
    let on_stage = |p: &PrivateStage| {
        if cfg.wine_wayland {
            we::env_var("WAYLAND_DISPLAY").as_deref() == Some(p.wayland_socket())
        } else {
            we::display().as_deref() == Some(p.display())
        }
    };
    if let Some(p) = private
        && !on_stage(p)
    {
        // started before this stage (or without it): Steam starts it again
        // through the launch option, which points it at our display
        eprintln!("stage: moving Wallpaper Engine onto the private stage");
        we::restart(&cfg.performance);
    }
    stage.hide_we_ui();
    true
}

/// A video wallpaper mpv can play.
fn is_video(c: &Choice) -> bool {
    video::file_of(&c.project).is_some()
}

/// Whether any monitor (`outputs`) shows, in any mode, a wallpaper WE must
/// render: everything but videos, when `videos` play in mpv.
fn needs_we(
    cfg: &Config,
    outputs: &[String],
    videos: bool,
    is_video: impl Fn(&Choice) -> bool,
) -> bool {
    if !videos {
        return true;
    }
    let mut modes = vec![Mode::Desktop, Mode::Lock];
    if cfg.screensaver.enabled {
        modes.push(Mode::Screensaver);
    }
    outputs
        .iter()
        .any(|o| modes.iter().any(|&m| !is_video(cfg.choice(o, m))))
}

/// Decides what each monitor shows in `mode` and which WE windows render
/// it: monitors showing the same wallpaper at the same aspect ratio share
/// the window of the largest one (`share_plan`), so WE renders it once.
/// Windows whose wallpaper changed (or that start owning one) are loaded;
/// the rest are left alone: no reload, no flash. `force` loads every owner
/// (start, WE restarted). Without WE (`RUSTPAPER_NO_WE`) nothing is
/// loaded, but the sharing still applies (to test it with a stand-in).
///
/// Owners of a video wallpaper (with `ctx.videos`) run mpv instead (one per
/// group of monitors sharing it), and the stage shows mpv's window for
/// them; an owner whose wallpaper is no video any more stops its mpv.
///
/// A window that stops being used (its monitor now shares another's, or
/// plays a video in mpv) is not closed: closing WE's windows degrades it
/// (see the top). It keeps rendering what it had until WE restarts (or WE
/// pauses: nothing needs it); its frames are dropped.
fn assign(cfg: &Config, mode: Mode, targets: &mut [Target], ctx: &Ctx, force: bool) {
    let share = ctx.share;
    let wants: Vec<Choice> = targets
        .iter()
        .map(|t| cfg.choice(&t.output.name, mode).clone())
        .collect();
    let plan = if share {
        share_plan(
            &targets
                .iter()
                .zip(&wants)
                .map(|(t, w)| (t.output.size, w))
                .collect::<Vec<_>>(),
        )
    } else {
        vec![None; targets.len()]
    };
    let names: Vec<String> = targets.iter().map(|t| t.output.name.clone()).collect();
    for ((t, want), leader) in targets.iter_mut().zip(wants).zip(plan) {
        let source = leader.map(|l| names[l].clone());
        if let Some(src) = &source {
            if t.source.as_ref() != Some(src) {
                eprintln!(
                    "{}: shows {src}'s window ({})",
                    t.output.name,
                    want.project.display()
                );
            }
            t.source = source;
            t.want = want;
            if t.player.take().is_some() {
                eprintln!("{}: mpv stopped", t.output.name);
            }
            continue;
        }
        let was_shared = t.source.take().is_some();
        let changed = force || was_shared || want != t.want;
        if let Some(p) = ctx.private.filter(|_| ctx.videos)
            && let Some(file) = video::file_of(&want.project)
        {
            let playback = video::Playback {
                file,
                mute: cfg.mute,
                volume: cfg.volume,
            };
            let playing = match &mut t.player {
                Some(player) => {
                    player.update(playback);
                    true
                }
                None => match video::Player::spawn(&t.output.name, p.wayland_socket(), playback) {
                    Ok(player) => {
                        t.player = Some(player);
                        true
                    }
                    Err(e) => {
                        eprintln!("{}: {e}; Wallpaper Engine plays it", t.output.name);
                        false
                    }
                },
            };
            if playing {
                p.set_feed(&t.output.name, Feed::Mpv);
                if changed {
                    eprintln!("{}: {} (mpv)", t.output.name, want.project.display());
                }
                t.want = want;
                continue;
            }
        }
        let was_video = t.player.take().is_some();
        if let Some(p) = ctx.private {
            p.set_feed(&t.output.name, Feed::We);
        }
        if changed || was_video {
            eprintln!("{}: {}", t.output.name, want.project.display());
            t.want = want;
            if ctx.we_on {
                open_window(t);
            }
        }
    }
}

/// For monitors (pixel size, wallpaper), the index of the monitor whose
/// window each one shows, None for those that render their own: equal
/// wallpapers with aspect ratios within 1% share the largest monitor's
/// window (scaled down by the user's compositor, no cropping or stretching).
fn share_plan(monitors: &[((u32, u32), &Choice)]) -> Vec<Option<usize>> {
    let aspect = |(w, h): (u32, u32)| w as f64 / h.max(1) as f64;
    let mut plan = vec![None; monitors.len()];
    for i in 0..monitors.len() {
        let (size, want) = monitors[i];
        // the largest monitor of i's group (lowest index on ties)
        let leader = (0..monitors.len())
            .filter(|&j| {
                let (s, w) = monitors[j];
                w == want && (aspect(s) / aspect(size) - 1.0).abs() < 0.01
            })
            .max_by_key(|&j| {
                (
                    monitors[j].0.0 as u64 * monitors[j].0.1 as u64,
                    std::cmp::Reverse(j),
                )
            })
            .unwrap_or(i);
        if leader != i {
            plan[i] = Some(leader);
        }
    }
    plan
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
/// also runs the watchdog and notices signals). `extra` fds (the private
/// stage's frames, the lock feed) also end the wait.
fn dispatch(
    queue: &mut wayland_client::EventQueue<Mirrors>,
    mirrors: &mut Mirrors,
    fd: i32,
    extra: &[i32],
) -> Result<(), String> {
    queue.dispatch_pending(mirrors).map_err(|e| e.to_string())?;
    queue.flush().map_err(|e| e.to_string())?;
    if let Some(guard) = queue.prepare_read() {
        let mut pfds: Vec<libc::pollfd> = std::iter::once(fd)
            .chain(extra.iter().copied())
            .map(|fd| libc::pollfd {
                fd,
                events: libc::POLLIN,
                revents: 0,
            })
            .collect();
        // SAFETY: valid pollfds (a negative fd is ignored by poll).
        let n = unsafe { libc::poll(pfds.as_mut_ptr(), pfds.len() as libc::nfds_t, 250) };
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
fn open_window(t: &mut Target) {
    let (w, h) = (t.output.size.0.to_string(), t.output.size.1.to_string());
    let file = we::wine_path(&t.want.project);
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

/// Keeps every target alive and decides pausing. Without WE (`we_on`
/// false) only mpv and the stage's arrangement are tended.
#[allow(clippy::too_many_arguments)]
fn watchdog(
    desktop: &dyn Desktop,
    stage: &dyn Stage,
    cfg: &Config,
    targets: &mut [Target],
    mirrors: &mut Mirrors,
    freeze: bool,
    we_audio: &mut audio::WeAudio,
    mode: Mode,
    we_on: bool,
) {
    for t in targets.iter_mut() {
        if t.source.is_some() {
            // shows another monitor's window: nothing to keep alive here
            continue;
        }
        if let Some(p) = &mut t.player {
            // restarted when it died
            p.tend();
            continue;
        }
        if !we_on {
            stage.tend(&t.output);
            continue;
        }
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
                open_window(t);
            }
            continue;
        }
        t.attempts = 0;
        if !t.configured && t.opened.elapsed() > PROPS_DELAY {
            t.configured = true;
            if !we::apply_properties(&t.want.properties, &t.title) {
                eprintln!(
                    "{}: Wallpaper Engine refused the property values",
                    t.output.name
                );
            }
        }
    }
    // the user's Playback settings, per monitor; WE itself pauses when
    // every monitor is paused
    let battery = power::on_battery();
    if let Some(cov) = desktop.coverage() {
        for m in &mut mirrors.list {
            m.paused = cov.iter().find(|c| c.name == m.output).is_some_and(|c| {
                if mode == Mode::Screensaver {
                    // the screensaver is over every window: only a display
                    // that is off stops it
                    c.asleep && cfg.playback.sleep == Action::Pause
                } else {
                    should_pause(&cfg.playback, c, battery)
                }
            });
        }
    }
    let locked = mode == Mode::Lock;
    // each renderer (WE's window or mpv) pauses when every monitor it feeds
    // is paused; while the lock screen shows the wallpaper it keeps playing
    // (only the lock covers the monitors; "display asleep" still pauses each
    // mirror)
    let paused: Vec<bool> = mirrors.list.iter().map(|m| m.paused).collect();
    let names: Vec<String> = mirrors.list.iter().map(|m| m.output.clone()).collect();
    let feeds_paused = |owner: &str| {
        targets
            .iter()
            .filter(|t| t.source.as_deref().unwrap_or(&t.output.name) == owner)
            .all(|t| {
                names
                    .iter()
                    .position(|n| *n == t.output.name)
                    .is_none_or(|i| paused[i])
            })
    };
    let owners: Vec<(String, bool)> = targets
        .iter()
        .filter(|t| t.source.is_none())
        .map(|t| (t.output.name.clone(), feeds_paused(&t.output.name)))
        .collect();
    let mut we_needed = false;
    let mut we_playing = false;
    for (t, (_, group_paused)) in targets
        .iter_mut()
        .filter(|t| t.source.is_none())
        .zip(&owners)
    {
        match &mut t.player {
            Some(p) => p.set_paused(!locked && *group_paused),
            None => {
                we_needed = true;
                we_playing |= locked || !group_paused;
            }
        }
    }
    // no monitor shows a WE window (every one plays a video in mpv, or the
    // Playback rules paused them all): WE pauses, so it costs nothing
    let all = !we_needed || !we_playing;
    if we_on && freeze {
        we::set_paused(all);
    }
    // muted then too: WE's own `-control mute` does nothing (audio.rs), and
    // with a desktop stage WE is never stopped
    if we_on {
        we_audio.apply(cfg.volume, cfg.mute || all);
    }
    if !we_on {
        return;
    }
    for (i, t) in targets.iter().enumerate() {
        if t.source.is_none()
            && t.player.is_none()
            && !mirrors.capturing(i)
            && t.opened.elapsed() > OPEN_TIMEOUT
        {
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
    fn equal_wallpapers_at_one_aspect_share_the_largest_window() {
        let c = |p: &str| Choice {
            project: std::path::PathBuf::from(p),
            properties: Default::default(),
        };
        let (a, b) = (c("a"), c("b"));
        // 4K and 1080p (16:9) with the same wallpaper: the 1080p shows the 4K
        assert_eq!(
            share_plan(&[((3840, 2160), &a), ((1920, 1080), &a)]),
            vec![None, Some(0)]
        );
        // the smaller one first: still the 4K renders
        assert_eq!(
            share_plan(&[((1920, 1080), &a), ((3840, 2160), &a)]),
            vec![Some(1), None]
        );
        // different wallpapers: each its own
        assert_eq!(
            share_plan(&[((3840, 2160), &a), ((1920, 1080), &b)]),
            vec![None, None]
        );
        // 16:9 and 21:9: no cropping or stretching, each its own
        assert_eq!(
            share_plan(&[((3840, 2160), &a), ((3440, 1440), &a)]),
            vec![None, None]
        );
        // equal sizes: the first renders, the others share it
        assert_eq!(
            share_plan(&[((1920, 1080), &a), ((1920, 1080), &a), ((1920, 1080), &a)]),
            vec![None, Some(0), Some(0)]
        );
    }

    #[test]
    fn wallpaper_engine_only_when_something_is_no_video() {
        let c = |p: &str| Choice {
            project: std::path::PathBuf::from(p),
            properties: Default::default(),
        };
        let mut cfg = Config {
            desktop: c("video"),
            monitors: Default::default(),
            lock: None,
            screensaver: Default::default(),
            mute: false,
            volume: 100,
            playback: Playback::default(),
            performance: Default::default(),
            stage: StageChoice::Private,
            video_player: VideoPlayer::Mpv,
            wine_wayland: false,
        };
        let is_video = |c: &Choice| c.project.starts_with("video");
        let outs = ["DP-1".to_string(), "HDMI-A-1".to_string()];
        assert!(!needs_we(&cfg, &outs, true, is_video));
        assert!(needs_we(&cfg, &outs, false, is_video), "videos in WE");
        cfg.monitors.insert("DP-1".into(), c("scene"));
        assert!(needs_we(&cfg, &outs, true, is_video), "one monitor's scene");
        cfg.monitors.clear();
        cfg.lock = Some(c("scene"));
        assert!(needs_we(&cfg, &outs, true, is_video), "the lock's scene");
        cfg.lock = None;
        cfg.screensaver.all = Some(c("web"));
        assert!(
            !needs_we(&cfg, &outs, true, is_video),
            "a screensaver that is off"
        );
        cfg.screensaver.enabled = true;
        assert!(needs_we(&cfg, &outs, true, is_video));
    }

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
