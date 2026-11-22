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
compose with children:  the children are drawn into the group's own target
                        (transparent with copybackground: false, else a copy
                        of the scene so far), which the compose shader reads
                        as _rt_FullFrameBuffer; its effects then apply to the
                        whole group
```

**Compose layers group their children.** WE's "Adjustable Composition Layer"
composes its children: Lofi Cafe 2370927443's chalkboard (photo, date, title
are children of compose 3479, `copybackground: false`, whose perspective
effect squeezes the group into the board). Drawn as plain layers they were
~4x too large and off the board. Gate 2026-11-21: Lofi Cafe FAIL -> PASS
(0.928 -> 0.981), Jinx 3078285611 and Subaru 3448290956 +0.01, no
regression. The group is resolved once at load (nearest compose ancestor);
groups nest.

**`instance` overrides.** Instanced models (`solidlayer_instance_4`) take
the object's `instance.textures` / `combos` over the shared material, like an
effect pass's scene overrides. Chainsaw Man 3577990983's dock icons drew as
white squares (the material's `util/white`).

**Vector strings may use commas.** Workshop effects store `"0.0, 1.0"`
(material constants and shader annotation defaults). Read on whitespace only
it was `[1.0]`: 3195212886's audio bars (`u_BarBounds`) stood at full height
in silence.

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
- **Motion blur accumulates once per WE frame.** `mix(past, current, rate)`
  runs once per frame, so the trail length depends on the frame rate.
  Measured with `harness/calibration/run_motionblur.sh`: a bar scrolled at
  120 px/s, with the slope of ln(trail) giving accumulations per second. WE
  at its 60 fps limit gave 58.1 (rate 0.3) and 60.3 (rate 0.13). Rustpaper
  rendering at 30 fps gave 29.9 and 29.9, so its trails were twice as long.
  Rustpaper now applies `1 - (1 - rate)^(dt * 60)` per frame, which gives
  59.6 and 60.2 (`WE_REFERENCE_FPS`). Two traps showed up while measuring:
  - The trail's area cannot measure it: the moving average conserves the sum
    of the profile.
  - A constant only applies under the name of its shader's `material`
    annotation. The base shader calls it `rate`; the old copy that rain
    1444077782 packs calls it `Accumulation rate`. WE and Rustpaper agree.

  Known gap: WE's 8-bit accumulator leaves a faint residue (3/255 at rate
  0.13) where Rustpaper's leaves 1/255. A value v stops decaying when
  v * rate < 0.5, and each 60 Hz step has a smaller rate.
- **Shimmer matches WE** (`harness/calibration/run_shimmer.sh`): sweep
  brightening +37 vs +36 and +82 vs +81 with BLENDMODE 31, the same lit
  share, and the same period at speeds 0.4 / 1 and delays 2-5. Its phase
  showed WE's clock running 0.23-0.30 s behind the harness's `we_time`.
  Fixing that in the harness (`WE_CLOCK_DELAY`) aligned the peaks to within
  one frame. Gate (2026-11-21): Ice Princess 2802243144 FAIL -> PASS, no
  regression.
- **Film grain matches WE** (`harness/calibration/run_grain.sh`: a flat grey
  layer, 16:9 texture). At the default scale 10 and at scale 2, grain std is
  21.98 vs 21.93, frame-to-frame |diff| 24.88 vs 24.89, and the 1 px
  neighbour correlation in x and y is equal. The triage linked filmgrain to
  low-energy FAILs, but this rules it out as their cause.

- **Screen mode (`SceneRenderer::set_output_size`), accepted 2026-11-21.**
  A scene larger than the screen is drawn straight at screen size, like
  WE. `rustpaper-render --screen` and the live engine call it.
  - The cover crop is built into the projection (one sampling, the same
    crop as `cover_rect`; `run_minify.sh m_*_2130`).
  - Each layer's LAST effect pass is drawn directly into the screen target:
    the layer quad goes in pre-transformed to clip space with an identity
    MVP, and the result is composited 1:1.
  - Intermediates stay at texture resolution (image layers) or layer
    resolution (compose layers), and compose layers keep layer-space UVs.
  - Textures use their own mip levels.
  - Scenes stay on the scene-size path when they contain:
    - refraction particles (they lose motion; 3576135309);
    - compose layers smaller than the scene (Blue Sky 2944773634,
      3287715210);
    - any passthrough layer when the crop is not the identity.
  Gate: +4 PASS (Forest 1959836574, Rei 3835770368, 3835806837,
  3835761947), no regression. The score went from 87 to 91/123
  (no SceneScript 64 -> 68/73).
- **Effect target sizes (`layer_target_size`).** Measured with
  `run_grain.sh`. WE runs a layer's effect passes at its TEXTURE resolution
  and draws the LAST pass straight onto the scene at the layer's on-scene
  size. Evidence: on a 1920x1080 layer with a 32x18 texture, grain followed
  by another effect comes out as 32x18 blobs (neighbour correlation 1.00 up
  to 3 px, std 14.0), while grain as the last effect keeps full detail (std
  22.0). Rustpaper now does the same, and all four 1080p/4K x last/not-last
  cases match. The gate showed no change on the corpus, where textures
  usually match the layer size.

  Known gap, scenes larger than the screen: on a 3840x2160 scene shown at
  1920x1080, WE's grain keeps its 1080p contrast (std 21.98). Rustpaper
  renders at scene size and downscales, which averages the grain to 15.23.
  In Anime girl v40 3835806837 that gives a grain-only energy ratio of 0.23
  (`isolate_effects.sh`). A global fix was REJECTED by the gate
  (2026-11-21): all render targets at screen resolution (layer FBOs, scene,
  bloom). The grain matched (22.01) and 3835806837 / 3835761947 went
  FAIL -> PASS, but 10 scenes went PASS -> FAIL (Aurora Lake 1275921440
  0.950 -> 0.894, Gaze 1195626192 0.981 -> 0.941...), 83 -> 75 PASS. So WE
  does not scale every target to the screen. A second try was also
  REJECTED: it scaled only the scene target, bloom and each layer's last
  pass, with intermediates at texture resolution. 4K grain matched and
  3835761947, 3835806837 and Rei 3835770368 went FAIL -> PASS, but Aurora
  Lake 1275921440, Sunset 1375031246, Dome 3113287126 and 3287715210 went
  PASS -> FAIL (net 83 -> 82). Minification is NOT the cause.
  `run_minify.sh` (a 4K layer with a 1 px checkerboard, 1 mip or a full
  chain, with or without an effect) gives flat grey in WE (std 0.4) and in
  Rustpaper (0.0). An exact 2x bilinear minification already averages 2x2
  texels. Aurora Lake 1275921440 under screen-resolution drawing:
  - the particles are unchanged (variant vs variant SSIM 0.9995);
  - the image layer changes (0.919), so the cause is its last pass
    (`clouds` after `waterripple`, texture 3840x2130 drawn at 1947x1080);
  - it is not a global scale or shift (best alignment is unscaled);
  - trilinear minification of the last pass's input barely helps (0.895 ->
    0.900).
  A per-effect ablation then showed that the cause is not an effect: with
  NO effect the layer still drops (0.977 -> 0.911).
  `run_minify.sh m_*_2130` (non-integer 1.97x) explains it:
  - with a mip chain, WE draws flat grey (it samples the texture at screen
    resolution with mips), while the current Rustpaper path ALIASES (it
    renders at 4K and blit-downscales);
  - with one level, WE aliases in the same pattern as Rustpaper's
    integer-crop blit (frame SSIM 0.93) and not like a separately resampled
    screen target (0.11).
  A third try drew the scene straight at screen size, with the cover crop
  built into the projection (one sampling). It matched both cases
  (0.93 / 0.999) and 4K grain, and Aurora Lake rose to 0.924. The gate
  still REJECTED it: no-SceneScript 63 -> 65/73 (Rei 3835770368,
  3835806837 and 3835761947 PASS), but 5 PASS -> FAIL (Aurora Lake, Ice
  Princess, 3022080536, Dome, 3287715210), net 86 -> 84. What is left is
  probably the last effect pass: it still renders into a layer FBO that
  is composited at a fractional offset (a second resampling). WE draws
  that pass straight onto the screen.

  Fourth try ("screen mode"): the last pass drawn straight into a
  screen-size target, with the layer quad pre-transformed to clip space,
  and scenes with compose/fullscreen passthrough layers kept on the old
  path. Aurora Lake came back to 0.951 and Dome to 0.964. The gate gave
  +4 PASS (Rei 3835770368, 3835806837, 3835761947, Forest 1959836574) and
  1 PASS -> FAIL: 3576135309, whose mean SSIM rose 0.950 -> 0.983 but
  whose energy fell 1.25 -> 0.64. Its rain drops are REFRACT particles
  (24 systems); in screen mode their motion halves (0.0088 -> 0.0044). Not
  merged. The patch is in `harness-out/experiments/screen_mode.patch`
  (not tracked).

  The raindrops isolated with the background (`isolate.sh 3576135309 17
  38 52 58 64 70 76`):
  - luminance frame differences: WE 0.0174, screen mode 0.0156 (0.90),
    current path 0.0307 (1.76; it renders the refraction at 8K);
  - the harness energy (frame differences taken in RGB): screen mode 0.68,
    current path 1.31.
  Correction: the "luminance 0.90" came from a broken metric
  (`format=gray` from limited-range YUV subtracts 16 and zeroes small
  differences). Measured properly, screen mode is 0.68x in Y, U and V
  alike, which matches the harness: the drops really move too little in
  screen mode (0.68) and too much on the current path (1.31).
  Mip ablation on those drops in screen mode (energy ratio vs WE):
  - all mips: 0.68;
  - no mips anywhere: 1.59;
  - no mips on `rain_drops_sheet_normal` only: 0.81;
  - no mips on the drop albedo and normal map: 1.41.
  Both drop textures are 4x4 sprite sheets with 7 mips and header flag
  0x4. A test .tex with flag 0x4 but no TEXS block draws NOTHING in WE.
  `make_particles.py tiny_8k_sheetred` copies that real sheet with RED mip
  levels: WE draws red (142, 36, 40) and so does Rustpaper (138, 34, 38),
  with the same coverage and motion (1087 vs 1037 px, 0.048 vs 0.050). So
  sprite-sheet mips are handled alike, and the drops' 0.68 has another
  cause.
  - **Refraction strength is right in both modes**
    (`run_refract.sh`: still drops over a coordinate-encoding background).
    Mean |drops - bg| WE 0.183 vs 0.174 at 1080p; at 4K, 0.132 vs 0.137 on
    the current path and 0.136 in screen mode. The deficit is in the drops'
    MOTION.
  - **The drops' operators.** They use `remapvalue -> velocity` (simplex
    noise, implemented) and `remapvalue -> speed` (fbm noise, range -5..7,
    flags 3; NOT implemented, reported as unsupported). Calibrated with
    `run_speed.sh` (one dot, velocity 100, range fixed at 3):
    - WITHOUT flags, WE multiplies the velocity by the value every frame:
      the dot leaves the screen within ~4 frames (20 px, then 173 px in one
      30 fps frame);
    - with flags 3 the constant range has no measurable effect (107 vs
      108 px/s).
    A constant range cannot show what flags 3 does with noise, though.
  - **Ablating the drop systems** (the isolated drops; WE vs Rustpaper motion
    energy):
    - with children, as shipped: 0.078 vs 0.053 in screen mode;
    - WITHOUT children: WE 0.0058 vs current path 0.0387 and screen mode
      0.0136. In WE almost all the motion comes from the children; the
      parent drops barely move.
    - parents with no `speed` op: WE 0.0101, Rustpaper 0.0386. In WE the
      flags-3 speed op DAMPS the motion (0.0101 -> 0.0058); Rustpaper does
      not implement it.
    - parents with no `velocity` remap: WE 0.0011 (nearly nothing),
      Rustpaper 0.0197.
    Three particle gaps follow:
    1. `remapvalue -> velocity` has since been calibrated (see Known gaps).
       It was not the drops' excess: still 0.0370 vs 0.0101 on the current
       path, so that excess comes from rendering the refraction at 8K.
    2. `remapvalue -> speed` is calibrated (`make_particles.py spd_*`, one
       dot, velocity 100; 8 variants all match WE). The fbm value stays at
       the range MINIMUM. The velocity is multiplied by it every 60 Hz
       frame, compounding without flags, clamped to 0..1 with flags 3. So
       the drops (-5..7, flags 3) stand still and their children carry the
       motion (isolated drops with children: energy 1.31 -> 0.84). The gate
       REJECTED it: 3576135309's energy became right (1.175 -> 0.782) but
       its mean SSIM slipped 0.9503 -> 0.9499 under 0.95. Combined with
       screen mode the scene drops to energy 0.49: in screen mode the
       children (small fast drops) lose motion. The patch is in
       `harness-out/experiments/remap_speed.patch`.
    4. Static child systems are right: `make_particles.py child_s1 /
       child_sx / child_neg` (a still box cloud under an invisible parent,
       object scale 1 / 0.2 1 1 / -0.2 1 1) matches WE in brightness, spread
       and centroid (0.677 vs 0.677-0.679, sx 0.030). Yet with both patches
       the isolated drops still have WE showing MANY tiny refraction specks
       and Rustpaper few.
    5. Tiny still refraction drops are faint in both
       (`run_refract.sh` with REFR_SETS=refr_8k_tiny: 400 drops of ~3 px on
       an 8K scene; mean |drops - bg| WE 0.008, Rustpaper 0.014, screen mode
       0.012). So the specks WE shows in the real scene come neither from
       per-drop refraction strength nor from child counts or scale. All four
       drop materials are pure refraction (-0.1 / -0.05). Still open; the
       next idea is a frame-by-frame comparison of the number of live
       particles per system (a `--describe` counter) against a WE capture
       of each child preset on its own.
    3. NOT a gap: still `spritetrail` particles are invisible in both WE and
       Rustpaper (`make_particles.py trail_still*`: total 0.000), and moving
       ones match (0.959 vs 0.991). So Rustpaper's leftover motion without
       the velocity remap (0.0197) comes from another system or operator in
       the drop presets; it still has to be ablated.
    Calibrate each in isolation (make_particles.py). Then the drops' screen
    mode energy can be judged.
  One drop system alone matches in luminance (0.0044 vs 0.0045); the
  glitter is negligible (0.0003). Next: calibrate a refraction particle
  over a coloured, detailed background on a 4K/8K scene: the sharpness of
  the framebuffer it samples and the mip level of its minified normal map.
- **Textures use their own mip levels** (`tex.rs` decodes every level,
  embedded PNG/JPEG levels included; `upload` sends them). WE samples the
  file's mips: a .tex shipping RED mips draws red in WE, both for an image
  layer (`run_minify.sh m_redmips_2130`) and for particles
  (`make_particles.py tiny_8k_redmips`). Rustpaper had generated a
  box-filtered chain. Ice Princess 2802243144's background ships 10 PNG
  levels. Gate (2026-11-21): no change on the default path (it rarely
  minifies textures); it is needed for screen mode. Known gap: tiny
  minified particles light ~30% more pixels in WE (1133 vs 830 px,
  `tiny_8k_redmips`).

  Known gap: with a SQUARE texture stretched over a 16:9 layer, WE's grain
  is 1.78x wider in x. WE takes the noise `aspect`
  (`g_Texture0Resolution.z / .w`) from the layer's texture, while Rustpaper
  takes it from the effect FBO. This only matters when the texture's aspect
  differs from the layer's size.

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
- **`createLayer` templates** (objects whose `visible` script clones them)
  are hidden only while scripts don't run (`--no-scripts`); with scripts the
  script decides (some use the template as bar 0). See SCENESCRIPT.md.
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
zoom (Kirito x Asuna 3463520581, amplitude 0.5, fits best unzoomed). The
first model was fitted on Ice Princess 2800248288 (amplitude 0.35, speed 1).
It was then calibrated in isolation with `harness/calibration/run_shake.sh`:
a coordinate-encoding scene, with the camera offset tracked per frame in WE,
over 8 runs (amplitude 0.25-1, speed 0.3-2, roughness 0-1). WE's path is
deterministic in scene time:

    x = -sin(t + 1.34) * k,   y = sin(1.33 t + 2.88) * k
    t = (time - 0.18) * speed^2,   k = 0.0056 * amplitude * width

The measurements behind it:
- **Frequency grows with speed squared.** A free-phase frequency fit gave
  1.0 x speed^2 rad/s in x and 1.33 x speed^2 in y at speeds 1, 1.5 and 2.
  The old linear model lost the phase at speed 0.64 (correlation +0.23 /
  -0.41); this one correlates at 0.95-0.99 on every run.
- **The x axis was mirrored** (correlation -0.96 before the fix).
- **Amplitude is linear, and the old model was ~18% too large.**
- **Roughness changes nothing in WE:** 0 vs 1 gives std 3.53 vs 3.54 px.
- **The 0.18 s delay** from the joint fit was the harness's own clock
  offset. `capture_time` now subtracts `WE_CLOCK_DELAY` (0.25 s): WE starts
  the scene clock ~0.25 s after the openWallpaper command. The shimmer
  sweeps measured the same lag (see Shimmer).

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
- Known gap, energy of こきん 3146507587 (ratio 0.664, mean SSIM 0.974),
  traced by isolation:
  - every image layer alone matches WE (energy ratio 0.84-1.00);
  - the particles are fine or too strong (snow 1.16, ember 1.00, ember
    small 3.7);
  - the whole scene WITHOUT its two compose layers is 1.13;
  - `isolate_effects.sh 3146507587 22` on the compose layer: motionblur
    1.11, bokeh 0.97, shine 1.09, vignette 1.07, but **filmgrain 0.556**
    (Rustpaper 2.04 vs WE 3.66).
  It is the 4K film-grain resolution gap (grain computed at scene size and
  averaged down) on a compose/passthrough layer, the case screen mode does
  not cover yet. A first try (`harness-out/experiments/screen_mode_passthrough.patch`)
  ran compose layers that cover the scene in SCREEN space: a screen-size
  FBO, the cropped scene transform for both drawing and sampling, effects
  in that FBO, and a 1:1 composite. It made 3146507587 worse (mean SSIM
  0.974 -> 0.868, energy 0.54): the image comes out shifted right and
  slightly enlarged. Some transform (camera shake? zoom?) is applied once
  too often or too rarely once the crop sits in the same matrix. Not
  merged.
  Follow-up (`harness/calibration/make_compose.py`): a compose layer scaled
  1.03 x 0.715 and rotated like object 205 does NOT change the mapping in WE
  (cmp_scaled = cmp_plain), nor on Rustpaper's current path, nor with the
  screen-space patch. So the compose geometry is fine, and the regression
  must come from one of compose 22's effects in screen space. The
  per-effect copies (`isolate_effects.sh 3146507587 22`) only seemed not
  to enter screen mode (a stale render). Re-rendered, the variant with
  compose 22 carrying NO effect already drops to 0.822. The culprit is
  compose 205's workshop effect: it draws bars limited to v 0..0.47 of its
  SCALED layer (1.03 x 0.715), so it needs layer-space UVs. **Compose
  effects must run in layer space.** In screen mode the old layer-space
  passthrough path is right whenever the cover crop is the identity (a
  16:9 scene on a 16:9 screen): 3146507587 then renders 0.9745 (accepted
  0.9744). Only scenes that need a crop AND have passthrough layers have
  to stay out. Scaling the compose target to screen density lowered
  3146507587's grain energy further (0.577), so its film grain still
  waits for a calibration of compose intermediates. Patch with this rule:
  `harness-out/experiments/screen_mode_v2.patch`. Screen mode as a whole
  is still blocked by 3576135309's drops.
- **Compose layers follow the image-layer rule** (`make_compose.py`, 4K
  scene, plain compose; |diff| = frame-to-frame grain):
  - film grain as the compose's LAST pass: WE 24.5 (screen-pixel grain),
    Rustpaper 17.0;
  - film grain before another pass: WE 16.93, Rustpaper 16.92.
  So intermediates run at layer resolution, and the last pass at screen
  density, in layer space. Screen mode v3
  (`harness-out/experiments/screen_mode_v3.patch`) gives passthrough
  layers a last pass at screen density, and both cases then match (24.57 /
  16.93). 3146507587 does not move (0.657): its grain is an intermediate
  pass, so its deficit comes from compose 22's five effects COMBINED
  (each alone is 0.97-1.11 except grain-last).
  Pairs (`isolate_effects.sh 3146507587 22 0+1 1+2 1+3 1+4`, energy vs WE):
  motionblur+grain 0.55 (grain last), grain+bokeh 0.66, grain+shine 0.77,
  grain+vignette 0.79, all five 0.64. With compose 22's grain constants
  (scale 30, strength 0.7, `make_compose.py *_s30`), an intermediate grain
  still matches (|diff| 4.25 vs 4.19) and a last-pass grain is half in
  Rustpaper (4.12 vs 8.57; screen mode v3 fixes that case). So the pairs'
  deficit is not the grain alone. It comes from how the following effect
  (multi-pass bokeh, shine) carries it, which no synthetic case reproduces
  yet.
  **Found: bokeh's painted mask.** grain+bokeh WITHOUT the pass-1 texture
  `masks/coc_mask_e311f85a` matches WE (energy 1.055); WITH it, 0.658.
  - The mask is R8 480x270, LZ4; decoded it is almost black (mean 14/255,
    max 119, only the bottom rows), so WE blurs almost nothing.
  - Rustpaper's `_coc` target is right too (`--dump-passes`: ~0 except
    the bottom), but the bokeh's LAST pass (gaussian.frag, VERTICAL, not
    PRECISE: mix(_full2, blurred, u_alpha * saturate(depth * 9 *
    aperture)) with depth from _coc) turns _full2's detail 1.69 into 0.17.
    It picks the blurred image where the coc is 0.
  Suspects in the translated shader: ApplyBlending with BLENDMODE 0, the
  HLSL-ordered `clamp(0, 0.15, depth)`, or the VERTICAL / !PRECISE combo
  path.
  **FIXED: `clamp` now has HLSL semantics** (`GL_PRELUDE` in
  `rustpaper_core::shader`: `min(max(x, lo), hi)`). GLSL leaves clamp
  undefined when lo > hi, and NVIDIA returned lo = 0.15, so every in-focus
  pixel was blurred. The grain+bokeh pair went 0.658 -> 0.847. Gate
  (2026-11-21): 3146507587 FAIL -> PASS (energy 0.664 -> 0.864), no
  regression.
- Known gap, Winter Comfy 2282120494 (mean SSIM 0.943): the diff sits on
  its `Light shafts 1` (object scale 2, rotated 0.413). Isolated, WE's
  shafts spread far to the left (cx 0.49, sx 0.37) while Rustpaper's
  cluster top-right (cx 0.73, sx 0.17). Ruled out with make_particles.py,
  all matching WE:
  - velocity scaled by the object scale (`spd_s2`: 202 vs 202 px/s);
  - prewarm age and drift (`drift_pre*`: cx -0.424 vs -0.423);
  - object rotation (`drift_pre_rot`).
  The `sphererandom` spawn model is RIGHT. The spread differences above
  came from small samples (40 dots, different seeds).
  - `make_particles.py sph_hist_xy` (400 dots, directions 1 1 0) in WE:
    mean|x| 151.6, rms x 179, rms y 181.
  - Rustpaper's simulator sampled directly (20000 spawns): 150.8 / 179.6
    / 178.2, isotropic, the uniform-ball model.
  Winter Comfy's shafts are 16 big random sprites, and its WE-vs-WE floor
  is only 0.959, so its FAIL is dominated by randomness, not by a model
  error.
- Known gap, energy of CITRUS 2719898311 (ratio 1.65, mean SSIM 0.986):
  its whole background flickers in Rustpaper and stays still in WE. The
  background layer's only effect is an audio-reactive workshop shader
  (`test_shader`, brightness 0.15, frequency range 0-1), and the capture
  had music playing (-33.5 dB mean). Camera shake and parallax are off.
  This is the music-spectrum gap below, not an effect bug.
  Measured 2026-11-21 with make_audio.py's cells while CITRUS's own music
  played (`WE_PLAY_AUDIO`, `harness-out/calib/audio_citrus`; per-band means
  in `bands_we_vs_rp.json`):
  - band 0: WE 0.36 (std 0.31), Rustpaper 0.19 (std 0.26);
  - bands 1-3: WE 0.36 / 0.24 / 0.15 vs 0.40 / 0.36 / 0.25;
  - bands 40-63: WE ~0.18 vs ~0.02.
  The shader reads band 0 only (`test_shader`, max freq range 1). A power
  curve on the normalised bands (x^0.5) halves the mean band error (0.164
  -> 0.091) but makes CITRUS worse (energy 1.68 -> 1.76), so the gap lies
  in the band dynamics (variation, attack and release), not only the
  level.
  **FIXED (audio.rs `LOW_SPREAD`, `BAND_SMOOTH`).** Under music, WE's band 0
  moves in small steps (0.075, 90 of 225 frames) around 0.36, about equal to
  band 1. Rustpaper's sat near 0 and jumped (0.169, 28 frames): its band 0
  (centre 45 Hz) is narrower than the music's bass and took only 0.3 of
  band 1. The 8 lowest bands now take 3x the spread from the bands above
  (consistent with "WE lights 2-3x more bands below ~700 Hz"), and every
  band is smoothed by 0.3 per 60 Hz step. Band 0 is then 0.36 / step 0.084.
  The tone sweep keeps its peak bands and values, and the level response
  slightly improves (1 kHz at 0.125: 0.380 = WE). Gate (2026-11-21):
  CITRUS FAIL -> PASS (energy 1.68 -> 1.19), no regression.
  **FIXED (audio.rs `BAND_REF_*`): music highs.** Bands from 8 up are
  normalized by their own adaptive reference (max with one neighbour each
  side), see the comment there. Fitted offline on the two music captures,
  their 8-14 s recordings replayed three times so the references carry
  history: mean band error 0.169 -> 0.074 / 0.173 -> 0.094. A spectral
  tilt (x f^a, a = 0.5..1) did NOT help: the single reference rescales it
  away (0.161 -> 0.147 at best, bass ruined). The bands below 8 keep the
  global reference (per band, CITRUS's ring flickered: energy 3.2).
- Music vs tones: with real music WE's bands all average ~0.22, the high
  ones included (band 48: 0.25); the single-reference model gave band 48
  0.02. Per-band references (above) were the missing piece; the tone sweep
  cannot show them because a new tone always becomes its band's peak.
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
- **Additive layers add their key's difference to the BIND pose** (the
  bone's local matrix), not to the animation's own first key. Lucy
  3521337568's intro layer (`single`, root 0.8x and offset at frame 0, the
  bind pose at its last frame) ended 1.25x bigger and shifted up-left when
  taken relative to its first key. Mean SSIM 0.611 -> 0.651; the other 6
  scenes with additive layers unchanged or better (Chainsaw Man motion
  0.427 -> 0.450).
- **Attachments (MDAT0001)**: named points on bones (u32, u16 count, then per
  attachment u16 bone, name, f32x16). Scene objects with `"attachment": name`
  and a puppet as `parent` are placed at parent world x current bone pose x
  attachment x their own local transform, every frame, parents first: hair
  and bodies built from many puppets follow each other (Kirito x Asuna
  3463520581: mean SSIM 0.76 -> 0.84).
- MDLV0023 has extra blocks between the indices and MDLS (the parser looks
  for the magic), and MDLS0004's animation offset points at the MDAT block
  that precedes MDLA.
- **A puppet is drawn into its layer target with the material's blending**,
  not copied opaque like a quad: its triangles overlap and their hulls are
  mostly transparent texels of a parts atlas, so a hair strand drawn after
  the face erased it. Chainsaw Man 3577990983's head was missing (only the
  last-drawn strands showed); Jinx 3078285611's hair was pink holes. Gate
  2026-11-21: Jinx 0.714 -> 0.944, Chainsaw Man 0.613 -> 0.653.
- **`cullmode: normal`** culls back faces. Our targets keep row 0 at the top
  (y flipped), so WE's back faces are GL front faces: culling GL BACK
  removed Chainsaw Man's whole puppet, FRONT leaves WE's image (stray hair
  pieces behind the head disappear).

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

## Puppet format notes (MDLV0023 / MDLA0006)

- Vertex flag `0x10000` adds 4 bytes right after the position (0 in
  Chainsaw Man 3577990983's `sl_puppet.mdl`: 84-byte vertices = position |
  4 bytes | normal (0,0,1) | tangent (1,0,0,1) | bones | weights | uv).
- MDLA0006 puts a large block after each animation's tracks (~43 KB in that
  file): the next header is searched up to the MDLA block's end offset (it
  was 4 KB). Jinx 3078285611 0.649 -> 0.701, Chainsaw Man 0.593 -> 0.611.
- After the indices MDLV0023 stores `u8 1, u32 1, u32 size` and one vec3
  per vertex (3741 x 12 bytes), and each MDLA0006 animation carries
  `u32 0, u32 1, name "", u32 244` and frames + 1 floats (61 x 1.0). The
  vec3s are each vertex's place IN THE TEXTURE, in pixels from its center:
  (u x 3840 - 1920, (0.5 - v) x 2867) = (119, 1117.5) for vertex 0, exactly
  its UV. The positions are the assembled pose (the bones' bind matrices sit
  on the position centroids, not on these). So it is the editor's
  "original image" shape with weight 1.0 everywhere: the same as drawing
  with the UVs, nothing to apply. The missing head was the blending above.
- Vertex flag `0x10000`'s 4 bytes are 0 except on 19 vertices that hold
  1.0, 2.0, 3.0... (as floats): unknown, ignored.

## Known gaps

- **SceneScript** runs sandboxed (`docs/SCENESCRIPT.md`). **`web`
  wallpapers** are not supported.
- Lonely Cat 3299228616: the HDR bloom of its water reflection is much
  brighter than WE's (the layer's own passes match; `general.bloom` with
  `hdr: true`). Not calibrated yet.
- Scene lights (`LIGHTING=1` materials use ambient only).
- Particles: **the prewarm (`starttime`) runs in real time**: it ignores
  `instanceoverride.rate`. In `make_particles.py pre_rate001` (rate 0.01,
  starttime 10, emission 2/s, life 12), WE shows 0.82x the dots of rate 1
  (0.338 vs 0.413), the count 10 s of real-time emission gives. Rustpaper
  had scaled the prewarm too, spawned nothing, and so hid rain
  1444077782's `Light shafts 0` (rate 0.01). Now 0.342, and rain's shafts
  are back at the right place. Gate: rain 0.862 -> 0.884, no regression.
- Known gap, rain 1444077782 (0.884, WE-vs-WE floor 0.981): ablating its
  road layer #692 (`isolate_effects.sh 1444077782 692`) points at its FIRST
  `waterripple` (variant 0.875; no effect 0.948; the second waterripple
  0.965, pulse 0.965). The scene stores that effect's constants under their
  LABEL keys (`ui_editor_properties_scroll_speed`...). `make_motionblur.py
  lbl_scroll` shows WE ignores label keys exactly like Rustpaper (77 vs 77
  px/s, the default speed), so both run the defaults. The difference lies
  elsewhere.
  **FIXED: effect passes sample textures in WE's padded layout.** Without
  pass 0's third texture the ripple matched (0.975). The rain packs an OLD
  waterripple whose slots are (mask, normal), while the scene fills them in
  the NEW order, so the 1280x800-in-2048x1024 "mask" is sampled AS THE
  NORMAL MAP at ripple scale 5.4, tiled, without any UV rescale. WE tiles
  the padded texture; Rustpaper had cropped effect textures to their
  content. `draw_pass` now substitutes `padded_texture` for named textures
  in effect passes too; g_TextureNResolution reports size and content, so
  rescaling shaders are unaffected. Gate (2026-11-21): rain FAIL -> PASS
  (0.884 -> 0.969), no regression.
- Particles: `instanceoverride.colorn` replaces the initializer color AND
  applies again on top (colorn^2). Calibrated with make_particles.py: on
  white dots `ov_colorn05` gives 0.25x the light in WE, while `ov_alpha05`
  gives 0.5x (alpha applies once). Found through Mai Sakurajima 1982790211's
  Light shafts: 4x too bright, found by ablating its overrides one at a
  time. Gate (2026-11-21): Mai and wind v7 2884352976 FAIL -> PASS, no
  regression.
- Particles: `remapvalue` with `output: velocity` + `operation: remap` is
  calibrated (`make_particles.py rv_*`, one dot). A constant range sets the
  velocity (100 vs 104 px/s). With noise, the input is the particle's AGE IN
  SECONDS x inputscale, not its life fraction: WE's direction changes stay
  ~2/s from lifetime 3 to 6 s and halve with inputscale 10 -> 5. The range
  is swept almost fully (std 130 of +-200), so the noise is scaled x2.3 and
  clamped. Before, the input was the life fraction and only half the range
  was used (std 65; 0.43 changes/s at lifetime 6). Gate (2026-11-21): no
  regression. Other outputs are still unsupported; for `speed` see the
  screen-mode notes. `mapsequencearoundcontrolpoint` (N even directions,
  velocity rotated per particle) STARTS the particle at its control point:
  `make_particles.py seqcp_v` (control point 0 at (400, 200), emitter at the
  center) puts WE's still dots at (394, 227), ours were at the center
  (now identical; seqcp with velocity too). Frieren 3195212886's cursor
  petals were a spinning star on her face (energy 1.79 -> 1.45).
  `mapsequencebetweencontrolpoints` (evenly spaced between control points 0
  and 1) is implemented but not calibrated. Sprite
  `orientation` modes coincide for the front-facing ortho camera (WE passes
  world camera axes; compensating object rotation broke White Angel).
- Particle geometry on scaled + rotated objects is VERIFIED
  (`make_particles.py srot_*`, `quad_*` with an elongated texture: object
  scale "2 1 1" / "0.3 1 1", rotated 0 / 90 deg, particle rotated 45 deg).
  Positions take the scale before the rotation, quads are squashed in
  object axes after the particle's own rotation, and everything matches WE
  within 0.002. Akame 1827211233's remaining fog difference (isolated fog:
  WE sx 0.425 vs Rustpaper 0.522) comes from 20 huge random fog sprites
  with random frames, on a scene whose WE-vs-WE floor is only 0.959 / 1.347:
  randomness-dominated like Winter Comfy.
- Particles on a non-uniformly scaled, rotated object: Akame 1827211233's fog
  (object 3.245 x 2.624, rotated ~90 deg) spreads less along x in WE than in
  RP. Stretching each quad along its own axes by sx/sy
  (`g_OrientationUp = (0, sx/sy, 0)` and texture ratio x sy/sx) was REJECTED
  by the regression gate (2026-11-20). Akame went 0.9448 -> 0.9469, still
  FAIL. 3576135309 went PASS -> FAIL: energy ratio 1.26 -> 2.72.
  1959836574 dropped 0.015. The real rule is still unknown. Next step: isolate
  the fog (`isolate.sh 1827211233 53`) and vary scale and rotation separately
  in a synthetic scene.
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
