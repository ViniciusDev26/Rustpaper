# The engine: Wallpaper Engine under Proton, mirrored onto the desktop

`rustpaper <wallpaper dir>` (crate `rustpaper-engine`) does not render
wallpapers. The real Wallpaper Engine (WE) does, under Proton, and Rustpaper
puts its output on the Wayland desktop. Until 2026-11 Rustpaper had its own
OpenGL reimplementation of WE's renderer. It reached 70/73 scenes without
SceneScript and 93/123 overall against a fidelity harness, but every
wallpaper still differed somewhere and the long tail never ended, so it was
replaced by this (the old code is in git history before the migration
commit).

## Architecture: one core, a stage, desktop adapters

```
                         core (identical on every desktop)
 ┌────────────────────────────────────────────────────────────────────────┐
 │ we.rs      WE under Proton: start via Steam, -control, config.json,    │
 │            applyProperties, stop its whole launch                      │
 │ launch.rs  `we-launch %command%` (WE's Steam launch option), setup-steam│
 │ config.rs  CLI / state.json (--service, SIGHUP reload, --stage)        │
 │ main.rs    targets, reload, watchdog, Playback rules                   │
 │ video.rs   video wallpapers in mpv (hardware decoding), private stage  │
 │ mirror.rs  layer-shell surfaces on the user's compositor               │
 │ lock/      the animated lock screen; feed.rs hands it the frames       │
 └──────────────┬─────────────────────────────────────┬───────────────────┘
                │ trait Stage                         │ trait Desktop
   ┌────────────┴────────────────────┐   ┌────────────┴─────────────────────┐
   │ stage.rs: private stage         │   │ hyprland.rs  sway.rs  generic.rs │
   │ (our Smithay compositor +       │   │ monitors, what covers them;      │
   │  XWayland; WE's dmabufs go      │   │ Hyprland and Sway also offer a   │
   │  straight to the surfaces)      │   │ desktop stage (headless outputs) │
   └─────────────────────────────────┘   └──────────────────────────────────┘
```

The core never talks to a compositor. Everything desktop-specific sits
behind two traits (`desktop/mod.rs`):
- **`Desktop`**: the real monitors and their pixel sizes, and what covers
  each one (fullscreen, maximized or tiled, focused, asleep), for the
  Playback rules.
- **`Stage`**: a place where WE's windows live out of the user's sight but
  rendered. It must:
  - prepare it once per monitor;
  - keep WE's own UI hidden;
  - repair it every 2 s (`tend`).

  Normally this is the private stage (`stage.rs`). The Hyprland and Sway
  adapters also offer a **desktop stage** inside the user's compositor, the
  original design. Compositors only render (and so only let us capture)
  windows they consider visible, so a desktop stage needs headless outputs
  and rules.

Supporting a desktop = a `Desktop` adapter. `desktop::detect()` picks
Hyprland (`HYPRLAND_INSTANCE_SIGNATURE`), Sway (`SWAYSOCK`), or else the
generic adapter. The generic adapter reads the monitors from the
compositor's `wl_output`s and works with the private stage on any
compositor with layer-shell; it cannot tell what covers a monitor, so the
Playback rules never pause there.

| Desktop | Adapter | Stage | Status |
|---|---|---|---|
| any with layer-shell | the private stage (below) hosts WE; the adapter only reports monitors and coverage | private | verified on Hyprland (with WE, 4K + 1080p at 60 fps) and on a nested Sway 1.12 (test X client) |
| Hyprland 0.56+ | `desktop/hyprland.rs` | private, or headless outputs `RP-<output>` + window rules | both stages verified with WE |
| Sway 1.11+ | `desktop/sway.rs` | private, or `create_output` + `workspace ... output` + `for_window` | both stages verified on a nested Sway 1.12 with a test X client (class `steam_app_431960`); not yet with WE on a real Sway session |
| niri, KDE, others | `desktop/generic.rs` (monitors from `wl_output`, no coverage) | private only | verified on a nested niri 26.04 with the test window (frames on niri's background, 2026-11-22); KDE not tried |
| GNOME | none | none | no layer-shell: needs a Shell extension to show the wallpaper |

What the user's compositor must offer: `zwlr-layer-shell` (the wallpaper
surfaces) and `zwp_linux_dmabuf_v1` (to show WE's buffers without a copy;
shm works too, at a CPU cost). Desktop stages also need
`ext-image-copy-capture` + `ext-foreign-toplevel-list` to capture WE's
windows. The mirror uses only these standard protocols.

