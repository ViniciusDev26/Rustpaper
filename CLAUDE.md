# Rustpaper

Wallpaper Engine (WE) wallpapers on Linux/Wayland. The **engine** runs the
user's own WE (Steam, under Proton), has it render each wallpaper into a
hidden window, and mirrors those windows onto the desktop. The **selector**
(a Tauri app) browses the library, customizes and applies. No wallpaper
content or renderer ships with it.

Goal: make WE work on Linux. Fidelity comes from WE itself; the work is the
integration (Proton, Hyprland, capture, the selector). Explain non-obvious
behaviour in comments, with the measurement or observation that justified it.

## Rules

- **Workshop JavaScript runs only inside Wallpaper Engine** (SceneScript, `web`
  wallpapers), as it does on Windows. Never execute it in Rustpaper's own
  processes. Property `condition`s in the selector stay parsed, never
  evaluated as code.
- **Everything in the repo is in English**: code, comments, logs, docs,
  commit messages.
- **Commit messages follow Conventional Commits** (`feat(engine): ...`,
  `fix(selector): ...`; CI checks every commit, release-please derives
  versions from them): see `docs/RELEASING.md`.
- **Never steal the user's focus or cover their monitors**: WE's windows live
  on headless outputs; any focus change must restore the previous focus.
- Gate before committing (CI runs the same): `cargo fmt --all`,
  `cargo clippy --workspace --all-targets --all-features -- -D warnings`,
  `cargo test --workspace`; UI changes also `cd crates/rustpaper-selector/ui
  && npm run typecheck && npm test`.

## Docs

- `docs/ENGINE.md`: how the engine drives WE and mirrors it, every Hyprland
  and WE quirk it works around, measurements, known gaps.
- `docs/SELECTOR.md`: gallery, customization panel, engine service,
  Performance tab, persistence.
- `docs/LOCKSCREEN.md`: animated lock screen, the routes and their
  prerequisites (not implemented yet).
- `docs/RELEASING.md`: Conventional Commits, release-please, release
  tarball and AUR packaging (`packaging/`).

## Workspace (`crates/`)

- **`rustpaper-core`**: shared by the engine and the selector.
  - `paths`: Steam library, WE install, Workshop dir.
  - `project`: project.json.
  - `state`: `~/.config/rustpaper/state.json`, the selector's decisions,
    which the engine reads.
  - `performance`: WE's quality settings and the Playback conditions as
    data.
- **`rustpaper-engine`**: binary **`rustpaper`**, the live wallpaper.
  - `main.rs`: one headless output + workspace + WE window per monitor, the
    watchdog (arrangement and Playback rules), `--service` mode (state.json,
    SIGHUP reload).
  - `we.rs`: WE under Proton (`-control` commands through `nsenter` into its
    container, config.json tweaks, starting it through Steam).
  - `mirror.rs`: ext-image-copy-capture of WE's windows into GBM dmabufs,
    shown on layer-shell surfaces.
  - `gbm.rs`: minimal libgbm FFI.
  - `hyprland.rs`: IPC socket client (also what covers each monitor).
  - `power.rs`: on battery or not.
- **`rustpaper-selector`**: Tauri v2 app, binary **`rustpaper-selector`**.
  The UI lives in `ui/`: React + TypeScript + Tailwind v4 (Vite), Zustand
  stores, Zod-validated command replies (`src/api`). build.rs builds it
  (needs Node.js + npm) into `$OUT_DIR/ui`; run `cd ui && npm test` for its
  unit tests. Undecorated under Hyprland, colors from the active Omarchy
  theme (`src/omarchy.rs`, see SELECTOR.md). Runs the engine as the systemd
  user service `rustpaper.service` (`src/service.rs`): every change saves
  state.json and reloads the service; "Start on login" enables it.
- **`rustpaper-workshop`**: binary **`rustpaper-workshop`**, the selector's
  Steam Workshop store.
  - Talks to the running Steam client through Steamworks as WE (AppID
    431960): query, subscribe/unsubscribe, download progress.
  - Exchanges JSON lines with the selector (`workshop.rs` there).
  - Links `libsteam_api.so` (RUNPATH `$ORIGIN:$ORIGIN/../lib/rustpaper`,
    copied by its build.rs).

## Build and run (host)

```sh
cargo build --release                       # toolchain pinned by mise.toml
./target/release/rustpaper-selector         # gallery; applies via the engine
./target/release/rustpaper <wallpaper dir> [--props file.json] [--mute] [--volume 0..100] [--output NAME]
./target/release/rustpaper --service        # what the selector chose (state.json); SIGHUP reloads
systemctl --user status rustpaper           # the engine as the selector runs it
journalctl --user -u rustpaper -f           # its log
./scripts/install.sh                        # ~/.local/bin + launcher entry
```

- Needs WE installed through Steam with Proton, Hyprland 0.56+, `nsenter`
  (util-linux) and libgbm. The engine starts WE through Steam when it is not
  running.
- WE paths: `rustpaper_core::paths` finds the Steam library. Override with
  `WE_DIR` and `WE_WORKSHOP_DIR`.
- Stop a running engine with `kill $(pgrep -x rustpaper)`. `pkill -f` also
  matches your own shell and `rustpaper-selector`. The WE windows
  (`rustpaper-<output>`) and headless outputs (`RP-<output>`) are long-lived
  and shared by engine instances; never open or close WE windows per switch
  (docs/ENGINE.md explains why).
- Run only one engine at a time; the selector hands over automatically.
  Disable other wallpaper daemons (`linux-wallpaperengine`, the shell's own
  background).

## Testing conventions

- Unit tests inline (`#[cfg(test)] mod tests`) for pure logic (settings,
  parsing, naming).
- Wayland/WE/UI: verify live by screenshotting with `grim -o <output>`
  (headless outputs too) and checking `hyprctl clients -j` /
  `hyprctl monitors -j`.
- When unsure about an API (wayland-client, Hyprland's Lua API, Tauri), read
  the source in `~/.cargo/registry` or `/usr/share/hypr/stubs` instead of
  guessing.
