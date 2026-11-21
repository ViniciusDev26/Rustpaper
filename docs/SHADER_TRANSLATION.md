# Final boss: translating WE shaders to WGSL

Goal: render Wallpaper Engine scenes faithfully (materials/effects use the WE
shaders). This is the largest missing component — the bulk of
linux-wallpaperengine.

## What WE shaders are (investigated)

**Legacy** GLSL (GLSL ES 1.0 style). E.g. `assets/shaders/genericimage2.frag`:
- `#include "common_pbr.h"`, `#include "common_blending.h"` — custom includes
  (resolve from `assets/shaders/` and `assets/shaders/base/`).
- `uniform sampler2D g_Texture0; // {"label":...,"combo":"NORMALMAP","default":...}`
  — uniforms annotated with JSON (defaults, ranges, combos, texture format).
- `#if COMBO` / `#ifndef VERSION` — **combos** are feature flags set per
  material (`passes[].combos` in material.json) and by texture presence.
- `varying vec2 v_TexCoord;`, `texture2D(...)`, probably `gl_FragColor` —
  old GLSL constructs.

## Why lwe has an advantage

It uses **OpenGL**, so it runs the WE GLSL almost directly: it only resolves includes +
injects the combo `#define`s + a few patches. We use **wgpu (WGSL)**, so
we need GLSL→WGSL.

## PROVEN pipeline (compiles end to end with the real genericimage2)

    WE .vert/.frag
      │  shader::translate  (rustpaper-core)
      ▼  resolves #include + prelude (dialect→GLSL) + combos + SPLITS samplers
    GLSL 450 (Vulkan-flavored, separate samplers)
      │  glslangValidator -V -R --amb --aml --sdub WeGlobals 0 0 -S <stage>
      ▼
    SPIR-V Vulkan   (spirv-val limpo)
      │  naga spv-in + Validator   (== o que o wgpu faz)
      ▼
    naga::Module    → wgpu::ShaderSource::SpirV

Resulting bindings (deterministic, see `shader::texture_bindings`):
the free-uniforms block `WeGlobals` = set 0 binding 0; texture `g_TextureN` =
binding `2N+1`; sampler `_smp_g_TextureN` = binding `2N+2`.

### Empirical findings (why this path and not another)

- **naga glsl-in is too weak**: it rejects `#version 330`, requires free uniforms
  to live in blocks with a binding, and does NOT implement sampler qualifiers
  (`uniform sampler2D` → `NotImplemented("variable qualifier")`). Discarded.
- **glslang accepts the dialect** (with the prelude) and emits clean Vulkan SPIR-V.
  However, Vulkan forbids free uniforms → `-R` (relaxed rules) + `--sdub` gathers all
  of them into a named default block.
- **naga spv-in rejects COMBINED samplers** (the GL-style `sampler2D` that
  glslang emits: `InvalidId` on the `OpLoad` of the sampled image). Besides, wgpu's
  bind group model is SEPARATE (distinct texture + sampler) — there is no
  combined sampler. So `translate` rewrites
  `uniform sampler2D g_TexN` → `uniform texture2D g_TexN` + `uniform sampler
  _smp_g_TexN`, and the `texSample2D/Lod` macros reassemble `sampler2D(tex, smp)` at
  sampling time. glslang then emits separate image+sampler, which naga spv-in
  ACCEPTS.

Proven by `crates/rustpaper-core/tests/naga_compile.rs` (genericimage2 vert+frag).

## Render status (offscreen, verified by PNG)

- **Real background** renders through the real WE shader (`render_scene` example, Zoro scene).
- **Effect chain** works (`render_effect`): background → `tint` effect sampling
  the previous frame. `postprocess::Pass` is the reusable pass primitive.
- **Reflection** (`shader_compile::reflect`): reads the uniform block layout from the SPIR-V
  (offsets by name) and the texture/sampler bindings via naga.
- **Parsers** (rustpaper-core): `scene::MaterialInfo` (shader/combos/constants),
  `effects` (effect graph: object→effect.json→passes/fbos/binds + overrides),
  `shader::parse_params` (`// {json}` annotations: uniform↔material + defaults).

