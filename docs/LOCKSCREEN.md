# Animated lock screen: prerequisites

Goal: the lock screen shows the live wallpaper (the same Wallpaper Engine
render as the desktop) instead of a still picture. This document lists what
exists today, the routes that could get there, and what must be verified or
built first. Nothing here is implemented yet.

## What exists today (Omarchy, 2026-11)

- **The lock screen is a Quickshell plugin**, `omarchy.lock`
  (`/usr/share/omarchy/shell/plugins/lock/`), loaded by `omarchy-shell`
  (Quickshell 0.3.2). `omarchy-system-lock` runs `omarchy-shell lock lock`.
  Hyprlock is not installed. The plugin holds the session with
  `WlSessionLock` (`ext-session-lock-v1`) and runs its own PAM flows
  (`/etc/pam.d/omarchy-lock-*`).
- **Its background** is `~/.local/state/omarchy/current/background`, the
  desktop background Omarchy themes set, drawn blurred (`MultiEffect`,
  `blurMax: 128`, `LockView.qml`).
  - For a video background the plugin shows a still poster
    (`poster.sh`): "nothing plays behind the lock".
- **A still lock background already works.** With "Theme from wallpaper" on,
  the selector's Omarchy theme sets the background to the wallpaper's
  preview (`omarchy-theme-set`), and the lock screen picks it up.
- **Plugin trust.** The shell grants the `authentication` capability only
  to first-party plugins (`__hostCapabilities`, `shell.qml`). A third-party
  plugin cannot become the lock screen. User plugins are enabled through
  `~/.config/omarchy/shell.json`.
- **Quickshell can show live content.** `Quickshell.Wayland` ships
  `ScreencopyView` (`captureSource`, `live`, `paintCursor`) and toplevel
  management (`_ToplevelManagement`). A QML view can show a window's live
  content.
- **The engine has the frames.** Each monitor's wallpaper is a long-lived
  WE window, `rustpaper-<output>` (docs/ENGINE.md), but where it lives
  changed:
  - with a **desktop stage** it is a Hyprland window on a headless output,
    so any client can capture it by title;
  - with the **private stage** (the default since 2026-11-22) it lives in
    Rustpaper's own compositor and is not a window of the user's
    compositor at all. Nothing outside Rustpaper can capture it, but the
    engine holds WE's dmabufs itself.

## Routes

| Route | What | Cost | Risk |
|---|---|---|---|
| **A. Live background in Omarchy's lock** | `LockView.qml` draws a live view of the wallpaper behind the password box, falling back to today's still picture. Proposed upstream as a background option (e.g. `lock.background: "toplevel:rustpaper-{output}"` in shell.json). With the private stage it needs a source the lock can read (see prerequisite 2) | Small QML change, an upstream PR, plus that source in the engine | Upstream may refuse. A local patch of the first-party plugin is overwritten by Omarchy updates |
| **B. Rustpaper's own lock client** | An `ext-session-lock-v1` client in Rust: shows WE's frames (the engine already has them as dmabufs, straight from the private stage), with its own PAM authentication | Large: authentication, fingerprint, multi-monitor, keyboard layouts, crash safety | Security-critical code; it would replace Omarchy's lock, and its UI would differ from Omarchy's |
| **C. Still picture only** | What works today: the preview as the lock background | None | Not animated |

**Recommended: route A.** It keeps Omarchy's audited authentication and
look, and the change is a background source. With the private stage it also
needs the engine to offer that source (prerequisite 2). Route B is the
fallback if upstream refuses and a local patch is not acceptable; the
private stage makes its rendering side easy (the frames are already in the
engine), but the authentication side is still the hard part.

## Prerequisites to verify (in order)

1. **WE keeps rendering while locked.**
   - **Private stage:** WE renders in Rustpaper's own compositor, which the
     session lock does not touch. Only the engine's layer surfaces are
     covered, and the Playback rules do not see a lock as a covering window
     (the "display asleep" rule still pauses when the screens power off).
     To confirm with a minute of lock and the engine's fps log.
   - **Desktop stage:** Hyprland covers every output with a lock surface,
     our headless outputs included, and only renders windows on visible
     workspaces. Check that the capture of `rustpaper-<output>` keeps
     producing frames while locked.
2. **A source the lock can show.**
   - **Desktop stage:** the toplevel `rustpaper-<output>`, found through
     Quickshell's toplevel manager and fed to `ScreencopyView { live: true }`
     (check which capture protocol Quickshell 0.3.2 uses for toplevels, and
     that it works inside the lock's QML context).
   - **Private stage:** WE's window is not a toplevel of the user's
     compositor. Options, all zero-copy:
     - the engine also shows the same dmabufs on a hidden `xdg_toplevel`
       per monitor (on a headless output), which Quickshell captures by
       title;
     - the engine publishes a PipeWire video stream per monitor and the
       lock plays it (needs a Quickshell element that plays PipeWire);
     - route B, where the engine is the lock and uses the frames directly.
3. **Monitor mapping.** The lock creates one surface per output; each needs
   its own `rustpaper-<output>` window (same name scheme as the engine).
   Outputs without a window keep the still picture.
4. **Cost.** A 4K, 60 fps live background blurred with `blurMax: 128` every
   frame is expensive. Measure it. Options: no blur on live backgrounds, a
   lighter blur, or a capped frame rate while locked.
5. **Engine behaviour while locked.**
   - Locked is a new Playback condition. Detect it through `LOCK` in a
     monitor's `solitaryBlockedBy`, which is what
     `omarchy-hyprland-session-locked` checks.
   - With the animated lock on, the engine keeps WE running while locked:
     today our layer surfaces are hidden under the lock, but WE keeps
     rendering.
   - It pauses when the displays sleep (DPMS), already covered by the
     "Display asleep" rule.
6. **Safety.**
   - Never show anything but the wallpaper. The capture source is the WE
     window only, never an output.
   - Fall back to the still picture when the capture fails or produces no
     frame.
   - Keep the password UI fully responsive: a slow capture must not block
     authentication.
7. **Selector.** A "Lock screen" setting with three values: Still
   (wallpaper preview), Animated (needs route A or B available), or Omarchy
   default. With Animated, the selector configures the lock (shell.json
   option or plugin) and the engine's "locked" rule.

## Next step

1. Lock the session for a minute with the private stage running and read
   the engine's fps log (prerequisites 1 and 4).
2. Prototype the source of prerequisite 2: a hidden `xdg_toplevel` per
   monitor showing the stage's dmabufs.
3. Check it with a throwaway QML snippet run by `quickshell -p`: a plain
   layer window, not the real lock, showing `ScreencopyView` of that
   toplevel.
4. If it holds, write the upstream proposal for route A.
