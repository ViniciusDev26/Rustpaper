// Selector: app desktop Tauri pra escolher wallpaper. O backend (Rust) varre o
// catálogo do Workshop usando o rustpaper-core e expõe comandos pro frontend (web):
//   list_wallpapers -> lista {id, título, tipo, preview}
//   apply(id)       -> spawna o engine renderizando aquele wallpaper

mod settings;

use std::path::{Path, PathBuf};
use std::process::Child;
use std::sync::Mutex;

use serde::Serialize;
use rustpaper_core::project::{Project, WallpaperKind};

// Pasta do Workshop montada no container. TODO: tornar configurável.
const WORKSHOP_DIR: &str = "/home/vscode/wallpapers";

// Guarda o processo do engine em execução, pra matá-lo ao trocar de wallpaper.
struct EngineState(Mutex<Option<Child>>);

#[derive(Serialize)]
struct WallpaperEntry {
    id: String,          // nome da pasta (id do Workshop)
    title: String,
    kind: String,        // "video" | "scene" | "web" | "unknown"
    preview: Option<String>, // caminho absoluto da imagem de preview
    supported: bool,     // o engine sabe tocar? (video/scene)
}

// Acha o arquivo de preview da pasta (estático primeiro, gif por último).
fn find_preview(dir: &Path) -> Option<PathBuf> {
    for name in ["preview.jpg", "preview.png", "preview.jpeg", "preview.gif"] {
        let p = dir.join(name);
        if p.exists() {
            return Some(p);
        }
    }
    None
}

#[tauri::command]
fn list_wallpapers() -> Vec<WallpaperEntry> {
    let base = Path::new(WORKSHOP_DIR);
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(base) else {
        return out;
    };
    for e in entries.flatten() {
        let dir = e.path();
        if !dir.is_dir() {
            continue;
        }
        // Só entra quem tem project.json legível.
        let Ok(project) = Project::load(&dir) else {
            continue;
        };
        let (kind, supported) = match project.kind {
            WallpaperKind::Video => ("video", true),
            WallpaperKind::Scene => ("scene", true),
            WallpaperKind::Web => ("web", false),
            WallpaperKind::Unknown(_) => ("unknown", false),
        };
        out.push(WallpaperEntry {
            id: e.file_name().to_string_lossy().into_owned(),
            title: project.title,
            kind: kind.to_string(),
            preview: find_preview(&dir).map(|p| p.to_string_lossy().into_owned()),
            supported,
        });
    }
    out.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()));
    out
}

// Caminho do binário do engine (mesma pasta do selector: target/debug/).
fn engine_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let candidate = exe.parent()?.join("rustpaper");
    candidate.exists().then_some(candidate)
}

#[tauri::command]
fn apply(id: String, state: tauri::State<EngineState>) -> Result<(), String> {
    let dir = Path::new(WORKSHOP_DIR).join(&id);
    if !dir.join("project.json").exists() {
        return Err(format!("project.json não encontrado para {id}"));
    }
    let engine = engine_path().ok_or("binário do engine não encontrado")?;

    let mut guard = state.0.lock().unwrap();
    // Mata o wallpaper anterior antes de subir o novo.
    if let Some(mut child) = guard.take() {
        let _ = child.kill();
        let _ = child.wait();
    }
    let child = std::process::Command::new(&engine)
        .arg(&dir)
        .spawn()
        .map_err(|e| format!("falha ao iniciar o engine: {e}"))?;
    *guard = Some(child);
    drop(guard);

    // Persiste o último wallpaper e, se o autostart estiver ligado, aponta o
    // .desktop pra ele (pra voltar no próximo login).
    let mut s = settings::load();
    s.last_id = Some(id);
    if s.autostart {
        let _ = settings::write_autostart(&engine, &dir);
    }
    settings::save(&s);
    Ok(())
}

#[tauri::command]
fn get_settings() -> settings::Settings {
    settings::load()
}

#[tauri::command]
fn set_autostart(enabled: bool) -> Result<(), String> {
    let mut s = settings::load();
    s.autostart = enabled;
    settings::save(&s);

    if enabled {
        // Precisa de um wallpaper já aplicado pra saber o que restaurar.
        let engine = engine_path().ok_or("binário do engine não encontrado")?;
        match &s.last_id {
            Some(id) => {
                let dir = Path::new(WORKSHOP_DIR).join(id);
                settings::write_autostart(&engine, &dir).map_err(|e| e.to_string())?;
            }
            None => return Err("aplique um wallpaper antes de ligar o autostart".into()),
        }
    } else {
        settings::remove_autostart();
    }
    Ok(())
}

fn main() {
    setup_linux_env();

    tauri::Builder::default()
        .manage(EngineState(Mutex::new(None)))
        .invoke_handler(tauri::generate_handler![
            list_wallpapers,
            apply,
            get_settings,
            set_autostart
        ])
        .run(tauri::generate_context!())
        .expect("erro ao rodar o app Tauri");
}

// Acomodações pra rodar o webview (webkit2gtk) dentro do devcontainer:
// - GDK_BACKEND=x11: o backend Wayland dá "Protocol error" pelo socket montado.
// - WEBKIT_DISABLE_*: desliga compositing/DMABUF por GPU (falham no container).
// NÃO mexemos em WAYLAND_DISPLAY: o engine (filho) precisa dele pro layer-shell.
// TODO: rever ao empacotar pra distribuição.
fn setup_linux_env() {
    let defaults = [
        ("GDK_BACKEND", "x11"),
        ("WEBKIT_DISABLE_COMPOSITING_MODE", "1"),
        ("WEBKIT_DISABLE_DMABUF_RENDERER", "1"),
    ];
    for (key, val) in defaults {
        if std::env::var_os(key).is_none() {
            unsafe { std::env::set_var(key, val) };
        }
    }
}
