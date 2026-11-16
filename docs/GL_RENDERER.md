# rustpaper-gl: the OpenGL scene renderer

`crates/rustpaper-gl` renders Wallpaper Engine **scenes** with OpenGL 3.3 core
through a headless EGL context. It replaces the wgpu/SPIR-V scene path of
`rustpaper-engine` for one reason: WE shaders are written in a GL-flavoured
dialect, so running them on GL removes the whole translation layer (separate
samplers, uniform blocks, bindings, reflection) that kept breaking effects.

```
cargo run --release -p rustpaper-gl --bin rustpaper-render -- <workshop id | folder> \
    [--time 0,1.5] [--out x.png] [--describe] [--dump-passes dir] \
    [--video out.mp4 --duration 10 --fps 30 --width 1920]
```

`--only id,id` draws just those scene objects (isolate one layer or particle
system). `--describe` lists layers + every load/compile warning. `--dump-passes` writes
every intermediate target (layer, each effect pass, named FBOs) as PNG — the
fastest way to find which pass breaks a scene. `RUSTPAPER_TRACE=1` prints each
draw with its program variant and the texture bound to every slot.

## Live wallpaper (`rustpaper-engine/src/gl_wallpaper.rs`)

`rustpaper <wallpaper folder>` runs scenes on this renderer (videos stay on the
wgpu path; `RUSTPAPER_LEGACY_SCENE=1` brings back the old scene compositor).

- One EGL context on the compositor's `wl_display`, one EGL window surface per
  output. Each timer tick (~30 fps) renders the scene ONCE into its FBO, then
  cover-blits it to every output and swaps with vsync off (a covered surface
  must never block the loop).
- With `wp_viewporter`, buffers use the output's PHYSICAL mode (3840x2160 on a
  1.5x-scaled 4K screen) shown at the logical size: sharp under fractional scale.
- Layer `Bottom`, not `Background`: shells like Omarchy draw their own
  background on `Background` and would cover ours. The surface has an empty
  input region, so desktop clicks still reach the shell background.
- Gotcha: a context first made current with NO surface (surfaceless, as here)
  keeps `DRAW_BUFFER = GL_NONE` after a window is bound — `present_cover`
  selects `GL_BACK` explicitly, otherwise every frame is silently empty.
- Cost (Yor Forger, 4K scene, 17 effects, 3 outputs, RTX 4060 Ti): ~2% of one
  CPU core, ~4% GPU, 215 MB RSS.

## Render graph (`scene.rs`)

```
image without effects:  material shader --(object MVP, blending)--> scene
image with effects:     material shader --> layer target A  (object size)
                        effect pass       --> B (previous = A) ... ping-pong
                        named FBOs (_rt_QuarterCompoBuffer1...) for multi-pass
                        result --(object MVP, blending)--> scene
compose/fullscreen:     like above, but the first pass reads _rt_FullFrameBuffer
                        (snapshot of what is already drawn behind the layer)
```

Nothing is effect-specific: an effect is effect.json + materials + WE shaders.
If the runtime fills uniforms/textures/combos like WE, every effect works.

## WE behaviours we had to match (each one broke ALL effects when wrong)

- **Texture padding.** .tex images are stored padded to a power of two. WE
  keeps the padding and an image layer's quad gets UVs 0..content/size of
  texture 0, so the material pass samples every other texture over the same
  share of ITS padded size. Flow-map wallpapers depend on it: Water effect 13
  820654165 (a 800x600 flow mask in 1024x1024) 0.856 -> 0.993, nebula
  833227004 0.924 -> 0.985. Rustpaper uploads textures cropped (effects and
  particles rescale their UVs with `g_TextureNResolution`, which is
  equivalent) and, for plain image layers (no puppet, no flipbook), binds
  padded copies in the material pass with scaled quad UVs.
- **Object brightness** overrides the material's `Brightness` constant only
  when the object sets `brightness` (Water effect 13: "Bright": 0.86 in the
  material, none on the object).
- **Effect command passes and persistent FBOs.** `"command": "copy"` passes
  blit one FBO into another, and FBOs marked `"unique": true` keep their
  content across frames (motion blur accumulates into one). A cold render
  (first frame, or a jump in time) of a scene that has them first runs 1 s
  of frames, so the accumulator does not start black.