Testing a stage without WE: `RUSTPAPER_NO_WE=1` runs only the stage and the
mirror, and `scripts/stage-test-window.py` (an X window titled
`rustpaper-<output>` with WE's window class) stands in for WE. Other
compositors were tested nested on a hidden headless output of Hyprland
(Sway with `WLR_BACKENDS=wayland`, niri); the nested compositor's window
must be on a *visible* workspace, or it gets no frames (Sway's measured
rate dropped to 0.1 fps until it was moved).

### The private stage (`stage.rs`)

The desktop stages borrow the user's compositor: headless outputs, rules,
and constant repairs. The private stage is Rustpaper's own instead:
- **What it is.** A headless Smithay compositor on its own thread, with its
  own XWayland. WE opens its windows there.
- **Monitors.** Each monitor is a `wl_output` of ours, so WE sees the real
  monitors.
- **Windows.** The X11 window manager gives `rustpaper-<output>` its
  monitor's exact geometry. WE's other windows (UI, dialogs) are mapped
  and shown nowhere. Nothing needs hiding, rules or repairs: Hyprland sees
  no WE window at all.
- **Buffers.** Each commit of a wallpaper window hands its dmabuf to the
  main thread, which attaches it as-is to the monitor's layer-shell surface
  on the user's compositor: no capture, no copy. The client buffer goes
  back to WE once the user's compositor released it, and shm buffers are
  copied.
- **Formats.** Our XWayland is offered exactly the dmabuf layouts the
  user's compositor imports, read from its `zwp_linux_dmabuf_v1` default
  feedback (Hyprland's main device gave XWayland glamor).
- **Measured.** WE on the private stage drew 60.0 fps on both monitors (4K
  and 1080p), with no WE window in Hyprland (2026-11-22).
- **Wayland clients.** The stage also listens on a Wayland socket of its
  own, `rustpaper-stage-<pid>` (its name in
  `$XDG_RUNTIME_DIR/rustpaper/stage-wayland`), with xdg-shell,
  subsurfaces, viewporter and a seat without devices. An xdg_toplevel
  titled like a wallpaper window is told to go fullscreen at its monitor's
  size, and only a dmabuf covering at least 90% of the monitor is
  forwarded. mpv plays video wallpapers through it (below); Wine's Wayland
  driver can too (not wired to WE's launch).

Choosing it: `state.json` `stage` (`auto`, `desktop` or `private`), or
`--stage`. `auto` picks the private stage once WE's launch option is set
up, or when the desktop adapter has no stage of its own.

**Pointing WE at our display.** WE inherits Steam's environment. Its Steam
launch option `"<rustpaper>" we-launch %command%` (`launch.rs`) runs us
first:
- with a private stage up (its display in
  `$XDG_RUNTIME_DIR/rustpaper/stage-display`), we set `DISPLAY` to it,
  drop `WAYLAND_DISPLAY` and `exec` Steam's command;
- otherwise we change nothing. Verified: WE launched through the wrapper
  with no stage behaves as before.

`rustpaper setup-steam` writes that option into every account's
`localconfig.vdf` (`rustpaper_core::steam`, a KeyValues reader/writer whose
output reproduced the user's real file byte for byte). Steam keeps that
file in memory and rewrites it on exit, so the command closes Steam
(`steam -shutdown`), writes the option with a one-time backup, and starts
Steam again. When WE runs on another display at engine start, the engine
restarts it so Steam relaunches it onto the stage.

Not yet: explicit sync (`wp_linux_drm_syncobj`) is not offered. XWayland
then uses implicit sync, which showed no corruption on the NVIDIA driver in
the tests.

**Pointer.** WE's wallpapers follow the cursor (parallax, eye tracking)
and take clicks. With the private stage the mirror's layer surfaces take
the pointer (their whole input region) and send its position, as a
fraction of the monitor, and its buttons and scrolls to the stage
(`PrivateStage::pointer`). The stage's `wl_seat` pointer delivers them to
the surface of WE's window on that monitor: XWayland turns them into the X
pointer WE reads, and Wine's Wayland driver reads them directly. A monitor
sharing another's window points into that window; one showing mpv's video
sends nothing to WE. The wallpaper gets the pointer only where no window
covers the desktop, as on Windows. Over it the cursor is the desktop's
default (`cursor-shape-v1`; a compositor without it keeps the old empty
input region, since the cursor would be undefined there). Verified on
Hyprland with the Wayland driver: 十二时辰-SYKM 3496849678's sundial
followed the mouse, and the clicks reached `rustpaper-HDMI-A-1`
(2026-11-23). `RUSTPAPER_DEBUG_POINTER=1` logs the stage's leaves and
buttons. Desktop stages keep the old behaviour (no input).

**Stopping WE cleanly** (`we::restart`). Killing only `wallpaper64.exe`
left Wine's helpers (`wineserver`, `winedevice.exe`, ...) and the embedded
Chromium's `CrGpuMain` alive in WE's Steam container. Steam then refused
every new launch: `WaitingPrevProcess` -> `AppError_16` in
`~/.local/share/Steam/logs/console_log.txt`, until those processes were
gone. The engine now stops every process carrying `SteamAppId=431960` or
WE's Wine prefix in its environment (SIGTERM, then SIGKILL after 10 s).

## Engine lifecycle (any stage)

For each real monitor there is ONE long-lived WE window, `rustpaper-<output>`,
asked for with `-control openWallpaper -playInWindow` (`we.rs`).

**Switching wallpapers never opens or closes windows.** `openWallpaper` with
an existing window name loads the new wallpaper into that window. A first
design opened a window per engine and closed it on exit, and WE degraded
within minutes:
- a close (`closeWallpaper -location` or the compositor's) took down another
  window showing the same wallpaper;
- later `closeWallpaper` did nothing at all;
- new windows came up black, or pixelated with a wrong clock, at ~3 fps.

**What each monitor shows** (`config.rs` `Config::choice`, `main.rs`
`retarget`). Each monitor has a wallpaper per mode, chosen in the selector
(docs/SELECTOR.md, "Where each wallpaper shows"):
- **Desktop**: its own, else the one for every monitor;
- **Screensaver**, while idle: its screensaver wallpaper, else the
  screensaver for all, else its desktop's. The mirror's layer surfaces are
  raised from Bottom to Overlay with all input (`Mirrors::set_screensaver`,
  `zwlr_layer_surface.set_layer`), so the wallpaper covers every window;
  the first key or click ends it and does not reach the window under it.
  Idle comes from `ext-idle-notify-v1` (idle inhibitors, e.g. a playing
  video, hold it off). Only "display asleep" pauses it;
- **Lock**, while the lock screen shows the frames (`lock/feed.rs`): the
  lock screen's wallpaper, else its desktop's.

The engine checks the mode every loop and loads into each window what it
should show, only where that changed: same-wallpaper monitors are never
reloaded. A switch takes WE 1-3 s to load, during which the window keeps
showing the previous wallpaper. Verified with WE (2026-11-22): DP-1 and
HDMI-A-1 showing different wallpapers; a lock client connecting loaded the
lock wallpaper into both windows, and leaving gave each its desktop one
back. The screensaver was verified on a nested Sway (raised over a
terminal after the timeout, lowered by a key, the key not delivered, focus
back on the terminal).

**One render per wallpaper** (private stage; `main.rs` `assign` and
`share_plan`). Monitors that show the same wallpaper (same project and
property values) at aspect ratios within 1% share one WE window: the
largest monitor's. Its frames go to every monitor of the group, each
mirror scaling them to its monitor (`wp_viewport`, on the user's
compositor's GPU), so WE renders the wallpaper once instead of once per
monitor (with two monitors, half the WE cost). Different aspect ratios keep
a window each: sharing would crop or stretch. Each WE buffer goes back to WE
once every surface showing it (one bit per monitor's mirror and per
monitor on the lock screen) released it. A window a monitor stops using is
not closed (closing degrades WE, above): it keeps rendering what it had
until WE restarts, and its frames are dropped. Verified on a nested Sway
with two outputs (2026-11-23): one test window shown on both, the lock
screen showing it on both, buffers flowing after an unlock.

**Wine's X11 or Wayland driver** (`proton.rs`, `launch.rs`). WE renders
into a child window (`WS_CHILD`) of its top-level window, and Wine's X11
driver always renders child windows offscreen (`needs_offscreen_rendering`
in winex11: `client_surface_update_offscreen ... offscreen 1` with
`WINEDEBUG=+x11drv`, on Proton Experimental and GE-Proton alike) and
composites them into the top-level. So every frame is copied three times
inside XWayland: DXVK's Present into the offscreen window (always `Copy`,
never `Flip`), Wine's copy into the visible window, XWayland's copy into the
buffer it hands the stage. No registry key or variable changes that.
Wine's Wayland driver (GE-Proton only; Steam's builds ship winex11 alone)
makes the window an xdg_toplevel of the stage and the Vulkan swapchain a
subsurface, whose dmabufs the stage forwards as they are. Measured on the
real session (4K + 1080p sharing one window, a copy of WE with the same
wallpaper, 2026-11-23): CPU 15.8% -> 5.4% of a core (XWayland 9.5% -> 0),
the card 37.8 W -> 33.3 W, the whole GPU 35.8% -> 35.0% (WE's own GPU time
rose from 0.9% to 7.9%: NVIDIA's Wayland Vulkan path copies too). Again
with the installed 0.7.0 and the user's own WE, both drivers in turn: CPU
17.1% -> 4.7% (XWayland 10.5% -> 0), the card 33.0 W -> 31.7 W, the whole
GPU 36.6% -> 38.2% (noise). The first start on GE-Proton takes about 2
minutes (Steam runs WE's install script and `wineboot` updates the prefix
to GE's Wine; the engine's start timeout fires once and it asks again);
later switches pause the wallpaper for 30 s to a minute. Hence an
opt-in (`rustpaper we-proton enable`, Performance tab), off by default.
With it on, `we-launch` sets `WAYLAND_DISPLAY` to the stage's socket and
`PROTON_ENABLE_WAYLAND=1` and removes `DISPLAY`, and the engine checks
WE's `WAYLAND_DISPLAY` (not its `DISPLAY`) to know it runs on the stage;
changing the setting restarts WE.

**Service mode.** `rustpaper --service` (the systemd user service the
selector manages, docs/SELECTOR.md) shows what `~/.config/rustpaper/state.json`
says (`rustpaper_core::state`). It re-reads it on SIGHUP
(`systemctl --user reload rustpaper`) and applies the change in place:
- a new wallpaper or new property values load into the same windows
  (only the windows whose wallpaper changed);
- new quality settings restart WE (see below);
- a volume or mute change reaches WE's audio streams on the next tick.

The process never restarts, so the desktop never shows the plain
background. With nothing chosen yet it exits 0, so the service does not
loop. `rustpaper <dir> [...]` still works for manual runs.

When an engine exits:
- with a desktop stage, the windows and headless outputs stay for the next
  engine; when it is the last `rustpaper` running it sends `-control stop`,
  so WE renders nothing while no wallpaper is shown;
- with the private stage, its XWayland goes with it and WE loses its
  display. The next engine starts a new stage and has Steam launch WE onto
  it (15-30 s).

The systemd service avoids both by reloading in place. Without systemd, the
selector starts the next engine before stopping the old one.

A **watchdog** runs every 2 s. It lets the stage repair itself (`tend`:
workspace and fullscreen state on desktop stages), and asks WE again for a
window that is missing after 40 s. After two
unanswered requests it restarts WE, which is also the cure for the degraded
state above. It also applies the **Playback rules** (state.json, the
selector's Performance tab).
- **Conditions**, per monitor (from the desktop adapter's `coverage`, and
  `power::on_battery`):
  - a fullscreen window;
  - a maximized or tiled window on the active workspace (Hyprland tiles by
    default, so the wallpaper only shows in the gaps);
  - the focused window on that monitor;
  - display off (DPMS);
  - running on battery (a Mains adapter offline).
- **Actions.** Each condition is Keep running or Pause. A paused monitor
  stops capturing (desktop stage) or drops the stage's frames (private
  stage); when every monitor is paused, the engine pauses WE (private
  stage: SIGSTOP on its renderers, SIGCONT when one resumes; see "WE only
  when needed").
- **Defaults:** pause under fullscreen windows and when the display sleeps,
  like WE.
- **While the lock screen shows the wallpaper** (`lock/feed.rs`, private
  stage) WE is never paused: only the lock covers the monitors. The
  mirror's hidden surfaces park on a 1x1 buffer, and each WE buffer goes
  back to WE once neither the mirror nor the lock holds it
  (docs/LOCKSCREEN.md).

## Video wallpapers in mpv (`video.rs`)

**The one exception to "fidelity comes from WE".** A video wallpaper
(project.json `"type": "video"`) is a looping video file, so any player
shows the same picture. WE under Proton decodes it on the CPU: Proton's
Media Foundation goes through GStreamer/FFmpeg in software, and NVDEC stays
at 0%. The same 1080p24 H.264 wallpaper (Gargantua 827148653) on an RTX
4060 Ti, 2026-11-23:

| player | CPU (one core) | video decoder |
|---|---|---|
| WE under Proton (Proton Experimental or GE-Proton) | 22.7% | 0% |
| `mpv --hwdec=no` | 14.3% | 0% |
| `mpv --hwdec=auto` | 1.9% | 3% (NVDEC) |
| Rustpaper's mpv on the private stage (`--vo=gpu`, Vulkan decoding) | 1.85% (+ 0.9% for the debug engine) | 3% |

Scenes and web wallpapers stay on WE.

**How it works.** With the private stage and mpv installed (and the
selector's "Play videos with" on mpv, the default: `state.json`
`video_player`, `mpv` or `we`), the engine plays every video wallpaper that
a monitor *renders* (the owner of a shared group, `share_plan`) in mpv:
- **The window.** mpv is a Wayland client of the stage (`WAYLAND_DISPLAY`
  = the stage's socket, `DISPLAY` removed). Its window is titled
  `rustpaper-mpv-<output>`, never WE's `rustpaper-<output>`; the stage
  makes it fullscreen at the monitor's size and forwards its dmabufs like
  WE's, to the mirror and the lock screen.
- **One feed per monitor.** The engine tells the stage which window each
  monitor shows (`PrivateStage::set_feed`: WE's or mpv's); the other one's
  buffers go straight back to it. WE's long-lived window of a monitor that
  now plays a video is not closed (closing degrades WE), and without this
  its frames and mpv's would alternate on screen. Verified with a stand-in
  WE window on the same monitor: only mpv's frames shown, and switching to
  a scene showed the stand-in again at once.
- **Options.** `--no-config` (the user's mpv.conf, scripts and shaders stay
  out), `--vo=gpu`, `--hwdec=auto-safe`, `--loop-file=inf`, no OSC, OSD,
  input or cursor, `--panscan=1.0` with `--keepaspect-window=no` (cover:
  fill the monitor, crop the overflow), `--mute` and `--volume` from
  state.json (mpv takes the real volume, WE only mute), an
  `--input-ipc-server` socket in `$XDG_RUNTIME_DIR/rustpaper/`.
- **Over IPC:** another video loads with `loadfile` into the same window;
  mute and volume change in place; the Playback rules send `pause`.
- **Watchdog.** An mpv that exits is started again (at most every 10 s).
  mpv dies with the engine (`PR_SET_PDEATHSIG`; SIGKILL, because after its
  compositor vanished mpv hung on SIGTERM).
- **WE only when needed.** WE is started only when some monitor shows, in
  any mode (desktop, lock, screensaver when enabled), a wallpaper that is
  not a video; a reload that needs it starts it then. When no monitor shows
  a WE window, the engine pauses WE, so a running WE costs nothing. WE's
  own `-control pause` does nothing to `-playInWindow` windows (measured
  like its mute, see "Volume and mute"), so the engine stops WE's
  renderers with SIGSTOP (`wallpaper64.exe` and its Chromium's `--type=`
  subprocesses; Wine's services keep running) and continues them with
  SIGCONT. Measured: the frames froze, WE took no CPU, the card went from
  ~32 W to 24 W, and after SIGCONT WE played on with no restart
  (2026-11-23). Every `-control` command continues WE first (a stopped WE
  never takes it; the next tick pauses it again), and the engine continues
  it on exit and on start (one killed while WE was stopped). Only with the
  private stage: nothing there pings its clients, while a desktop's
  compositor could flag a stopped window as not responding, so with a
  desktop stage WE keeps rendering and only its audio is muted. Pausing is
  per renderer: each mpv pauses when every monitor it feeds is paused, WE
  when every monitor showing one of its windows is.
- **Falls back to WE**: without mpv on PATH, with a desktop stage (mpv
  needs the stage's Wayland socket), with `video_player: "we"`, or when mpv
  cannot be started.

`--vo=dmabuf-wayland` (decoded frames handed to the compositor as they are)
was not used: the stage forwards one buffer per monitor to a layer surface,
and `--vo=gpu` already costs 1.85% of a core.

Verified on a nested Sway (2026-11-23, `RUSTPAPER_NO_WE=1` and once
without it, all monitors on videos: WE was left alone): video moving
between screenshots; switching videos in place; a killed mpv restarted;
pause under a fullscreen window (mpv 0% CPU) and play after it closed, per
monitor; two outputs with one video sharing one mpv, two videos with an
mpv each; the nested lock screen showing the video live and unlocking;
mpv gone after the engine was killed with SIGKILL.

## The desktop stage on Hyprland

The original stage, still available (`--stage desktop`, or `auto` without
WE's launch option). Per monitor:

1. **Headless output.** A headless Hyprland output `RP-<output>` is created
   at the monitor's pixel size, scale 1. A workspace rule pins workspace
   `rustpaper-<output>` as its default *before* it is created, so the output
   opens straight on it. Hyprland only renders windows on a visible
   workspace, and only rendered windows can be captured: when a headless
   output showed another workspace, the capture stalled at 0 fps.
2. **WE window.** WE plays the wallpaper in window `rustpaper-<output>`
   (`we.rs`). The engine starts a second `wallpaper64.exe` inside the running
   WE's Steam Runtime container (`nsenter` into its user + mount namespaces,
   with its environment) to send `-control openWallpaper -file Z:\...
   -playInWindow rustpaper-<output> -width W -height H`. A window rule
   matching that title sends it to the workspace with
   `fullscreen_state = '2 0'`, no focus and no animation:
   - fullscreen for the compositor only (internal 2, client 0). Told it was
     fullscreen, the Wine window kept leaving it (tiled at 1896x1026, every
     couple of seconds);
   - no animation: the fullscreen grow resized the capture every frame, and
     GBM refused one of the odd sizes (3649x2051).
3. **Mirror** (`mirror.rs`). The window is found by title through
   `ext-foreign-toplevel-list-v1` and captured with `ext-image-copy-capture-v1`
   (window source from `ext-foreign-toplevel-image-capture-source-v1`) into
   GBM dmabufs (`gbm.rs`) allocated on the device the compositor names. Each
   filled buffer is attached as-is to a `zwlr-layer-shell` surface (layer
   Bottom, namespace `rustpaper`, empty input region) on the monitor, scaled
   to the logical size with `wp_viewporter`. The compositor answers a capture
   only when the window has new content, so WE paces the loop. There is no GL
   context and no CPU copy.

**Config reloads.** A Hyprland config reload drops every rule added at
runtime: window rules, workspace pins, headless output modes. Omarchy
reloads on every theme switch, which "Theme from wallpaper" triggers. The
adapter listens on Hyprland's event socket (`.socket2.sock`,
`configreloaded>>`) and re-applies the stage.
- Verified with `hyprctl reload`: the stage was restored and ended at the
  right sizes.
- Hyprland animates WE's windows in the instant of the reload, before any
  client can react: a 1-2 s zoom (7680x4320 down to 3840x2160 in the
  capture).
- The fix is to put the same rules in the Hyprland config, where reloads
  keep them, through a file that `hyprland.lua` would `require`. That means
  editing the user's config, so it is not done automatically.

## WE's configuration

`we::prepare_config` edits WE's `config.json` (one-time backup:
`config.json.rustpaper-backup`) and restarts WE when something changed. The
edit happens while WE is stopped, because a running WE rewrites its config
on exit:

- `safemode: false`. WE sets it while running and clears it on a clean exit;
  after a crash, the next start shows a "Safe Start" dialog that blocks every
  `-control` command.
- `playbackfullscreen`, `playbackmaximized` and `playbackfocus: run`. Every
  wallpaper window is a fullscreen app to WE, so with two monitors each one
  paused the other (DP-1 went 60 -> 29 -> 0 fps). The engine applies the
  user's Playback rules itself.
- The user's **quality settings** (the Performance tab: fps, msaa,
  resolution, postprocessing, shadows, volumetrics, reflection, preset;
  `rustpaper_core::performance`, sanitized to WE's own values). On reload,
  `we::needs_restart` compares them with WE's config and restarts WE when
  they differ.
  - Verified: FPS 24 saved -> reload -> WE restarted with `fps: 24`,
    `preset: custom`, windows back in ~30 s.

WE's own windows (UI, dialogs) are moved to `special:rustpaper-we` and never
take focus.

## Properties and sound

The selector's property values go to WE with
`-control applyProperties -location <window> -properties RAW~({...})~END`,
3 s after the wallpaper was requested (the window may still show the
previous wallpaper while the new one loads with its defaults).
- **No `-location`**: WE applies them to its desktop wallpapers only.
- **Passed as an argument**: Wine rebuilds the Windows command line with the
  JSON's quotes escaped (`\"`), and WE, which reads the raw line between
  `RAW~(` and `)~END`, rejected it (exit code 4).
- **What works**: the line goes into a .bat run by `cmd.exe`, which keeps it
  as written. String values that cannot survive cmd's quoting (quotes,
  control characters, non-ASCII) are left out, and `%` is doubled.
- **Verified** with Lonely Cat 3299228616: `{"clockcolor":"1 0 0"}` turned its
  clock red.

**Volume and mute.** WE has no volume command, and its `-control
mute|unmute` (like `pause|play`) does nothing to `-playInWindow` windows:
with a scene playing music, the stream's peak stayed ~3400 after each one,
also with `-location <window>` or `-monitor 0`, on Wine's X11 and Wayland
drivers alike (2026-11-23; the selector's volume 0 had never muted WE). So
the watchdog sets WE's audio streams (`application.name` "Wallpaper
Engine") in the user's sound server through `pactl` (`audio.rs`): the
user's volume, muted at 0 and while no monitor shows a WE window (a window
a video in mpv left behind keeps playing its old wallpaper, music
included). WE opens a stream per wallpaper, hence a check every tick.

## Measurements (desktop stage, 2026-11-22, RTX 4060 Ti, Hyprland 0.56.2)

Jinx 3078285611 on HDMI-A-1 (3840x2160, scale 1.5) plus Lonely Cat 3299228616
on DP-1 (1920x1080), 20 s averages:

| | CPU | GPU util | GPU power | fps |
|---|---|---|---|---|
| old native renderer (same two wallpapers) | rustpaper 21.9% + Hyprland 2.7% | 43% | 70 W | 60 / 60 |
| WE + mirror (dmabuf) | WE 14.5% + Hyprland 6.9% + Xwayland 1.9% + mirror 0.3% | 69% | 105 W | 60 / 60 |
| WE + mirror, shm buffers (4K) | Hyprland **46%** + WE 17% | 93% | 108 W | 60 / 60 |
| WE idle, no wallpaper window | ~0% | 19% | 18 W | - |

- CPU is on par with the old renderer. The GPU costs about 35 W more: WE's
  own rendering, Hyprland compositing the headless outputs, and the copy.
- shm buffers made the compositor read every frame back to RAM: a single
  1080p60 mirror took Hyprland from 2.8% to 23% CPU (dmabuf: 3.1%).
- RAM: `wallpaper64.exe` takes 0.9 to 1.2 GiB with two wallpapers.

## Known gaps

- **Pointer with desktop stages**: WE's window sits in the user's
  compositor, which never moves its pointer there, so cursor-following
  wallpapers do not react (the private stage forwards it, see "Pointer").
- **No keyboard input** reaches WE's windows (WE's wallpapers rarely use
  it).
- **Explicit sync** (`wp_linux_drm_syncobj`) is not offered to the stage's
  XWayland; implicit sync showed no corruption on NVIDIA so far.
- **Desktop stages never pause WE** (no SIGSTOP there, see "WE only when
  needed"): under the Playback rules' Pause it keeps rendering, muted.
- **Video wallpapers in mpv**: WE's window of a monitor that switched to a
  video keeps rendering its old wallpaper while another monitor still needs
  WE (WE pauses only when no monitor shows one of its windows). Property
  values of video wallpapers are not mapped to mpv: none of the 30 video
  wallpapers installed here declares anything but `schemecolor`, so
  alignment and playback rate would have been guesses; videos cover the
  monitor.
- **Degraded WE** (black or pixelated windows, a few fps; seen with the
  desktop stage after dozens of window open/close cycles) is only detected
  when windows stop appearing.
- **Leftovers of desktop stages**: their headless outputs and WE windows
  stay after the last engine exits (stopped, so they cost nothing); the
  next engine reuses them. The private stage leaves nothing behind (its
  XWayland dies with the engine, and WE with it).
- **Selector**: no stage choice and no `setup-steam` button yet (the engine
  picks the private stage once the launch option exists).
- **Measurements** above are for the desktop stage; the private stage has
  not been measured on the same wallpaper pair yet.
- **Untrusted code.** SceneScript and web wallpapers run inside WE as they do
  on Windows, with no sandbox, and Proton maps the host root as `Z:`.
