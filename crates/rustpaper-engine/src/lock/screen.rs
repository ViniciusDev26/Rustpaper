//! The lock screen: an `ext-session-lock-v1` client.
//!
//! Each monitor gets a lock surface showing the live wallpaper (WE's
//! buffers from the engine's frame feed, lock/feed.rs, attached as they
//! are), a dimming layer, and the panel (lock/ui.rs) as subsurfaces. With no
//! engine (or the desktop stage) the background is a still picture: the
//! wallpaper's preview, else the adapter's (Omarchy: the theme background),
//! else the theme's background color.
//!
//! Safety rules this follows (see docs/LOCKSCREEN.md):
//! - only the wallpaper is ever shown: frames come from WE's windows, never
//!   from an output capture;
//! - PAM runs on its own thread, so a slow check (PAM sleeps after a wrong
//!   password) never blocks typing or drawing;
//! - the compositor keeps the session locked if this process dies; the
//!   session only unlocks through `unlock_and_destroy` after PAM succeeded.

use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{
    wl_buffer::{self, WlBuffer},
    wl_compositor::WlCompositor,
    wl_keyboard::{self, WlKeyboard},
    wl_output::{self, WlOutput},
    wl_region::WlRegion,
    wl_registry::{self, WlRegistry},
    wl_seat::{self, WlSeat},
    wl_shm::{self, WlShm},
    wl_shm_pool::WlShmPool,
    wl_subcompositor::WlSubcompositor,
    wl_subsurface::WlSubsurface,
    wl_surface::WlSurface,
};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, WEnum, delegate_noop};
use wayland_protocols::ext::session_lock::v1::client::{
    ext_session_lock_manager_v1::ExtSessionLockManagerV1,
    ext_session_lock_surface_v1::{self, ExtSessionLockSurfaceV1},
    ext_session_lock_v1::{self, ExtSessionLockV1},
};
use wayland_protocols::wp::linux_dmabuf::zv1::client::{
    zwp_linux_buffer_params_v1::{self, ZwpLinuxBufferParamsV1},
    zwp_linux_dmabuf_v1::{self, ZwpLinuxDmabufV1},
};
use wayland_protocols::wp::viewporter::client::{
    wp_viewport::WpViewport, wp_viewporter::WpViewporter,
};
use xkbcommon::xkb;

use super::adapter::LockAdapter;
use super::feed::{FeedClient, FeedEvent, FeedFrame};
use super::ui::{self, Panel, PanelState, Status};

/// How often a missing engine is looked for again.
const FEED_RETRY: Duration = Duration::from_secs(2);
/// Dimming over the wallpaper (alpha of black), so the panel reads well.
const DIM_ALPHA: u8 = 0x50;
/// Longest still picture kept (wider ones are downscaled once).
const STILL_MAX: u32 = 3840;

/// Key repeat, client-side as Wayland wants it: after `delay` a held key
/// repeats `rate` times a second (the compositor's `repeat_info`; until it
/// says, the common 25/s after 600 ms).
#[derive(Debug, Clone, PartialEq)]
struct Repeat {
    rate: u32,
    delay: Duration,
    /// The held key and when it fires next.
    held: Option<(u32, Instant)>,
}

impl Default for Repeat {
    fn default() -> Self {
        Repeat {
            rate: 25,
            delay: Duration::from_millis(600),
            held: None,
        }
    }
}

impl Repeat {
    fn set(&mut self, rate: i32, delay_ms: i32) {
        self.rate = rate.max(0) as u32;
        self.delay = Duration::from_millis(delay_ms.max(0) as u64);
        if self.rate == 0 {
            self.held = None;
        }
    }

    fn press(&mut self, key: u32, now: Instant) {
        self.held = (self.rate > 0).then_some((key, now + self.delay));
    }

    fn release(&mut self, key: u32) {
        if self.held.is_some_and(|(k, _)| k == key) {
            self.held = None;
        }
    }

    /// When the next repeat is due.
    fn next(&self) -> Option<Instant> {
        self.held.map(|(_, t)| t)
    }

    /// The key to repeat now, if one is due (and schedules the next).
    fn due(&mut self, now: Instant) -> Option<u32> {
        let (key, at) = self.held?;
        if now < at {
            return None;
        }
        // one repeat per call: a stalled loop does not burst
        let interval = Duration::from_secs(1) / self.rate.max(1);
        self.held = Some((key, now + interval));
        Some(key)
    }
}

/// The typed password; zeroed when cleared or dropped.
#[derive(Default)]
struct Secret(Vec<u8>);

