//! The private stage: Rustpaper's own headless Wayland compositor with its
//! own XWayland, where Wallpaper Engine opens its windows.
//!
//! Desktop stages (desktop/) hide WE's windows inside the user's compositor
//! with headless outputs and rules, and repair them all the time. Here the
//! windows live in a compositor of ours, so nothing needs hiding or
//! repairing:
//! - every monitor is a `wl_output` of ours, so WE sees the real monitors;
//! - the X11 window manager below gives `rustpaper-<output>` exactly its
//!   monitor's size and position;
//! - WE's other windows (UI, dialogs) are mapped but shown nowhere.
//!
//! WE's buffers reach us directly: each commit of a wallpaper window hands
//! its dmabuf to the main thread, which attaches it to the monitor's
//! layer-shell surface on the user's compositor (mirror.rs). No capture, no
//! copy. The client buffer goes back to WE once the user's compositor
//! released it. shm buffers (no GPU path) are copied instead.
//!
//! WE connects to our XWayland through its Steam launch option
//! (`rustpaper we-launch %command%`, launch.rs): we publish our display in
//! `$XDG_RUNTIME_DIR/rustpaper/stage-display`.
//!
//! Runs on its own thread (calloop); talks to the main thread through
//! channels plus a pipe the main loop polls.

use std::collections::{HashMap, HashSet};
use std::io::Write as _;
use std::os::fd::{AsRawFd, OwnedFd};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use smithay::backend::allocator::{Buffer as _, Format, Fourcc, Modifier};
use smithay::output::{Mode, Output as WlOutputState, PhysicalProperties, Subpixel};
use smithay::reexports::calloop::channel::{self, Channel, Sender};
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::{EventLoop, Interest, LoopHandle, Mode as CMode, PostAction};
use smithay::reexports::wayland_server::backend::{ClientData, ClientId, DisconnectReason};
use smithay::reexports::wayland_server::protocol::wl_buffer::WlBuffer;
use smithay::reexports::wayland_server::protocol::wl_shm;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{Client, Display, DisplayHandle, Resource};
use smithay::utils::{Logical, Rectangle};
use smithay::wayland::buffer::BufferHandler;
use smithay::wayland::compositor::{
    BufferAssignment, CompositorClientState, CompositorHandler, CompositorState, SurfaceAttributes,
    with_states,
};
use smithay::wayland::dmabuf::{
    DmabufFeedbackBuilder, DmabufGlobal, DmabufHandler, DmabufState, ImportNotifier, get_dmabuf,
};
use smithay::wayland::output::OutputManagerState;
use smithay::wayland::shm::{ShmHandler, ShmState, with_buffer_contents};
use smithay::wayland::xwayland_shell::{XWaylandShellHandler, XWaylandShellState};
use smithay::xwayland::xwm::{Reorder, ResizeEdge, WmWindowProperty, X11Window, XwmId};
use smithay::xwayland::{
    X11Surface, X11Wm, XWayland, XWaylandClientData, XWaylandEvent, XwmHandler,
};
use smithay::{
    delegate_compositor, delegate_dmabuf, delegate_output, delegate_shm, delegate_xwayland_shell,
};

use crate::desktop::{Output, Stage, window_title};
use crate::launch;

/// The dmabuf layouts the user's compositor imports (from its
/// `zwp_linux_dmabuf_v1` default feedback): advertised to our XWayland, so
/// every buffer WE renders can be shown without conversion.
#[derive(Debug, Clone, Default)]
pub struct HostDmabuf {
    /// `dev_t` of the GPU the compositor renders with.
    pub main_device: u64,
    /// (fourcc, modifier) pairs.
    pub formats: Vec<(u32, u64)>,
}

/// One plane of a dmabuf frame.
pub struct Plane {
    pub fd: OwnedFd,
    pub offset: u32,
    pub stride: u32,
}

