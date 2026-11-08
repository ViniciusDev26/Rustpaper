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
  8-bit values. Mipmaps are generated for every texture.
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
  evolving noise field (not accelerate, not curl); `offset` of
  turbulentvelocityrandom is a noise phase, not a push.

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

## Known gaps

- Text objects, puppet/skeletal animation, SceneScript (JS-driven properties,
  e.g. audio visualizers), audio uniforms, camera parallax.
- Particles: rope renderers (`rope`, `ropetrail`: genericropeparticle shader,
  9 scenes), operator `remapvalue`, initializers `mapsequence*`, audio
  response, `orientation` other than screen-facing.
- ~10 Workshop shaders rely on HLSL implicit conversions (vec4 -> vec3,
  int -> uint) that GLSL rejects; those passes are skipped (image passes
  through unchanged).
