<p align="center">
  <img src="crates/rustpaper-selector/icons/icon.png" width="140" alt="Rustpaper icon">
</p>

<h1 align="center">Rustpaper</h1>
<p align="center"><i>Your Wallpaper Engine wallpapers on your Linux desktop.</i></p>

<p align="center">
  <a href="https://github.com/ViniciusDev26/Rustpaper/actions/workflows/ci.yml">
    <img alt="CI" src="https://github.com/ViniciusDev26/Rustpaper/actions/workflows/ci.yml/badge.svg">
  </a>
  <img alt="Rust" src="https://img.shields.io/badge/rust-stable-orange?logo=rust">
  <img alt="Platform" src="https://img.shields.io/badge/platform-linux%20%2F%20wayland-blue">
  <img alt="Status" src="https://img.shields.io/badge/status-beta-yellow">
  <a href="./LICENSE"><img alt="License" src="https://img.shields.io/badge/license-MIT-green"></a>
</p>

---

**Rustpaper** puts **Wallpaper Engine** (Steam) wallpapers on the Wayland
desktop. The wallpaper is rendered by the **real Wallpaper Engine**, running
under Proton, so it looks and behaves exactly like on Windows (scenes,
SceneScript, audio visualizers, videos, web wallpapers). Rustpaper hosts WE's
windows in a compositor of its own and puts their frames on each monitor's
desktop, and gives you a *selector* to browse your library, **customize each
wallpaper's properties**, browse the Workshop and tune performance.

Personal project, not an official product, and not affiliated with Valve or
Wallpaper Engine.

---

## ⚠️ Requirements

This project **does not ship any wallpaper content or renderer**: it drives
your own copy of **Wallpaper Engine**, purchased on Steam.