### UBO unified across stages (the last blocker, resolved)

Most effects need the WE VERTEX shader (e.g. filmgrain computes
`v_TexCoordNoise` from `g_Time` in the vertex stage). But vertex and fragment declare
DIFFERENT members in the `WeGlobals` block — compiled separately, they would collide on
binding 0. Solution: compile vertex+fragment LINKED (`glslang -l`), which UNIFIES the
default block — both SPIR-V outputs have the SAME struct and the SAME offsets. See
`shader_compile::compile_linked` and the test `rustpaper-engine/tests/linked_compile.rs`
(genericimage2: unified 96-byte UBO, `g_ModelViewProjectionMatrix@0` +
`g_Brightness@88` visible in both stages).

### What is missing for faithful animated effects

Orchestration (no unknowns): use `compile_linked` (WE vert+frag) per pass,
build the unified UBO from `parse_params` (defaults) + constants (material/scene) +
builtins (`g_Time`, resolutions), resolve the effect's input textures (previous
framebuffer via `bind`, plus base textures such as `util/noise`), and orchestrate
multi-pass/multi-FBO (blur, godrays). Live integration into `wallpaper.rs` last.

### Phases (for a future session, with fresh context)

1. **Resolve includes** — a module that reads a WE shader and expands `#include "x.h"`
   recursively from `we-assets/shaders/` (+ `/base`). Pure text logic →
   testable in rustpaper-core.
2. **Combos/defines** — read `passes[].combos` from the material + the implicit combos
   (e.g. a present texture turns on a combo). Build the list of `#define NAME value`.
   The `// {json}` annotations on uniforms give the combo defaults.
3. **Modernize the GLSL** so naga accepts it: `#version 450`, `varying`→`in/out`
   (per stage), `attribute`→`in`, `texture2D`→`texture`, `gl_FragColor`→a
   `layout(location=0) out vec4`, remove/adapt whatever naga cannot swallow. This is
   the most tedious and iterative step (naga's GLSL frontend is demanding).
4. **Bindings/uniforms** — map the `g_*` (MVP matrices, g_Time, g_Texture0..N,
   material parameters) to a uniform buffer + bind groups. It must match
   what the vert/frag expect (the `varying`s connect the two stages).
5. **Wire into the engine** — instead of our fixed per-scene `shader.wgsl`, compile the
   scene's material (shader + combos + textures) and draw each image layer
   with its material. This also unlocks **multiple image layers**.
6. **Effects / FBO** — the effect chain (bloom, etc.) renders to a texture
   (FBO) and chains passes. HDR/tonemapping. This is what "heavy" scenes need so they
   do not blow out to white (the current problem with additive particles).

### Risks / reality

- naga's GLSL frontend does not accept everything (old ES 1.0, extensions). Phase 3 is
  trial and error; some shaders may not compile without specific patches.
- Combos: the explosion of variants (`#if`) is large; start only with the combos that
  the target scenes use (e.g. `genericimage2` without PBR/lighting).
- This is **weeks** of work. Strategy: get ONE simple material
  (`genericimage2`, no combos) rendering through naga first (milestone 1), then
  grow. Do not try everything at once.

## Suggested starting point (milestone 1)

Take `genericimage2.vert/.frag`, resolve includes, no combos, modernize the
minimum, compile via `ShaderSource::Glsl`, and draw the background layer of a
simple image scene with IT (instead of our fixed shader). If a textured quad
shows up through the WE shader translated by naga, the path is proven.

## `clamp` argument order (GL renderer)

Some Workshop shaders call `clamp(x, lo, hi)` with lo > hi, e.g. bokeh
2798319181's `clamp(0, 0.15, depth)`. HLSL defines that as
`min(max(x, lo), hi)`, while GLSL leaves it undefined (NVIDIA returns lo). The
GL prelude maps `clamp` to the HLSL form (こきん 3146507587 FAIL -> PASS, see
GL_RENDERER.md).
