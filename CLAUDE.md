# Rustpaper

Wallpaper Engine (WE) wallpapers on Linux/Wayland. The **engine** runs the
user's own WE (Steam, under Proton) and has it render each wallpaper into a
window per monitor. Normally that window lives in the **private stage**
(our own Smithay compositor + XWayland), and its buffers go straight to a
layer-shell surface on the user's desktop. The **selector** (a Tauri app)
browses the library, customizes, tunes performance and applies. No
wallpaper content or renderer ships with it.

Goal: make WE work on Linux, on any layer-shell compositor. Fidelity comes
from WE itself; the work is the integration (Proton, Steam, the stage,
desktop adapters, the selector). Explain non-obvious behaviour in comments,
with the measurement or observation that justified it.

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
  in the private stage (or, with a desktop stage, on headless outputs); any
  focus change must restore the previous focus. Test compositors and
  windows go on a hidden output, and runtime window rules you add for a
  test must be disabled afterwards (a leftover rule once hid the selector).
- **Never compositor calls in the core**: everything desktop-specific goes
  into an adapter behind `desktop::Desktop` / `desktop::Stage`.
- Gate before committing (CI runs the same): `cargo fmt --all`,
  `cargo clippy --workspace --all-targets --all-features -- -D warnings`,
  `cargo test --workspace`; UI changes also `cd crates/rustpaper-selector/ui
  && npm run typecheck && npm test`.

## Docs

- `docs/ENGINE.md`: the architecture (core, private stage, desktop
  adapters), how the engine drives WE and Steam, every WE / Steam /
  compositor quirk it works around, measurements, known gaps.
- `docs/SELECTOR.md`: gallery, customization panel, engine service,
  Performance tab, persistence.
- `docs/LOCKSCREEN.md`: the animated lock screen: the lock, the frame
  feed, the daemon, the lock adapters (Omarchy, generic), what was
  verified.
- `docs/RELEASING.md`: Conventional Commits, release-please, release
  tarball and AUR packaging (`packaging/`).

## Workspace (`crates/`)

- **`rustpaper-core`**: shared by the engine and the selector.
  - `paths`: Steam library, WE install, Workshop dir.
  - `project`: project.json.
  - `state`: `~/.config/rustpaper/state.json`, the selector's decisions,
    which the engine reads: the wallpaper of each role (desktop per
    monitor, lock screen, screensaver) and their fallbacks.
  - `steam`: Steam's `localconfig.vdf` (KeyValues reader/writer), for WE's
    launch option.
  - `performance`: WE's quality settings and the Playback conditions as
    data.
