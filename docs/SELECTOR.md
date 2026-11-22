# Selector (`crates/rustpaper-selector`)

Tauri v2 desktop app: a gallery of the installed wallpapers and a
customization panel like Wallpaper Engine's "Properties" column.

```
cargo build --release -p rustpaper-selector -p rustpaper-engine
./target/release/rustpaper-selector
```

The selector starts the engine binary found next to itself
(`target/release/rustpaper`, or the install prefix), else `rustpaper` from
`PATH`.

## Gallery

- Lists every folder with a `project.json` in the Steam Workshop folder and the
  test corpus (`rustpaper_core::paths::wallpaper_dirs`), sorted by title, with
  a search box. Previews are served through Tauri's asset protocol; the
  allowed scope is set at startup to exactly those folders.
- Clicking a supported card (scene, video) applies it and opens its panel.
  `web` cards open a panel explaining they are not supported (they run the
  author's JavaScript).

## Customization panel

Built from `general.properties` of project.json, in WE's `order`:

| WE type | control | stored value |
|---|---|---|
| `slider` | range (min/max/step; integer unless `fraction`) | number |
| `bool` | checkbox | bool |
| `color` | color picker | `"r g b"`, floats 0..1 (WE's format) |
| `combo` | dropdown of `options[].label` | the option's `value` (type kept) |
| `textinput`, `text` | text field | string |
| `group` | section header | — |
| `file`, `directory` | shown, "not supported yet" | — |
| no type | author's note / separator, as plain text | — |

- **`condition`** (e.g. `background_enable.value && background_type.value == 3`)
  hides controls exactly like WE. It is evaluated by a small parser in
  `ui/properties.js` (`|| && ! == != < > <= >=`, parentheses, literals,
  `name.value`) — **never** with `eval`/`Function`; Workshop text is never
  executed. Unparseable conditions show the control.
- Labels: WE localization keys (`ui_browse_properties_scheme_color`) map to
  English names; author HTML in labels is reduced to plain text through
  `DOMParser` (no markup is inserted into the page).
- Changes are saved (debounced 0.7 s) to
  `~/.config/rustpaper/properties/<id>.json` — only values that differ from
  the defaults; **Reset** deletes the file.
- If that wallpaper is on screen, the engine is restarted with
  `--props <file>`. The new engine starts first and the old one is killed
  2.5 s later, so the desktop never flashes between them.

## Persistence

- `~/.config/rustpaper/state.json`: last applied wallpaper, the autostart
  flag, `no_scripts` and `volume`; the panel reopens the last wallpaper at startup.
- One engine at a time: applying a wallpaper starts the new engine, and
  2.5 s later (after its first frame, so the desktop never flashes) kills
  the previous one and every `rustpaper` process of the user this selector
  did not start (not its child), e.g. the engine a closed selector session
  left running. Its own children are only stopped by the handover: keeping
  "the newest engine" instead raced when applying twice within 2.5 s.
- Autostart: `~/.config/autostart/rustpaper.desktop` runs the engine with the
  last wallpaper and its `--props` file (arguments quoted per the Desktop Entry
  spec).
- **Run scripts** (header checkbox, on by default): off adds `--no-scripts`
  to every engine start; toggling restarts the wallpaper on screen and
  rewrites the autostart entry. State files from before the option keep
  scripts on (`no_scripts` defaults to false).
- **Volume** (header slider, 0..100, default 100): master volume of the
  wallpapers' own music/sounds. The engine gets `--volume N` (`--mute` at 0,
  nothing at 100); it scales every sound object, including volumes a
  SceneScript sets. Applied on release of the slider, restarting the
  wallpaper like the scripts checkbox. Older state files load as 100.

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

`list_wallpapers`, `apply(id)`, `get_properties(id)` (definitions + saved
values), `save_properties(id, values)`, `get_settings`,
`set_autostart(enabled)`, `set_scripts(enabled)`, `set_volume(volume)`,
`workshop_query(query)`, `workshop_subscribe(id)`, `workshop_unsubscribe(id)`,
`workshop_status(ids)`, `workshop_subscriptions()`. Ids are validated as plain folder names before any
path is built.
