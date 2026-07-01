// Módulo `project`: lê o `project.json` de um wallpaper do Wallpaper Engine e
// descobre o tipo (video/scene/web) e o arquivo principal.

use std::path::{Path, PathBuf};

// Os tipos de wallpaper do WE que nos importam. `Unknown` guarda o texto original
// (inclui os sem tipo, que viram string vazia).
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
    pub file: String,  // arquivo principal, relativo à pasta (ex.: "video.mp4")
    pub title: String,
}

// Espelho cru do project.json (só os campos que usamos). serde preenche a partir
// do JSON; #[serde(default)] deixa campos ausentes virarem None em vez de erro.
#[derive(serde::Deserialize)]
struct RawProject {
    // `type` é palavra-chave em Rust; r#type mapeia pra chave "type" do JSON.
    #[serde(default)]
    r#type: Option<String>,
    #[serde(default)]
    file: Option<String>,
    #[serde(default)]
    title: Option<String>,
}

impl Project {
    // Parse PURO (string JSON -> Project). Testável sem tocar em disco.
    pub fn parse(json: &str) -> Result<Project, serde_json::Error> {
        let raw: RawProject = serde_json::from_str(json)?;
        // Casing varia no WE (scene/Scene, video/Video) -> normaliza.
        let kind = match raw.r#type.as_deref().unwrap_or("").to_ascii_lowercase().as_str() {
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

    // Lê e parseia o project.json dentro de uma pasta de wallpaper.
    pub fn load(dir: &Path) -> std::io::Result<Project> {
        let json = std::fs::read_to_string(dir.join("project.json"))?;
        Project::parse(&json)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }

    // Caminho absoluto do arquivo principal, dado a pasta do wallpaper.
    pub fn file_path(&self, dir: &Path) -> PathBuf {
        dir.join(&self.file)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_video() {
        let json = r#"{ "type": "video", "file": "clip.mp4", "title": "Meu Vídeo" }"#;
        let p = Project::parse(json).unwrap();
        assert_eq!(p.kind, WallpaperKind::Video);
        assert_eq!(p.file, "clip.mp4");
        assert_eq!(p.title, "Meu Vídeo");
    }

    #[test]
    fn parses_scene() {
        let json = r#"{ "type": "scene", "file": "scene.json" }"#;
        let p = Project::parse(json).unwrap();
        assert_eq!(p.kind, WallpaperKind::Scene);
        assert_eq!(p.file, "scene.json");
        assert_eq!(p.title, ""); // ausente -> string vazia
    }

    #[test]
    fn type_is_case_insensitive() {
        // No WE aparecem tanto "Video" quanto "video".
        assert_eq!(Project::parse(r#"{"type":"Video","file":"a.mp4"}"#).unwrap().kind, WallpaperKind::Video);
        assert_eq!(Project::parse(r#"{"type":"Scene","file":"scene.json"}"#).unwrap().kind, WallpaperKind::Scene);
    }

    #[test]
    fn unknown_and_missing_type() {
        // Tipo não suportado (web) -> Unknown com o texto normalizado.
        assert_eq!(Project::parse(r#"{"type":"web","file":"index.html"}"#).unwrap().kind, WallpaperKind::Web);
        // Sem campo type -> Unknown("").
        assert_eq!(Project::parse(r#"{"file":""}"#).unwrap().kind, WallpaperKind::Unknown(String::new()));
    }

    #[test]
    fn rejects_invalid_json() {
        assert!(Project::parse("nao é json").is_err());
    }

    #[test]
    fn file_path_joins_dir() {
        let p = Project::parse(r#"{"type":"video","file":"clip.mp4"}"#).unwrap();
        assert_eq!(p.file_path(Path::new("/wallpapers/123")), PathBuf::from("/wallpapers/123/clip.mp4"));
    }
}
