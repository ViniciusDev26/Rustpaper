# SceneScript

WE wallpapers can attach JavaScript ("SceneScript") to almost any property:
`{"script": "...", "value": x, "scriptproperties": {...}}`. In the local
corpus 50 of 123 scenes carry 1423 scripts. They drive clocks and dates
(`text`), show/hide layers (`visible`), move and scale them, animate effect
constants, and clone layers (audio visualizer bars).

Workshop scripts are **untrusted code**. Rustpaper runs them, but never in
its own process: crate `rustpaper-script`.

## The sandbox

```
renderer (rustpaper / rustpaper-render)          rustpaper-script-worker
  scripting.rs: collects scripts,      stdin      1. lock_down() — before any
  applies results                   ---------->      script byte is read
  ScriptHost: spawn, timeouts,      <----------   2. QuickJS (heap/stack/time
  kill                                 stdout        limits) + prelude.js
                         plain JSON lines         3. Init -> Reply, Frame -> Reply
```

The worker jails itself (`sandbox.rs`) before reading its input:

- **seccomp-bpf allowlist**: memory management (`mmap`/`mprotect` only
  without `PROT_EXEC`), `futex`, clocks, `getrandom`, `read` on fd 0 and
  `write` on fds 1/2. Everything else (`open`, `socket`, `execve`, `clone`,
  `kill`, `ptrace`...) fails with `EPERM`.
- **rlimits**: no new file descriptors, no processes, no file writes, no core
  dumps, 2 GiB address space.
