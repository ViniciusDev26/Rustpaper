# Selector (`crates/rustpaper-selector`)

Tauri v2 desktop app: a gallery of the installed wallpapers and a
customization panel like Wallpaper Engine's "Properties" column.

```
cargo build --release -p rustpaper-selector -p rustpaper-engine
./target/release/rustpaper-selector
```

## UI (`ui/`)

React 19 + TypeScript + Tailwind CSS v4, bundled by Vite; state in Zustand
stores, every command reply validated with Zod.

- `src/api/schemas.ts`: Zod schemas mirroring the Rust replies
  (`WallpaperEntry`, `Settings`, `PropertiesView`, the Workshop protocol);
  `src/api/tauri.ts`: typed wrappers of the commands, which reject with
  the Rust error string or on a reply that doesn't match its schema.
- `src/store/`: `library` (wallpapers, selection, search, order,
  unsubscribe), `settings` (volume, autostart, theme from wallpaper,
  animated lock screen),
  `workshop` (queries, subscriptions, download polling), `toast`.
- `src/components/`: `LibraryView`, `PropertiesPanel` + `PropertyControl`
  (one control per WE property type), `WorkshopView`, `PerformanceView`,
  shared pieces in `common.tsx`.
- `src/lib/properties.ts`: labels, color conversion and the `condition`
  parser, pure and unit-tested (`npm test`, vitest). Author texts are
  rendered as React text, never HTML; conditions are parsed, never
  evaluated as JavaScript.

Build: the selector's `build.rs` runs `npm ci` (only when `ui/node_modules`
is missing), `npm run typecheck` and `vite build` into `$OUT_DIR/ui`, and
points Tauri's `frontendDist` there through `TAURI_CONFIG` (merged over
tauri.conf.json at compile time). A build output dir, not `ui/dist`:
watching a folder the script writes would make cargo rerun it on every
build. `RUSTPAPER_UI_DIST=<dir>` uses a prebuilt bundle. Working on the UI
alone: `cd ui && npm run dev` (the Tauri commands need the real app; the
page loads, but the API calls fail outside it).

## Desktop look (Hyprland / Omarchy)

- **No title bar under Hyprland**, like Omarchy's own apps: tauri.conf.json
  opens the window with `decorations: false`, so Hyprland's border,
  rounding, shadow and opacity apply and the UI header is the top of the
  window (`data-tauri-drag-region` on its empty parts drags a floating
  window; capability `core:window:allow-start-dragging`). Outside Hyprland
  (no `HYPRLAND_INSTANCE_SIGNATURE`) setup turns the toolkit decorations
  back on, so GNOME & co. keep their title bar and buttons.
- Window class `rustpaper-selector`, title `Rustpaper`: what Hyprland
  window rules match, e.g. in Lua
  `o.window({ class = "^rustpaper-selector$" }, { float = true })`.
- **Colors follow the active Omarchy theme** (`src/omarchy.rs`): it reads
  `$XDG_STATE_HOME/omarchy/current/theme/colors.toml` (Omarchy's semantic
  palette + `mode`), keeps only hex colors (they become CSS custom
  properties), and polls `current/theme.generation` every 2 s to emit
  `theme-changed` on a switch. The UI maps the palette onto its tokens
  (`ui/src/lib/theme.ts`; colors Omarchy has no match for are mixed from
  background/foreground, so light themes stay readable) and falls back to
  the built-in Catppuccin palette outside Omarchy.

## Desktop theme from the wallpaper (like Aether)

With **Theme from wallpaper** on (header, Omarchy only, off by default), every
applied wallpaper also themes the whole desktop; **Use as theme** in the
panel does it once.

- `src/palette.rs` (pure, unit-tested): the preview image (downscaled to
  160 px) -> dominant colors by k-means in OKLab -> an Omarchy v4 palette in
  OKLCH. Backgrounds take the darkest significant color's hue (chroma kept
  subtle), foregrounds the same tint at high lightness; the accent is the
  most vivid color (chroma² × √share, so a small vivid spot beats a large
  dull area); each ANSI color takes the image's hue when a vivid color lies
  within 35° of the canonical one (pulled halfway back so red stays red),
  else the canonical hue, at a lightness that keeps it readable (tests
  check WCAG contrast > 7 for text, > 4.5 for accent/ANSI). Light theme
  only for mostly very light images. Out-of-gamut colors lose chroma, not
  hue. Icon theme: the Yaru variant nearest the accent's hue.
