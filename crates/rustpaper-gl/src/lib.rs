// rustpaper-gl: renders Wallpaper Engine scenes with OpenGL, offscreen.
//
// Module map:
//   audio    spectrum analysis for audio-reactive wallpapers
//   context  headless EGL + GL context (picks the real GPU, not llvmpipe)
//   assets   file lookup: scene.pkg -> wallpaper folder -> base WE assets
//   value    scene.json value helpers ({"user":..,"value":..} wrappers)
//   math     column-major matrices + the coordinate conventions
//   particles particle systems: definition, deterministic simulation, vertices
//   gpu      textures, render targets, blending, the shared quad
//   program  a WE shader compiled for one set of combos; uniforms by name
//   puppet   puppet warp: skeletal animation + CPU skinning of .mdl meshes
//   scene    the render graph: layers, effect chains, compositing
//   text     text objects: native clock/date presets, glyph rasterization
pub mod assets;
pub mod audio;
pub mod context;
pub mod gpu;
pub mod keyframes;
pub mod math;
pub mod particles;
pub mod program;
pub mod puppet;
pub mod scene;
pub mod text;
pub mod value;