pub enum FramePixels {
    /// WE's GPU buffer, shown as-is; `key` identifies it until
    /// [`PrivateStage::release`] gives it back.
    Dmabuf {
        fourcc: u32,
        modifier: u64,
        planes: Vec<Plane>,
    },
    /// A copy of an shm buffer (`wl_shm` format code), already given back.
    Shm {
        format: u32,
        stride: u32,
        data: Vec<u8>,
    },
}

pub struct Frame {
    pub output: String,
    /// The client buffer (its protocol id): the same key comes back as WE
    /// cycles through its few buffers.
    pub key: u64,
    pub size: (u32, u32),
    pub pixels: FramePixels,
}

impl Frame {
    /// The same frame for monitor `output` (its own fds), when one WE
    /// window feeds several monitors.
    pub fn for_output(&self, output: &str) -> Option<Frame> {
        let pixels = match &self.pixels {
            FramePixels::Dmabuf {
                fourcc,
                modifier,
                planes,
            } => FramePixels::Dmabuf {
                fourcc: *fourcc,
                modifier: *modifier,
                planes: planes
                    .iter()
                    .map(|p| {
                        Some(Plane {
                            fd: p.fd.try_clone().ok()?,
                            offset: p.offset,
                            stride: p.stride,
                        })
                    })
                    .collect::<Option<Vec<_>>>()?,
            },
            FramePixels::Shm {
                format,
                stride,
                data,
            } => FramePixels::Shm {
                format: *format,
                stride: *stride,
                data: data.clone(),
            },
        };
        Some(Frame {
            output: output.to_string(),
            key: self.key,
            size: self.size,
            pixels,
        })
    }
}

pub enum StageEvent {
    Frame(Frame),
    /// The client buffer `key` was destroyed: drop anything cached for it.
    BufferGone(u64),
}

/// The main thread's handle on the stage.
pub struct PrivateStage {
    display: String,
    events: mpsc::Receiver<StageEvent>,
    /// Readable when `events` has something (the main loop polls it).
    notify: OwnedFd,
    release: Sender<u64>,
    /// Titles of WE's mapped wallpaper windows.
    windows: Arc<Mutex<HashSet<String>>>,
}

impl PrivateStage {
    /// Starts the compositor and XWayland for `outputs`; returns once
    /// XWayland is ready.
    pub fn start(outputs: &[Output], host: HostDmabuf) -> Result<Self, String> {
        let (tx, events) = mpsc::channel();
        let (release, release_rx) = channel::channel::<u64>();
        let (notify, wake) = pipe().map_err(|e| format!("pipe: {e}"))?;
        let windows = Arc::new(Mutex::new(HashSet::new()));
        let (ready_tx, ready_rx) = mpsc::channel();
        let outputs = outputs.to_vec();
        let shared = windows.clone();
        std::thread::Builder::new()
            .name("rustpaper-stage".into())
            .spawn(move || {
                if let Err(e) = run(
                    outputs,
                    host,
                    tx,
                    wake,
                    release_rx,
                    shared,
                    ready_tx.clone(),
                ) {
                    let _ = ready_tx.send(Err(e));
                }
            })
            .map_err(|e| e.to_string())?;
        let display = ready_rx
            .recv_timeout(Duration::from_secs(20))
            .map_err(|_| "the private stage's XWayland did not start".to_string())??;
        if let Some(file) = launch::display_file() {
            if let Some(dir) = file.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            std::fs::write(&file, &display).map_err(|e| format!("{}: {e}", file.display()))?;
        }
        eprintln!("stage: private, XWayland on {display}");
        Ok(PrivateStage {
            display,
            events,
            notify,
            release,
            windows,
        })
    }

    pub fn display(&self) -> &str {
        &self.display
    }

    /// The fd to poll for frames.
    pub fn notify_fd(&self) -> i32 {
        self.notify.as_raw_fd()
    }

    /// Everything the stage sent since the last call.
    pub fn drain(&self) -> Vec<StageEvent> {
        let mut buf = [0u8; 64];
        // SAFETY: reading into our own buffer from our own non-blocking pipe.
        while unsafe { libc::read(self.notify.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) } > 0
        {
        }
        self.events.try_iter().collect()
    }

