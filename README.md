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
SceneScript, audio visualizers, videos, web wallpapers); Rustpaper mirrors it
onto the desktop of each monitor and gives you a *selector* to browse your
library, **customize each wallpaper's properties**, browse the Workshop and
apply wallpapers.

Personal project for learning Rust and low-level graphics programming — not
an official product, and not affiliated with Valve or Wallpaper Engine.

---

## ⚠️ Hard requirement: you need Wallpaper Engine

This project **does not ship any wallpaper content or renderer**: it drives
your own copy of **Wallpaper Engine**, purchased on Steam.

To use it you need:

1. [Wallpaper Engine](https://store.steampowered.com/app/431960/Wallpaper_Engine/)
   purchased and installed through Steam, **running under Proton** (Steam ->
   Wallpaper Engine -> Properties -> Compatibility);
2. wallpapers subscribed from its Workshop;
3. **Hyprland 0.56+** (or Sway 1.11+, adapter not yet tested live). Each
   desktop is an adapter (`crates/rustpaper-engine/src/desktop/`); see
   [`docs/ENGINE.md`](./docs/ENGINE.md) for adding one.

---

## 🚧 Project status: beta

**Arch Linux:** each [GitHub release](https://github.com/ViniciusDev26/Rustpaper/releases)
carries prebuilt binaries and a ready `rustpaper-bin.PKGBUILD`:

```sh
mkdir rustpaper && cd rustpaper
curl -LO https://github.com/ViniciusDev26/Rustpaper/releases/latest/download/rustpaper-bin.PKGBUILD
mv rustpaper-bin.PKGBUILD PKGBUILD && makepkg -si
```

(AUR packages `rustpaper-bin` / `rustpaper` will follow; their PKGBUILDs are
in `packaging/aur/`.) **From source**, for your user:

```sh
./scripts/install.sh          # cargo build --release + ~/.local/bin + launcher entry
rustpaper-selector            # or "Rustpaper" in your app launcher
```

Needs a Rust toolchain (pinned in `mise.toml`), Node.js 20+ with npm (the
selector's UI is built by its build.rs), libgbm (mesa), `nsenter`
(util-linux), Steam with Wallpaper Engine, and Hyprland 0.56+. Wallpaper
Engine is found in your Steam library automatically (`WE_DIR` /
`WE_WORKSHOP_DIR` override it).

---

## ✨ What works

- **Every wallpaper type WE plays**: scenes, videos, web and application
  wallpapers, pixel-identical to WE because WE renders them.
- **Multi-monitor**, each at its native resolution (fractional scaling too),
  60 fps; frames go from WE's window to the desktop without leaving the GPU.
- **Customization**: the selector shows each wallpaper's properties
  (sliders, colors, toggles, dropdowns, text) like WE and sends them to WE.
- **Workshop store**: browse, search and subscribe to Workshop wallpapers
  from the selector (through your running Steam client).
- **Performance tab** like WE's: quality presets and settings (FPS,
  anti-aliasing, texture resolution, post-processing, shadows, lighting) and
  Playback rules (pause when a window is fullscreen, maximized/tiled or
  focused, when the display sleeps, on battery).
- **Desktop integration**: runs as a systemd user service ("Start on login"
  enables it), Omarchy theme from the wallpaper's colors.

Not yet: cursor interaction (eye tracking, parallax, clicks), per-wallpaper
volume (only mute), an animated lock screen (prerequisites in
[`docs/LOCKSCREEN.md`](./docs/LOCKSCREEN.md)). Workshop JavaScript (SceneScript, web wallpapers) runs
inside Wallpaper Engine as it does on Windows: **not sandboxed**.

How it works, measurements and known gaps: [`docs/ENGINE.md`](./docs/ENGINE.md),
[`docs/SELECTOR.md`](./docs/SELECTOR.md).

---

## 🧩 Architecture (summary)

Cargo workspace under `crates/`:

- **`rustpaper-core`** — where Wallpaper Engine's files live, what a
  `project.json` says.
- **`rustpaper-engine`** — the live wallpaper (`rustpaper` binary): drives
  Wallpaper Engine under Proton and mirrors its windows onto the desktop.
- **`rustpaper-selector`** — Tauri desktop app (gallery + properties +
  Workshop store).
- **`rustpaper-workshop`** — the store's bridge to the Steam client
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
