//! `rustpaper lock-daemon` (the `rustpaper-lock.service` user unit): what
//! starts the lock screen without a keypress.
//!
//! - **Before suspend**: it holds a logind "delay" inhibitor for sleep. On
//!   `PrepareForSleep(true)` it runs `rustpaper lock` (which returns once the
//!   session is locked) and only then drops the inhibitor, so the machine
//!   never sleeps unlocked. After resume it takes a new inhibitor.
//! - **`loginctl lock-session`**: logind's `Lock` signal on our session
//!   (what idle daemons, desktop menus and `loginctl` use).
//! - **Idle**, when the adapter asks for it (Omarchy: the timeout taken over
//!   from its shell): `ext-idle-notify-v1` on the user's compositor, on its
//!   own thread.
//!
//! Locks run as separate processes; the unit's `KillMode=process` keeps a
//! lock alive across a daemon restart.

use std::os::fd::{FromRawFd, OwnedFd};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use dbus::blocking::Connection;
use dbus::message::MatchRule;

use super::adapter::LockAdapter;

const LOGIN1: &str = "org.freedesktop.login1";
/// How long a lock may take before suspend goes ahead anyway (logind would
/// stop waiting for the inhibitor at its InhibitDelayMaxSec regardless).
const LOCK_BEFORE_SLEEP: Duration = Duration::from_secs(8);
/// How often the idle timeout is re-read (the adapter may adopt a new one).
const IDLE_RECHECK: Duration = Duration::from_secs(60);

enum Event {
    Sleep(bool),
    Lock,
}

pub fn run(adapter: Box<dyn LockAdapter>) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let conn = Connection::new_system().map_err(|e| format!("system bus: {e}"))?;
    let (tx, rx) = mpsc::channel::<Event>();

    let sleep_tx = tx.clone();
    conn.add_match(
        MatchRule::new_signal("org.freedesktop.login1.Manager", "PrepareForSleep"),
        move |(start,): (bool,), _, _| {
            let _ = sleep_tx.send(Event::Sleep(start));
            true
        },
    )
    .map_err(|e| format!("PrepareForSleep: {e}"))?;

    match session_path(&conn) {
        Some(path) => {
            let lock_tx = tx.clone();
            conn.add_match(
                MatchRule::new_signal("org.freedesktop.login1.Session", "Lock")
                    .with_path(path.clone()),
                move |(): (), _, _| {
                    let _ = lock_tx.send(Event::Lock);
                    true
                },
            )
            .map_err(|e| format!("Lock: {e}"))?;
            eprintln!("lock-daemon: locking on `loginctl lock-session` ({path})");
        }
        None => eprintln!(
            "lock-daemon: no graphical session found; `loginctl lock-session` is not handled"
        ),
    }

    if adapter.idle_lock_seconds().is_some() {
        let exe = exe.clone();
        std::thread::spawn(move || idle::run(super::adapter::detect(), exe));
    }

    let mut inhibitor = take_inhibitor(&conn);
    eprintln!(
        "lock-daemon: {} adapter; locking before suspend{}",
        adapter.name(),
        if inhibitor.is_some() {
            ""
        } else {
            " (no inhibitor: logind refused)"
        }
    );
    loop {
        conn.process(Duration::from_secs(5))
            .map_err(|e| format!("system bus: {e}"))?;
        while let Ok(e) = rx.try_recv() {
            match e {
                Event::Sleep(true) => {
                    eprintln!("lock-daemon: suspending, locking first");
                    lock(&exe, Some(LOCK_BEFORE_SLEEP));
                    // the session is locked (or the time is up): let it sleep
                    inhibitor = None;
                }
                Event::Sleep(false) => {
                    if inhibitor.is_none() {
                        inhibitor = take_inhibitor(&conn);
                    }
                }
                Event::Lock => {
                    eprintln!("lock-daemon: lock requested (loginctl lock-session)");
                    lock(&exe, None);
                }
            }
        }
    }
}