    /// The user's compositor released the buffer `key`: WE may reuse it.
    pub fn release(&self, key: u64) {
        let _ = self.release.send(key);
    }
}

impl Drop for PrivateStage {
    fn drop(&mut self) {
        if let Some(file) = launch::display_file()
            && std::fs::read_to_string(&file).is_ok_and(|d| d.trim() == self.display)
        {
            let _ = std::fs::remove_file(file);
        }
    }
}

impl Stage for PrivateStage {
    /// Nothing to prepare: the outputs were created at start.
    fn prepare(&self, _out: &Output) {}

    /// WE's UI windows are mapped in our compositor only: never visible.
    fn hide_we_ui(&self) {}

    fn tend(&self, out: &Output) -> bool {
        self.windows
            .lock()
            .unwrap()
            .contains(&window_title(&out.name))
    }
}

/// A non-blocking pipe (read end, write end).
fn pipe() -> std::io::Result<(OwnedFd, std::fs::File)> {
    let mut fds = [0; 2];
    // SAFETY: plain pipe2 into a local array.
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_NONBLOCK | libc::O_CLOEXEC) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: both fds were just created and are owned here.
    unsafe {
        use std::os::fd::FromRawFd;
        Ok((
            OwnedFd::from_raw_fd(fds[0]),
            std::fs::File::from_raw_fd(fds[1]),
        ))
    }
}

// ---- the compositor thread -------------------------------------------------

struct State {
    handle: LoopHandle<'static, State>,
    dh: DisplayHandle,
    compositor: CompositorState,
    shm: ShmState,
    dmabuf: DmabufState,
    _dmabuf_global: Option<DmabufGlobal>,
    _output_manager: OutputManagerState,
    xwayland_shell: XWaylandShellState,
    xwm: Option<X11Wm>,
    /// Monitor name -> its rectangle in our (logical = pixel) space.
    layout: Vec<(String, Rectangle<i32, Logical>)>,
    /// WE's windows we know (to find the window of a committed surface).
    surfaces: Vec<X11Surface>,
    /// Client buffers handed to the main thread, until released.
    held: HashMap<u64, WlBuffer>,
    tx: mpsc::Sender<StageEvent>,
    wake: std::fs::File,
    windows: Arc<Mutex<HashSet<String>>>,
    started: Instant,
}

