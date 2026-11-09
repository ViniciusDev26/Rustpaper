// Module `paths`: where the Wallpaper Engine files live on this machine.
//
// Two folders matter:
//   - the base assets shipped with WE (`.../common/wallpaper_engine/assets`):
//     shared shaders, effects, materials and particle sprites that scenes
//     reference OUTSIDE their own pkg;
//   - the Workshop content folder (`.../workshop/content/431960`): one `<id>/`
//     folder per subscribed wallpaper.
//
// Both can be overridden with env vars (useful for containers / CI); otherwise
// we look in the default Steam library.

use std::path::PathBuf;

/// Steam app id of Wallpaper Engine (also the Workshop content folder name).
pub const WE_APP_ID: &str = "431960";

fn steam_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        roots.push(home.join(".local/share/Steam"));
        roots.push(home.join(".steam/steam"));
        roots.push(home.join(".var/app/com.valvesoftware.Steam/.local/share/Steam"));
    }
    roots
}

fn first_existing(env: &str, suffix: &str) -> Option<PathBuf> {
    if let Some(p) = std::env::var_os(env).map(PathBuf::from) {
        return Some(p);
    }
    steam_roots()
        .into_iter()
        .map(|r| r.join(suffix))
        .find(|p| p.is_dir())
}

/// Base WE assets dir (`$WE_ASSETS_DIR`, or the Steam install).
pub fn assets_dir() -> Option<PathBuf> {
    first_existing("WE_ASSETS_DIR", "steamapps/common/wallpaper_engine/assets")
}

/// Shaders dir inside the base assets (`$WE_SHADERS_DIR` wins, for old tooling).
pub fn shaders_dir() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("WE_SHADERS_DIR").map(PathBuf::from) {
        return Some(p);
    }
    assets_dir().map(|a| a.join("shaders"))
}

/// Workshop content dir (`$WE_WORKSHOP_DIR`, or the Steam library).
pub fn workshop_dir() -> Option<PathBuf> {
    first_existing(
        "WE_WORKSHOP_DIR",
        &format!("steamapps/workshop/content/{WE_APP_ID}"),
    )
}

/// Folder of the downloaded test corpus (harness/download-testset.sh):
/// `$RUSTPAPER_TESTSET_DIR` or ~/.local/share/rustpaper-testset.
pub fn testset_dir() -> Option<PathBuf> {
    let root = std::env::var_os("RUSTPAPER_TESTSET_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .map(|h| PathBuf::from(h).join(".local/share/rustpaper-testset"))
        })?;
    let p = root.join(format!("steamapps/workshop/content/{WE_APP_ID}"));
    p.is_dir().then_some(p)
}

/// Every folder that holds wallpapers (`<id>/project.json`): the Steam Workshop
/// folder and the test corpus, in that order.
pub fn wallpaper_dirs() -> Vec<PathBuf> {
    workshop_dir().into_iter().chain(testset_dir()).collect()
}

/// Resolves a Workshop id to its folder, searching `wallpaper_dirs()`.
pub fn find_wallpaper(id: &str) -> Option<PathBuf> {
    wallpaper_dirs()
        .into_iter()
        .map(|d| d.join(id))
        .find(|d| d.join("project.json").is_file())
}