- `src/omarchy.rs`: writes `~/.config/omarchy/themes/rustpaper/`
  (colors.toml with Aether's keys, icons.theme, the preview as background
  and theme preview, GIF previews converted to PNG) and runs
  `omarchy-theme-set rustpaper`. Omarchy itself then generates every app's
  config from colors.toml (verified: hyprland.lua's border took the
  accent) and sets the background, shown under the live wallpaper. One
  folder rewritten each time, like Aether's `aether`; a `rustpaper` folder
  whose colors.toml lacks Rustpaper's marker line is never overwritten.
- The selector's own colors follow (see above), since they come from the
  active Omarchy theme.

The engine binary is the one next to the selector
(`target/release/rustpaper`, or the install prefix), else `rustpaper` from
`PATH`; it runs as a systemd user service (see "Engine service" below).

## Gallery

- Lists every folder with a `project.json` in the Steam Workshop folder and the
  test corpus (`rustpaper_core::paths::wallpaper_dirs`), sorted by title, with
  a search box. Previews are served through Tauri's asset protocol; the
  allowed scope is set at startup to exactly those folders.
- Clicking a card applies it (desktop, every monitor) and opens its panel.
  Every type is supported: the real Wallpaper Engine plays it
  (docs/ENGINE.md).
- Each card carries where the wallpaper is used ("Desktop · DP-1", "Lock
  screen", "Screensaver"...).

## Where each wallpaper shows (desktop, lock screen, screensaver)

The panel's **Use for…** menu puts the selected wallpaper:
- on the **desktop** of every monitor (what Apply and a card click do), or
  of one monitor;
- on the **lock screen** (docs/LOCKSCREEN.md);
- as the **screensaver** of every monitor, or of one.

Per-monitor entries appear with more than one monitor; the names come from
`rustpaper outputs` (the engine's desktop adapter). Below the menu, the
places it is used, each removable with ×: the fallback then shows. Each
role falls back to the next: a monitor's screensaver -> the screensaver for
all -> that monitor's desktop -> the desktop for all; the lock screen ->
the desktop. The desktop for all cannot be removed: everything falls back
to it. Pure helpers in `ui/src/lib/roles.ts` (unit-tested), store
`ui/src/store/roles.ts`.

The engine shows them by mode (docs/ENGINE.md, "What each monitor shows"):
the desktop's; while idle the screensaver's, raised over every window;
while the lock screen shows the frames, the lock's. A window is reloaded
only when its wallpaper changes.

**Screensaver** (Performance tab): on/off and the delay in minutes
(default 2.5, Omarchy's). Picking a screensaver wallpaper turns it on. On
Omarchy, while it is on, Omarchy's own screensaver is off (shell.json
`idle.screensaver` set to 0, restored when it is turned off; the lock
adapter does it, docs/LOCKSCREEN.md).

## Customization panel

Built from `general.properties` of project.json, in WE's `order`:

| WE type | control | stored value |
|---|---|---|
| `slider` | range (min/max/step; integer unless `fraction`) | number |
| `bool` | checkbox | bool |
| `color` | picker + hex field + default + colors from the wallpaper | `"r g b"`, floats 0..1 (WE's format) |
| `combo` | dropdown of `options[].label` | the option's `value` (type kept) |
| `textinput`, `text` | text field | string |
| `group` | section header | — |
| `file`, `directory` | shown, "not supported yet" | — |
| no type | author's note / separator, as plain text | — |

- **`condition`** (e.g. `background_enable.value && background_type.value == 3`)
  hides controls exactly like WE. It is evaluated by a small parser in
  `ui/src/lib/properties.ts` (`|| && ! == != < > <= >=`, parentheses, literals,
  `name.value`) — **never** with `eval`/`Function`; Workshop text is never
  executed. Unparseable conditions show the control.
- Labels: WE localization keys (`ui_browse_properties_scheme_color`) map to
  English names; author HTML in labels is reduced to plain text through
  `DOMParser` (no markup is inserted into the page).
- Changes are saved (debounced 0.7 s) to
  `~/.config/rustpaper/properties/<id>.json` — only values that differ from
  the defaults; **Reset** deletes the file.
- If that wallpaper is on screen, the engine reloads (SIGHUP) and hands the
  values to WE (`-control applyProperties`) in place: no restart, no flash.
- **Color properties** (WE's "Scheme color" above all): users saw a lone
  color picker and did not know what it changes. The control shows the
  current color and its hex code, a picker, the author's default (one
  click to go back), and up to 8 colors taken from the wallpaper's preview
  (`wallpaper_colors`: k-means in OKLab, palette.rs; near-duplicates closer
  than 70/765 merged). Scheme color also gets a sentence explaining it: the
  wallpaper's theme color, which the parts of the scene its author linked
  to it take.

## Persistence

- `~/.config/rustpaper/state.json` (`rustpaper_core::state`): the
  wallpaper of each role (`last_id` = the desktop for every monitor,
  `monitors`, `lock_id`, `screensaver`), volume, "Theme from wallpaper", the Performance tab's quality
  settings and Playback rules, `video_player` (`mpv` or `we`), and `stage`
  (`auto`, `desktop` or `private`;
  no UI yet, docs/ENGINE.md). The selector writes it; the engine
  (`rustpaper --service`) reads it and re-reads it on reload. The panel
  reopens the last wallpaper at startup. Unknown fields of older files
  (`autostart`, `no_scripts`) are ignored.
- `~/.config/rustpaper/properties/<id>.json`: per-wallpaper property values.

## Engine service

- The engine is the systemd user service `rustpaper.service`
  (`src/service.rs`). The selector writes the unit
  (`~/.config/systemd/user/rustpaper.service`, `ExecStart=<engine> --service`,
  `ExecReload=kill -HUP`, `PartOf=`/`WantedBy=graphical-session.target`) and
  rewrites it when the engine moved.
- Every change (apply, properties, volume, quality, playback) saves
  state.json and runs `systemctl --user reload-or-restart rustpaper.service`.
  - A running engine reloads in place (new wallpaper loaded into the same WE
    windows): the desktop never shows the plain background.
  - A stopped one starts.
  - Engines systemd does not run (an older selector's, started by hand) are
    stopped with SIGTERM 2.5 s later.
- **Start on login** = `systemctl --user enable/disable rustpaper.service`;
  the checkbox shows `is-enabled`. The old
  `~/.config/autostart/rustpaper.desktop` is removed when the unit is
  installed.
- WE's Steam launch option for the private stage is set with
  `rustpaper setup-steam` (no button yet). The engine binary the service
  runs and the one that option names should be the same install: after
  moving from a dev build to a package, run `setup-steam` again.
- Without a systemd user manager the selector falls back to its own child
  engine (`rustpaper --service`, handed over after 2.5 s), and "Start on
  login" reports an error.
- Closing the selector leaves the wallpaper running (systemd owns it).
- **Animated lock screen** (header checkbox) = `rustpaper lock-setup
  enable|disable`: the engine's lock adapter routes the desktop's lock
  triggers to Rustpaper's lock screen (Omarchy: lock key, lid, menu, idle,
  suspend), or gives them back. The checkbox shows `lock-setup status
  --json`; the toast shows what the engine says (on desktops without an
  Omarchy-like setup, what to point at `rustpaper lock`).
  docs/LOCKSCREEN.md has the rest.
- **Volume** (header slider, 0..100, default 100): 0 mutes WE and anything
  else unmutes it, because Wallpaper Engine has no volume command (WE's own
  volume setting applies). Applied on release of the slider.

## Performance tab

Like WE's own Performance settings, drawn from data
(`rustpaper_core::performance`): adding a WE setting is one table entry, no
UI code.

- **Quality.** WE's settings with WE's own keys and values, read from its UI
  bundle:
  - FPS 10..60;
  - anti-aliasing none / MSAA x2 / x4 / x8;
  - texture resolution full / half / auto;
  - post-processing disabled / enabled / ultra;
  - reflections;
  - shadows and volumetric lighting disabled..ultra;
  - WE's presets Low / Medium / High / Ultra, with "Custom" shown when no
    preset matches.

  Saved to state.json (debounced 0.9 s). The engine then stops WE, writes
  the values into WE's config.json and starts it again: written while WE
  runs, WE would overwrite them on exit. Shown values: the saved choice,
  else WE's config, else WE's defaults.
- **Playback.** WE's conditions:
  - other application focused;
  - other application maximized or tiled;
  - other application fullscreen;
  - display asleep;
  - on battery.

  Each one is Keep running or Pause. The engine evaluates them per monitor
  from Hyprland (and `/sys/class/power_supply`), because WE's own copies are
  forced to "run" (every wallpaper window is a fullscreen app to WE).
  Defaults: pause under fullscreen windows and when the display sleeps, as
  WE does.
- **Video wallpapers.** "Play videos with": mpv (hardware decoding, the
  default) or Wallpaper Engine (`state.json` `video_player`,
  `set_video_player`). WE decodes videos on the CPU under Proton (22.7% of
  a core for a 1080p24 video, mpv 1.9%: docs/ENGINE.md, "Video wallpapers
  in mpv"). The hint says when mpv is not installed (`mpv_installed`, from
  PATH); without it, or without the private stage, WE plays them anyway.
  Applied with a reload, in place.

`RUSTPAPER_SELECTOR_TAB=library|store|performance` opens the selector on a
tab (screenshots, launcher shortcuts).

## Workshop tab (store)

Browse the Steam Workshop for Wallpaper Engine, subscribe and unsubscribe
without leaving the selector (`ui/workshop.js`, backend `src/workshop.rs`,
helper `crates/rustpaper-workshop`).

- The helper uses the Steamworks API as Wallpaper Engine (AppID 431960),
  the same calls WE makes, through the running Steam client: no API key, no
  login in the app. Requirements: Steam running, signed in to an account
  that owns WE. While it runs Steam shows the user "playing Wallpaper
  Engine", so it is spawned on the first store request and exits after 5
  idle minutes (or when the selector closes its stdin).
- Queries: trending this week, most popular, most subscribed, most recent,
  recently updated, or the user's subscriptions; text search; type filter
  (Scene / Video / Web, never "Application"); "Questionable"/"Mature" items
  hidden unless **Mature** is checked. 50 per page, **Load more**. Steam
  rejects tag filters on the user-list query, so "My subscriptions" is
  filtered on the results.
- **Subscribe** asks Steam to download now (high priority). Steam itself
  writes into `steamapps/workshop/content/431960`, the folder the Library
  scans; the tab polls the item's state every second (progress from
  GetItemDownloadInfo) and, when it is installed, reloads the Library and
  shows a toast. Installed items get **Apply**, which switches to the
  Library and applies it.
- **Unsubscribe**: Steam keeps the files until a later cleanup (measured:
  still there 30 s after unsubscribing), so the selector deletes
  `<workshop>/<id>` itself (numeric ids only) and reloads the Library.
- Web items can be subscribed but are marked as not running yet.
- **Library order** (header, remembered in localStorage): title, or
  **Recently subscribed**. Steam keeps no subscription time on disk
  (appworkshop_431960.acf's `timetouched` changes on every update check),
  so the times come from the helper's `subscriptions` command (Steam's
  subscriptions list, `time_added_to_user_list`, every page). Subscribed
  items come first by that time, then folders without an active
  subscription (old installs) by folder creation time; without Steam,
  everything is by folder creation time (the download).
- **Unsubscribe in the Library**: the panel of a Workshop item has an
  Unsubscribe button (two clicks: the folder is deleted). Refused for the
  wallpaper on screen, whose files the engine may still read (videos
  stream from disk).
- Steam callbacks must not start new Steam calls (steamworks-rs holds the
  callback registry's lock while running them: registering a call there
  deadlocked the paged subscriptions query); the helper queues the next
  page and its main loop sends it.
- Privacy: the Steam API prints the user's SteamID on stderr, so the
  helper's stderr is not forwarded.

## Commands (Rust -> UI)

`list_wallpapers`, `apply(id, role, output)` (role default desktop, output
null = every monitor), `get_roles`, `clear_role(role, output)`,
`set_screensaver(enabled, seconds)`, `get_properties(id)` (definitions + saved
values), `save_properties(id, values)`, `wallpaper_colors(id)`,
`get_settings`, `set_autostart(enabled)`, `set_volume(volume)`,
`set_animated_lock(enabled)`,
`get_performance`, `set_performance(values)`, `set_playback(playback)`,
`set_video_player(player)`,
`workshop_query(query)`, `workshop_subscribe(id)`, `workshop_unsubscribe(id)`,
`workshop_status(ids)`, `workshop_subscriptions()`, `get_theme`,
`theme_from_wallpaper(id)`, `set_wallpaper_theme(enabled)`, `desktop_info`.
Ids are validated as plain folder names before any path is built.