- **`rustpaper-engine`**: binary **`rustpaper`**, the live wallpaper.
  - `main.rs`: the core, identical on every desktop. One long-lived WE
    window per monitor, the watchdog (Stage repairs, WE restarts, Playback
    rules), reload.
  - `config.rs`: CLI and `--service` mode (state.json, SIGHUP reload).
  - `desktop/`: **the only compositor-specific code**, behind the
    `Desktop` and `Stage` traits (`mod.rs`). Adapters: `hyprland.rs`
    (IPC; also re-applies the stage on `configreloaded`), `sway.rs`, and
    `generic.rs` (any layer-shell compositor, private stage only). A new
    desktop is a new adapter; never call a compositor from the core.
  - `stage.rs`: the private stage, our own Smithay compositor + XWayland
    hosting WE; its dmabufs go straight to the layer surfaces.
  - `launch.rs`: `rustpaper we-launch %command%` (WE's Steam launch option,
    points WE at the private stage) and `rustpaper setup-steam` (writes it).
  - Debugging: `RUSTPAPER_NO_WE=1` runs only the stage and the mirror (test
    with any X client titled `rustpaper-<output>`);
    `RUSTPAPER_DEBUG_POINTER=1` logs the pointer input the stage hands WE's
    windows (leaves, buttons).
  - `proton.rs`: `rustpaper we-proton enable|disable|status`, the opt-in
    for Wine's Wayland driver (GE-Proton as WE's compatibility tool,
    `wine_wayland` in state.json; launch.rs then points WE at the stage's
    Wayland socket).
  - `we.rs`: WE under Proton (`-control` commands through `nsenter` into its
    container, config.json tweaks, starting it through Steam).
  - `mirror.rs`: the layer-shell surfaces on the user's compositor. They
    show the private stage's frames (`show`), or capture WE's windows with
    ext-image-copy-capture into GBM dmabufs (desktop stages). It also reads
    the host's dmabuf formats for the stage's XWayland.
  - `gbm.rs`: minimal libgbm FFI.
  - `media.rs`: WE's media integration: installs and registers
    `rustpaper_media.dll` in WE's prefix, writes what the host's MPRIS
    players play into `~/.cache/rustpaper/media`, carries out WE's
    play/pause/next.
  - `power.rs`: on battery or not.
  - `lock/`: the animated lock screen (`rustpaper lock`, `lock-daemon`,
    `lock-setup`). `screen.rs` is the `ext-session-lock-v1` client, `ui.rs`
    its panel, `pam.rs` minimal libpam FFI, `feed.rs` the engine -> lock
    frame socket (dmabufs with `SCM_RIGHTS`), `daemon.rs` logind + idle.
    Desktop-specific lock behaviour (PAM service, look, routing the
    desktop's lock triggers) only in adapters behind `LockAdapter`
    (`adapter.rs`): `omarchy.rs`, `generic.rs`.
- **`rustpaper-we-helper`**: no_std Windows artifacts the engine runs
  inside WE's Proton prefix (Windows-only code, stubs on the host). Build
  both: `cargo build --release -p rustpaper-we-helper --target
  x86_64-pc-windows-msvc` (rustup target; linked by rust-lld,
  `.cargo/config.toml`, entry points in its build.rs).
  - `rustpaper-we-helper.exe` (`main.rs`, run by `we_helper.rs`): marks our
    WE windows as the desktop, without which web wallpapers take no clicks.
  - `rustpaper_media.dll` (`lib.rs`): Windows' media session API
    (`gsmtc.rs`) and a DataReader for its album art (`datareader.rs`, a
    stub in Wine), hand-written COM in `com.rs`. `format.rs` (built on the
    host too) is the file format shared with the engine's `media.rs`. A
    `media\trace` file next to the DLL logs every call to `media\log.txt`.
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
./target/release/rustpaper setup-steam      # WE's launch option (closes/reopens Steam), once
./target/release/rustpaper ... --stage auto|desktop|private
systemctl --user status rustpaper           # the engine as the selector runs it
journalctl --user -u rustpaper -f           # its log
./scripts/install.sh                        # ~/.local/bin + launcher entry
```

- Needs WE installed through Steam with Proton, a layer-shell compositor,
  `Xwayland`, `nsenter` (util-linux) and libgbm. The engine starts WE
  through Steam when it is not running.
- The private stage needs WE's Steam launch option (`setup-steam`); it
  points at the binary that wrote it, so rerun it after moving from a dev
  build to the installed one. The installed selector manages
  `rustpaper.service` and stops engines it did not start: stop the service
  (and leave the selector alone) while testing a dev engine.
- WE paths: `rustpaper_core::paths` finds the Steam library. Override with
  `WE_DIR` and `WE_WORKSHOP_DIR`.
- Stop a running engine with `kill $(pgrep -x rustpaper)`. **Never `pkill
  -f`**: it matches your own shell (it killed the session's shell several
  times) and `rustpaper-selector`.
- Steam refusing to start WE (`WaitingPrevProcess` -> `AppError_16` in
  `~/.local/share/Steam/logs/console_log.txt`): a previous launch left
  processes behind (Wine helpers, Chromium's `CrGpuMain`). `we::restart`
  stops them all; by hand, stop every process with `SteamAppId=431960` in
  its environment. The WE windows
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
  `hyprctl monitors -j`. The selector can open on a tab
  (`RUSTPAPER_SELECTOR_TAB=performance`) on a headless output for
  screenshots.
- Stages without WE: `RUSTPAPER_NO_WE=1` runs only the stage and the
  mirror; `scripts/stage-test-window.py rustpaper-<output> W H secs` (with
  `DISPLAY` set to the stage's display) stands in for WE.
- Other compositors: run them nested on the hidden output, e.g. `sway`
  (`WLR_BACKENDS=wayland`) or `niri`, and point the engine at them with
  `WAYLAND_DISPLAY`, `SWAYSOCK`, and without `HYPRLAND_INSTANCE_SIGNATURE`.
  The window must sit on a *visible* workspace of the headless output, or
  the nested compositor gets no frames.
- Roles/modes with WE without touching the user's choices: run the engine
  with `XDG_CONFIG_HOME=<tmp>` holding its own `rustpaper/state.json`. A
  script connecting to `$XDG_RUNTIME_DIR/rustpaper/feed-*.sock` (and
  answering `release` for each frame) stands in for the lock. Test the
  screensaver nested only: on the real session it would cover the user's
  monitors.
- Lock screen: **never lock the real session in a test** (it covers the
  user's monitors). Lock a nested compositor instead, with
  `RUSTPAPER_LOCK_ADAPTER=generic` (the Omarchy adapter's post-lock steps
  touch the real session) and, in debug builds,
  `RUSTPAPER_LOCK_PAM_CONFDIR=<dir>` holding the adapter's service file
  with `pam_permit` / `pam_deny` (`rustpaper-lock` once a package
  installed /etc/pam.d/rustpaper-lock, else `login`: write both); type
  with `WAYLAND_DISPLAY=<nested> wtype` (`-P BackSpace -s 1500 -p
  BackSpace` holds a key).
  Never run `lock-setup enable` on the user's session without asking: it
  rewires their lock triggers.
- When unsure about an API (wayland-client, Hyprland's Lua API, Tauri), read
  the source in `~/.cargo/registry` or `/usr/share/hypr/stubs` instead of
  guessing.