impl Secret {
    fn push(&mut self, s: &str) {
        self.0.extend_from_slice(s.as_bytes());
    }
    fn pop(&mut self) {
        // drop the last UTF-8 character
        while let Some(b) = self.0.pop() {
            if b & 0xC0 != 0x80 {
                break;
            }
        }
    }
    fn chars(&self) -> usize {
        self.0.iter().filter(|b| *b & 0xC0 != 0x80).count()
    }
    fn clear(&mut self) {
        self.0.fill(0);
        self.0.clear();
    }
    fn take(&mut self) -> Vec<u8> {
        let copy = self.0.clone();
        self.clear();
        copy
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        self.clear();
    }
}

/// A frame buffer imported from the feed.
struct LiveBuf {
    id: u64,
    key: u64,
    buffer: WlBuffer,
    on_screen: bool,
    /// The engine destroyed it: drop once the compositor releases it.
    gone: bool,
}

/// What a wl_buffer is, for its release event.
#[derive(Clone, Copy)]
enum BufKind {
    /// A live frame (its id in some output's `live`).
    Live(u64),
    /// A panel buffer: destroyed when released.
    Panel,
    /// The still picture / dim layer: shared, kept.
    Static,
}

struct Out {
    /// The wl_output's registry name.
    global: u32,
    output: WlOutput,
    name: String,
    /// Current mode in pixels.
    mode: (u32, u32),
    surface: Option<WlSurface>,
    lock_surface: Option<ExtSessionLockSurfaceV1>,
    viewport: Option<WpViewport>,
    dim: Option<(WlSurface, WlSubsurface, WpViewport)>,
    panel: Option<(WlSurface, WlSubsurface, WpViewport)>,
    /// Logical size from the lock surface's configure.
    size: (u32, u32),
    configured: bool,
    live: Vec<LiveBuf>,
    showing_live: bool,
}

/// The still picture: an shm buffer at the image's size, cropped to each
/// monitor by its viewport.
struct Still {
    buffer: WlBuffer,
    size: (u32, u32),
}

struct State {
    compositor: WlCompositor,
    subcompositor: WlSubcompositor,
    shm: WlShm,
    viewporter: WpViewporter,
    dmabuf: Option<ZwpLinuxDmabufV1>,
    lock_mgr: ExtSessionLockManagerV1,
    lock: Option<ExtSessionLockV1>,
    locked: bool,
    finished: bool,
    outputs: Vec<Out>,
    /// Bound for its keyboard (kept for the connection's lifetime).
    _seat: Option<WlSeat>,
    keyboard: Option<WlKeyboard>,
    xkb_ctx: xkb::Context,
    xkb: Option<xkb::State>,
    password: Secret,
    status: Status,
    caps_lock: bool,
    panel: Panel,
    panel_dirty: bool,
    still: Option<Still>,
    dim: Option<WlBuffer>,
    feed: Option<FeedClient>,
    next_id: u64,
    /// The password to check, handed to the PAM thread.
    submit: Option<Vec<u8>>,
    repeat: Repeat,
}

pub struct Options {
    pub pam_service: String,
    /// Still pictures to try, in order.
    pub stills: Vec<PathBuf>,
    /// Written once the session is locked (the `rustpaper lock` parent waits
    /// on it).
    pub notify: Option<OwnedFd>,
}

