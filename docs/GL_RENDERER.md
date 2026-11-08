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

`--describe` lists layers + every load/compile warning. `--dump-passes` writes
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

## Known gaps (as of the first version)

- Particles, text objects, puppet/skeletal animation, SceneScript (JS-driven
  properties, e.g. audio visualizers), audio uniforms, camera parallax.
- ~10 Workshop shaders rely on HLSL implicit conversions (vec4 -> vec3,
  int -> uint) that GLSL rejects; those passes are skipped (image passes
  through unchanged).