- **Coordinates.** Scene space is Y-up. Every render target keeps the TOP of
  the image in row 0 (like a texture from disk), so uv (0,0) = top-left
  everywhere. Projections map "top" to clip y = -1. See `math.rs`.
- **Effect passes draw a clip-space quad with an identity MVP.** Some effect
  vertex shaders skip the matrix (`gl_Position = vec4(a_Position, 1.0)`).
- **Sampler uniforms must be set to their unit** (`g_TextureN` -> N). GL
  defaults them to 0, which silently makes every mask read the image itself.
- **Texture slot precedence:** effect.json `bind` > texture from scene /
  material > the sampler's annotated `default` > `previous` (slot 0 of effects).
  The editor serializes bound targets into the scene too
  (`_rt_imageLayerComposite_<self>_b`); that name just means `previous`.
- **Texture-driven combos:** a sampler annotated `"combo":"MASK"` defines
  `MASK=1` only when a texture is actually provided (not for the default).
- **Uniform hoisting:** includes (e.g. `common_blur.h`) use `g_Texture0` before
  the main file declares it. `translate_gl` hoists `uniform sampler2D` lines.
- **Hidden parents hide their descendants** unless the child sets
  `disablepropagation` (Kirito x Asuna 3463520581 keeps a hidden "lil" copy
  of both characters; drawing its children duplicated them).
- **Conditional visibility** `{"user": {"name": p, "condition": "1"}, "value": v}`
  is true when property `p` equals the condition (combo options that swap
  objects).
- **User properties:** `{"user": "x", "value": v}` wrappers take their value
  from project.json `general.properties` (`value::apply_user_properties`).
- **Hidden layers referenced by others** (`_rt_imageLayerComposite_<id>_a`) are
  still rendered into their own target before the layers that sample them.
- **Linear, not sRGB.** Textures and targets are plain RGBA8; WE blends raw
  8-bit values.
- **Mipmaps only when the .tex has them** (`mip_count > 1`). Generating them
  for single-level textures made `filmgrain` noise (sampled minified) average
  out to a flat gray: the grain was static.
- **`colorBlendMode` on a layer drawn through a target** (it has effects) is
  applied by an extra composite pass when the layer's result is drawn onto the
  scene, not inside the layer's own FBO (that blended the layer with nothing
  and produced a mini-scene).
- **`createLayer` templates** (objects only referenced by SceneScript) are
  hidden: SceneScript is not executed yet (it will only run in a sandbox).
- **TEXB0004:** the extra per-mipmap header only exists for mp4 textures.

## Particles (`particles.rs`)

Particle objects are drawn in scene.json order, interleaved with layers, by the
REAL `genericparticle` shader: the simulation only produces 4 vertices per
particle in the layout its vertex stage reads (center, uv + rotation z + size,
color, rotation xy, velocity + sprite-sheet phase) and the shader expands the
quad, the trail (`TRAILRENDERER`) or the sprite-sheet frame (`SPRITESHEET`,
`g_RenderVar1`).

- Supported: emitters `sphererandom`, `boxrandom` (+ `instantaneous` bursts);
  initializers lifetime/size/color/alpha/velocity/rotation/angular velocity/
  turbulent velocity; operators movement, alphafade, size/alpha/color change,
  oscillate alpha/size/position, turbulence, controlpointattract, vortex,
  angularmovement; renderers `sprite`, `spritetrail`; child systems
  (`eventfollow`, `eventspawn`, `eventdeath`); `starttime` prewarm;
  `instanceoverride` multipliers, also applied to child systems (count
  included), except the color: `colorn` / `color` REPLACE the initializer's
  color (wind v7 2884352976 draws orange `Light shafts` lavender; rain
  1444077782's gray smoke is white). Children without the override broke
  Elden Ring 3276312017 (a full-size ember glow band: 0.979 -> 0.992,
  energy ratio 1.66 -> 1.03) and Early Morning Forest 1959836574 (leaves
  at count 0.5 with full-count children: energy 1.48 -> 1.34).
  Measured one at a time in WE (`make_particles.py ov_*`): `rate` scales
  the whole system's dt (emission, ageing, motion: rate 0.25 cut the motion
  of sparse dots 0.047 -> 0.019 in WE; the scene clock still advances by
  the unscaled dt), `count` scales the emission rate but NOT `maxcount`
  (count 0.5 halves an uncapped stream, a capped one stays at maxcount),
  `speed` scales the initial velocity, `lifetime` the lifetime, `size`
  the size AND sizechange's values (so it enters squared while sizechange
  runs: calibration `sc_start07_ov`, size 0.5 -> WE 0.101 of light vs 0.414
  if linear; found isolating the Ember preset of Sunset 1375031246, whose
  embers were 4x too bright). `alpha` does not touch alphachange /
  oscillatealpha and `size` does not touch oscillatesize (`sc_*_ov`).
  Prewarm runs in fixed 0.05 s steps (0.2 s from maxcount 500), and
  emission follows WE's accumulator: a burst is subtracted from it (the
  stream pauses ~N / rate s after a burst of N), emitter `flags` 2 = at
  most one particle per step, `delay` / `duration`. Sprite sheets come from the `TEXS` block
  inside the .tex (Workshop) or the `.tex-json` (base assets).
