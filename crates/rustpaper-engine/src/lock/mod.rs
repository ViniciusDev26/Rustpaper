//! The animated lock screen (docs/LOCKSCREEN.md): the live wallpaper behind
//! a password prompt, on any compositor with `ext-session-lock-v1`.
//!
//! - `rustpaper lock`: locks the session and returns once it is locked (so
//!   it can run before suspend); the lock itself runs on in the background
//!   (`lock --foreground`, screen.rs) until the password is accepted.
//! - `rustpaper lock-daemon`: locks before suspend, on `loginctl
//!   lock-session` and on idle (daemon.rs; the `rustpaper-lock.service` user
//!   unit).
//! - `rustpaper lock-setup enable|disable|status [--json]`: routes the
//!   desktop's own lock triggers to Rustpaper, or gives them back
//!   (adapter.rs, one adapter per desktop: omarchy.rs, generic.rs).
//!
//! The frames come from the running engine (feed.rs); without one the lock
//! shows a still picture.

pub mod adapter;
mod daemon;
pub mod feed;
mod generic;
mod omarchy;
mod pam;
mod screen;
mod ui;

use std::io::Read;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::time::Duration;

/// The environment variable holding the fd the locker reports "locked" on.
const NOTIFY_ENV: &str = "RUSTPAPER_LOCK_NOTIFY_FD";
/// How long `rustpaper lock` waits for the session to be locked.
const LOCK_TIMEOUT: Duration = Duration::from_secs(10);

/// Handles the lock subcommands; never returns.
pub fn main(argv: &[String]) -> ! {
    let cmd = argv.get(1).map(String::as_str).unwrap_or_default();
    let rest: Vec<&str> = argv.iter().skip(2).map(String::as_str).collect();
    let code = match cmd {
        "lock" if rest.contains(&"--foreground") => foreground(),
        "lock" => lock(),
        "lock-daemon" => match daemon::run(adapter::detect()) {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("lock-daemon: {e}");
                1
            }
        },
        "lock-setup" => setup(&rest),
        _ => 2,
    };
    std::process::exit(code);
}

/// `$XDG_RUNTIME_DIR/rustpaper/lock.lock`: held by the running lock.
fn instance_file() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR")?).join("rustpaper/lock.lock"))
}

/// Takes the one-lock-at-a-time file lock; None when a lock already runs.
fn take_instance() -> Option<std::fs::File> {
    let path = instance_file()?;
    let _ = std::fs::create_dir_all(path.parent()?);
    let f = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)
        .ok()?;
    // SAFETY: flock on our own open file.
    (unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0).then_some(f)
}

/// `rustpaper lock`: starts the locker in the background and waits until
/// it reports the session locked.
fn lock() -> i32 {
    match take_instance() {
        // released right away: the locker takes it itself
        Some(f) => drop(f),
        None => {
            eprintln!("rustpaper lock: already locked");
            return 0;
        }
    }
    let mut fds = [0; 2];
    // SAFETY: pipe2 into a 2-int array; the read end stays close-on-exec.
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
        eprintln!("rustpaper lock: pipe failed");
        return 1;
    }
    // SAFETY: fresh fds we own.
    let (read, write) = unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
    let Ok(exe) = std::env::current_exe() else {
        return 1;
    };
    let wfd = write.as_raw_fd();
    let mut cmd = std::process::Command::new(exe);
    cmd.args(["lock", "--foreground"])
        .env(NOTIFY_ENV, wfd.to_string())
        .stdin(std::process::Stdio::null());
    // SAFETY: between fork and exec only async-signal-safe calls: the pipe's
    // write end survives exec, and a new session detaches the locker from
    // the caller (a keybinding, the daemon) so it outlives it.
    unsafe {
        cmd.pre_exec(move || {
            let flags = libc::fcntl(wfd, libc::F_GETFD);
            libc::fcntl(wfd, libc::F_SETFD, flags & !libc::FD_CLOEXEC);
            libc::setsid();
            Ok(())
        });
    }
    if let Err(e) = cmd.spawn() {
        eprintln!("rustpaper lock: {e}");
        return 1;
    }
    drop(write);
    // one byte once locked; EOF if the locker failed
    let mut file = std::fs::File::from(read);
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut b = [0u8; 1];
        let _ = tx.send(file.read(&mut b).map(|n| n == 1).unwrap_or(false));
    });
    match rx.recv_timeout(LOCK_TIMEOUT) {
        Ok(true) => 0,
        Ok(false) => {
            eprintln!("rustpaper lock: the session could not be locked");
            1
        }
        Err(_) => {
            eprintln!("rustpaper lock: no answer from the lock screen in {LOCK_TIMEOUT:?}");
            1
        }
    }
}