#[allow(clippy::too_many_arguments)]
fn run(
    outputs: Vec<Output>,
    host: HostDmabuf,
    tx: mpsc::Sender<StageEvent>,
    wake: std::fs::File,
    release_rx: Channel<u64>,
    windows: Arc<Mutex<HashSet<String>>>,
    ready: mpsc::Sender<Result<String, String>>,
) -> Result<(), String> {
    let mut event_loop: EventLoop<'static, State> =
        EventLoop::try_new().map_err(|e| e.to_string())?;
    let display: Display<State> = Display::new().map_err(|e| e.to_string())?;
    let dh = display.handle();

    // One wl_output per monitor, side by side, 1:1 pixels (WE renders at the
    // monitor's pixel size; the user's compositor scales on its side).
    let mut layout = Vec::new();
    let mut x = 0;
    for o in &outputs {
        let (w, h) = (o.size.0 as i32, o.size.1 as i32);
        let out = WlOutputState::new(
            o.name.clone(),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: "Rustpaper".into(),
                model: o.name.clone(),
            },
        );
        let mode = Mode {
            size: (w, h).into(),
            refresh: 60_000,
        };
        out.change_current_state(
            Some(mode),
            None,
            Some(smithay::output::Scale::Integer(1)),
            Some((x, 0).into()),
        );
        out.set_preferred(mode);
        out.create_global::<State>(&dh);
        layout.push((o.name.clone(), Rectangle::new((x, 0).into(), (w, h).into())));
        x += w;
    }

    let mut dmabuf = DmabufState::new();
    let formats: Vec<Format> = host
        .formats
        .iter()
        .filter_map(|&(code, modifier)| {
            Some(Format {
                code: Fourcc::try_from(code).ok()?,
                modifier: Modifier::from(modifier),
            })
        })
        .collect();
    let dmabuf_global = if host.main_device != 0 && !formats.is_empty() {
        let feedback = DmabufFeedbackBuilder::new(host.main_device as libc::dev_t, formats)
            .build()
            .map_err(|e| format!("dmabuf feedback: {e}"))?;
        Some(dmabuf.create_global_with_default_feedback::<State>(&dh, &feedback))
    } else {
        eprintln!("stage: the user's compositor gave no dmabuf formats; XWayland will use shm");
        None
    };

    let mut state = State {
        handle: event_loop.handle(),
        dh: dh.clone(),
        compositor: CompositorState::new::<State>(&dh),
        shm: ShmState::new::<State>(&dh, vec![]),
        dmabuf,
        _dmabuf_global: dmabuf_global,
        _output_manager: OutputManagerState::new_with_xdg_output::<State>(&dh),
        xwayland_shell: XWaylandShellState::new::<State>(&dh),
        xwm: None,
        layout,
        surfaces: Vec::new(),
        held: HashMap::new(),
        tx,
        wake,
        windows,
        started: Instant::now(),
    };

    let handle = event_loop.handle();
    handle
        .insert_source(
            Generic::new(display, Interest::READ, CMode::Level),
            |_, display, state| {
                // SAFETY: the display is not dropped while the loop runs.
                unsafe {
                    display
                        .get_mut()
                        .dispatch_clients(state)
                        .map_err(std::io::Error::other)?
                };
                Ok(PostAction::Continue)
            },
        )
        .map_err(|e| e.to_string())?;
    handle
        .insert_source(release_rx, |event, _, state| {
            if let channel::Event::Msg(key) = event
                && let Some(buf) = state.held.remove(&key)
            {
                buf.release();
            }
        })
        .map_err(|e| e.to_string())?;

    let (xwayland, client) = XWayland::spawn(
        &dh,
        None,
        std::iter::empty::<(String, String)>(),
        true,
        std::process::Stdio::null(),
        std::process::Stdio::inherit(),
        |_| {},
    )
    .map_err(|e| format!("could not start Xwayland: {e}"))?;
    let ready_once = ready.clone();
    handle
        .insert_source(xwayland, move |event, _, state| match event {
            XWaylandEvent::Ready {
                x11_socket,
                display_number,
            } => match X11Wm::start_wm(state.handle.clone(), x11_socket, client.clone()) {
                Ok(wm) => {
                    state.xwm = Some(wm);
                    let _ = ready_once.send(Ok(format!(":{display_number}")));
                }
                Err(e) => {
                    let _ = ready_once.send(Err(format!("X11 window manager: {e}")));
                }
            },
            XWaylandEvent::Error => {
                let _ = ready_once.send(Err("Xwayland exited during startup".into()));
            }
        })
        .map_err(|e| e.to_string())?;

    loop {
        event_loop
            .dispatch(Some(Duration::from_millis(500)), &mut state)
            .map_err(|e| e.to_string())?;
        let _ = state.dh.flush_clients();
    }
}

impl State {
    fn send(&mut self, event: StageEvent) {
        if self.tx.send(event).is_ok() {
            let _ = self.wake.write(&[1]);
        }
    }

    /// The monitor whose wallpaper window `title` is.
    fn output_for_title(&self, title: &str) -> Option<(String, Rectangle<i32, Logical>)> {
        self.layout
            .iter()
            .find(|(name, _)| window_title(name) == title)
            .cloned()
    }

    /// Places a wallpaper window on its monitor (and records it), or leaves
    /// any other window where it asked to be.
    fn place(&mut self, window: &X11Surface) {
        if let Some((_, rect)) = self.output_for_title(&window.title()) {
            let _ = window.configure(rect);
            let _ = window.set_fullscreen(true);
        }
        self.refresh_windows();
    }

