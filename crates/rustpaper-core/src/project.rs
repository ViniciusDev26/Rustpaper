// `project` module: reads the `project.json` of a Wallpaper Engine wallpaper and
// determines its type (video/scene/web) and main file.

use std::path::{Path, PathBuf};

// The WE wallpaper types we care about. `Unknown` keeps the original text
// (including untyped ones, which become an empty string).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WallpaperKind {
    Video,
    Scene,
    Web,
    Unknown(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    pub kind: WallpaperKind,
    pub file: String, // main file, relative to the folder (ex.: "video.mp4")
    pub title: String,
}

// Raw mirror of project.json (only the fields we use). serde fills it in from
// the JSON; #[serde(default)] lets missing fields become None instead of an error.
#[derive(serde::Deserialize)]
struct RawProject {
    // `type` is a Rust keyword; r#type maps to the JSON key "type".
    #[serde(default)]
    r#type: Option<String>,
    #[serde(default)]
    file: Option<String>,
    #[serde(default)]
    title: Option<String>,
}

impl Project {
    // PURE parse (JSON string -> Project). Testable without touching the disk.
    pub fn parse(json: &str) -> Result<Project, serde_json::Error> {
        let raw: RawProject = serde_json::from_str(json)?;
        // Casing varies in WE (scene/Scene, video/Video) -> normalize.
        let kind = match raw
            .r#type
            .as_deref()
            .unwrap_or("")
            .to_ascii_lowercase()
            .as_str()
        {
            "video" => WallpaperKind::Video,
            "scene" => WallpaperKind::Scene,
            "web" => WallpaperKind::Web,
            other => WallpaperKind::Unknown(other.to_string()),
        };
        Ok(Project {
            kind,
            file: raw.file.unwrap_or_default(),
            title: raw.title.unwrap_or_default(),
        })
    }

    // Reads and parses the project.json inside a wallpaper folder.
    pub fn load(dir: &Path) -> std::io::Result<Project> {
        let json = std::fs::read_to_string(dir.join("project.json"))?;
        Project::parse(&json).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }

    // Absolute path of the main file, given the wallpaper folder.
    pub fn file_path(&self, dir: &Path) -> PathBuf {
        dir.join(&self.file)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_video() {
        let json = r#"{ "type": "video", "file": "clip.mp4", "title": "My Video" }"#;
        let p = Project::parse(json).unwrap();
        assert_eq!(p.kind, WallpaperKind::Video);
        assert_eq!(p.file, "clip.mp4");
        assert_eq!(p.title, "My Video");
    }

    #[test]
    fn parses_scene() {
        let json = r#"{ "type": "scene", "file": "scene.json" }"#;
        let p = Project::parse(json).unwrap();
        assert_eq!(p.kind, WallpaperKind::Scene);
        assert_eq!(p.file, "scene.json");
        assert_eq!(p.title, ""); // missing -> empty string
    }

    #[test]
    fn type_is_case_insensitive() {
        // In WE both "Video" and "video" appear.
        assert_eq!(
            Project::parse(r#"{"type":"Video","file":"a.mp4"}"#)
                .unwrap()
                .kind,
            WallpaperKind::Video
        );
        assert_eq!(
            Project::parse(r#"{"type":"Scene","file":"scene.json"}"#)
                .unwrap()
                .kind,
            WallpaperKind::Scene
        );
    }

    #[test]
    fn unknown_and_missing_type() {
        // Unsupported type (web) -> Unknown with the normalized text.
        assert_eq!(
            Project::parse(r#"{"type":"web","file":"index.html"}"#)
                .unwrap()
                .kind,
            WallpaperKind::Web
        );
        // No type field -> Unknown("").
        assert_eq!(
            Project::parse(r#"{"file":""}"#).unwrap().kind,
            WallpaperKind::Unknown(String::new())
        );
    }

    #[test]
    fn rejects_invalid_json() {
        assert!(Project::parse("not json").is_err());
    }

    #[test]
    fn file_path_joins_dir() {
        let p = Project::parse(r#"{"type":"video","file":"clip.mp4"}"#).unwrap();
        assert_eq!(
            p.file_path(Path::new("/wallpapers/123")),
            PathBuf::from("/wallpapers/123/clip.mp4")
        );
    }
}