/// `rustpaper lock --foreground`: the lock screen itself.
fn foreground() -> i32 {
    let notify = std::env::var(NOTIFY_ENV)
        .ok()
        .and_then(|v| v.parse::<i32>().ok())
        .filter(|fd| *fd > 2)
        // SAFETY: the fd our parent left open for us, used once.
        .map(|fd| unsafe { OwnedFd::from_raw_fd(fd) });
    // SAFETY: removing our own variable before any thread starts.
    unsafe { std::env::remove_var(NOTIFY_ENV) };
    let Some(_instance) = take_instance() else {
        // another lock runs: the session is (being) locked
        if let Some(n) = notify {
            // SAFETY: one byte into the parent's pipe.
            unsafe { libc::write(n.as_raw_fd(), [1u8].as_ptr().cast(), 1) };
        }
        return 0;
    };
    // localized dates on the panel
    // SAFETY: before any other thread exists.
    unsafe { libc::setlocale(libc::LC_TIME, c"".as_ptr()) };
    let adapter = adapter::detect();
    let mut stills = Vec::new();
    stills.extend(wallpaper_preview());
    stills.extend(adapter.fallback_background());
    let opts = screen::Options {
        pam_service: adapter.pam_service(),
        stills,
        notify,
    };
    eprintln!(
        "lock: {} adapter, PAM service {}",
        adapter.name(),
        opts.pam_service
    );
    match screen::run(adapter.as_ref(), opts) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("lock: {e}");
            1
        }
    }
}

/// The chosen wallpaper's preview image (project.json `preview`).
fn wallpaper_preview() -> Option<PathBuf> {
    let id = rustpaper_core::state::load().last_id?;
    let dir = rustpaper_core::paths::find_wallpaper(&id)?;
    let text = std::fs::read_to_string(dir.join("project.json")).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    let file = v["preview"].as_str()?;
    // a file name inside the wallpaper's folder, nothing else
    if file.contains('/') || file.contains("..") {
        return None;
    }
    Some(dir.join(file)).filter(|p| p.is_file())
}

/// `rustpaper lock-setup enable|disable|status [--json]`.
fn setup(args: &[&str]) -> i32 {
    let adapter = adapter::detect();
    let json = args.contains(&"--json");
    match args.first().copied() {
        Some("enable") => {
            let Ok(exe) = std::env::current_exe() else {
                return 1;
            };
            match adapter.enable(&exe) {
                Ok(notes) => {
                    for n in notes {
                        println!("{n}");
                    }
                    0
                }
                Err(e) => {
                    eprintln!("lock-setup: {e}");
                    1
                }
            }
        }
        Some("disable") => match adapter.disable() {
            Ok(()) => {
                println!("the desktop's own lock screen is back");
                0
            }
            Err(e) => {
                eprintln!("lock-setup: {e}");
                1
            }
        },
        Some("status") => {
            let enabled = adapter.enabled();
            if json {
                println!(
                    "{}",
                    serde_json::json!({ "adapter": adapter.name(), "enabled": enabled })
                );
            } else {
                println!(
                    "{} adapter: animated lock screen {}",
                    adapter.name(),
                    if enabled { "enabled" } else { "disabled" }
                );
            }
            0
        }
        _ => {
            eprintln!("usage: rustpaper lock-setup enable|disable|status [--json]");
            2
        }
    }
}
