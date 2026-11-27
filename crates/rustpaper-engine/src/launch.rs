//! `rustpaper we-launch %command%`: the Steam launch option for Wallpaper
//! Engine that lets the private stage host it.
//!
//! WE inherits Steam's environment, so it would open its windows on the
//! user's display. With this launch option Steam runs us first: when an
//! engine runs a private stage it has written the stage's X display to
//! `$XDG_RUNTIME_DIR/rustpaper/stage-display`. We then point WE at it
//! (`DISPLAY`) and drop `WAYLAND_DISPLAY`, so Proton uses X11 and not the
//! host's Wayland socket. Otherwise WE runs exactly as without us. Either
//! way we replace ourselves with the command Steam gave (`exec`).
//!
//! With `wine_wayland` on (state.json, `rustpaper we-proton enable`: WE's
//! compatibility tool is then GE-Proton) WE uses Wine's Wayland driver on
//! the stage's Wayland socket instead (`PROTON_ENABLE_WAYLAND=1`, no
//! `DISPLAY`), with no XWayland in between (docs/ENGINE.md).
//!
//! WE's audio never goes through Wine's PulseAudio driver ([`WeAudio`]).
//! The watchdog pauses WE with SIGSTOP (we::set_paused), and a stopped
//! winepulse client made pipewire-pulse 1.6.9 grow by the rate of its
//! stream (float32 2ch 48 kHz, 384 kB/s; measured ~440 kB/s) for as long as
//! WE stayed stopped: 37 GB in 4.6 days with a video wallpaper (WE paused
//! the whole time), returned only by restarting pipewire-pulse. Running,
//! the same stream stayed flat; through Wine's ALSA driver (which the Steam
//! runtime routes to pipewire-pulse too, "ALSA plug-in [wine64-preloader]")
//! it stayed flat stopped as well (90 s each, 2026-11-27). With the volume
//! at 0 WE gets no audio driver at all, so it opens no stream.

use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;

/// Where an engine with a private stage publishes its X display (`:N`).
pub fn display_file() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR")?).join("rustpaper/stage-display"))
}

/// Where an engine with a private stage publishes its Wayland socket name
/// (for Wine's Wayland driver).
pub fn wayland_file() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR")?).join("rustpaper/stage-wayland"))
}

/// The private stage's display, when one is up (its X socket exists).
pub fn active_display() -> Option<String> {
    let d = std::fs::read_to_string(display_file()?).ok()?;
    let d = d.trim().to_string();
    let n = d.strip_prefix(':')?.parse::<u32>().ok()?;
    std::path::Path::new(&format!("/tmp/.X11-unix/X{n}"))
        .exists()
        .then_some(d)
}

/// WE's Steam launch option runs us (in any of the Steam accounts).
pub fn configured() -> bool {
    rustpaper_core::steam::local_configs().iter().any(|f| {
        rustpaper_core::steam::read_launch_options(f, rustpaper_core::paths::WE_APP_ID)
            .is_some_and(|o| o.contains("we-launch"))
    })
}

/// `rustpaper setup-steam`: sets WE's launch option to run this binary
/// first. Steam rewrites its config on exit, so a running Steam is closed
/// (`steam -shutdown`), the option written (one-time backup of each
/// localconfig.vdf), and Steam started again.
pub fn setup_steam() -> Result<(), String> {
    use rustpaper_core::steam;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let option = format!("\"{}\" we-launch %command%", exe.display());
    let files = steam::local_configs();
    if files.is_empty() {
        return Err("no Steam account found (userdata/<id>/config/localconfig.vdf)".into());
    }
    let app = rustpaper_core::paths::WE_APP_ID;
    if files
        .iter()
        .all(|f| steam::read_launch_options(f, app).as_deref() == Some(option.as_str()))
    {
        println!("already set: {option}");
        return Ok(());
    }
    with_steam_closed(|| {
        for f in &files {
            steam::write_launch_options(f, app, &option)?;
            println!("{}: LaunchOptions = {option}", f.display());
        }
        Ok(())
    })
}