/// Locks the session and returns once it is unlocked. Err when it could not
/// lock (another lock screen holds the session, no ext-session-lock).
pub fn run(adapter: &dyn LockAdapter, mut opts: Options) -> Result<(), String> {
    let conn = Connection::connect_to_env().map_err(|e| format!("no Wayland display: {e}"))?;
    let (globals, mut queue) = registry_queue_init::<State>(&conn).map_err(|e| e.to_string())?;
    let qh = queue.handle();
    let lacks = |what: &str| format!("the compositor lacks {what}");
    let mut st = State {
        compositor: globals
            .bind(&qh, 4..=6, ())
            .map_err(|_| lacks("wl_compositor"))?,
        subcompositor: globals
            .bind(&qh, 1..=1, ())
            .map_err(|_| lacks("wl_subcompositor"))?,
        shm: globals.bind(&qh, 1..=1, ()).map_err(|_| lacks("wl_shm"))?,
        viewporter: globals
            .bind(&qh, 1..=1, ())
            .map_err(|_| lacks("wp_viewporter"))?,
        dmabuf: globals.bind(&qh, 3..=5, ()).ok(),
        lock_mgr: globals
            .bind(&qh, 1..=1, ())
            .map_err(|_| lacks("ext-session-lock-v1 (a lock screen protocol)"))?,
        lock: None,
        locked: false,
        finished: false,
        outputs: Vec::new(),
        _seat: globals.bind(&qh, 1..=9, ()).ok(),
        keyboard: None,
        xkb_ctx: xkb::Context::new(xkb::CONTEXT_NO_FLAGS),
        xkb: None,
        password: Secret::default(),
        status: Status::Idle,
        caps_lock: false,
        panel: Panel::new(adapter.theme()),
        panel_dirty: true,
        still: None,
        dim: None,
        feed: FeedClient::connect(),
        next_id: 1,
        submit: None,
        repeat: Repeat::default(),
    };
    for g in globals.contents().clone_list() {
        if g.interface == WlOutput::interface().name {
            st.add_output(&qh, globals.registry(), g.name, g.version);
        }
    }
    // output names and modes
    queue.roundtrip(&mut st).map_err(|e| e.to_string())?;

    st.dim = st.solid_buffer(&qh, [0, 0, 0], DIM_ALPHA);
    st.still = opts
        .stills
        .iter()
        .find_map(|p| st.still_buffer(&qh, p))
        .or_else(|| {
            let bg = adapter.theme().background;
            st.solid_buffer(&qh, bg, 0xff).map(|buffer| Still {
                buffer,
                size: (1, 1),
            })
        });
    if st.feed.is_some() {
        eprintln!("lock: showing the live wallpaper");
    }

    st.lock = Some(st.lock_mgr.lock(&qh, ()));
    for i in 0..st.outputs.len() {
        st.create_lock_surface(&qh, i);
    }

    // PAM off the UI thread; `wake` interrupts poll when it answers
    let (to_pam, pam_rx) = mpsc::channel::<Vec<u8>>();
    let (pam_tx, from_pam) = mpsc::channel::<Result<(), String>>();
    let (wake_r, wake_w) = pipe()?;
    let service = opts.pam_service.clone();
    std::thread::spawn(move || {
        for mut password in pam_rx {
            let r = super::pam::authenticate(&service, &password);
            password.fill(0);
            if pam_tx.send(r).is_err() {
                break;
            }
            // SAFETY: one byte into our own pipe.
            unsafe { libc::write(wake_w.as_raw_fd(), [1u8].as_ptr().cast(), 1) };
        }
    });

    let mut feed_retry = Instant::now() + FEED_RETRY;
    let mut minute = Instant::now() + Duration::from_secs(ui::secs_to_next_minute());
    let fd = conn.backend().poll_fd().as_raw_fd();
    loop {
        queue.dispatch_pending(&mut st).map_err(|e| e.to_string())?;
        if st.finished {
            if st.locked {
                // the compositor ended the lock itself
                return Ok(());
            }
            return Err("the session is already locked by another program".into());
        }
        if st.locked
            && let Some(n) = opts.notify.take()
        {
            // SAFETY: one byte into the parent's pipe, then it is closed.
            unsafe { libc::write(n.as_raw_fd(), [1u8].as_ptr().cast(), 1) };
            adapter.on_locked();
        }
        if let Some(pw) = st.submit.take() {
            st.status = Status::Checking;
            st.panel_dirty = true;
            let _ = to_pam.send(pw);
        }
        if Instant::now() >= minute {
            minute = Instant::now() + Duration::from_secs(ui::secs_to_next_minute());
            st.panel_dirty = true;
        }
        if st.feed.is_none() && Instant::now() >= feed_retry {
            feed_retry = Instant::now() + FEED_RETRY;
            st.feed = FeedClient::connect();
        }
        if st.panel_dirty {
            st.panel_dirty = false;
            for i in 0..st.outputs.len() {
                st.draw_panel(&qh, i);
            }
        }
        queue.flush().map_err(|e| e.to_string())?;

        let Some(guard) = queue.prepare_read() else {
            continue;
        };
        let feed_fd = st.feed.as_ref().map_or(-1, FeedClient::fd);
        let mut pfds = [
            libc::pollfd {
                fd,
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: feed_fd,
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: wake_r.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        if let Some(key) = st.repeat.due(Instant::now()) {
            st.key(key);
        }
        let until = st
            .repeat
            .next()
            .map_or(minute.min(feed_retry), |r| r.min(minute).min(feed_retry))
            .saturating_duration_since(Instant::now());
        let timeout = until.as_millis().clamp(1, 60_000) as i32;
        // SAFETY: three valid pollfds (a negative fd is ignored).
        let n = unsafe { libc::poll(pfds.as_mut_ptr(), 3, timeout) };
        if n > 0 && pfds[0].revents != 0 {
            if let Err(e) = guard.read() {
                use wayland_client::backend::WaylandError;
                let harmless = matches!(&e, WaylandError::Io(io) if matches!(io.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted));
                if !harmless {
                    return Err(format!("Wayland connection lost: {e}"));
                }
            }
        } else {
            drop(guard);
        }
        if n > 0 && pfds[1].revents != 0 {
            st.read_feed(&qh);
        }
        if n > 0 && pfds[2].revents != 0 {
            let mut b = [0u8; 16];
            // SAFETY: reading into our own buffer from our own pipe.
            unsafe { libc::read(wake_r.as_raw_fd(), b.as_mut_ptr().cast(), b.len()) };
            while let Ok(r) = from_pam.try_recv() {
                match r {
                    Ok(()) => {
                        eprintln!("lock: unlocked");
                        st.unlock(&qh);
                        let _ = queue.roundtrip(&mut st);
                        return Ok(());
                    }
                    Err(msg) => {
                        eprintln!("lock: authentication failed: {msg}");
                        st.status = Status::Failed(friendly(&msg));
                        st.panel_dirty = true;
                    }
                }
            }
        }
    }
}

/// PAM's messages are terse ("Authentication failure"); the common case
/// reads better as the user would say it.
fn friendly(msg: &str) -> String {
    if msg.eq_ignore_ascii_case("Authentication failure") {
        "Wrong password".into()
    } else {
        msg.to_string()
    }
}

fn pipe() -> Result<(OwnedFd, OwnedFd), String> {
    let mut fds = [0; 2];
    // SAFETY: pipe2 into a 2-int array.
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) } != 0 {
        return Err("pipe".into());
    }
    // SAFETY: fresh fds we own.
    Ok(unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) })
}

