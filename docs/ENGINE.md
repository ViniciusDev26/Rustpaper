# The engine: Wallpaper Engine under Proton, mirrored onto the desktop

`rustpaper <wallpaper dir>` (crate `rustpaper-engine`) does not render
wallpapers. The real Wallpaper Engine (WE) does, under Proton, and Rustpaper
puts its output on the Wayland desktop. Until 2026-11 Rustpaper had its own
OpenGL reimplementation of WE's renderer. It reached 70/73 scenes without
SceneScript and 93/123 overall against a fidelity harness, but every
wallpaper still differed somewhere and the long tail never ended, so it was
replaced by this (the old code is in git history before the migration
commit).

## Architecture: one core, desktop adapters

```
            core (identical on every desktop)                 adapters (desktop/)
 ┌──────────────────────────────────────────────────┐   ┌─────────────────────────────┐
 │ we.rs      WE under Proton: start, -control,     │   │ trait Desktop               │
 │            config.json, applyProperties          │   │   outputs()   real monitors │
 │ config.rs  CLI / state.json (--service, SIGHUP)  │◀──│   coverage()  Playback info │
 │ main.rs    targets, reload, watchdog, Playback   │   │   stage()                   │
 │ mirror.rs  capture WE's windows -> layer-shell   │   │ trait Stage                 │
 │            (standard Wayland protocols only)     │   │   prepare() hide_we_ui()    │
 └──────────────────────────────────────────────────┘   │   tend()                    │
                                                        │ hyprland.rs  sway.rs  ...   │
                                                        └─────────────────────────────┘
```

The core never talks to a compositor. Everything desktop-specific sits
behind two traits (`desktop/mod.rs`):
- **`Desktop`**: the real monitors and their pixel sizes, and what covers
  each one (fullscreen, maximized or tiled, focused, asleep), for the
  Playback rules.
- **`Stage`**: a place where WE's windows live out of the user's sight but
  rendered, because compositors only render (and so only let us capture)
  windows they consider visible. It must:
  - prepare it once per monitor;
  - keep WE's own UI hidden;
  - repair it every 2 s (`tend`).

Supporting a desktop = one adapter implementing both traits.
`desktop::detect()` picks it from the session (`HYPRLAND_INSTANCE_SIGNATURE`,
`SWAYSOCK`). An unsupported session gets an error naming exactly what is
missing.

| Desktop | Adapter | Stage | Status |
|---|---|---|---|
| Hyprland 0.56+ | `desktop/hyprland.rs` | headless outputs `RP-<output>`, pinned workspaces, window rules | in use, measured |
| Sway 1.11+ | `desktop/sway.rs` | `create_output` headless outputs, `workspace ... output`, `for_window` | written against sway-ipc(7), unit-tested on recorded replies; not run on a live Sway yet |
| KDE, niri, others with layer-shell | none yet | the private stage (below) avoids needing compositor-specific hiding | planned |
| GNOME | none | no layer-shell: needs a Shell extension to show the wallpaper | not planned |

What every adapter can rely on: `zwlr-layer-shell` to show the wallpaper
and `ext-image-copy-capture` + `ext-foreign-toplevel-list` to capture WE's
windows. The mirror uses only these.

### The private stage (planned)

Today's stages borrow the user's compositor (headless outputs, rules). The
general answer is a Stage that is Rustpaper's own:
- **What it is.** A minimal headless compositor (Smithay) with its own
  XWayland, where WE opens its windows. Window sizes, focus and visibility
  are then ours, without rules or repairs.
- **What it gives.** WE's buffers reach us directly, without capture, and
  pointer motion can be injected into WE: the missing cursor interaction.
- **Adapters shrink.** Any compositor with layer-shell then only needs a
  `Desktop` adapter, or none at all, with a generic one built on
  `wlr-foreign-toplevel`.

Prerequisites, in order of risk:
1. **WE must connect to our X display.** WE inherits Steam's environment, so
   this needs a Steam launch option for WE, for example a
   `rustpaper-we-launch %command%` wrapper that sets `DISPLAY` when the
   engine runs and changes nothing otherwise. Set it once (Steam rewrites
   `localconfig.vdf` while it runs, so not behind its back).
