# Reference harness: Rustpaper vs. the real Wallpaper Engine

Fidelity is measured against the real WE (running under Proton), not by eye.

```
./harness/run.sh <workshop id> [seconds=8]     # everything below, in one go
./harness/we-capture.sh <id> <out dir> [secs]  # real WE -> we.png + we.mp4
./harness/compare.sh rp.mp4 we.mp4 <out dir>   # metrics + sheet + side-by-side
./harness/batch.sh [--recapture] [--update] [id...]  # every scene -> results.tsv + report.md
./harness/download-testset.sh                  # the 100-wallpaper test corpus
```

Output in `harness-out/<id>/` (git-ignored): `sheet.png`, `side_by_side.mp4`,
`summary.txt`, the raw `rp.*` / `we.*` captures and `rp_warnings.txt`.

## Fidelity target

The goal is more than 95% of scenes passing. It is measured only on **scene
wallpapers that use no SceneScript**. Web wallpapers and scenes that rely on
SceneScript are out of scope for now. They will come later, once JS can run
in a sandbox (Rustpaper never executes wallpaper JS today). `score.py` reports
this subset separately from the full corpus.

## Requirements

- Hyprland (uses `hyprctl` Lua dispatchers, `grim`), `ffmpeg`, `nsenter`.
- Wallpaper Engine installed through Steam with Proton and **already running**.

## How the capture works

1. `lib.sh` starts a second `wallpaper64.exe` inside the running WE's Steam
   Runtime container + wineserver (`nsenter` into its namespaces) to send
   `-control openWallpaper -file ... -playInWindow rpref_<id>`.
2. The WE window is moved to workspace `rpref` on a **headless output**
   (`RPREF`, 1920x1080, scale 1) and made fullscreen, so nothing covers the
   user's real monitors. Focus goes back to whatever was focused.
3. `grim -o RPREF` grabs frames at the output refresh rate (~60 fps) with real
   timestamps; they are resampled to 30 fps CFR to line up with
   `rustpaper-render --video --fps 30`.

The headless output stays around between runs; remove it with
`hyprctl output remove RPREF`.

Focus: a freshly opened WE window grabs focus, so `we-capture.sh` records the
focused window BEFORE asking WE to open the wallpaper and restores it after
parking and on exit (otherwise the keyboard ends up on the invisible output).

### Unattended runs

- **DPMS:** when the idle daemon powers the monitors off, the headless
  output stops rendering too (grim hangs, WE captures black).
  `ensure_headless` switches only `RPREF` back on before every capture.
- **Black captures** (WE failed to load the wallpaper in its window) are
  rejected instead of being used as a reference.
- **Session lock:** the lock screen covers every output, RPREF included;
  `ensure_headless` refuses to capture while Hyprland reports an active
  session lock (`LOCK` in `solitaryBlockedBy`).
- **WE crashes** (some wallpapers take it down): `we_control` restarts it via
  `steam -applaunch 431960` and moves its own windows (UI, dialogs) to the
  `rpref` workspace (`WE_AUTOSTART=0` disables this).

- **Safe Start:** WE sets `safemode: true` in its `config.json` while running
  and clears it on a clean exit; after a forced restart the "Safe Start"
  dialog blocks every `-control` command. `restart_we` clears only that flag
  (one-time backup: `config.json.rustpaper-backup`).
- **Executable path:** the control instance is started through `Z:` (host
  root). The `S:` library drive is not mapped in every Proton session
  ("wine: failed to open S:\\..."), which made WE look wedged.
- Focus goes back to the user's window, or to their monitor when no window
  was focused.

- **Focus:** a runtime window rule (`rustpaper-harness-we`) opens WE's windows
  on the `rpref` workspace without focus; it disappears on the next Hyprland
  reload. The user's monitor is the focused one, or the one under the cursor.
- **Parallel:** captures are serial; rendering + comparing run as background
  jobs (`BATCH_JOBS_CAPTURE`, default 3, while WE captures — more would steal
  GPU time from WE and skew its motion — and `BATCH_JOBS`, default 8, for
  scenes whose capture is cached). Rows are collected at the end.

### Recovering a stuck WE

If every capture fails with "did not open a window", WE is wedged (a leftover
`rpref_*` window that ignores close requests). Restart it:
`pkill -f wallpaper_engine/launcher.exe; pkill -f wallpaper64.exe;
steam -applaunch 431960`. After a crash WE shows a "Safe Start" dialog; the
`-control` commands work regardless.