fn memfd(len: usize) -> Option<OwnedFd> {
    // SAFETY: memfd_create with a static name; ftruncate on our own fd.
    unsafe {
        let fd = libc::memfd_create(c"rustpaper-lock".as_ptr(), libc::MFD_CLOEXEC);
        if fd < 0 {
            return None;
        }
        let fd = OwnedFd::from_raw_fd(fd);
        (libc::ftruncate(fd.as_raw_fd(), len as libc::off_t) == 0).then_some(fd)
    }
}

/// An shm buffer of `w`x`h` ARGB8888 filled by `fill` (B, G, R, A bytes).
fn shm_buffer(
    shm: &WlShm,
    qh: &QueueHandle<State>,
    (w, h): (u32, u32),
    kind: BufKind,
    fill: impl FnOnce(&mut [u8]),
) -> Option<WlBuffer> {
    let len = (w * h * 4) as usize;
    let fd = memfd(len)?;
    // SAFETY: mapping our own memfd of `len` bytes, unmapped below.
    let ptr = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            len,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_SHARED,
            fd.as_raw_fd(),
            0,
        )
    };
    if ptr == libc::MAP_FAILED {
        return None;
    }
    // SAFETY: the mapping is `len` bytes long.
    fill(unsafe { std::slice::from_raw_parts_mut(ptr.cast(), len) });
    // SAFETY: our mapping.
    unsafe { libc::munmap(ptr, len) };
    let pool: WlShmPool = shm.create_pool(fd.as_fd(), len as i32, qh, ());
    let buffer = pool.create_buffer(
        0,
        w as i32,
        h as i32,
        (w * 4) as i32,
        wl_shm::Format::Argb8888,
        qh,
        kind,
    );
    pool.destroy();
    Some(buffer)
}

impl State {
    fn add_output(
        &mut self,
        qh: &QueueHandle<Self>,
        registry: &WlRegistry,
        name: u32,
        version: u32,
    ) {
        let output: WlOutput = registry.bind(name, version.min(4), qh, name);
        self.outputs.push(Out {
            global: name,
            output,
            name: String::new(),
            mode: (0, 0),
            surface: None,
            lock_surface: None,
            viewport: None,
            dim: None,
            panel: None,
            size: (0, 0),
            configured: false,
            live: Vec::new(),
            showing_live: false,
        });
    }

    fn create_lock_surface(&mut self, qh: &QueueHandle<Self>, i: usize) {
        let Some(lock) = self.lock.clone() else {
            return;
        };
        let o = &mut self.outputs[i];
        if o.lock_surface.is_some() {
            return;
        }
        let surface = self.compositor.create_surface(qh, ());
        let lock_surface = lock.get_lock_surface(&surface, &o.output, qh, o.global);
        let viewport = self.viewporter.get_viewport(&surface, qh, ());
        // stacked in creation order: the dim layer, then the panel on top
        let sub = || {
            let s = self.compositor.create_surface(qh, ());
            // input goes to the lock surface itself
            let empty: WlRegion = self.compositor.create_region(qh, ());
            s.set_input_region(Some(&empty));
            empty.destroy();
            let sub = self.subcompositor.get_subsurface(&s, &surface, qh, ());
            let vp = self.viewporter.get_viewport(&s, qh, ());
            (s, sub, vp)
        };
        let dim = sub();
        let panel = sub();
        o.surface = Some(surface);
        o.lock_surface = Some(lock_surface);
        o.viewport = Some(viewport);
        o.dim = Some(dim);
        o.panel = Some(panel);
    }

