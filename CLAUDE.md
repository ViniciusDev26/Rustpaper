# Rustpaper

From-scratch Rust reimplementation of Wallpaper Engine (WE) for Linux/Wayland:
an **engine** that renders WE scene and video wallpapers on the desktop, and a
**selector** (Tauri app) to browse the library, customize and apply. It reads
the user's own WE install (Steam): no wallpaper content ships with it.

Learning project — prefer clarity and explain non-obvious WE behaviour in
comments, with the measurement that justified it.

## Rules

- **Never execute Workshop JavaScript outside a sandbox** (SceneScript, `web`
  wallpapers, property `condition`s): it is untrusted code. SceneScript runs
  only in `rustpaper-script-worker` (seccomp + rlimits applied before it
  reads any script; see `docs/SCENESCRIPT.md`) — never add host
  capabilities (files, network, processes) to it, and keep the worker's
  syscall allowlist minimal. `web` wallpapers and property `condition`s
  still use static values or native, data-driven reimplementations.
- **Everything in the repo is in English**: code, comments, logs, docs,
  commit messages.
- **Commit messages follow Conventional Commits** (`feat(gl): ...`,
  `fix(script): ...`; CI checks every commit, release-please derives versions
  from them): see `docs/RELEASING.md`.
- **Validate rendering changes with the harness**, not by eye:
  `harness/batch.sh` -> `harness-out/report.md`. PASS = mean SSIM >= 0.95
  and motion energy ratio 0.7..1.4 against the real WE.
- Gate before committing (CI runs the same): `cargo fmt --all`,
  `cargo clippy --workspace --all-targets --all-features -- -D warnings`,
  `cargo test --workspace`; UI changes also `cd crates/rustpaper-selector/ui
  && npm run typecheck && npm test`.

## Docs

- `docs/GL_RENDERER.md` — the OpenGL scene renderer and every WE behaviour
  it had to match (each with its measurement), known gaps.
- `docs/HARNESS.md` — capturing the real WE under Proton and comparing.
- `docs/SELECTOR.md` — gallery, customization panel, persistence.
- `docs/SCENESCRIPT.md` — the SceneScript sandbox, its API, where results go.
- `docs/RELEASING.md` — Conventional Commits, release-please, release
  tarball and AUR packaging (`packaging/`).
- `docs/SHADER_TRANSLATION.md` — WE shader dialect (older wgpu path, still
  relevant to `rustpaper_core::shader`).

## Workspace (`crates/`)

- **`rustpaper-core`** — pure WE formats, no GPU: `project`, `pkg` (PKGV),
  `tex` (.tex -> RGBA, embedded sprite sheets), `scene`, `particle`, `layout`,
  `effects`, `mdl` (puppet models), `shader` (dialect -> GLSL 330 +
  log-driven fixer), `paths` (Steam library, test corpus).
- **`rustpaper-gl`** — OpenGL 3.3 scene renderer (glow + EGL): layers, effects,
  particles, text, audio spectrum, puppets, bloom, parallax. Binary
  **`rustpaper-render`** (offline: PNG/MP4, `--describe`, `--dump-passes`,
  `--only`, `--screen WxH`).
- **`rustpaper-script`** — SceneScript: `ScriptHost` (spawns the worker,
  JSON over pipes) and binary **`rustpaper-script-worker`** (seccomp jail +
  QuickJS + `prelude.js`, the WE script API). Renderer side:
  `rustpaper-gl/src/scripting.rs`.
- **`rustpaper-engine`** — binary **`rustpaper`**: the live wallpaper.
  Scenes go through rustpaper-gl (`gl_wallpaper.rs`, one EGL surface per
  output, layer `Bottom`); videos use the older wgpu path (`wallpaper.rs`,
  `gpu.rs`, `video.rs`). `hyprland.rs` (fullscreen pause, cursor, no layer animations for
  namespace `rustpaper`: the compositor fade flashed the previous wallpaper),
  `audio_capture.rs` (PipeWire), `sound.rs` (plays scene `sound` objects
  with ffplay, killed with the engine via PR_SET_PDEATHSIG; `--volume`,
  `--mute`). `RUSTPAPER_LEGACY_SCENE=1` = old wgpu scenes.
