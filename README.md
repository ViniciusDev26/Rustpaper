<p align="center">
  <img src="crates/rustpaper-selector/icons/icon.png" width="140" alt="Rustpaper icon">
</p>

<h1 align="center">Rustpaper</h1>
<p align="center"><i>Wallpaper Engine, rebuilt from scratch in Rust — for your Linux box.</i></p>

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

**Rustpaper** is a from-scratch reimplementation, in Rust, of a **Wallpaper
Engine for Linux**: an *engine* that renders Wallpaper Engine (Steam)
**scenes** with OpenGL — through the wallpapers' own shaders — and video
wallpapers, straight onto the Wayland desktop (wlr-layer-shell, one surface
per monitor), plus a *selector* to browse your library, **customize each
wallpaper's properties like Wallpaper Engine does**, and apply it.

Personal project for learning Rust and low-level graphics programming — not
an official product, and not affiliated with Valve or Wallpaper Engine.

---

## ⚠️ Hard requirement: you need Wallpaper Engine

This project **does not ship any wallpaper content**. It only reads and
renders the files (`project.json`, `.pkg`, `.tex`, `scene.json`, etc.) that
already exist in your local **Wallpaper Engine** library, purchased on Steam.

In other words, to use it you need:

1. [Wallpaper Engine](https://store.steampowered.com/app/431960/Wallpaper_Engine/)
   purchased and installed (via Steam);
2. Downloaded/subscribed wallpapers from its Workshop — which creates the
   content folders (`project.json` + assets) this engine knows how to read.

Without that, there's nothing for this project to render.

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

Needs a Rust toolchain (pinned in `mise.toml`), OpenGL/EGL, Wayland with
wlr-layer-shell (Hyprland, Sway, KDE...), `ffmpeg` for video wallpapers and,
optionally, PipeWire's `pw-record` for audio-reactive ones. Wallpaper
Engine's files are found in your Steam library automatically
(`WE_ASSETS_DIR` / `WE_WORKSHOP_DIR` override it).

---

## ✨ What works

Fidelity is **measured against the real Wallpaper Engine** (running under
Proton) by an automated harness, not judged by eye: every scene in the
library is captured from WE and rendered by Rustpaper, and compared on its
average image (SSIM) and on how much it moves
([`docs/HARNESS.md`](./docs/HARNESS.md)).

- **Scenes**: layers, blend modes, effects (multi-pass, named render
  targets, masks), particle systems (emitters, operators, sprite sheets,
  trails, ropes, child systems), text layers (clocks/dates), puppet warp
  (skeletal animation), audio-reactive spectrum, bloom/HDR, camera parallax.
- **Videos**: loop, cover, multi-monitor.
- **Customization**: the selector shows each wallpaper's properties
  (sliders, colors, toggles, dropdowns, text) like WE; changes are saved
  per wallpaper and applied to the running wallpaper within a few seconds.
- **Workshop store**: browse, search and subscribe to Workshop wallpapers
  from the selector (through your running Steam client; Steam downloads
  them, the gallery picks them up).
- **Desktop integration**: physical-resolution rendering under fractional
  scaling, pauses while a fullscreen window covers the monitor (Hyprland),
  autostart.

**SceneScript** (the author's JavaScript) runs in a separate, locked-down
process (seccomp allowlist, no files/network/exec, memory and time limits):
see [`docs/SCENESCRIPT.md`](./docs/SCENESCRIPT.md). `--no-scripts` (or
**Run scripts** off in the selector) keeps scripted properties at their
static values. Not supported yet: **web
wallpapers**.

Details and known gaps: [`docs/GL_RENDERER.md`](./docs/GL_RENDERER.md),
[`docs/SELECTOR.md`](./docs/SELECTOR.md).

---

## 🧩 Architecture (summary)

Cargo workspace under `crates/`:

- **`rustpaper-core`** — Wallpaper Engine formats (project, pkg, tex,
  scene, particles, effects, puppet models, shader dialect), no GPU/UI.
- **`rustpaper-gl`** — the OpenGL scene renderer (+ `rustpaper-render`, an
  offline renderer to PNG/MP4 used by the harness).
- **`rustpaper-engine`** — the live wallpaper: `rustpaper` binary.
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
