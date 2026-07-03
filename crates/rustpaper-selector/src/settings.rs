// Persistência do selector: guarda o último wallpaper aplicado e a preferência
// de autostart, e gerencia o arquivo .desktop de autostart (padrão XDG).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct Settings {
    pub last_id: Option<String>,
    pub autostart: bool,
}

fn config_home() -> PathBuf {
    if let Some(x) = std::env::var_os("XDG_CONFIG_HOME") {
        PathBuf::from(x)
    } else {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default();
        home.join(".config")
    }
}

fn state_path() -> PathBuf {
    config_home().join("rustpaper").join("state.json")
}

fn autostart_file() -> PathBuf {
    config_home().join("autostart").join("rustpaper.desktop")
}

pub fn load() -> Settings {
    std::fs::read_to_string(state_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save(s: &Settings) {
    if let Some(dir) = state_path().parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(json) = serde_json::to_string_pretty(s) {
        let _ = std::fs::write(state_path(), json);
    }
}

// Escreve o .desktop de autostart apontando pro engine + a pasta do wallpaper.
// Aspas nos caminhos (a spec do .desktop pede pra caminhos com caracteres especiais).
pub fn write_autostart(engine: &Path, wallpaper_dir: &Path) -> std::io::Result<()> {
    if let Some(dir) = autostart_file().parent() {
        std::fs::create_dir_all(dir)?;
    }
    let content = format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Rustpaper\n\
         Exec=\"{}\" \"{}\"\n\
         X-GNOME-Autostart-enabled=true\n\
         NoDisplay=true\n",
        engine.display(),
        wallpaper_dir.display()
    );
    std::fs::write(autostart_file(), content)
}

pub fn remove_autostart() {
    let _ = std::fs::remove_file(autostart_file());
}