## What the comparison measures

Desktop notifications pop up in the top-right corner of every output,
including the headless one; that corner (460x64 px under the bar) is blacked
out on both sides before comparing.

WE's clock can't be controlled, so per-frame diffs are meaningless for
animated scenes. Instead (top 32 px ignored: the desktop bar is drawn on the
headless output):

- **mean** — temporal average of all frames: what the scene looks like.
- **motion** — average |frame − previous frame|: where and how much it moves.
- **SSIM** of both (1 = identical), plus `side_by_side.mp4` to judge timing.

Rustpaper renders with `--screen 1920x1080`: the scene goes through the same
centered cover crop as the 1920x1080 WE window. (Comparing the scene's own
aspect stretched to 16:9 made every non-16:9 scene look misaligned.)

**Verdict** (`compare.sh`, column `verdict` of `metrics.tsv`): PASS when mean
SSIM >= 0.95 AND the motion energy ratio (mean |frame - previous| of RP over
WE) is within 0.7..1.4 — the scene looks the same and moves about as much.
Motion SSIM is reported but not gated (random particles never line up).
The energy skips frame pairs with exactly zero difference: WE presents fewer
distinct frames than the capture grabs (~1/3 of its 15 fps pairs repeat),
which halved WE's energy for content that changes every frame (Zero Two
1384678183's film grain: ratio 1.47 -> 1.02 without them).

