//! What the engine shows and how: from the command line, or from the
//! selector's state.json (`--service`).
//!
//! A monitor shows one of three wallpapers depending on the mode: its
//! desktop wallpaper, its screensaver wallpaper (while idle) or the lock
//! screen's (while the lock shows it). [`Config::choice`] resolves the
//! fallbacks (rustpaper_core::state has the same order).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use rustpaper_core::performance;
use rustpaper_core::state::{self, Playback, StageChoice, VideoPlayer};
use serde_json::{Map, Value};

/// One wallpaper as WE plays it.
#[derive(Clone, PartialEq, Debug)]
pub struct Choice {
    /// The wallpaper's project.json (absolute).
    pub project: PathBuf,
    /// User property values `{ "name": value }`.
    pub properties: Map<String, Value>,
}

/// What every monitor shows right now.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Desktop,
    /// Idle: the screensaver covers the windows.
    Screensaver,
    /// The lock screen shows the frames.
    Lock,
}

#[derive(Clone, PartialEq, Debug, Default)]
pub struct ScreensaverConfig {
    pub enabled: bool,
    pub seconds: u32,
    pub all: Option<Choice>,
    pub monitors: BTreeMap<String, Choice>,
}

/// What to show and how.
#[derive(Clone, PartialEq)]
pub struct Config {
    /// The desktop wallpaper of every monitor without its own.
    pub desktop: Choice,
    /// Desktop wallpaper per monitor (output name).
    pub monitors: BTreeMap<String, Choice>,
    /// The lock screen's (None: the desktop's).
    pub lock: Option<Choice>,
    pub screensaver: ScreensaverConfig,
    pub mute: bool,
    /// 0..100. WE has no volume command (it plays at its own volume unless
    /// muted); mpv takes it as is.
    pub volume: u8,
    pub playback: Playback,
    /// WE quality settings (sanitized); empty = leave WE's as they are.
    pub performance: Map<String, Value>,
    pub stage: StageChoice,
    /// What plays video wallpapers (video.rs).
    pub video_player: VideoPlayer,
    /// WE on Wine's Wayland driver (state.json, `rustpaper we-proton`).
    pub wine_wayland: bool,
}

impl Config {
    /// What monitor `output` shows in `mode`.
    pub fn choice(&self, output: &str, mode: Mode) -> &Choice {
        let desktop = self.monitors.get(output).unwrap_or(&self.desktop);
        match mode {
            Mode::Desktop => desktop,
            Mode::Lock => self.lock.as_ref().unwrap_or(desktop),
            Mode::Screensaver => self
                .screensaver
                .monitors
                .get(output)
                .or(self.screensaver.all.as_ref())
                .unwrap_or(desktop),
        }
    }
}

/// Where the Config comes from.
pub enum Source {
    /// `rustpaper <dir> [...]`: fixed.
    Args(Box<Config>),
    /// `rustpaper --service`: the selector's state.json, re-read on SIGHUP
    /// (an explicit `--stage` wins over its stage choice).
    Service(Option<StageChoice>),
}

fn usage() -> ! {
    eprintln!(
        "usage: rustpaper <wallpaper folder> [--props properties.json] [--mute] [--volume 0..100] [--output NAME] [--stage auto|desktop|private]\n       rustpaper --service [--output NAME] [--stage ...]   (the selector's choice; SIGHUP re-reads it)\n       rustpaper setup-steam                                (WE's launch option, for the private stage)\n       rustpaper lock | lock-daemon | lock-setup ...       (the lock screen, docs/LOCKSCREEN.md)\n       rustpaper outputs                                    (the monitors, as JSON)\n       rustpaper we-proton enable|disable|status            (WE on Wine's Wayland driver, GE-Proton)\n       rustpaper we-launch <command>                        (that launch option)"
    );
    std::process::exit(2);
}