    fn refresh_windows(&mut self) {
        let titles: HashSet<String> = self
            .surfaces
            .iter()
            .filter(|w| w.is_mapped() && self.output_for_title(&w.title()).is_some())
            .map(|w| w.title())
            .collect();
        *self.windows.lock().unwrap() = titles;
    }

    fn output_of_surface(&self, surface: &WlSurface) -> Option<String> {
        let w = self
            .surfaces
            .iter()
            .find(|w| w.wl_surface().as_ref() == Some(surface))?;
        self.output_for_title(&w.title()).map(|(name, _)| name)
    }
}

impl CompositorHandler for State {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        if let Some(x) = client.get_data::<XWaylandClientData>() {
            return &x.compositor_state;
        }
        &client
            .get_data::<OtherClient>()
            .expect("unknown client")
            .compositor
    }

    fn commit(&mut self, surface: &WlSurface) {
        let (buffer, callbacks) = with_states(surface, |states| {
            let mut attrs = states.cached_state.get::<SurfaceAttributes>();
            let cur = attrs.current();
            (cur.buffer.take(), std::mem::take(&mut cur.frame_callbacks))
        });
        // pacing is WE's own (its FPS setting): answer frame callbacks now
        let now = self.started.elapsed().as_millis() as u32;
        for cb in callbacks {
            cb.done(now);
        }
        let Some(BufferAssignment::NewBuffer(buffer)) = buffer else {
            return;
        };
        let Some(output) = self.output_of_surface(surface) else {
            // WE's UI, dialogs: shown nowhere
            buffer.release();
            return;
        };
        let key = buffer.id().protocol_id() as u64;
        if let Ok(dmabuf) = get_dmabuf(&buffer) {
            let planes: Option<Vec<Plane>> = dmabuf
                .handles()
                .zip(dmabuf.offsets())
                .zip(dmabuf.strides())
                .map(|((fd, offset), stride)| {
                    Some(Plane {
                        fd: fd.try_clone_to_owned().ok()?,
                        offset,
                        stride,
                    })
                })
                .collect();
            let Some(planes) = planes else {
                buffer.release();
                return;
            };
            let format = dmabuf.format();
            let size = (dmabuf.width(), dmabuf.height());
            let pixels = FramePixels::Dmabuf {
                fourcc: format.code as u32,
                modifier: u64::from(format.modifier),
                planes,
            };
            self.held.insert(key, buffer);
            self.send(StageEvent::Frame(Frame {
                output,
                key,
                size,
                pixels,
            }));
            return;
        }
        let copied = with_buffer_contents(&buffer, |ptr, len, data| {
            let start = data.offset as usize;
            let bytes = (data.stride * data.height) as usize;
            if start + bytes > len {
                return None;
            }
            // SAFETY: smithay maps the client's pool; the range was checked.
            let slice = unsafe { std::slice::from_raw_parts(ptr.add(start), bytes) };
            Some((slice.to_vec(), data))
        });
        buffer.release();
        if let Ok(Some((data, info))) = copied {
            let format = match info.format {
                wl_shm::Format::Argb8888 => 0,
                wl_shm::Format::Xrgb8888 => 1,
                other => other as u32,
            };
            self.send(StageEvent::Frame(Frame {
                output,
                key,
                size: (info.width as u32, info.height as u32),
                pixels: FramePixels::Shm {
                    format,
                    stride: info.stride as u32,
                    data,
                },
            }));
        }
    }
}

impl BufferHandler for State {
    fn buffer_destroyed(&mut self, buffer: &WlBuffer) {
        let key = buffer.id().protocol_id() as u64;
        self.held.remove(&key);
        self.send(StageEvent::BufferGone(key));
    }
}

impl ShmHandler for State {
    fn shm_state(&self) -> &ShmState {
        &self.shm
    }
}

impl DmabufHandler for State {
    fn dmabuf_state(&mut self) -> &mut DmabufState {
        &mut self.dmabuf
    }

    fn dmabuf_imported(
        &mut self,
        _global: &DmabufGlobal,
        _dmabuf: smithay::backend::allocator::dmabuf::Dmabuf,
        notifier: ImportNotifier,
    ) {
        // the layouts were the user's compositor's own: it validates on import
        let _ = notifier.successful::<State>();
    }
}

