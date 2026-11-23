# Animated lock screen

The lock screen shows the live wallpaper (the same Wallpaper Engine render
as the desktop) behind a password prompt. It is Rustpaper's own lock
screen, so it works on any compositor with `ext-session-lock-v1`; what
depends on the desktop (how it authenticates, how it looks, which of its
lock triggers to route) lives in a lock adapter. Code: `crates/rustpaper-engine/src/lock/`.

## How it works

```
 rustpaper --service (engine)                 rustpaper lock --foreground
 ┌──────────────────────────────┐             ┌───────────────────────────────┐
 │ private stage: WE's dmabufs  │  frames     │ ext-session-lock-v1 surfaces  │
 │   ├─ mirror (layer surfaces) │ ──────────► │   wallpaper (WE's buffers)    │
 │   └─ feed server (feed.rs)   │ ◄────────── │   dim layer + panel (ui.rs)   │
 └──────────────────────────────┘  releases   │ PAM on its own thread         │
     $XDG_RUNTIME_DIR/rustpaper/              └───────────────────────────────┘
       feed-<WAYLAND_DISPLAY>.sock                   ▲ started by
                                              rustpaper lock  ◄── keybinding, menu,
                                                    ▲             idle daemon
                                              rustpaper lock-daemon ◄── logind (suspend,
                                                                        lock-session), idle
```

- **`rustpaper lock`** (`lock/mod.rs`): starts the lock in the background
  (`lock --foreground`, in a new session so it outlives its caller) and
  returns once the session is locked, so it can run right before suspend.
  A second `rustpaper lock` while locked returns at once
  (`$XDG_RUNTIME_DIR/rustpaper/lock.lock`).
- **The lock** (`lock/screen.rs`): one lock surface per monitor (monitors
  plugged in while locked get one too) with three layers: the wallpaper,
  a dim layer (a 1x1 buffer scaled by `wp_viewport`, so the panel reads on
  any wallpaper) and the panel (`lock/ui.rs`: clock, date, password field,
  status; drawn on the CPU with tiny-skia and ab_glyph into a small shm
  buffer, only when it changes).
- **The frames** (`lock/feed.rs`): the engine sends the private stage's
  dmabufs over a Unix seqpacket socket with `SCM_RIGHTS`. Each buffer's
  fds cross once; later frames of the same buffer carry only its key. The
  lock imports them on the user's compositor like the mirror does: zero
  copy, no rendering of its own. The socket is per Wayland display and only
  accepts peers of the same uid.