pub fn parse_args() -> (Source, Option<String>) {
    let mut args = std::env::args().skip(1);
    let mut dir = None;
    let mut props_file: Option<PathBuf> = None;
    let mut mute = false;
    let mut volume = 100;
    let mut output = None;
    let mut service = false;
    let mut stage = None;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--stage" => {
                stage = match args.next().as_deref() {
                    Some("auto") => Some(StageChoice::Auto),
                    Some("desktop") => Some(StageChoice::Desktop),
                    Some("private") => Some(StageChoice::Private),
                    _ => usage(),
                }
            }
            "--service" => service = true,
            "--props" => props_file = args.next().map(PathBuf::from),
            "--mute" | "--no-audio" => mute = true,
            // WE has no volume command: anything above 0 plays at WE's own
            // volume (set in WE's settings); mpv plays at this volume
            "--volume" => {
                volume = args
                    .next()
                    .and_then(|v| v.parse::<u32>().ok())
                    .unwrap_or(100)
                    .min(100) as u8;
                mute = volume == 0;
            }
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
        return (Source::Service(stage), output);
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
        desktop: Choice {
            project,
            properties,
        },
        monitors: BTreeMap::new(),
        lock: None,
        screensaver: ScreensaverConfig::default(),
        mute,
        volume,
        playback: Playback::default(),
        performance: Map::new(),
        stage: stage.unwrap_or_default(),
        video_player: VideoPlayer::default(),
        wine_wayland: false,
    };
    (Source::Args(Box::new(config)), output)
}

fn project_file(dir: &Path) -> Result<PathBuf, String> {
    let project = dir.join("project.json");
    if !project.is_file() {
        return Err(format!("{} not found", project.display()));
    }
    Ok(std::fs::canonicalize(&project).unwrap_or(project))
}

/// Wallpaper `id` with its saved property values.
fn resolve(id: &str) -> Result<Choice, String> {
    let dir =
        rustpaper_core::paths::find_wallpaper(id).ok_or(format!("wallpaper {id} not found"))?;
    Ok(Choice {
        project: project_file(&dir)?,
        properties: state::load_properties(id),
    })
}

/// An optional role's wallpaper; one that is gone falls back (logged).
fn resolve_opt(id: Option<&str>) -> Option<Choice> {
    let id = id?;
    resolve(id)
        .map_err(|e| eprintln!("config: {e}; using the fallback"))
        .ok()
}

fn resolve_map(m: &BTreeMap<String, String>) -> BTreeMap<String, Choice> {
    m.iter()
        .filter_map(|(out, id)| Some((out.clone(), resolve_opt(Some(id))?)))
        .collect()
}

/// The selector's choice (`~/.config/rustpaper/state.json`).
pub fn service_config() -> Result<Config, String> {
    let st = state::load();
    let id = st
        .last_id
        .clone()
        .ok_or("no wallpaper chosen yet (pick one in rustpaper-selector)")?;
    Ok(Config {
        desktop: resolve(&id)?,
        monitors: resolve_map(&st.monitors),
        lock: resolve_opt(st.lock_id.as_deref()),
        screensaver: ScreensaverConfig {
            enabled: st.screensaver.enabled,
            seconds: st.screensaver.seconds.max(10),
            all: resolve_opt(st.screensaver.all.as_deref()),
            monitors: resolve_map(&st.screensaver.monitors),
        },
        mute: st.volume == 0,
        volume: st.volume.min(100),
        playback: st.playback,
        performance: performance::sanitize(&st.performance),
        stage: st.stage,
        video_player: st.video_player,
        wine_wayland: st.wine_wayland,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(name: &str) -> Choice {
        Choice {
            project: PathBuf::from(name),
            properties: Map::new(),
        }
    }

    #[test]
    fn each_mode_falls_back_to_the_desktop() {
        let mut cfg = Config {
            desktop: c("all"),
            monitors: BTreeMap::from([("DP-1".into(), c("dp"))]),
            lock: None,
            screensaver: ScreensaverConfig::default(),
            mute: false,
            volume: 100,
            playback: Playback::default(),
            performance: Map::new(),
            stage: StageChoice::Auto,
            video_player: VideoPlayer::Mpv,
            wine_wayland: false,
        };
        assert_eq!(cfg.choice("DP-1", Mode::Desktop), &c("dp"));
        assert_eq!(cfg.choice("HDMI-A-1", Mode::Desktop), &c("all"));
        assert_eq!(cfg.choice("DP-1", Mode::Lock), &c("dp"));
        assert_eq!(cfg.choice("DP-1", Mode::Screensaver), &c("dp"));
        cfg.lock = Some(c("lock"));
        cfg.screensaver.all = Some(c("saver"));
        cfg.screensaver
            .monitors
            .insert("HDMI-A-1".into(), c("hdmi-saver"));
        assert_eq!(cfg.choice("DP-1", Mode::Lock), &c("lock"));
        assert_eq!(cfg.choice("DP-1", Mode::Screensaver), &c("saver"));
        assert_eq!(cfg.choice("HDMI-A-1", Mode::Screensaver), &c("hdmi-saver"));
    }
}
