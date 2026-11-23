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
the tests. Cursor injection into our XWayland (eye tracking, parallax) is
now possible, but not done yet.

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

**Service mode.** `rustpaper --service` (the systemd user service the
selector manages, docs/SELECTOR.md) shows what `~/.config/rustpaper/state.json`
says (`rustpaper_core::state`). It re-reads it on SIGHUP
(`systemctl --user reload rustpaper`) and applies the change in place:
- a new wallpaper or new property values load into the same windows
  (only the windows whose wallpaper changed);
- new quality settings restart WE (see below);
- a mute change is sent to WE.

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
  stage); when every monitor is paused, the engine sends
  `-control pause` to WE (and `play` when one resumes).
- **Defaults:** pause under fullscreen windows and when the display sleeps,
  like WE.
- **While the lock screen shows the wallpaper** (`lock/feed.rs`, private
  stage) WE is never paused: only the lock covers the monitors. The
  mirror's hidden surfaces park on a 1x1 buffer, and each WE buffer goes
  back to WE once neither the mirror nor the lock holds it
  (docs/LOCKSCREEN.md).

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

WE has no volume command, so `--volume 0` / `--mute` send `-control mute`
and any other volume sends `unmute`; the volume itself is WE's own setting.

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

- **Cursor.** WE reads the X pointer, which never moves over a layer
  surface, so eye-tracking and parallax wallpapers do not react and clicks
  do nothing. The private stage makes this solvable: its XWayland is ours,
  so the engine can feed it pointer motion (a `wl_seat` in `stage.rs`) from
  the cursor position the desktop adapter reports. Not done yet.
- **Explicit sync** (`wp_linux_drm_syncobj`) is not offered to the stage's
  XWayland; implicit sync showed no corruption on NVIDIA so far.
- **Volume** is only mute/unmute (WE has no volume command).
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
