# Reference harness: Rustpaper vs. the real Wallpaper Engine

Fidelity is measured against the real WE (running under Proton), not by eye.

```
./harness/run.sh <workshop id> [seconds=8]     # everything below, in one go
./harness/we-capture.sh <id> <out dir> [secs]  # real WE -> we.png + we.mp4
./harness/compare.sh rp.mp4 we.mp4 <out dir>   # metrics + sheet + side-by-side
```

Output in `harness-out/<id>/` (git-ignored): `sheet.png`, `side_by_side.mp4`,
`summary.txt`, the raw `rp.*` / `we.*` captures and `rp_warnings.txt`.

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

## What the comparison measures

WE's clock can't be controlled, so per-frame diffs are meaningless for
animated scenes. Instead (top 32 px ignored: the desktop bar is drawn on the
headless output):

- **mean** — temporal average of all frames: what the scene looks like.
- **motion** — average |frame − previous frame|: where and how much it moves.
- **SSIM** of both (1 = identical), plus `side_by_side.mp4` to judge timing.

First result, Yor Forger (2824109832, 17 effects incl. multi-pass): mean SSIM
0.94, motion SSIM 0.88.