Clocks: `we-capture.sh` stores the capture start in `we_start.txt`; the
renders run with `RUSTPAPER_CLOCK=<that time>` so clock/date texts show the
same time as the capture (older captures fall back to the video's mtime).

Rustpaper's video starts at the scene time WE had reached when its recording
started (`we_time.txt`: open command -> first frame, ~6-7 s; 6 s for older
captures). WE opens the wallpaper fresh for every capture, so this puts
time-driven animations in phase: starting at a fixed 10 s instead cost
Painting the Sharks 2468489223 0.966 -> 0.945, wlop sky2 2900431360
0.975 -> 0.928, Dome 3113287126 0.949 -> 0.897.

Cursor: WE runs under Wine on XWayland and reads the **X pointer**, clamped
to its window. That is not the live Wayland cursor: XWayland only learns the
position while the cursor is over an X window and keeps the last one (X said
2003,39 while Hyprland's cursor was at 3536,696), and X coordinates differ
from Hyprland's layout (the WE window Hyprland put at 4480,0 is at 1920,0
for X). `harness/xpointer.py` reads the X pointer and the window's X
geometry; `we-capture.sh` stores the result in `we_pointer.txt` and renders
use it as `--pointer` (older captures assume the center). Captures before
2026-11-09 13:00 recorded Hyprland's cursor instead (42 of them the same
`0.000,0.681`), which is why the parallax scenes kept failing; the corpus was
recaptured. Moving the X pointer with XWarpPointer does NOT work: it changes
what XQueryPointer reports (the user's cursor stays put), but two captures
warped to (0.5, 0.5) and (0.1, 0.2) were identical, so WE ignores warps and
the reading would no longer be what WE saw.

`calibration/run_parallax.sh` measures WE's camera parallax: the coordinate
scene (`make_scene.py`) at `parallaxDepth 1` with camera parallax on,
captured from the moment the window opens (`WE_WARMUP=0`) for several
`cameraparallaxdelay` values; `measure_parallax.py` prints which scene point
is at the screen center over time, for WE and for Rustpaper rendered with
the pointer WE saw.

For random particles the motion SSIM is a weak signal (positions never match);
compare the zoomed motion maps and measure sizes/brightness of features.

| Scene | What it exercises | mean SSIM | motion SSIM |
|---|---|---|---|
| Yor Forger 2824109832 | 17 effects, multi-pass, layer refs | 0.976 | 0.915 |
| Ayanami Rei 3571376089 | 10 particle systems, hierarchy | 0.958 | 0.738 |
| Akame ga Kill 1827211233 | fog/fire particles, children | 0.957 | 0.553 |

## Pending work (`harness/pending.sh`)

Captures that could not run (locked session) are queued: contaminated scenes
in `harness-out/recapture_after_unlock.txt`, then every failing scene
captured before `we_pointer.txt`/`we_time.txt` existed, the second particle
calibration round, then a full re-render and `score.py`. Run it with the
session unlocked.

## Batch and report

`batch.sh` runs every `scene` wallpaper of the Steam Workshop folder and the
test corpus (or the ids given). WE captures are cached in
`harness-out/<id>/we.mp4` (`--recapture` redoes them). Output:
`harness-out/results.tsv` and `harness-out/report.md` (worst first, with each
scene's unsupported features / warnings).

## Test corpus (`harness/testset.tsv`)

100 Workshop items (75 scene, 15 video, 10 web; "Everyone" rating, ~5.8 GB),
chosen to cover effects, particles, text, audio, puppets and properties.
`download-testset.sh` asks the RUNNING Steam client to download each item
without subscribing (`steam "+workshop_download_item 431960 <id>"`, your
logged-in session) and copies it into `~/.local/share/rustpaper-testset`
(override: `RUSTPAPER_TESTSET_DIR`) — Steam may later delete unsubscribed
items from its own folder. With `STEAM_USER` set it uses `steamcmd` instead.
`rustpaper_core::paths` and the harness pick the corpus up automatically. Web items are kept to verify
they are listed but refused.

## Particle calibration (`harness/calibration/run_particles.sh`)

`make_particles.py <dir> <experiment>` builds a black scene with ONE particle
system varying one feature (turbulentvelocityrandom offset/scale, turbulence
operator, velocity baseline); `run_particles.sh` captures each in WE,
renders it from the same scene time and prints `measure_particles.py`
statistics for both: centroid, spread, share of light above the emitter,
motion. Calibrating one feature at a time is what found the real
turbulentvelocityrandom model (GL_RENDERER.md). Dark captures are allowed
there (`WE_ALLOW_DARK=1`).

## Audio calibration (`harness/calibration/run_audio.sh`)

`make_audio.py` builds a scene whose shader draws `g_AudioSpectrum64Left`,
`64Right` and `16Left` as gray cells (0.25 + 0.75 * value: the gray floor
proves the shader runs in silence). `make_audio_track.py` writes a 31 s
track: a 30 Hz .. 16 kHz sweep, 1 kHz at seven levels, silence, and a
left-only tone. `run_audio.sh` captures the scene in WE **while the track
plays on the default output (audible)**, renders it with Rustpaper from the
recording, and `measure_audio.py` prints both per segment; `fit_audio.py`
fits the analyzer model (GL_RENDERER.md, Audio).

WE quirks found on the way:
- WE only feeds `g_AudioSpectrum*` to projects with
  `general.supportsaudioprocessing: true`.
- a `general` object without `properties` makes WE show a black window.

## Audio during captures

`we-capture.sh` records the monitor of the default output with `pw-record`
during the video (`we_audio.wav`, t = 0 = first frame) and `batch.sh` passes
it to `rustpaper-render --audio`, so audio-reactive scenes see the same
sound in both. Captures run one at a time (one WE, one headless output), so
each file holds only its own scene's audio. `WE_PLAY_AUDIO=<file>` plays a
file during the capture (calibration only). Background jobs of the script
ignore SIGINT: the recorder is stopped with SIGTERM.

## Exclusions (`harness/excluded.tsv`)

Scenes that cannot be compared are listed with the reason and skipped by
`batch.sh`: WE crashes or shows a black window for them, or they are built
entirely by SceneScript (which Rustpaper never runs).

## Calibration scenes (`harness/calibration/`)

`make_scene.py` writes a synthetic scene whose texture encodes scene
coordinates as colors (TEXB0002 .tex writer included); `measure.py` reads the
WE capture back into coordinates. That is how the camera behaviour (cover crop,
eye/center ignored) was established.

## Current results (2026-11-08, 56 scenes of the local library)

- 35/56 PASS; 43/56 reach mean SSIM >= 0.95; average mean SSIM 0.957.
- Remaining failures are mostly motion-energy mismatches of random particle
  systems (turbulence noise differs from WE's), plus harness limits: audio
  visualizers (before 2026-11-09 captures did not record the audio WE heard), parallax scenes (no cursor in
  WE's window), clocks (captured at a different time), media integration
  (album art), fast flicker effects aliased by the 60 -> 30 fps resampling.
- `--update` re-runs only the given ids and keeps the other rows.
