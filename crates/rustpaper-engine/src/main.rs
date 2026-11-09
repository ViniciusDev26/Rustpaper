use rustpaper_core::project::{Project, WallpaperKind};
use rustpaper_engine::{gl_wallpaper, wallpaper};
use std::path::PathBuf;

fn main() {
    // rustpaper <wallpaper folder> [--fps N] [--props properties.json] [--no-audio]
    let mut args = std::env::args().skip(1);
    let mut dir = None;
    let mut fps = 30;
    let mut props_file: Option<PathBuf> = None;
    let mut audio = true;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--fps" => fps = args.next().and_then(|v| v.parse().ok()).unwrap_or(30),
            "--props" => props_file = args.next().map(PathBuf::from),
            "--no-audio" => audio = false,
            _ => dir = Some(PathBuf::from(a)),
        }
    }
    let Some(dir) = dir else {
        eprintln!(
            "usage: rustpaper <wallpaper folder> [--fps N] [--props properties.json] [--no-audio]"
        );
        std::process::exit(2);
    };
    let kind = Project::load(&dir).map(|p| p.kind).unwrap_or_else(|e| {
        eprintln!("error: reading {}/project.json: {e}", dir.display());
        std::process::exit(1);
    });
    // User property values chosen in the selector: { "name": value, ... }
    let properties = props_file
        .and_then(|f| std::fs::read_to_string(f).ok())
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();
    // Scenes run on the OpenGL renderer (rustpaper-gl). The previous wgpu
    // compositor stays reachable with RUSTPAPER_LEGACY_SCENE=1 for comparison;
    // videos keep using the wgpu path.
    if kind == WallpaperKind::Scene && std::env::var_os("RUSTPAPER_LEGACY_SCENE").is_none() {
        gl_wallpaper::run(
            &dir,
            gl_wallpaper::Options {
                fps,
                properties,
                audio,
            },
        );
    } else {
        wallpaper::run(&dir);
    }
}
