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

**Rustpaper** (repo `wallpaper-engine-rs`) is a from-scratch reimplementation,
in Rust, of a **Wallpaper Engine for Linux**: an *engine* that renders video
wallpapers and Wallpaper Engine (Steam) **scenes** on the GPU (Vulkan, via
[wgpu](https://wgpu.rs)) straight onto the desktop background
(Wayland/KWin), plus a *selector* (desktop app) to browse your library and
apply a wallpaper with one click.

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

## 🚧 Project status: beta, no releases

The project is under **active development and has no downloadable
builds/binaries yet**. For now, the only way to use it is to build the
source yourself (see `CLAUDE.md` for the build steps, which currently
depend on a devcontainer with Rust, Vulkan, and `glslangValidator`).

The plan is to eventually ship ready-to-install packages (e.g. AppImage/deb)
so you don't need to compile anything — but that only makes sense once
rendering fidelity (effects, particles) is more mature. Until then,
**there are no releases** of this project.

---

## ✨ What already works (fidelity)

- **Video**: faithful — loop, cover, multi-monitor.
- **Scenes (layers)**: faithful — multi-layer compositing with correct
  transform, blend, alpha, and sRGB.
- **Particles**: good approximation (sprite sheet, scale, dispersion), but
  the simulator is still simple (some operators and emitter shapes are
  missing).
- **Post-processing effects/animations**: still disabled — the biggest gap
  today, so scenes that rely on animated effects appear static.

No support for audio, music, or widgets (clock, etc.) — out of scope.

---

## 🧩 Architecture (summary)

Cargo workspace with 3 crates under `crates/`:

- **`rustpaper-core`** — parsing of Wallpaper Engine formats (project, pkg,
  tex, scene, particles, layout, effects, shaders), with no GPU/UI
  dependency.
- **`rustpaper-engine`** — the actual renderer: `rustpaper` binary, using
  wgpu/Vulkan + wlr-layer-shell (Wayland) to draw the wallpaper onto the
  desktop background.
- **`rustpaper-selector`** — desktop app (Tauri) to browse the catalog of
  already-downloaded wallpapers and apply one (spawns `rustpaper` as a
  child process).

Build details, supported formats, and architecture decisions are in
[`CLAUDE.md`](./CLAUDE.md).

---

## 📜 License

Rustpaper is free and open-source software, licensed under the
[MIT License](./LICENSE).

## Legal notice

This is an independent, unofficial project made for personal use and
learning purposes. It has no affiliation with Valve or the Wallpaper Engine
developers. It is designed to operate only on content you already obtained
legitimately through the official Wallpaper Engine.