- **`PR_SET_NO_NEW_PRIVS`**.
- The host starts it with an **empty environment** (only `TZ`, as a POSIX
  offset string so glibc needs no zone file), cwd `/`, and only its three
  pipes (stderr is piped too: the jail can't write files, and the
  renderer's stderr may be one).

Inside, QuickJS (`rquickjs`) has a 256 MiB heap limit, a 1 MiB stack, and an
interrupt handler that stops a module's top-level code after 500 ms and a
frame after 40 ms (1 s for the first frame, which runs `init()`). A script
that hits the budget is disabled; the others keep running. There is no
`std`/`os` module and no host object: the only modules are the built-in
`WEMath`, `WEColor`, `WEVector`; any other `import` fails.

The host (`host.rs`) kills the worker when it answers late (15 s init, 250 ms
per frame), sends a line that doesn't parse, or sends more than 4 MiB. The
scene then keeps its static values (`SceneScripts stopped: ...` warning).

`rustpaper-script-worker --self-test` locks down and then tries open, file
creation, socket, fork, execve, writing fd 3, signalling its parent and
executable memory: every one must be denied (`tests/sandbox.rs` runs it and
checks `Seccomp: 2` / `NoNewPrivs: 1` of a live worker).

## API (`prelude.js`)

Everything a script can reach is defined in `prelude.js`:

- `Vec2`/`Vec3`/`Vec4` (arithmetic with vectors or scalars, `copy`,
  `length`, `normalize`, `mix`...), `WEMath`, `WEColor` (`hsv2rgb`,
  `rgb2hsv`), `WEVector`.
- `engine`: `frametime`, `runtime`, `timeOfDay`, `canvasSize`,
  `screenResolution`, `userProperties`, `registerAudioBuffers(16|32|64)`
  (filled every frame from the renderer's 64-band spectrum), `setTimeout`/
  `setInterval`, `registerAsset`, device queries.
- `input.cursorWorldPosition`, `shared` (one object for all scripts),
  `localStorage` (in memory), `console` (to the renderer's stderr, capped).
- `createScriptProperties().addSlider(...)...finish()`: declared defaults,
  overridden by the stored `scriptproperties` (user property bindings
  already resolved).
- `thisLayer` / `thisObject` (the object the script is attached to) and
  `thisScene.getLayer(name|id)`: `origin`, `scale`, `angles`, `color`,
  `alpha`, `brightness`, `visible`, `text` read and write the layer state
  the renderer applies; `getParent`, `getChildren`, animation stubs, sound
  stubs (`play`/`stop`), `getTextureAnimation()` (flipbook control, see
  below).
- `thisScene.getCameraTransforms()` / `setCameraTransforms({zoom})`: the
  camera transform's zoom reaches the renderer, where it MULTIPLIES
  `general.zoom` (it starts at 1); eye / center / up are read-only
  placeholders. Persona 3 3151551777 (`general.zoom` 1.01, and a script
  that sets the camera zoom to 1.01 from a user property): WE's image is
  1.01 x 1.01 = ~2% larger than unzoomed; with the script's 1.01
  replacing the scene's, the whole frame was ~1% too small (its 128
  audio-bar stripes +9 px off at x 200 and -8 px at x 1715 against WE's
  mean, the window frame the same). Mean SSIM 0.848 -> 0.944. It is the
  only corpus scene calling setCameraTransforms.
- `thisScene.createLayer(path | config)` + `sortLayer(layer, index)`.
- `Date` and `engine.timeOfDay`: the renderer's clock (`text::clock_now`,
  pinned by `RUSTPAPER_CLOCK` in the harness to the capture's start), so
  offline renders show what WE showed. `g_Daytime` (shaders) uses the same
  clock: it was a fixed 0.5 before — Persona 3 3151551777's day/night cycle
  0.739 -> 0.849, city corner 2734266359 0.860 -> 0.952, nothing regressed.

Lifecycle, as WE: all `init(value)`, then all
`applyUserProperties(engine.userProperties)`, then `update(value)` every
frame; a returned value replaces the property's value. On the first frame
the renderer fast-forwards the scripts from t = 0 to the frame's time at 30
fps (WE has been running them since the wallpaper started).

## Where results go (`scripting.rs`)

- Object properties (`origin`, `scale`, `angles`, `color`, `alpha`,
  `brightness`, `visible`, `text`) and writes through `thisLayer`: world
  matrices are re-resolved through the parent hierarchy, visibility through
  ancestors (`disablepropagation` respected), text replaces the native
  clock/date presets.
- `effects[n].passes[p].constantshadervalues.<key>`: the pass constant
  (vectors as `"x y z"` strings, like materials).
- `effects[n].visible`: hidden effects with a visibility script are loaded
  but parked, and moved in or out of the chain (in scene order).
- Created layers: a copy of the creating script's own layer (visualizer bars
  clone themselves) with the configuration's image/properties on top. Asset
  paths resolve as written, then under the script's workshop folder
  (`export let __workshopId = 'N'`: `models/bar.json` ->
  `models/workshop/N/bar.json`, which is how WE finds them), then fall back
  to the template's own model. `sortLayer` decides the draw position.
- Particle `instanceoverride.<key>` (rate, count, alpha, size, colorn...):
  the systems' overrides, at run time.
- Puppet `animationlayers[n].rate`: each animation layer accumulates its
  own phase (`dt * rate`), so a rate that changes every frame (3448290956
  ties it to the audio) doesn't make the animation jump.
- Puppet `animationlayers[n].visible`: hidden layers are loaded and
  skipped while posing, so a script can show them.
- Sound `volume`: the live engine applies it from the next file it plays
  (ffplay takes the volume at start).
- Scene settings `general.<key>` (the corpus has `bloomstrength`): the
  bloom constants; `general.zoom` the camera zoom.
- Sound layers' `play()` / `stop()`: the live engine starts or holds that
  sound (`startsilent` sounds are loaded, held until a script plays them).
- `thisLayer.getTextureAnimation()` on a flipbook layer (sprite sheet or
  GIF): `frameCount`, `duration`, `fps`, `rate`, `getFrame()`,
  `setFrame()`, `play()`, `pause()`, `stop()` (rewinds to frame 0),
  `isPlaying()`. The host sends each flipbook's frame durations in the
  layer state; once a script asks for the animation, the worker runs its
  clock (starting at `engine.runtime`, the phase the renderer was drawing)
  and reports it as `anim_time`, which then drives that layer's flipbook
  instead of the scene clock. Chainsaw Man 3577990983: five 4.9 s
  fireworks (140 frames x 35 ms, frame 0 empty) stop on their last frame
  and replay after a random 1.8-6.7 s; the stub (always frame 0, 1 frame)
  left all five looping non-stop. Harness: motion energy ratio 1.45 ->
  0.70 (WE-vs-WE floor 0.91), mean SSIM 0.653 -> 0.660; Lonely Cat
  3299228616 (also calls it) 0.8567 -> 0.8568.

## Live engine

`rustpaper` starts the worker with the first frame (scripts need the screen
size). Measured on the headless output (`rustpaper <dir> --output RPREF`,
2026-11-21, RTX 4060 Ti, 30 fps):

| scene | scripts | engine CPU | worker CPU | worker RSS |
|---|---|---|---|---|
| Persona 3 3151551777 | 162 | 8.9% | 4.1% | 6 MB |
| 3027298897 | 455 | 5.6% | 11.6% | 10 MB |

(% of one core, steady state over 10 s.) Killing the worker (`kill -9`)
mid-session: the engine keeps drawing, prints `warning: SceneScripts
stopped: script worker: Broken pipe`, and the scene keeps its last values.
It does not restart the worker: a script that crashed or exhausted it would
just do it again. The selector's **Run scripts** checkbox turns them off
(`--no-scripts`).

## Input

The pointer ([0,1] of the screen) is mapped to scene coordinates through
the cover crop (`cursor_in_scene`): `input.cursorWorldPosition`. Each frame
the renderer hit-tests the visible layers (their world matrix and size) and
the worker fires `cursorMove` when the cursor moves and `cursorEnter` /
`cursorLeave` when it enters or leaves a script's layer. There is no click
input (the live engine only knows the pointer position; the harness doesn't
click), so `cursorDown` / `cursorUp` / `cursorClick` never fire.

## Measurements (harness, 2026-11-21)

Against the last run without scripts (`20261121T085643Z`):

- Snowy Plains 2406282996 0.637 -> 0.990 PASS (visibility scripts),
  Bunk 2134765860 0.730 -> 0.954 PASS, 02-raindrops 3050160027 PASS
  (visualizer bars from `createLayer`), city corner 2734266359 0.445 ->
  0.860 (day/night `alpha` from `engine.timeOfDay`), Subaru 3448290956
  0.819 -> 0.879, 3514942235 0.953 -> 0.983.
- The same run found a renderer bug the static values hid: a scalar user
  property bound to a vector (`"scale": {"user": "size", "value": 0.278}`)
  must fill every component (`value::get_vec3`); we fell back to scale 1.
- Three scenes are excluded: WE shows its crash dialog over them in every
  capture (`harness/excluded.tsv`).

Text falls back per character to a system font that has the glyph
(`text::FontSet`, fontconfig `:charset=`): CJK weekdays in Latin clock fonts
(3832902426's "星期六") drew boxes before.

## Triage of the scripted failures (2026-11-21)

The corpus' scripts call almost nothing the prelude lacks: a survey of every
`thisScene.` / `thisLayer.` / `engine.` / `input.` use found only
`setCameraTransforms` (Persona 3, added), `getTransformMatrix` (1 scene),
`getVideoTexture` (2) and text-layer fields (`font`, `pointsize`,
`alignment`, 1-3 scenes). Looking at each of the 15 scripted failures, the
causes were renderer gaps the scripts merely exposed:

- compose layers grouping their children (Lofi Cafe: PASS);
- puppet blending and culling (Jinx 0.714 -> 0.944, Chainsaw Man);
- `"0.0, 1.0"` vector strings (Frieren's audio bars);
- `mapsequencearoundcontrolpoint` starting at its control point (Frieren);
- `instance` texture overrides (Chainsaw Man's dock);
- music high bands of the audio spectrum (2983846453's visualizer: PASS).

Still open: 2938612768's WE capture shows WE's crash dialog (recapture or
exclude), Lonely Cat's HDR bloom, Frieren's parallax with the cursor in a
corner, Subaru's and Camellya's puppet poses. Clicks and media integration
events are never fired.