    fn solid_buffer(&self, qh: &QueueHandle<Self>, rgb: [u8; 3], alpha: u8) -> Option<WlBuffer> {
        let a = alpha as u32;
        let px = [
            (rgb[2] as u32 * a / 255) as u8,
            (rgb[1] as u32 * a / 255) as u8,
            (rgb[0] as u32 * a / 255) as u8,
            alpha,
        ];
        shm_buffer(&self.shm, qh, (1, 1), BufKind::Static, |d| {
            d.copy_from_slice(&px)
        })
    }

    fn still_buffer(&self, qh: &QueueHandle<Self>, path: &std::path::Path) -> Option<Still> {
        let img = image::open(path).ok()?;
        let img = if img.width() > STILL_MAX {
            img.resize(STILL_MAX, STILL_MAX, image::imageops::FilterType::Triangle)
        } else {
            img
        };
        let rgba = img.to_rgba8();
        let size = (rgba.width(), rgba.height());
        let buffer = shm_buffer(&self.shm, qh, size, BufKind::Static, |d| {
            for (s, p) in rgba
                .as_raw()
                .as_chunks::<4>()
                .0
                .iter()
                .zip(d.as_chunks_mut::<4>().0)
            {
                // opaque: the lock surface must cover the monitor
                p.copy_from_slice(&[s[2], s[1], s[0], 0xff]);
            }
        })?;
        eprintln!("lock: still picture {}", path.display());
        Some(Still { buffer, size })
    }

    /// The still picture on monitor `i`, cropped to cover it.
    fn show_still(&mut self, i: usize) {
        let o = &mut self.outputs[i];
        let (Some(still), Some(surface), Some(vp)) = (&self.still, &o.surface, &o.viewport) else {
            return;
        };
        let (iw, ih) = (still.size.0 as f64, still.size.1 as f64);
        let (ow, oh) = (o.size.0.max(1) as f64, o.size.1.max(1) as f64);
        // crop the image to the monitor's aspect ("cover")
        let (sw, sh) = if iw / ih > ow / oh {
            (ih * ow / oh, ih)
        } else {
            (iw, iw * oh / ow)
        };
        vp.set_source((iw - sw) / 2.0, (ih - sh) / 2.0, sw, sh);
        vp.set_destination(o.size.0 as i32, o.size.1 as i32);
        surface.attach(Some(&still.buffer), 0, 0);
        surface.damage_buffer(0, 0, i32::MAX, i32::MAX);
        surface.commit();
        o.showing_live = false;
    }

    fn on_configure(&mut self, qh: &QueueHandle<Self>, i: usize, serial: u32, w: u32, h: u32) {
        let o = &mut self.outputs[i];
        if let Some(ls) = &o.lock_surface {
            ls.ack_configure(serial);
        }
        o.size = (w, h);
        o.configured = true;
        if let (Some((ds, _, dvp)), Some(dim)) = (&o.dim, &self.dim) {
            dvp.set_destination(w as i32, h as i32);
            ds.attach(Some(dim), 0, 0);
            ds.damage_buffer(0, 0, 1, 1);
            ds.commit();
        }
        if !o.showing_live {
            self.show_still(i);
        } else if let (Some(s), Some(vp)) = (&o.surface, &o.viewport) {
            vp.set_destination(w as i32, h as i32);
            s.commit();
        }
        self.draw_panel(qh, i);
    }

    fn draw_panel(&mut self, qh: &QueueHandle<Self>, i: usize) {
        let o = &self.outputs[i];
        if !o.configured || o.size.0 == 0 {
            return;
        }
        let Some((ps, sub, pvp)) = &o.panel else {
            return;
        };
        // pixels per logical pixel (fractional scaling included)
        let scale = if o.mode.0 > 0 {
            o.mode.0 as f32 / o.size.0 as f32
        } else {
            1.0
        }
        .max(1.0);
        let (time, date) = ui::now_strings();
        let state = PanelState {
            time: &time,
            date: &date,
            typed: self.password.chars(),
            status: &self.status,
            caps_lock: self.caps_lock,
        };
        let Some(pm) = self.panel.draw(&state, scale) else {
            return;
        };
        let size = (pm.width(), pm.height());
        let Some(buffer) = shm_buffer(&self.shm, qh, size, BufKind::Panel, |d| {
            ui::rgba_to_argb(pm.data(), d)
        }) else {
            return;
        };
        let (lw, lh) = ui::PANEL;
        // centered, a little above the middle
        let x = (o.size.0 as i32 - lw as i32) / 2;
        let y = (o.size.1 as i32 - lh as i32) / 2 - (o.size.1 as i32 / 12);
        sub.set_position(x, y.max(0));
        pvp.set_destination(lw as i32, lh as i32);
        ps.attach(Some(&buffer), 0, 0);
        ps.damage_buffer(0, 0, size.0 as i32, size.1 as i32);
        ps.commit();
        // subsurfaces are synchronized: the parent's commit applies them
        if let Some(s) = &o.surface {
            s.commit();
        }
    }

