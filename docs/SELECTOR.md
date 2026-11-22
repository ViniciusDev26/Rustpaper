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
  the previous one and any other `rustpaper` process of the user, e.g. the
  engine a closed selector session left running.
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

## Commands (Rust -> UI)

`list_wallpapers`, `apply(id)`, `get_properties(id)` (definitions + saved
values), `save_properties(id, values)`, `get_settings`,
`set_autostart(enabled)`, `set_scripts(enabled)`, `set_volume(volume)`. Ids are validated as plain folder names before any
path is built.
