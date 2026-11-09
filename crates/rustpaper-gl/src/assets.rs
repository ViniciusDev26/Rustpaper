// Where a scene's files come from.
//
// A scene references files by relative path ("materials/x.json",
// "shaders/effects/shake.frag", "materials/util/noise.tex"...). The same path can
// live in three places, checked in this order:
//   1. the wallpaper's `<scene file>.pkg`, usually `scene.pkg` (most scenes
//      pack everything here);
//   2. the wallpaper folder itself (unpacked scenes);
//   3. the base WE assets (shared effects, shaders, util textures).

use std::borrow::Cow;
use std::path::{Path, PathBuf};

use rustpaper_core::pkg::Pkg;

pub struct Assets {
    pkg: Option<Pkg>,
    dir: PathBuf,
    base: PathBuf,
}

impl Assets {
    pub fn open(wallpaper_dir: &Path, base_assets: &Path) -> std::io::Result<Assets> {
        // The pkg is named after the project's scene file: scene.json ->
        // scene.pkg, gifscene.json -> gifscene.pkg (Aesthetic City 843532366).
        let stem = std::fs::read_to_string(wallpaper_dir.join("project.json"))
            .ok()
            .and_then(|t| {
                serde_json::from_str::<serde_json::Value>(t.trim_start_matches('\u{feff}')).ok()
            })
            .and_then(|p| {
                Some(
                    p.get("file")?
                        .as_str()?
                        .trim_end_matches(".json")
                        .to_string(),
                )
            })
            .filter(|s| !s.is_empty() && !s.contains('/'))
            .unwrap_or_else(|| "scene".to_string());
        let mut pkg_path = wallpaper_dir.join(format!("{stem}.pkg"));
        if !pkg_path.is_file() {
            pkg_path = wallpaper_dir.join("scene.pkg");
        }
        let pkg = if pkg_path.is_file() {
            Some(Pkg::open(&pkg_path)?)
        } else {
            None
        };
        Ok(Assets {
            pkg,
            dir: wallpaper_dir.to_path_buf(),
            base: base_assets.to_path_buf(),
        })
    }

    pub fn read(&self, path: &str) -> Option<Cow<'_, [u8]>> {
        if let Some(b) = self.pkg.as_ref().and_then(|p| p.read(path)) {
            return Some(Cow::Borrowed(b));
        }
        for root in [&self.dir, &self.base] {
            if let Ok(b) = std::fs::read(root.join(path)) {
                return Some(Cow::Owned(b));
            }
        }
        None
    }

    pub fn read_string(&self, path: &str) -> Option<String> {
        let bytes = self.read(path)?;
        // Some WE JSON files start with a UTF-8 BOM; serde_json rejects it.
        let s = String::from_utf8_lossy(&bytes);
        Some(s.trim_start_matches('\u{feff}').to_string())
    }

    pub fn read_json(&self, path: &str) -> Option<serde_json::Value> {
        serde_json::from_str(&self.read_string(path)?).ok()
    }

    /// Base shaders folder, used to resolve `#include "common.h"` and friends.
    pub fn shaders_dir(&self) -> PathBuf {
        self.base.join("shaders")
    }
}
