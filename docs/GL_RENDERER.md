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
  hidden: SceneScript is never executed (security decision).
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
  `instanceoverride` multipliers. Sprite sheets come from the `TEXS` block
  inside the .tex (Workshop) or the `.tex-json` (base assets).
- Deterministic: seeded RNG + fixed 1/60 s steps; going back in time replays.
- Calibrated against WE with the harness (each choice is commented in code):
  rendered size = `size` x 0.5; oscillation frequencies are rad/s and oscillate
  between scalemin..scalemax; turbulence makes particles FOLLOW a slowly
  evolving noise field (not accelerate, not curl).
- **turbulentvelocityrandom** (calibrated on single-system scenes from
  `harness/calibration/make_particles.py`, measured with
  `measure_particles.py`): the direction is `forward` (default up) rotated
  in the scene plane by `offset` radians plus a random angle of up to
  `+-2 * scale` rad. WE vs Rustpaper: offset 0 -> 82% / 83% of the light
  above the emitter; offset 3 -> 25% / 31% (down, slightly left); scale 0.1
  -> x spread 0.029 / 0.029. Earlier readings (offset as a noise phase or a
  push along forward) were fitted on whole scenes and were wrong.
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
  band average (see Audio below).

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
- 2048-sample Hann FFT -> 64 log bands (30 Hz .. 16 kHz), -60..0 dB -> 0..1,
  instant attack and 0.85 decay per frame. `g_AudioSpectrum16/32/64Left/Right`
  are averages of the 64 bands.
- `rustpaper-render` and the harness use silence (what WE hears there too).
- `--no-audio` disables capture.

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

## Timeline animations (`keyframes.rs`)

Object properties can carry an `animation` (the editor timeline): one curve
per component (`c0`, `c1`, `c2`) of `{frame, value}` keys, linear or cubic
Bezier when a key's `front`/`back` handle is enabled, `options.fps`,
`options.length` (frames) and `mode` loop / mirror / single. Layers evaluate
alpha, color, brightness, origin, angles and scale every frame (blinking
crossing lights, flashes in Summer Feeling 2477602742). Particle objects and
effect constants with timelines are not animated yet.

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

## Known gaps

- **Not supported by design:** SceneScript (JavaScript) and `web` wallpapers —
  executing Workshop code is a security risk. Properties driven only by a
  script keep their static value.
- Scene lights (`LIGHTING=1` materials use ambient only); timelines on
  particle objects and effect constants.
- Particles: `remapvalue` only for `output: velocity` + `operation: remap`
  (unverified: no captured scene shows its rain); other outputs and the
  `mapsequence*` initializers are reported as unsupported. Sprite
  `orientation` modes coincide for the front-facing ortho camera (WE passes
  world camera axes; compensating object rotation broke White Angel).
- Camera parallax is implemented (`Parallax` in scene.rs): cursor offset
  from the center x `amount` x `mouseinfluence` x `parallaxDepth` x scene
  size, content moving away from the cursor. Only one data point so far: in
  the harness WE's window never sees the cursor, and Saki Ayase 2617953025
  (depth `0 1`) matches WE with `--pointer 0.5,0.9` (mean SSIM 0.63 ->
  0.94), i.e. WE assumes the pointer near the bottom edge there. Calibrating
  the magnitude needs a capture with a known cursor position.
- Video wallpapers still use the older wgpu path in `rustpaper-engine`.
- Shaders the fixer cannot repair are skipped (the image passes through) and
  listed by `--describe`.