impl XWaylandShellHandler for State {
    fn xwayland_shell_state(&mut self) -> &mut XWaylandShellState {
        &mut self.xwayland_shell
    }

    fn surface_associated(&mut self, _xwm: XwmId, _surface: WlSurface, window: X11Surface) {
        self.place(&window);
    }
}

impl XwmHandler for State {
    fn xwm_state(&mut self, _xwm: XwmId) -> &mut X11Wm {
        self.xwm.as_mut().expect("the X11 window manager runs")
    }

    fn new_window(&mut self, _xwm: XwmId, window: X11Surface) {
        self.surfaces.push(window);
    }

    fn new_override_redirect_window(&mut self, _xwm: XwmId, window: X11Surface) {
        self.surfaces.push(window);
    }

    fn map_window_request(&mut self, _xwm: XwmId, window: X11Surface) {
        let _ = window.set_mapped(true);
        self.place(&window);
    }

    fn map_window_notify(&mut self, _xwm: XwmId, window: X11Surface) {
        self.place(&window);
    }

    fn mapped_override_redirect_window(&mut self, _xwm: XwmId, _window: X11Surface) {
        self.refresh_windows();
    }

    fn unmapped_window(&mut self, _xwm: XwmId, window: X11Surface) {
        if !window.is_override_redirect() {
            let _ = window.set_mapped(false);
        }
        self.refresh_windows();
    }

    fn destroyed_window(&mut self, _xwm: XwmId, window: X11Surface) {
        self.surfaces.retain(|w| w != &window);
        self.refresh_windows();
    }

    fn configure_request(
        &mut self,
        _xwm: XwmId,
        window: X11Surface,
        x: Option<i32>,
        y: Option<i32>,
        w: Option<u32>,
        h: Option<u32>,
        _reorder: Option<Reorder>,
    ) {
        if let Some((_, rect)) = self.output_for_title(&window.title()) {
            // a wallpaper window keeps its monitor's geometry
            let _ = window.configure(rect);
            return;
        }
        let mut g = window.geometry();
        if let Some(x) = x {
            g.loc.x = x;
        }
        if let Some(y) = y {
            g.loc.y = y;
        }
        if let Some(w) = w {
            g.size.w = w as i32;
        }
        if let Some(h) = h {
            g.size.h = h as i32;
        }
        let _ = window.configure(g);
    }

    fn configure_notify(
        &mut self,
        _xwm: XwmId,
        _window: X11Surface,
        _geometry: Rectangle<i32, Logical>,
        _above: Option<X11Window>,
    ) {
    }

    fn property_notify(&mut self, _xwm: XwmId, window: X11Surface, property: WmWindowProperty) {
        // WE may set the title after mapping: place it once it is ours
        if matches!(property, WmWindowProperty::Title) {
            self.place(&window);
        }
    }

    fn fullscreen_request(&mut self, _xwm: XwmId, window: X11Surface) {
        self.place(&window);
    }

    fn unfullscreen_request(&mut self, _xwm: XwmId, window: X11Surface) {
        // wallpaper windows stay fullscreen on their monitor
        self.place(&window);
    }

    fn resize_request(
        &mut self,
        _xwm: XwmId,
        _window: X11Surface,
        _button: u32,
        _edge: ResizeEdge,
    ) {
    }

    fn move_request(&mut self, _xwm: XwmId, _window: X11Surface, _button: u32) {}
}

/// Data of clients other than XWayland (none are expected: the stage has no
/// socket). Kept so the compositor state lookup is total.
#[derive(Default)]
struct OtherClient {
    compositor: CompositorClientState,
}

impl ClientData for OtherClient {
    fn initialized(&self, _client_id: ClientId) {}
    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {}
}

impl smithay::wayland::output::OutputHandler for State {}

delegate_compositor!(State);
delegate_shm!(State);
delegate_dmabuf!(State);
delegate_output!(State);
delegate_xwayland_shell!(State);