- [Wallpaper Engine](https://store.steampowered.com/app/431960/Wallpaper_Engine/)
  installed through Steam, **running under Proton** (Steam -> Wallpaper
  Engine -> Properties -> Compatibility), and some wallpapers subscribed
  from its Workshop.
- A Wayland compositor with **layer-shell**:

  | Desktop | Status |
  |---|---|
  | Hyprland 0.56+ | in daily use, with its own adapter (Playback rules know what covers each monitor) |
  | Sway 1.11+ | its own adapter; verified on a nested Sway with a test window, not yet with WE on a real session |
  | niri, KDE, other layer-shell compositors | generic adapter (the Playback rules cannot pause there: it cannot see what covers a monitor); verified on a nested niri 26.04 with a test window; KDE not tried |
  | GNOME | not supported (no layer-shell) |

- `Xwayland`, libgbm (mesa) and `nsenter` (util-linux): already there on
  most desktops.
- Optional: `mpv`, for video wallpapers (below).

---

## 🚀 Install and first run

1. **Wallpaper Engine**: install it in Steam, force Proton for it, and
   launch it once to get past its first-run dialog.
2. **Rustpaper** (Arch Linux): from Rustpaper's own pacman repository,
   so `pacman -Syu` keeps it up to date. Once:

   ```sh
   # the repository's signing key (its fingerprint is in the notes of the
   # pacman-repo release: compare it before signing it locally)
   curl -sL https://github.com/ViniciusDev26/Rustpaper/releases/download/pacman-repo/rustpaper.gpg -o /tmp/rustpaper.gpg
   sudo pacman-key --add /tmp/rustpaper.gpg
   sudo pacman-key --lsign-key <fingerprint>
   printf '\n[rustpaper]\nServer = https://github.com/ViniciusDev26/Rustpaper/releases/download/pacman-repo\n' \
     | sudo tee -a /etc/pacman.conf
   sudo pacman -Sy rustpaper-bin
   ```

   Updates then come with the system's: `sudo pacman -Syu`, or on Omarchy
   `omarchy update` (Omarchy refuses direct `pacman -Syu` system upgrades;
   installing with `pacman -Sy rustpaper-bin` is fine).

   Or without the repository (no automatic updates): each
   [GitHub release](https://github.com/ViniciusDev26/Rustpaper/releases)
   carries a ready `rustpaper-bin.PKGBUILD`:

   ```sh
   mkdir rustpaper && cd rustpaper
   curl -LO https://github.com/ViniciusDev26/Rustpaper/releases/latest/download/rustpaper-bin.PKGBUILD
   mv rustpaper-bin.PKGBUILD PKGBUILD && makepkg -si
   ```

   (makepkg downloads the release tarball itself. AUR packages
   `rustpaper-bin` / `rustpaper` will follow once the AUR takes new
   accounts again; their PKGBUILDs are in `packaging/aur/`.)

   **From source**, for your user: `./scripts/install.sh` (needs a Rust
   toolchain, pinned in `mise.toml`, and Node.js 20+ with npm for the
   selector's UI; with rustup it also builds the small Windows helper that
   lets web wallpapers take clicks).
3. **Let Rustpaper host Wallpaper Engine** (once):

   ```sh
   rustpaper setup-steam
   ```

   It sets WE's Steam launch option to `"…/rustpaper" we-launch %command%`,
   which points WE at Rustpaper's own compositor while the wallpaper runs
   and changes nothing otherwise. Steam rewrites its config when it exits,
   so the command closes Steam for a few seconds, writes the option (with a
   backup) and starts Steam again.
4. Open **Rustpaper** from your app launcher, click a wallpaper, and tick
   **Start on login**. The first wallpaper takes 15-20 s while Steam starts
   WE.

Disable other wallpaper daemons first (`linux-wallpaperengine`, swaybg...).
WE's folders are found in your Steam library (`WE_DIR` / `WE_WORKSHOP_DIR`
override them).

---

## ✨ What works

- **Every wallpaper type WE plays**: scenes, videos, web and application
  wallpapers, pixel-identical to WE because WE renders them (video
  wallpapers play in mpv with hardware decoding when it is installed: the
  same video for a fraction of the CPU).
- **Multi-monitor**, each at its native resolution (fractional scaling too),
  60 fps. WE's frames go from its GPU buffers to your desktop without a
  copy.
- **Customization**: the selector shows each wallpaper's properties
  (sliders, colors with suggestions taken from the wallpaper, toggles,
  dropdowns, text) like WE, and sends them to WE live.
- **Workshop store**: browse, search and subscribe from the selector
  (through your running Steam client).
- **Performance tab** like WE's:
  - quality presets and settings (FPS, anti-aliasing, texture resolution,
    post-processing, shadows, lighting);
  - Playback rules: pause when a window is fullscreen, maximized/tiled or
    focused, when the display sleeps, or on battery.
- **Desktop integration**: runs as a systemd user service ("Start on
  login"), changes apply in place without flashing, Omarchy theme from the
  wallpaper's colors.
- **A wallpaper per monitor, per role**: each monitor's desktop, the lock
  screen and a screensaver (each monitor's own) can each show a different
  wallpaper ("Apply to" in the selector). The screensaver covers the windows
  after a delay without input and replaces Omarchy's.
- **Animated lock screen**: the live wallpaper behind the password prompt,
  on any compositor with `ext-session-lock-v1`. One switch in the selector
  routes your desktop's lock key, idle lock and lock before suspend to it
  (Omarchy fully; elsewhere logind's `loginctl lock-session` and suspend,
  plus `rustpaper lock` for your own keybinding). See
  [`docs/LOCKSCREEN.md`](./docs/LOCKSCREEN.md).

Wallpapers that follow the cursor (parallax, eye tracking) or take clicks
react to the mouse wherever no window covers the desktop, as on Windows.

**Not yet:**
- cursor interaction with a desktop stage (the private stage, the default,
  has it);
- per-wallpaper volume (one volume for all);
- fingerprint unlock on the lock screen (password only).

Workshop JavaScript (SceneScript, web wallpapers) runs inside Wallpaper
Engine as it does on Windows: **not sandboxed**.

How it works, measurements and known gaps:
[`docs/ENGINE.md`](./docs/ENGINE.md), [`docs/SELECTOR.md`](./docs/SELECTOR.md).

---

## ⚡ Maximizing performance

- **Video wallpapers: install mpv.** Under Proton, Wallpaper Engine decodes
  videos on the CPU (a 1080p video took 22.7% of a core). With `mpv`
  installed, Rustpaper plays video wallpapers in mpv with your GPU's video
  decoder instead (1.9% for the same video), on its private stage; scenes
  and web wallpapers stay on Wallpaper Engine, and Wallpaper Engine pauses
  when no monitor needs it. Performance tab -> "Play videos with" switches
  back to Wallpaper Engine.
- **Pause when covered**: the Playback rules (Performance tab) pause a
  monitor's wallpaper under fullscreen or tiled windows.
- **Lower FPS** in the Quality settings: Wallpaper Engine renders scenes at
  that rate.
- **Same wallpaper on several monitors** renders once (monitors with the
  same aspect ratio share the largest one's render): nothing to set.
- **Wine's Wayland driver (advanced, opt-in).** With Steam's own Proton,
  Wallpaper Engine runs on Wine's X11 driver and an XWayland copies every
  frame. With GE-Proton it can use Wine's Wayland driver instead, with no
  XWayland work: measured with a 4K and a 1080p monitor, the CPU went from
  17.1% to 4.7% of a core and the graphics card from 33.0 W to 31.7 W; on
  NVIDIA the GPU load stayed the same (its Vulkan driver does the copy
  instead). Turn it on in the Performance tab ("Wine's Wayland driver") or
  with `rustpaper we-proton enable`. It uses the newest GE-Proton in
  Steam's `compatibilitytools.d` or downloads the latest one (~600 MB,
  checksum verified), makes it Wallpaper Engine's compatibility tool
  (Steam closes and reopens) and restarts Wallpaper Engine. The wallpaper
  pauses meanwhile: about 2 minutes the first time (Steam updates Wallpaper
  Engine's Wine prefix for GE-Proton), under a minute on later switches.
  `rustpaper we-proton disable` gives Wallpaper Engine back Steam's Proton.

---

## 🧩 Architecture (summary)

```
 Steam ──▶ Wallpaper Engine (Proton) ──X11──▶ private stage: Rustpaper's own compositor + XWayland
                                                     │ WE's GPU buffers (dmabuf), no copy
 selector ──state.json──▶ engine (systemd service) ──┴──▶ layer-shell surface on each monitor
                              │
                              └── desktop adapter: monitors, what covers them (Hyprland, Sway, generic)
```

Cargo workspace under `crates/`:

- **`rustpaper-core`**: what the engine and the selector share. Steam/WE
  paths, project.json, the selector's `state.json`, WE's quality settings
  as data, Steam's `localconfig.vdf`.
- **`rustpaper-engine`**: the live wallpaper (`rustpaper` binary). It drives
  Wallpaper Engine, hosts it in the private stage, and has desktop adapters
  for each compositor.
- **`rustpaper-selector`**: Tauri desktop app (gallery, properties,
  Workshop store, Performance tab).
- **`rustpaper-workshop`**: the store's bridge to the Steam client
  (Steamworks).

Contributor notes: [`CLAUDE.md`](./CLAUDE.md).

---

## 📜 License

Rustpaper is free and open-source software, licensed under the
[MIT License](./LICENSE).

## Legal notice

This is an independent, unofficial project made for personal use and
learning purposes. It has no affiliation with Valve or the Wallpaper Engine
developers. It is designed to operate only on content you already obtained
legitimately through the official Wallpaper Engine.