- **`rustpaper-selector`** — Tauri v2 app, binary **`rustpaper-selector`**;
  UI in `ui/`: React + TypeScript + Tailwind v4 (Vite), Zustand stores,
  Zod-validated command replies (`src/api`). build.rs builds it (needs
  Node.js + npm) into `$OUT_DIR/ui`; `cd ui && npm test` for its unit tests.
- **`rustpaper-workshop`** — binary **`rustpaper-workshop`**: the selector's
  Steam Workshop store. Talks to the running Steam client through Steamworks
  as WE (AppID 431960): query, subscribe/unsubscribe, download progress;
  JSON lines with the selector (`workshop.rs` there). Links
  `libsteam_api.so` (RUNPATH `$ORIGIN:$ORIGIN/../lib/rustpaper`, copied by
  its build.rs). Runs no Workshop content.

## Build and run (host)

```sh
cargo build --release                       # toolchain pinned by mise.toml
./target/release/rustpaper-selector         # gallery; applies via the engine
./target/release/rustpaper <wallpaper dir> [--props file.json] [--fps N] [--no-audio] [--mute] [--volume 0..100] [--no-scripts] [--output NAME]
./target/release/rustpaper-render <workshop id> --describe
./scripts/install.sh                        # ~/.local/bin + launcher entry
```

- WE paths: `rustpaper_core::paths` finds the Steam library; override with
  `WE_ASSETS_DIR`, `WE_WORKSHOP_DIR`; test corpus in
  `~/.local/share/rustpaper-testset` (`RUSTPAPER_TESTSET_DIR`).
- Stop a running engine with `kill $(pgrep -x rustpaper)` — `pkill -f`
  also matches your own shell and `rustpaper-selector`.
- Only one engine at a time (two on the same layer flicker); the selector
  hands over automatically. Disable other wallpaper daemons
  (`linux-wallpaperengine`, the shell's own background on `Background`).
- Runtime deps: ffmpeg (video; ffplay for scene sounds), `pw-record` (audio, optional), Hyprland IPC
  for fullscreen pause/cursor (optional; other compositors just don't pause).

## WE formats (accumulated knowledge)

- **project.json**: `type` (`video` | `scene` | `web`, casing varies), `file`,
  `title`, `general.properties` (user properties, see SELECTOR.md). An
  "image" wallpaper is a `scene`.
- **.pkg** (`PKGV0001`): sized-string version, `u32` file count, entries
  (sized-string name, `u32` offset, `u32` length), data after the table.
- **.tex** (`TEXV0005`/`TEXI0001`): format (0 = RGBA8888 despite the name —
  never swap channels; 4/6/7 = DXT5/3/1; 8 = RG88; 9 = R8), flags, sizes.
  Containers `TEXB0001..0004` (LZ4 when compressed; 0003/0004 may embed a
  JPEG/PNG; 0004's extra per-mip header only for mp4). Strings are
  NUL-terminated (unlike .pkg). RG88/R8 stay in GL layout and shaders get
  `TEXnFORMAT`. Embedded flipbooks: `TEXS0003` = count, w, h, then exact
  per-frame records.
- **Scene chain**: `scene.json` object `image` -> `models/x.json`
  (`material`, optional `puppet`) -> `materials/x.json` (passes: shader,
  textures, combos, constants) -> `materials/<name>.tex`. Objects reference
  effects (`effects/*/effect.json`), particle systems (`particles/*.json`),
  text, parents (hierarchy).
- **Particles**: `colorrandom` uses 0-255, `colorchange` 0-1; `colorrandom`
  interpolates along one `t` (per-channel randomness gives rainbows).
- Shaders: WE's dialect (HLSL-ish macros, `#include`, `#require`,
  `[COMBO]` and uniform annotations — also inside includes).

## Testing conventions

- Unit tests inline (`#[cfg(test)] mod tests`) for pure logic (parsers,
  shader translation/fixer, simulation pieces, settings).
- GPU/Wayland/UI: verified with the harness (renderer) or by screenshotting
  on the headless `RPREF` output (UI) — never steal the user's focus.
- When unsure about an API (glow, khronos-egl, SCTK, Tauri), read the source
  in `~/.cargo/registry` instead of guessing.
