// rustpaper-core: what the engine and the selector share. Where Wallpaper
// Engine's files live, what a project.json says, the selector's decisions
// (state.json) and WE's quality settings as data. The wallpapers themselves
// are rendered by the real WE (see rustpaper-engine).
pub mod omarchy;
pub mod paths;
pub mod performance;
pub mod project;
pub mod state;
pub mod steam;