- **Fallbacks:** with no engine (or the desktop stage, whose buffers are
  the compositor's), the wallpaper's preview image, cropped to cover;
  then the adapter's picture (Omarchy: the theme background); then the
  theme's background color. If the engine goes away mid-lock, its last
  frame stays and the lock reconnects when an engine comes back.
- **Authentication** (`lock/pam.rs`): PAM with the adapter's service,
  every hidden prompt answered with the typed password; the password is
  zeroed after use. PAM runs on its own thread (it sleeps after a wrong
  password), so typing and drawing never block.
- **The daemon** (`lock/daemon.rs`, the `rustpaper-lock.service` user
  unit): holds a logind *delay* inhibitor for sleep and drops it only once
  `rustpaper lock` has returned (the session is locked) or 8 s passed;
  answers logind's `Lock` signal (`loginctl lock-session`); and, when the
  adapter asks, locks on idle through `ext-idle-notify-v1` (idle inhibitors,
  e.g. a playing video, are respected). `KillMode=process`: a lock started
  by the daemon survives a daemon restart.

### What the engine does while locked

- Frames go to the lock as long as it is connected. The engine tracks who
  holds each of WE's buffers (mirror, lock) and gives a buffer back to WE
  only when neither does. A plain counter is not enough: XWayland may
  commit the same buffer again before it was released, and the compositor
  releases it once.
- The mirror's layer surfaces, hidden under the lock, show a 1x1 black
  buffer instead (`Mirrors::set_locked`), so the compositor releases the WE
  buffer they held: WE renders into a few buffers only.
- The Playback rules do not pause WE while the lock shows the wallpaper
  (only the lock covers the monitors).

### Safety

- Only the wallpaper is shown: the frames come from WE's windows in the
  private stage, never from a capture of an output.
- The session unlocks only through `unlock_and_destroy` after PAM
  succeeded. If the lock process dies, the compositor keeps the session
  locked (Hyprland shows its "lock screen crashed" screen; recover from a
  TTY with `hyprctl --instance 0 'keyword misc:allow_session_lock_restore 1'`
  and `hyprctl --instance 0 dispatch exec rustpaper lock`).
- If another program already holds the session lock, `rustpaper lock`
  fails (exit 1) instead of fighting it.

## Lock adapters (`lock/adapter.rs`)

```rust
pub trait LockAdapter {
    fn name(&self) -> &'static str;
    fn pam_service(&self) -> String;
    fn theme(&self) -> Theme;                         // panel colors and font
    fn fallback_background(&self) -> Option<PathBuf>; // after the wallpaper's preview
    fn on_locked(&self);                              // what the desktop's own lock also did
    fn idle_lock_seconds(&self) -> Option<u32>;       // the daemon locks on idle
    fn enable(&self, exe: &Path) -> Result<Vec<String>, String>; // route the triggers
    fn disable(&self) -> Result<(), String>;          // give them back
    fn enabled(&self) -> bool;
}
```

`detect()` picks the adapter; `RUSTPAPER_LOCK_ADAPTER=omarchy|generic`
overrides it. A new desktop or distribution is one new adapter.

| Adapter | PAM service | Look | Triggers routed by `enable` |
|---|---|---|---|
| **Omarchy** (`omarchy.rs`) | `omarchy-lock-password` (Omarchy's own, with its faillock policy) | the theme's colors (`colors.toml`) and font (`omarchy-font-current`) | see below |
| **Generic** (`generic.rs`) | `rustpaper-lock` (installed by the packages: `auth include login`), else `login` | neutral dark | the daemon: lock before suspend and on `loginctl lock-session`. The keybinding and the idle daemon's lock command are the user's (`rustpaper lock`, or `loginctl lock-session`); `RUSTPAPER_LOCK_IDLE=<seconds>` in the daemon's environment makes it lock on idle itself |

### Omarchy

Omarchy's lock screen is a first-party plugin of its Quickshell shell, and
the shell grants the `authentication` capability only to first-party
plugins, so the live wallpaper cannot be added to Omarchy's lock from
outside. Every Omarchy lock trigger ends in `omarchy-shell lock lock`; the
adapter routes each one to `rustpaper lock` through Omarchy's own user
customization points, and `disable` reverts each:

| Trigger | Omarchy runs | Routed through |
|---|---|---|
| Super+Ctrl+L, lid closed | `omarchy-system-lock`, `omarchy-system-lid-close` | `o.rebind` in `~/.config/hypr/bindings.lua`, inside a block marked `-- rustpaper-lock-start` / `-- rustpaper-lock-end` (the lid command keeps Omarchy's logic: lock only without an external monitor, then reconcile the displays) |
| System menu > Lock | `omarchy-system-lock` | the `system.lock` id overridden in `~/.config/omarchy/extensions/omarchy-menu.jsonc` (marked block) |
| idle | the shell's idle service (`idle.lock` in `~/.config/omarchy/shell.json`) | the daemon takes the timeout over (kept in `~/.config/rustpaper/lock.json`) and sets `idle.lock` to 0; a timeout set later in Omarchy is taken over the same way |
| suspend | `omarchy-sleep-lock.service` | masked; the daemon locks before suspend |

After locking it does what `omarchy-system-lock` does: first keyboard
layout, lock 1Password, stop the screensaver.

## Using it

- Selector: **Animated lock screen** in the header (`rustpaper lock-setup
  enable|disable`).
- By hand: `rustpaper lock-setup enable|disable|status [--json]`;
  `rustpaper lock` locks now.
- The live wallpaper needs the engine running with the private stage (the
  default); otherwise the lock shows the still picture.

## Verified (2026-11-22, nested Sway 1.12 on a hidden output)

With a debug build, the generic adapter and a PAM config of our own
(`RUSTPAPER_LOCK_PAM_CONFDIR`, honored by debug builds only, with
`pam_deny` then `pam_permit`), the engine with `RUSTPAPER_NO_WE=1` and
`scripts/stage-test-window.py` standing in for WE:

- locked in 0.26 s; the lock showed the stage's frames live (the test
  window's colors kept changing behind the panel);
- typing showed one dot per character; a denied password showed "Wrong
  password" and kept the session locked; an accepted one unlocked, and the
  desktop's mirror took the frames back;
- `rustpaper lock` while locked returned 0 at once;
- engine stopped mid-lock: the last frame stayed; a new engine was picked
  up again and the frames resumed; with no frame yet, the wallpaper's
  preview showed;
- `rustpaper lock-daemon` took its sleep inhibitor (`systemd-inhibit
  --list`), found the session for `loginctl lock-session`, and locked after
  4 s idle (`RUSTPAPER_LOCK_IDLE=4`); killing the daemon left the lock
  running and released the inhibitor.

Not verified yet: the lock on a real session (only nested so far), suspend
(the `PrepareForSleep` path), the Omarchy adapter's `enable` on a real
Omarchy install (its file edits are unit-tested, `luac -p` checks the Lua),
real passwords through `omarchy-lock-password` and `rustpaper-lock`, the
cost of a 4K lock (no blur, so it should cost what the desktop mirror
costs).

## Known gaps

- No fingerprint: Omarchy's lock has a separate fingerprint PAM flow
  (`omarchy-lock-fingerprint`); ours only asks for the password.
- No key repeat in the password field (holding Backspace deletes one
  character; Ctrl+Backspace, Ctrl+U or Escape clear it).
- No blur behind the panel: a dim layer instead (blurring a live 4K
  wallpaper every frame would cost a GPU pass the lock does not have).
- The desktop stage (`--stage desktop`) feeds no frames: the still picture
  shows. Its buffers are the mirror's capture buffers, not WE's.
- One panel per monitor, all identical; no user menu, no power buttons.
