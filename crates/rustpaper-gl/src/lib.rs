// rustpaper-gl: renders Wallpaper Engine scenes with OpenGL, offscreen.
//
// Module map:
//   context  headless EGL + GL context (picks the real GPU, not llvmpipe)
//   assets   file lookup: scene.pkg -> wallpaper folder -> base WE assets
//   value    scene.json value helpers ({"user":..,"value":..} wrappers)
//   math     column-major matrices + the coordinate conventions
//   gpu      textures, render targets, blending, the shared quad
//   program  a WE shader compiled for one set of combos; uniforms by name
//   scene    the render graph: layers, effect chains, compositing
pub mod assets;
pub mod context;
pub mod gpu;
pub mod math;
pub mod program;
pub mod scene;
pub mod value;