    fn read_feed(&mut self, qh: &QueueHandle<Self>) {
        let Some(feed) = &self.feed else { return };
        match feed.read() {
            Ok(events) => {
                for e in events {
                    match e {
                        FeedEvent::Frame(f) => self.show_frame(qh, f),
                        FeedEvent::Gone(key) => self.forget(key),
                    }
                }
            }
            Err(()) => {
                eprintln!("lock: the engine went away; keeping its last frame");
                self.feed = None;
                // a new engine's keys mean new buffers
                let keys: Vec<u64> = self
                    .outputs
                    .iter()
                    .flat_map(|o| o.live.iter().map(|b| b.key))
                    .collect();
                for k in keys {
                    self.forget(k);
                }
            }
        }
    }

    fn show_frame(&mut self, qh: &QueueHandle<Self>, f: FeedFrame) {
        let release = |st: &Self, key| {
            if let Some(feed) = &st.feed {
                feed.release(&f.output, key);
            }
        };
        let Some(i) = self
            .outputs
            .iter()
            .position(|o| o.name == f.output && o.configured)
        else {
            return release(self, f.key);
        };
        let cached = self.outputs[i]
            .live
            .iter()
            .find(|b| b.key == f.key && !b.gone)
            .map(|b| b.id);
        let id = match cached {
            Some(id) => id,
            None => {
                let (Some(dmabuf), false) = (&self.dmabuf, f.fds.is_empty()) else {
                    return release(self, f.key);
                };
                let params = dmabuf.create_params(qh, ());
                for (n, (fd, (offset, stride))) in f.fds.iter().zip(&f.planes).enumerate() {
                    params.add(
                        fd.as_fd(),
                        n as u32,
                        *offset,
                        *stride,
                        (f.modifier >> 32) as u32,
                        f.modifier as u32,
                    );
                }
                let id = self.next_id;
                self.next_id += 1;
                let buffer = params.create_immed(
                    f.size.0 as i32,
                    f.size.1 as i32,
                    f.fourcc,
                    zwp_linux_buffer_params_v1::Flags::empty(),
                    qh,
                    BufKind::Live(id),
                );
                params.destroy();
                self.outputs[i].live.push(LiveBuf {
                    id,
                    key: f.key,
                    buffer,
                    on_screen: false,
                    gone: false,
                });
                id
            }
        };
        let o = &mut self.outputs[i];
        let (Some(surface), Some(vp)) = (&o.surface, &o.viewport) else {
            return;
        };
        let Some(b) = o.live.iter_mut().find(|b| b.id == id) else {
            return;
        };
        vp.set_source(-1.0, -1.0, -1.0, -1.0);
        vp.set_destination(o.size.0 as i32, o.size.1 as i32);
        surface.attach(Some(&b.buffer), 0, 0);
        surface.damage_buffer(0, 0, f.size.0 as i32, f.size.1 as i32);
        surface.commit();
        b.on_screen = true;
        o.showing_live = true;
    }

    fn forget(&mut self, key: u64) {
        for o in &mut self.outputs {
            o.live.retain_mut(|b| {
                if b.key != key {
                    return true;
                }
                if b.on_screen {
                    b.gone = true;
                    return true;
                }
                b.buffer.destroy();
                false
            });
        }
    }

    fn released(&mut self, id: u64) {
        let mut done = None;
        for o in &mut self.outputs {
            if let Some(pos) = o.live.iter().position(|b| b.id == id) {
                let b = &mut o.live[pos];
                b.on_screen = false;
                done = Some((o.name.clone(), b.key, b.gone));
                if b.gone {
                    b.buffer.destroy();
                    o.live.remove(pos);
                }
            }
        }
        if let (Some((output, key, gone)), Some(feed)) = (done, &self.feed)
            && !gone
        {
            feed.release(&output, key);
        }
    }

    /// Whether holding `keycode` repeats it: what the keymap says, except
    /// Enter (a held Enter must not submit twice).
    fn repeats(&self, keycode: u32) -> bool {
        let Some(xkb) = &self.xkb else { return false };
        let code = xkb::Keycode::new(keycode + 8);
        let sym = xkb.key_get_one_sym(code).raw();
        xkb.get_keymap().key_repeats(code)
            && sym != xkb::keysyms::KEY_Return
            && sym != xkb::keysyms::KEY_KP_Enter
    }