/// Runs `write` with Steam closed (it rewrites its config files on exit):
/// a running Steam is closed (`steam -shutdown`) and started again after.
pub fn with_steam_closed(write: impl FnOnce() -> Result<(), String>) -> Result<(), String> {
    use rustpaper_core::steam;
    let was_running = steam::steam_running();
    if was_running {
        println!("closing Steam to write its config...");
        let _ = Command::new("steam").arg("-shutdown").status();
        let t = std::time::Instant::now();
        while steam::steam_running() {
            if t.elapsed() > std::time::Duration::from_secs(90) {
                return Err("Steam did not close; close it and run this again".into());
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
    }
    let result = write();
    if was_running {
        println!("starting Steam again");
        let _ = Command::new("steam")
            .arg("-silent")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    }
    result
}

/// The stage's Wayland socket, when one is up.
pub fn active_wayland() -> Option<String> {
    let name = std::fs::read_to_string(wayland_file()?).ok()?;
    let name = name.trim().to_string();
    let runtime = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR")?);
    (!name.is_empty() && !name.contains('/') && runtime.join(&name).exists()).then_some(name)
}

/// WE's environment on the private stage: (variables to set, to remove).
/// Nothing when no stage is up.
fn stage_env(
    display: Option<String>,
    wayland: Option<String>,
    wine_wayland: bool,
) -> (Vec<(&'static str, String)>, Vec<&'static str>) {
    match (wine_wayland, wayland, display) {
        (true, Some(socket), _) => (
            vec![
                ("WAYLAND_DISPLAY", socket),
                ("PROTON_ENABLE_WAYLAND", "1".into()),
                ("RUSTPAPER_STAGE", "1".into()),
            ],
            vec!["DISPLAY"],
        ),
        (_, _, Some(display)) => (
            vec![("DISPLAY", display), ("RUSTPAPER_STAGE", "1".into())],
            vec!["WAYLAND_DISPLAY"],
        ),
        _ => (Vec::new(), Vec::new()),
    }
}

/// WE's audio driver, chosen by the launch option through Wine's DLL
/// overrides (the module docs say why never winepulse).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WeAudio {
    /// No driver: no stream at all (volume 0).
    Off,
    /// Wine's ALSA driver.
    Alsa,
    /// Wine's default, PulseAudio: a WE not started by this launch option.
    Pulse,
}

/// `WINEDLLOVERRIDES` entries that leave Wine's ALSA driver as WE's.
const NO_PULSE: &str = "winepulse.drv=d";
/// `WINEDLLOVERRIDES` entries that leave WE without audio (Proton ships
/// winepulse and winealsa).
const NO_AUDIO: &str = "winepulse.drv=d;winealsa.drv=d";

impl WeAudio {
    /// What the launch option gives WE: the selector's volume 0 turns
    /// audio off.
    pub fn wanted() -> Self {
        if rustpaper_core::state::load().volume == 0 {
            Self::Off
        } else {
            Self::Alsa
        }
    }

    /// The driver of a WE started with `WINEDLLOVERRIDES` = `overrides`.
    pub fn of(overrides: Option<&str>) -> Self {
        match overrides {
            Some(o) if o.contains(NO_AUDIO) => Self::Off,
            Some(o) if o.contains(NO_PULSE) => Self::Alsa,
            _ => Self::Pulse,
        }
    }

    /// `WINEDLLOVERRIDES` for WE: `current` (the user's own overrides,
    /// kept) plus ours.
    fn overrides(self, current: Option<String>) -> Option<String> {
        let ours = match self {
            Self::Off => NO_AUDIO,
            Self::Alsa => NO_PULSE,
            Self::Pulse => return current,
        };
        Some(match current.filter(|c| !c.is_empty()) {
            Some(c) => format!("{c};{ours}"),
            None => ours.to_string(),
        })
    }
}

pub fn run(command: &[String]) -> ! {
    let Some((program, args)) = command.split_first() else {
        eprintln!(
            "usage: rustpaper we-launch <command...>   (Steam launch option: rustpaper we-launch %command%)"
        );
        std::process::exit(2);
    };
    let mut cmd = Command::new(program);
    cmd.args(args);
    let wine_wayland = rustpaper_core::state::load().wine_wayland;
    let (set, remove) = stage_env(active_display(), active_wayland(), wine_wayland);
    if !set.is_empty() {
        eprintln!("rustpaper we-launch: Wallpaper Engine on the private stage ({set:?})");
    }
    for (k, v) in set {
        cmd.env(k, v);
    }
    for k in remove {
        cmd.env_remove(k);
    }
    // Proton appends its own overrides to ours
    let audio = WeAudio::wanted();
    if let Some(o) = audio.overrides(std::env::var("WINEDLLOVERRIDES").ok()) {
        eprintln!("rustpaper we-launch: Wallpaper Engine's audio: {audio:?}");
        cmd.env("WINEDLLOVERRIDES", o);
    }
    let err = cmd.exec();
    eprintln!("rustpaper we-launch: {program}: {err}");
    std::process::exit(127);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn we_gets_the_stage_it_can_use() {
        let x11 = stage_env(Some(":1".into()), Some("rustpaper-stage-7".into()), false);
        assert!(x11.0.contains(&("DISPLAY", ":1".to_string())));
        assert_eq!(x11.1, vec!["WAYLAND_DISPLAY"]);
        let way = stage_env(Some(":1".into()), Some("rustpaper-stage-7".into()), true);
        assert!(
            way.0
                .contains(&("WAYLAND_DISPLAY", "rustpaper-stage-7".to_string()))
        );
        assert!(way.0.contains(&("PROTON_ENABLE_WAYLAND", "1".to_string())));
        assert_eq!(way.1, vec!["DISPLAY"]);
        // asked for Wayland but no socket (older engine): X11 as before
        let fallback = stage_env(Some(":1".into()), None, true);
        assert!(fallback.0.contains(&("DISPLAY", ":1".to_string())));
        assert!(
            stage_env(None, None, true).0.is_empty(),
            "no stage: untouched"
        );
    }

    #[test]
    fn we_audio_never_uses_winepulse() {
        assert_eq!(WeAudio::Off.overrides(None).as_deref(), Some(NO_AUDIO));
        assert_eq!(
            WeAudio::Off.overrides(Some("d3d11=n".into())).as_deref(),
            Some("d3d11=n;winepulse.drv=d;winealsa.drv=d")
        );
        assert_eq!(
            WeAudio::Alsa.overrides(Some(String::new())).as_deref(),
            Some(NO_PULSE)
        );
        assert_eq!(
            WeAudio::Alsa.overrides(Some("d3d11=n".into())).as_deref(),
            Some("d3d11=n;winepulse.drv=d")
        );
        // read back from a running WE's environment (Proton appended its own)
        for a in [WeAudio::Off, WeAudio::Alsa] {
            let env = format!("{};nvapi=n", a.overrides(Some("x=n".into())).unwrap());
            assert_eq!(WeAudio::of(Some(&env)), a);
        }
        assert_eq!(WeAudio::of(Some("nvapi=n")), WeAudio::Pulse);
        assert_eq!(WeAudio::of(None), WeAudio::Pulse);
    }
}
