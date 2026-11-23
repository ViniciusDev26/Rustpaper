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
    for f in &files {
        steam::write_launch_options(f, app, &option)?;
        println!("{}: LaunchOptions = {option}", f.display());
    }
    if was_running {
        println!("starting Steam again");
        let _ = Command::new("steam")
            .arg("-silent")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    }
    Ok(())
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
    if let Some(display) = active_display() {
        eprintln!("rustpaper we-launch: Wallpaper Engine on the private stage ({display})");
        cmd.env("DISPLAY", &display)
            .env_remove("WAYLAND_DISPLAY")
            .env("RUSTPAPER_STAGE", "1");
    }
    let err = cmd.exec();
    eprintln!("rustpaper we-launch: {program}: {err}");
    std::process::exit(127);
}