/// Runs `exe lock`, which returns once the session is locked; waits at most
/// `limit` (None: in the background).
fn lock(exe: &std::path::Path, limit: Option<Duration>) {
    let child = std::process::Command::new(exe)
        .arg("lock")
        .stdin(std::process::Stdio::null())
        .spawn();
    let Ok(mut child) = child else {
        eprintln!("lock-daemon: could not start {} lock", exe.display());
        return;
    };
    let Some(limit) = limit else {
        // reap it from a thread (it returns quickly)
        std::thread::spawn(move || child.wait());
        return;
    };
    let t = Instant::now();
    while t.elapsed() < limit {
        if let Ok(Some(_)) = child.try_wait() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    eprintln!("lock-daemon: the lock did not confirm in {limit:?}");
    std::thread::spawn(move || child.wait());
}

/// Our graphical session's object path (`User.Display`).
fn session_path(conn: &Connection) -> Option<dbus::Path<'static>> {
    use dbus::blocking::stdintf::org_freedesktop_dbus::Properties;
    let user = conn.with_proxy(
        LOGIN1,
        "/org/freedesktop/login1/user/self",
        Duration::from_secs(5),
    );
    let (_, path): (String, dbus::Path<'static>) =
        user.get("org.freedesktop.login1.User", "Display").ok()?;
    (&*path != "/").then_some(path)
}

/// A delay inhibitor for sleep; dropping it lets the suspend go on.
fn take_inhibitor(conn: &Connection) -> Option<OwnedFd> {
    let manager = conn.with_proxy(LOGIN1, "/org/freedesktop/login1", Duration::from_secs(5));
    let (fd,): (dbus::arg::OwnedFd,) = manager
        .method_call(
            "org.freedesktop.login1.Manager",
            "Inhibit",
            (
                "sleep",
                "Rustpaper",
                "Lock the screen before suspend",
                "delay",
            ),
        )
        .map_err(|e| eprintln!("lock-daemon: Inhibit: {e}"))
        .ok()?;
    // SAFETY: into_fd hands over ownership of the inhibitor's fd.
    Some(unsafe { OwnedFd::from_raw_fd(fd.into_fd()) })
}

/// Idle locking on the user's compositor.
mod idle {
    use std::time::Instant;

    use wayland_client::globals::{GlobalListContents, registry_queue_init};
    use wayland_client::protocol::{wl_registry::WlRegistry, wl_seat::WlSeat};
    use wayland_client::{Connection, Dispatch, QueueHandle, delegate_noop};
    use wayland_protocols::ext::idle_notify::v1::client::{
        ext_idle_notification_v1::{self, ExtIdleNotificationV1},
        ext_idle_notifier_v1::ExtIdleNotifierV1,
    };

    use super::super::adapter::LockAdapter;
    use super::IDLE_RECHECK;

    struct State {
        idled: bool,
    }

    pub fn run(adapter: Box<dyn LockAdapter>, exe: std::path::PathBuf) {
        if let Err(e) = watch(adapter.as_ref(), &exe) {
            eprintln!("lock-daemon: idle locking is off: {e}");
        }
    }

    fn watch(adapter: &dyn LockAdapter, exe: &std::path::Path) -> Result<(), String> {
        let conn = Connection::connect_to_env().map_err(|e| format!("no Wayland display: {e}"))?;
        let (globals, mut queue) =
            registry_queue_init::<State>(&conn).map_err(|e| e.to_string())?;
        let qh = queue.handle();
        let notifier: ExtIdleNotifierV1 = globals
            .bind(&qh, 1..=1, ())
            .map_err(|_| "the compositor lacks ext-idle-notify-v1")?;
        let seat: WlSeat = globals.bind(&qh, 1..=1, ()).map_err(|_| "no seat")?;
        let mut st = State { idled: false };
        let mut current: Option<(u32, ExtIdleNotificationV1)> = None;
        let mut recheck = Instant::now();
        loop {
            if recheck.elapsed() >= IDLE_RECHECK || current.is_none() {
                recheck = Instant::now();
                let want = adapter.idle_lock_seconds();
                if current.as_ref().map(|c| c.0) != want {
                    if let Some((_, n)) = current.take() {
                        n.destroy();
                    }
                    if let Some(secs) = want {
                        eprintln!("lock-daemon: locking after {secs} s idle");
                        // respects idle inhibitors (a video playing)
                        let n = notifier.get_idle_notification(
                            secs.saturating_mul(1000),
                            &seat,
                            &qh,
                            (),
                        );
                        current = Some((secs, n));
                    } else {
                        // nothing to watch for now: look again later
                        std::thread::sleep(IDLE_RECHECK);
                        continue;
                    }
                }
            }
            // wake up at least every few seconds to re-read the timeout
            queue.flush().map_err(|e| e.to_string())?;
            if let Some(guard) = queue.prepare_read() {
                let fd = guard.connection_fd();
                use std::os::fd::AsRawFd;
                let mut p = libc::pollfd {
                    fd: fd.as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                };
                // SAFETY: one valid pollfd.
                if unsafe { libc::poll(&mut p, 1, 5000) } > 0 {
                    let _ = guard.read();
                }
            }
            queue.dispatch_pending(&mut st).map_err(|e| e.to_string())?;
            if std::mem::take(&mut st.idled) {
                eprintln!("lock-daemon: idle, locking");
                super::lock(exe, None);
            }
        }
    }

    impl Dispatch<ExtIdleNotificationV1, ()> for State {
        fn event(
            st: &mut Self,
            _: &ExtIdleNotificationV1,
            e: ext_idle_notification_v1::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            if let ext_idle_notification_v1::Event::Idled = e {
                st.idled = true;
            }
        }
    }

    impl Dispatch<WlRegistry, GlobalListContents> for State {
        fn event(
            _: &mut Self,
            _: &WlRegistry,
            _: wayland_client::protocol::wl_registry::Event,
            _: &GlobalListContents,
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
        }
    }

    delegate_noop!(State: ignore ExtIdleNotifierV1);
    delegate_noop!(State: ignore WlSeat);
}
