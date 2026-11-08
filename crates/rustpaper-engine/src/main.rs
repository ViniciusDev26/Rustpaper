use rustpaper_core::project::{Project, WallpaperKind};
use rustpaper_engine::{gl_wallpaper, wallpaper};
use std::path::PathBuf;

fn main() {
    // Takes the wallpaper folder (a Workshop item, with project.json).
    let dir = match std::env::args().nth(1) {
        Some(d) => PathBuf::from(d),
        None => {
            eprintln!("usage: rustpaper <wallpaper folder>");
            eprintln!(
                "e.g.:  rustpaper ~/.local/share/Steam/steamapps/workshop/content/431960/2824109832"
            );
            std::process::exit(2);
        }
    };
    let kind = Project::load(&dir).map(|p| p.kind).unwrap_or_else(|e| {
        eprintln!("error: reading {}/project.json: {e}", dir.display());
        std::process::exit(1);
    });
    // Scenes run on the OpenGL renderer (rustpaper-gl). The previous wgpu
    // compositor stays reachable with RUSTPAPER_LEGACY_SCENE=1 for comparison;
    // videos keep using the wgpu path.
    if kind == WallpaperKind::Scene && std::env::var_os("RUSTPAPER_LEGACY_SCENE").is_none() {
        gl_wallpaper::run(&dir);
    } else {
        wallpaper::run(&dir);
    }
}