    // xkbcommon names its keysym constants after X11 (KEY_Return)
    #[allow(non_upper_case_globals)]
    fn key(&mut self, keycode: u32) {
        let Some(xkb) = &self.xkb else { return };
        let code = xkb::Keycode::new(keycode + 8);
        let sym = xkb.key_get_one_sym(code).raw();
        let ctrl = xkb.mod_name_is_active(xkb::MOD_NAME_CTRL, xkb::STATE_MODS_EFFECTIVE);
        let text = xkb.key_get_utf8(code);
        if self.status == Status::Checking {
            return;
        }
        use xkb::keysyms::*;
        match sym {
            KEY_Return | KEY_KP_Enter => {
                if self.password.chars() > 0 {
                    self.submit = Some(self.password.take());
                }
            }
            KEY_BackSpace if ctrl => self.password.clear(),
            KEY_BackSpace => self.password.pop(),
            KEY_Escape => self.password.clear(),
            KEY_u if ctrl => self.password.clear(),
            _ if !ctrl && !text.is_empty() && !text.chars().any(char::is_control) => {
                self.password.push(&text);
            }
            _ => return,
        }
        if matches!(self.status, Status::Failed(_)) {
            self.status = Status::Idle;
        }
        self.panel_dirty = true;
    }

    fn unlock(&mut self, _qh: &QueueHandle<Self>) {
        for o in &mut self.outputs {
            if let Some(ls) = o.lock_surface.take() {
                ls.destroy();
            }
        }
        if let Some(lock) = self.lock.take() {
            lock.unlock_and_destroy();
        }
    }
}

// ---- Wayland events --------------------------------------------------------

