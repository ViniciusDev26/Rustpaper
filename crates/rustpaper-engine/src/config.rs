//! What the engine shows and how: from the command line, or from the
//! selector's state.json (`--service`).

use std::path::{Path, PathBuf};

use rustpaper_core::performance;
use rustpaper_core::state::{self, Playback};
use serde_json::{Map, Value};

/// What to show and how.
#[derive(Clone, PartialEq)]
pub struct Config {
    /// The wallpaper's project.json (absolute).
    pub project: PathBuf,
    /// User property values `{ "name": value }`.
    pub properties: Map<String, Value>,
    pub mute: bool,
    pub playback: Playback,
    /// WE quality settings (sanitized); empty = leave WE's as they are.
    pub performance: Map<String, Value>,
}

/// Where the Config comes from.
pub enum Source {
    /// `rustpaper <dir> [...]`: fixed.
    Args(Config),
    /// `rustpaper --service`: the selector's state.json, re-read on SIGHUP.
    Service,
}

fn usage() -> ! {
    eprintln!(
        "usage: rustpaper <wallpaper folder> [--props properties.json] [--mute] [--volume 0..100] [--output NAME]\n       rustpaper --service [--output NAME]   (the selector's choice; SIGHUP re-reads it)"
    );
    std::process::exit(2);
}

pub fn parse_args() -> (Source, Option<String>) {
    let mut args = std::env::args().skip(1);
    let mut dir = None;
    let mut props_file: Option<PathBuf> = None;
    let mut mute = false;
    let mut output = None;
    let mut service = false;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--service" => service = true,
            "--props" => props_file = args.next().map(PathBuf::from),
            "--mute" | "--no-audio" => mute = true,
            // WE has no volume command: anything above 0 plays at WE's own
            // volume (set in WE's settings)
            "--volume" => mute = args.next().and_then(|v| v.parse::<u32>().ok()) == Some(0),
            "--output" => output = args.next(),
            // options of the old native renderer, accepted so existing
            // autostart entries keep working
            "--fps" => {
                args.next();
            }
            "--no-scripts" => {}
            "-h" | "--help" => usage(),
            _ if a.starts_with('-') => usage(),
            _ => dir = Some(PathBuf::from(a)),
        }
    }
    if service {
        return (Source::Service, output);
    }
    let Some(dir) = dir else { usage() };
    let properties = props_file
        .and_then(|f| std::fs::read_to_string(f).ok())
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();
    let project = project_file(&dir).unwrap_or_else(|e| {
        eprintln!("error: {e}");
        std::process::exit(1);
    });
    let config = Config {
        project,
        properties,
        mute,
        playback: Playback::default(),
        performance: Map::new(),
    };
    (Source::Args(config), output)
}

fn project_file(dir: &Path) -> Result<PathBuf, String> {
    let project = dir.join("project.json");
    if !project.is_file() {
        return Err(format!("{} not found", project.display()));
    }
    Ok(std::fs::canonicalize(&project).unwrap_or(project))
}

/// The selector's choice (`~/.config/rustpaper/state.json`).
pub fn service_config() -> Result<Config, String> {
    let st = state::load();
    let id = st
        .last_id
        .clone()
        .ok_or("no wallpaper chosen yet (pick one in rustpaper-selector)")?;
    let dir =
        rustpaper_core::paths::find_wallpaper(&id).ok_or(format!("wallpaper {id} not found"))?;
    Ok(Config {
        project: project_file(&dir)?,
        properties: state::load_properties(&id),
        mute: st.volume == 0,
        playback: st.playback,
        performance: performance::sanitize(&st.performance),
    })
}