2. **Explicit sync on NVIDIA.** XWayland on the proprietary driver needs
   `wp_linux_drm_syncobj`. The stage must implement it and forward the sync
   points to the host compositor (Hyprland advertises the protocol).
3. **Buffer forwarding.** WE's dmabufs must be re-imported on the host
   (same GPU) and held until the host releases them; shm needs a fallback
   copy.

## How it works (Hyprland stage)

For each real monitor there is ONE long-lived WE window, shared by every
engine instance:

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

**Switching wallpapers never opens or closes windows.** `openWallpaper` with
an existing window name loads the new wallpaper into that window. A first
design opened a window per engine and closed it on exit, and WE degraded
within minutes:
- a close (`closeWallpaper -location` or the compositor's) took down another
  window showing the same wallpaper;
- later `closeWallpaper` did nothing at all;
- new windows came up black, or pixelated with a wrong clock, at ~3 fps.

**Service mode.** `rustpaper --service` (the systemd user service the
selector manages, docs/SELECTOR.md) shows what `~/.config/rustpaper/state.json`
says (`rustpaper_core::state`). It re-reads it on SIGHUP
(`systemctl --user reload rustpaper`) and applies the change in place:
- a new wallpaper or new property values load into the same windows;
- new quality settings restart WE (see below);
- a mute change is sent to WE.

The process never restarts, so the desktop never shows the plain
background. With nothing chosen yet it exits 0, so the service does not
loop. `rustpaper <dir> [...]` still works for manual runs.

An engine that exits leaves the windows and headless outputs for the next
one. When it is the last `rustpaper` running, it sends `-control stop` so WE
renders nothing while no wallpaper is shown. (Without systemd, the selector
starts the next engine before stopping the old one, so for 2.5 s both mirror
the same window.)

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

A **watchdog** runs every 2 s. It restores the workspace and fullscreen
state, and asks WE again for a window that is missing after 40 s. After two
unanswered requests it restarts WE, which is also the cure for the degraded
state above. It also applies the **Playback rules** (state.json, the
selector's Performance tab).
- **Conditions**, per monitor (`hyprland::monitors`, `power::on_battery`):
  - a fullscreen window;
  - a maximized or tiled window on the active workspace (Hyprland tiles by
    default, so the wallpaper only shows in the gaps);
  - the focused window on that monitor;
  - display off (DPMS);
  - running on battery (a Mains adapter offline).
- **Actions.** Each condition is Keep running or Pause. A paused monitor
  stops capturing; when every monitor is paused, the engine sends
  `-control pause` to WE (and `play` when one resumes).
- **Defaults:** pause under fullscreen windows and when the display sleeps,
  like WE.

### WE's configuration

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

### Properties and sound

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

## Measurements (2026-11-22, RTX 4060 Ti, Hyprland 0.56.2)

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

- **Cursor.** WE reads the X pointer through XWayland, which only moves
  while the cursor is over an X window. Over a layer surface it never moves,
  so eye-tracking and parallax wallpapers do not react and clicks do nothing.
  Fixing this needs pointer injection into XWayland (XTest or libei) from
  Hyprland's cursor position.
- **Volume** is only mute/unmute (see above).
- **Degraded WE** (black or pixelated windows, a few fps) is only detected
  when windows stop appearing. A WE that renders badly but keeps its windows
  needs a manual restart (`kill` wallpaper64.exe; the engine restarts it).
- **Leftovers**: the headless outputs and WE's windows stay after the last
  engine exits (stopped, so they cost nothing). The next engine reuses them.
- **Desktops:** Hyprland (0.56+, in use) and Sway (1.11+, not yet run
  live); others need an adapter or the private stage (see Architecture).
- **Untrusted code.** SceneScript and web wallpapers run inside WE as they do
  on Windows, with no sandbox, and Proton maps the host root as `Z:`.