- Deterministic: seeded RNG + fixed 1/60 s steps; going back in time replays.
- Calibrated against WE with the harness (each choice is commented in code):
  rendered size = `size` x 0.5.
- **World-space systems** (system `flags & 1`; calibration `ws*`, object
  scaled "1.5 0.08 1" and rotated): spawn position and initial velocity go
  through the object's transform, then the simulation runs in scene units
  (gravity "0 -300 0" falls straight down the screen: WE cy -0.220, RP
  -0.221) and quads are not scaled by the object (8.6x the light of the
  same system without the flag). Control points are transformed the same way.
- **turbulence / vortex / controlpointattract** (spec WE-RE-004): signed 3-D
  simplex noise (Gustavson's reference, x32) of `scale * (position + tau)`,
  `tau = r * phase range + clock * timescale`, axes sampled at cyclic
  permutations of the input, ADDED to velocity (`* speed * dt`, speed from
  the shared per-particle `r`). Vortex adds `speed * (n x axis)` (positive =
  clockwise seen from +axis), speed linear from inner to outer distance and
  held beyond. Attract adds a pull `(1 - d / threshold) * scale * dt` inside
  `threshold` (flags 2, default: no overshoot; flags 1: kill particles that
  pass within `deletethreshold`). The previous "follow the field" model had
  been fitted on calibration captures taken at WE's 15 fps limit, where WE
  weakens these operators; recaptured at 60 fps (`turb`): motion WE 0.844,
  RP 0.875 (was 0.566 vs 0.877 with the stale capture).
- **oscillatealpha / oscillatesize / oscillateposition** (spec WE-RE-001):
  one random `r` per particle sets frequency, phase and amplitude of every
  oscillator (`min + r * (max - min)`), so they move in lockstep. Wave
  `sin(f * (age + phase))`, f in rad/s, frequency default 1..10 (1..5 for
  position). Alpha / size multiplier `scalemin + r * (scalemax - scalemin) *
  (s + 1) / 2`, unclamped: only particles with r near 1 reach scalemax.
  Position adds the change of the raw wave each step (x and z share it, y is
  shifted by 2*pi*r in time). Optional `blendin*` / `blendout*` envelope over
  normalized lifetime. Calibration `oscalpha` (freq 1..20, scalemin 0.2):
  total light WE 0.376, before 0.556, now 0.380; motion WE 0.154, before
  0.237, now 0.172. Open: the default position `scalemax` (0.5 or 10) depends
  on a scene flag not identified yet; we use 10.
- **turbulentvelocityrandom** (spec WE-RE-003 3.5): `forward` rotated
  around `right` by `noise1(t) * pi * scale + offset`, with `t = (phase +
  clock) * timescale` and `noise1` the 1-D gradient noise on Perlin's
  permutation (x0.395); speed uniform in speedmin..speedmax. Particles
  spawned close in time share a direction that sweeps slowly. (The earlier
  "random angle of +-2 * scale" model was fitted on 15 fps captures.)
- **Embedded sprite sheets (`TEXS0003`)**: the header is `count, frame_w,
  frame_h`; the per-frame records carry the exact (possibly fractional) size
  (rosepetals: 5 frames of 102.5 x 128). Reading the header as
  `w, h, count` turned every Workshop flipbook into 128 frames of garbage —
  long bars for petals, flickering fog (Demon Angel 1851646226: FAIL ->
  PASS, motion SSIM 0.47 -> 0.74; Luffy 2534819596 mean 0.80 -> 0.87).
- **Sprite sheets crossfade.** WE always compiles `genericparticle` with the
  runtime-only combo `SPRITESHEETBLEND` (mix of the current and next frame).
  Without it, screen-sized flipbooks (fog) jump at every frame change:
  Demon Angel 1851646226 motion energy ratio 4.3 -> 2.7, Caitlyn 2853052034
  1.69 -> 1.28 (PASS).
- **Children without a `type` are `static`** (an independent system at the
  parent's origin). `rain_screen_4k`'s untyped static drops are drawn by WE
  (3576135309: motion energy ratio 0.05 -> 0.68). A detour that dropped them
  was fitting a glow excess actually caused by the sprite-sheet header bug.
- **`flags & 4` = perspective.** Those systems are drawn with a perspective
  camera (`general.fov`, eye on the center axis at the distance where z = 0
  matches the ortho view): emitters spread hundreds of units in depth, and
  near particles must look bigger and move faster (`math::scene_perspective`;
  Kakashi 2811157144 motion energy ratio 0.77 -> 0.88).
- **Rope renderers** (`rope`, `ropetrail`) use `genericropeparticle` with its
  own 21-float vertex layout; `ropetrail` keeps 16 history samples per
  particle.
- **Audio response** (`audioprocessingmode` on emitters) scales the rate by the
  band values (see Audio below).
- **The object's scale squashes the quads, not only the positions.** The
  vertices go through the full object matrix. Calibration
  (`make_particles.py` `sprite_sx`, `sprite_s2`, `trail_sx`, `trail_sy`): an
  object scaled `0.2 1 1` shrinks the drawn brightness of a box cloud to 0.2x
  (WE total 0.932 -> 0.185, Rustpaper 0.183), and trails on `1.5 0.08 1` /
  `0.2 1 1` match WE in centroid, spread and brightness (within 0.005).
- **No emission at t = 0.** A rate emitter accumulates `rate * dt` from zero.
  At rate 0.05 (`rare`), WE and Rustpaper both show the first particle at
  scene t = 20.1 s (25 s WE capture, warmup 0).

## More WE behaviours found while adding particles

- **Object hierarchy:** objects can have a `parent` (any object, including
  groups); origin/angles/scale are relative to it (`world_matrices`).
- **Depth:** scenes put things at large z (emitters spread hundreds of units in
  depth). The ortho projections scale z down so nothing is clipped.
- **`"image": null`** appears on particle/group objects: test for a string, or
  particle systems are silently dropped as broken image layers.
- **1/2-channel textures keep GL layout:** RG88 samples as (lum, alpha, 0, 1)
  and R8 as (r, 0, 0, 1), and WE passes each texture's format to shaders as
  `TEXnFORMAT` (ConvertTexture0Format turns RG88 into .rrrg). Pre-expanding
  them made every RG88 sprite an opaque quad and every RG88 flow map wrong
  (`shake`); fixing it took Yor from 0.94 to 0.98 mean SSIM.
- Engine aliases (`_alias_lightCookie`, shadow atlases) only exist with scene
  lighting; they resolve to white.

## Camera

The WE window shows the scene with a centered "cover" crop and **ignores the
scene camera's eye/center offsets** (measured with synthetic calibration
scenes, `harness/calibration/`: a scene whose pixels encode their own scene
coordinates). `view_proj()` is therefore the plain scene ortho. Earlier
misalignments blamed on the camera were the harness stretching non-16:9
scenes; `--screen WxH` renders through the same cover crop as a monitor.

`general.camerashake` (amplitude, speed, roughness) moves the camera without
zoom (Kirito x Asuna 3463520581, amplitude 0.5, fits best unzoomed). WE's
path, tracked as the whole-image shift of Ice Princess 2800248288 over 8 s
(amplitude 0.35, speed 1): near-sinusoids of +-4.1 px in x (period ~6 s)
and +-4.4 px in y (~4.8 s) on a 1920 screen, i.e. `0.0064 x amplitude` of the
width; the previous `1.2%` path was twice too large (energy ratio 1.30,
mean SSIM 0.79 -> 0.81). Roughness adds a faster component (not measured).

`general.zoom` zooms the camera around the scene center (CITRUS 2704773569,
zoom 1.1: mean SSIM 0.43 -> 0.98).

## Text (`text.rs`)

Text objects are rasterized on the CPU with `ab_glyph` into a texture that is
drawn by a synthetic image layer (so effects and blend modes apply).

- Size: `pointsize` x 4.15 scene units per em (measured against WE).
- Font: the object's font from the pkg, else a system font through
  `fc-match`.
- The canvas grows to fit the text (WE never clips).
- `horizontalalign` left/right puts the text's start/end AT the object's
  origin (center: centered on it), not at the edge of the `size` box
  (Toyota 3560329032's right-aligned clock; Asuna 3239767814).
- Dynamic text comes only from **native presets** recognized by their static
  values, never by running their script: `Clock` (12/24 h, seconds),
  `Time and Date`, `Format`, `DigitalDate` (weekday, day, month, delimiter,
  vertical). Anything else shows its static `value`.

## Audio (`audio.rs`, `rustpaper-engine/src/audio_capture.rs`)

- Live engine: `pw-record` captures the default sink monitor
  (`stream.capture.sink=true`), 48 kHz stereo f32, on a thread.
- 2048-sample Hann FFT, fitted to WE (`harness/calibration/run_audio.sh`,
  2026-11-09: a scene showing `g_AudioSpectrum*` as gray cells, captured
  while a known sweep plays; `fit_audio.py` fits the model frame by frame):
  - band k is a fixed frequency, log-interpolated between measured anchors:
    45 Hz -> 0, 250 Hz -> 8, 1 kHz -> 32, 4 kHz -> 47, 11 kHz -> 61; 16 kHz
    lights nothing. Bass bands narrower than an FFT bin read the spectrum
    interpolated at their center.
  - LINEAR magnitude (not dB): after a full-scale tone, amplitudes 0.5 / 0.25
    / 0.0625 show 0.47 / 0.25 / 0.08.
  - divided by one adaptive reference for all bands (rises toward the loudest
    band by 0.05 per 60 Hz step, decays by 0.9995): the same tone shows ~1.0
    after silence and ~0.5 after a tone twice as loud.
  - no smoothing of the bands: 1 -> 0 within ~70 ms when a tone stops.
  - a tone also lights its neighbours: 0.3 / 0.1 of it one / two bands below,
    0.4 / 0.15 above.
  - `g_AudioSpectrum16/32` are the MAXIMUM of each group of the 64 (RMSE vs
    WE 0.10, the average gives 0.15).
  - WE shows it ~0.167 s late; `AudioTrack` (replayed files) adds that delay.
  RMSE per band vs WE: 0.12 (the previous dB + 0.85-decay analyzer: 0.22).
  Known gap: WE lights 2-3x more bands than the model for tones below ~700 Hz.
- **WE publishes a new spectrum only every ~75 ms (~13 Hz).** In between,
  shaders read the same values. Measured 2026-11-10 on the cells scene while
  real music played (`calib/audio_music`): 187 changes in 421 frames at
  30 fps, with gaps of 1 / 2 / 3 frames about equally often (irregular). The
  analyzer still steps at 60 Hz, because attack / release are fitted per step,
  and `Publisher` holds its output. CITRUS 2719898311: energy ratio
  1.56 -> 1.31 (PASS).
- Known gap, music vs tones: with real music WE's bands all average ~0.22,
  the high ones included (band 48: 0.25). The model, fitted on isolated
  tones, gives band 32 0.05 and band 48 0.02. This suggests per-band (or
  tilted) normalization that the single-tone sweep cannot show.
- Harness caveat: `we_audio.wav` only covers the capture, but WE had been
  hearing the scene's music during the warmup, so its adaptive reference
  carries that history. Giving the replay 8 s of prior music lowered
  CITRUS's ring-glow energy from 0.87 to 0.68 per frame (WE: 0.36),
  measured with 60 Hz publishing, before `Publisher`.
- `rustpaper-render --audio file` replays a recording (the harness passes
  `we_audio.wav`, recorded during the WE capture); without it, silence.
- `--no-audio` disables capture.

## Scene sounds (`SceneSound`, `rustpaper-engine/src/sound.rs`)

`sound` objects (`sound: [files]`, `playbackmode` loop / single / random with
a `mintime`..`maxtime` pause, `volume` after user properties, `startsilent`,
`visible`) are collected at load (`--describe` lists them) and played by the
live engine with `ffplay` through the default output, paused with the
wallpaper. WE plays them too, and its audio-reactive layers react to that
music: CITRUS 2719898311's audio ring only moves with its own track
(energy ratio 0.18 silent, 1.42 with the track). The harness records the
audio WE played (`we_audio.wav`), so offline renders get it from there.

## Puppet warp (`puppet.rs`, `rustpaper_core::mdl`)

`models/*_puppet.mdl` (MDLV/MDLS/MDLA, layout documented in `mdl.rs`) gives a
skinned mesh, a bone hierarchy and keyframed animations. The scene object's
`animationlayers` pick animations (rate, blend, additive). Bones are posed per
frame and the mesh is skinned on the CPU (a few thousand vertices); the
unchanged WE material shader draws it.

- Animations can be followed by extra bytes (9 zero bytes between
  animations); the parser resyncs on the next header (`u32 id, u32, name,
  mode` with a known mode) instead of failing (Dome 3113287126: 0.45 -> 0.90).
- **MDLV0013** has no vertex-layout field: a fixed 52-byte vertex
  (position, 4 bone indices, 4 weights, uv) follows the size.
- Animations may carry an **event list** after their tracks (u32 count, then
  f32 frame + JSON per event, e.g. `{"name":"flashStart"}`); it is skipped.
- MDLA0001 uses the same per-animation fps as 0003/0004 (a wrong "rate x 30"
  reading made Summer Feeling 2477602742's train cross in one 1/30 s frame):
- The MDLA per-animation float is read as **frames per second**. Reading it as
  a duration (fps = frames / value) matched WE worse (2900431360: motion SSIM
  0.57 vs 0.60).
- Play modes `loop`, `mirror` (ping-pong), `single` (hold last frame).
- **Attachments (MDAT0001)**: named points on bones (u32, u16 count, then per
  attachment u16 bone, name, f32x16). Scene objects with `"attachment": name`
  and a puppet as `parent` are placed at parent world x current bone pose x
  attachment x their own local transform, every frame, parents first: hair
  and bodies built from many puppets follow each other (Kirito x Asuna
  3463520581: mean SSIM 0.76 -> 0.84).
- MDLV0023 has extra blocks between the indices and MDLS (the parser looks
  for the magic), and MDLS0004's animation offset points at the MDAT block
  that precedes MDLA.

## GIF scenes (animated images)

- The pkg is named after the project's scene file (`gifscene.json` ->
  `gifscene.pkg`).
- `orthogonalprojection: {"auto": true}`: the scene is the size of its
  image (centered, so size = 2 x origin).
- An image material with the `spritesheet` combo is a flipbook: the
  embedded `TEXS` block lists frames (image index, duration, rectangle) over
  SEVERAL images of the .tex (atlases). `TEXS0001` stores the rectangle as
  integers, 0002/0003 as floats. Each frame sets `g_Texture0Translation` /
  `g_Texture0Rotation` and binds its image.
- Pixel art (`flags & 1`, no interpolation) is scaled to the screen with
  nearest filtering, as WE draws it at screen resolution (Aesthetic City
  843532366: mean SSIM 0.91 -> 0.9997).

## Timeline animations (`keyframes.rs`)

Object properties can carry an `animation` (the editor timeline): one curve
per component (`c0`, `c1`, `c2`) of `{frame, value}` keys, linear or cubic
Bezier when a key's `front`/`back` handle is enabled, `options.fps`,
`options.length` (frames) and `mode` loop / mirror / single. Layers evaluate
alpha, color, brightness, origin, angles and scale every frame (blinking
crossing lights, flashes in Summer Feeling 2477602742). Effect constants
with a timeline (`constantshadervalues` entries carrying `animation`) are
kept whole and evaluated every frame (13 scenes use them; こきん 3146507587
0.915 -> 0.928). Particle objects animate origin / angles / scale timelines the same way.

## Bloom

`general.bloom` adds WE's post chain: LDR (`downsample_quarter_bloom` ->
`downsample_eighth_blur_v` -> `blur_h_bloom` -> `combine`) or, with
`hdr: true`, the dual-filter HDR chain (`hdr_downsample` with BLOOM/UPSAMPLE
-> `combine_hdr`) on RGBA16F targets.

## Uniform annotations live in headers too

`ShaderSource::params` expands `#include`s before reading
`// {"material": ...}` annotations. `g_CompositeColor` (default "1 1 1") is
declared in `common_composite.h`; left at GL's 0 it multiplied every
blur/composite result to black (wind v7 2884352976: mean SSIM 0.26 -> 0.89).

## Shader fixer (`rustpaper_core::shader::fix_glsl_from_log`)

Workshop shaders written for WE's HLSL path rely on implicit conversions GLSL
rejects. `Program::build` compiles, parses the driver log (NVIDIA `0(N)` and
Mesa `0:N(` formats) and applies targeted rewrites, up to 10 rounds:
truncating/wrapping a conversion (`vec4 -> vec3` etc., per statement, once),
ternary conditions, `uint` operands, swizzles of the nearest declaration,
multi-declarators, `%` on floats (`mod`), non-constant `const`, stray
`#endif`, bools in arithmetic (`(a < b) * 6.0` -> `float(a < b)`), functions
named like a prelude macro (`log10` -> `#undef` first), writes to fragment
inputs (the varying is copied into a global at the start of `main`) and
identifiers containing `__` (reserved in GLSL). A declared name is never
swizzled (`vec4 c.xy =` was a fixer bug). Varyings whose vector sizes differ between stages are promoted to
the larger type at link time. `RUSTPAPER_NO_FIX=1` disables it;
`RUSTPAPER_DUMP_SHADERS=dir` writes sources that still fail.

## User properties

`SceneRenderer::load_with_properties` overrides `general.properties` values
(name -> value) before `{"user": ...}` bindings are resolved. The live engine
reads them from `rustpaper <dir> --props file.json`, written by the selector's
customization panel (`docs/SELECTOR.md`).

## Rejected hypotheses (measured, kept out)

- Constants keyed by the editor label (`ui_editor_properties_*`) applied to
  their uniform: rain 1444077782 got worse (0.81 -> 0.76); WE uses the
  defaults for those.
- `shake` effect running at 2x speed: fixed Evangelion 3835770368 but on the
  52 scenes using `shake` PASS fell from 29 to 14.
- Default bloom strength/threshold changes: no effect on Water effect
  820654165 (its color mismatch comes from elsewhere).
- Turbulentvelocityrandom defaults other than speed 100..250: Akame
  1215952856 passes only with 100..250.

## Known gaps

- **Not supported yet:** SceneScript (JavaScript) and `web` wallpapers.
  Executing Workshop code is a security risk, so they will only run inside a
  sandbox (planned, after the no-script renderer reaches its target). Until
  then, properties driven only by a script keep their static value.
- Scene lights (`LIGHTING=1` materials use ambient only).
- Particles: `remapvalue` only for `output: velocity` + `operation: remap`
  (unverified: no captured scene shows its rain); other outputs are reported
  as unsupported. `mapsequencearoundcontrolpoint` (N even directions,
  velocity rotated per particle) and `mapsequencebetweencontrolpoints`
  (evenly spaced between control points 0 and 1) are implemented but not
  calibrated (only heavily scripted scenes use them). Sprite
  `orientation` modes coincide for the front-facing ortho camera (WE passes
  world camera axes; compensating object rotation broke White Angel).
- `g_ParallaxPosition` follows the (smoothed) cursor like the camera
  parallax (it was fixed at the center): `depthparallax` effects shift layers
  with it.
- Camera parallax (`Parallax` in scene.rs): cursor offset from the center x
  `amount` x `mouseinfluence` x `parallaxDepth` x scene size, content moving
  away from the cursor, following it exponentially with `delay` as the time
  constant. WE reads the cursor through XWayland: the last position the
  cursor had over an X window, in X coordinates (harness/xpointer.py records
  it; the harness's earlier hyprctl reading was wrong). XWarpPointer changes
  that reading but not what WE sees. The follow model is NOT settled: a
  per-frame `1 - delay` lerp fit 3366882745 (delay 1.0: 0.79 -> 0.90) but
  broke Ice Princess 2800248288 (delay 1.1: 0.81 -> 0.66).
  `harness/calibration/run_parallax.sh` measures it.
- Video wallpapers still use the older wgpu path in `rustpaper-engine`.
- Shaders the fixer cannot repair are skipped (the image passes through) and
  listed by `--describe`.