impl Dispatch<WlRegistry, GlobalListContents> for State {
    fn event(
        st: &mut Self,
        reg: &WlRegistry,
        e: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match e {
            wl_registry::Event::Global {
                name,
                interface,
                version,
            } if interface == WlOutput::interface().name => {
                // a monitor plugged in while locked: its lock surface follows
                // once its name is known (wl_output.done)
                st.add_output(qh, reg, name, version);
            }
            wl_registry::Event::GlobalRemove { name } => {
                if let Some(pos) = st.outputs.iter().position(|o| o.global == name) {
                    let o = st.outputs.remove(pos);
                    if let Some(ls) = o.lock_surface {
                        ls.destroy();
                    }
                    if let Some(s) = o.surface {
                        s.destroy();
                    }
                    o.output.release();
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<WlOutput, u32> for State {
    fn event(
        st: &mut Self,
        _: &WlOutput,
        e: wl_output::Event,
        global: &u32,
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        let Some(i) = st.outputs.iter().position(|o| o.global == *global) else {
            return;
        };
        match e {
            wl_output::Event::Name { name } => st.outputs[i].name = name,
            wl_output::Event::Mode {
                flags: WEnum::Value(f),
                width,
                height,
                ..
            } if f.contains(wl_output::Mode::Current) => {
                st.outputs[i].mode = (width as u32, height as u32);
            }
            wl_output::Event::Done if st.lock.is_some() => st.create_lock_surface(qh, i),
            _ => {}
        }
    }
}

impl Dispatch<ExtSessionLockV1, ()> for State {
    fn event(
        st: &mut Self,
        _: &ExtSessionLockV1,
        e: ext_session_lock_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match e {
            ext_session_lock_v1::Event::Locked => {
                eprintln!("lock: session locked");
                st.locked = true;
            }
            ext_session_lock_v1::Event::Finished => st.finished = true,
            _ => {}
        }
    }
}

impl Dispatch<ExtSessionLockSurfaceV1, u32> for State {
    fn event(
        st: &mut Self,
        _: &ExtSessionLockSurfaceV1,
        e: ext_session_lock_surface_v1::Event,
        global: &u32,
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let ext_session_lock_surface_v1::Event::Configure {
            serial,
            width,
            height,
        } = e
            && let Some(i) = st.outputs.iter().position(|o| o.global == *global)
        {
            st.on_configure(qh, i, serial, width, height);
        }
    }
}

impl Dispatch<WlBuffer, BufKind> for State {
    fn event(
        st: &mut Self,
        buffer: &WlBuffer,
        e: wl_buffer::Event,
        kind: &BufKind,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_buffer::Event::Release = e {
            match kind {
                BufKind::Live(id) => st.released(*id),
                BufKind::Panel => buffer.destroy(),
                BufKind::Static => {}
            }
        }
    }
}

impl Dispatch<WlSeat, ()> for State {
    fn event(
        st: &mut Self,
        seat: &WlSeat,
        e: wl_seat::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Capabilities {
            capabilities: WEnum::Value(caps),
        } = e
            && caps.contains(wl_seat::Capability::Keyboard)
            && st.keyboard.is_none()
        {
            st.keyboard = Some(seat.get_keyboard(qh, ()));
        }
    }
}

impl Dispatch<WlKeyboard, ()> for State {
    fn event(
        st: &mut Self,
        _: &WlKeyboard,
        e: wl_keyboard::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match e {
            wl_keyboard::Event::Keymap {
                format: WEnum::Value(wl_keyboard::KeymapFormat::XkbV1),
                fd,
                size,
            } => {
                // SAFETY: the compositor's keymap fd and size, mapped read-only by xkbcommon.
                let keymap = unsafe {
                    xkb::Keymap::new_from_fd(
                        &st.xkb_ctx,
                        fd,
                        size as usize,
                        xkb::KEYMAP_FORMAT_TEXT_V1,
                        xkb::KEYMAP_COMPILE_NO_FLAGS,
                    )
                };
                if let Ok(Some(k)) = keymap {
                    st.xkb = Some(xkb::State::new(&k));
                }
            }
            wl_keyboard::Event::Key {
                key,
                state: WEnum::Value(wl_keyboard::KeyState::Pressed),
                ..
            } => {
                st.key(key);
                if st.repeats(key) {
                    st.repeat.press(key, Instant::now());
                }
            }
            wl_keyboard::Event::Key {
                key,
                state: WEnum::Value(wl_keyboard::KeyState::Released),
                ..
            } => st.repeat.release(key),
            wl_keyboard::Event::RepeatInfo { rate, delay } => st.repeat.set(rate, delay),
            wl_keyboard::Event::Leave { .. } => st.repeat.held = None,
            wl_keyboard::Event::Modifiers {
                mods_depressed,
                mods_latched,
                mods_locked,
                group,
                ..
            } => {
                if let Some(x) = &mut st.xkb {
                    x.update_mask(mods_depressed, mods_latched, mods_locked, 0, 0, group);
                    let caps = x.mod_name_is_active(xkb::MOD_NAME_CAPS, xkb::STATE_MODS_LOCKED);
                    if caps != st.caps_lock {
                        st.caps_lock = caps;
                        st.panel_dirty = true;
                    }
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<ZwpLinuxDmabufV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &ZwpLinuxDmabufV1,
        _: zwp_linux_dmabuf_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwpLinuxBufferParamsV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &ZwpLinuxBufferParamsV1,
        e: zwp_linux_buffer_params_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zwp_linux_buffer_params_v1::Event::Failed = e {
            eprintln!("lock: the compositor refused a frame buffer");
        }
    }
}

delegate_noop!(State: ignore WlCompositor);
delegate_noop!(State: ignore WlSubcompositor);
delegate_noop!(State: ignore WlSubsurface);
delegate_noop!(State: ignore WlSurface);
delegate_noop!(State: ignore WlRegion);
delegate_noop!(State: ignore WlShm);
delegate_noop!(State: ignore WlShmPool);
delegate_noop!(State: ignore WpViewporter);
delegate_noop!(State: ignore WpViewport);
delegate_noop!(State: ignore ExtSessionLockManagerV1);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_edits_by_character_and_zeroes() {
        let mut s = Secret::default();
        s.push("ab");
        s.push("ç");
        assert_eq!(s.chars(), 3);
        s.pop();
        assert_eq!(s.0, b"ab");
        let taken = s.take();
        assert_eq!(taken, b"ab");
        assert_eq!(s.chars(), 0);
    }

    #[test]
    fn held_keys_repeat_after_the_delay_at_the_rate() {
        let t0 = Instant::now();
        let mut r = Repeat::default();
        r.set(20, 500);
        r.press(14, t0);
        assert_eq!(
            r.due(t0 + Duration::from_millis(499)),
            None,
            "not before the delay"
        );
        assert_eq!(r.due(t0 + Duration::from_millis(500)), Some(14));
        assert_eq!(
            r.next(),
            Some(t0 + Duration::from_millis(550)),
            "then every 1/20 s"
        );
        assert_eq!(r.due(t0 + Duration::from_millis(549)), None);
        assert_eq!(r.due(t0 + Duration::from_millis(550)), Some(14));
        r.release(30);
        assert!(r.next().is_some(), "another key's release changes nothing");
        r.release(14);
        assert_eq!(r.due(t0 + Duration::from_secs(5)), None);
        r.set(0, 500);
        r.press(14, t0);
        assert_eq!(r.next(), None, "rate 0: no repeat");
    }

    #[test]
    fn pam_failure_reads_naturally() {
        assert_eq!(friendly("Authentication failure"), "Wrong password");
        assert_eq!(friendly("Account locked"), "Account locked");
    }
}
