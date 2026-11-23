//! Mirrors WE's wallpaper windows onto the desktop, one per output.
//!
//! For each output: a `zwlr-layer-shell` surface (layer Bottom, namespace
//! `rustpaper`, no input) shows frames captured from WE's window with
//! `ext-image-copy-capture-v1` (window source from
//! `ext-foreign-toplevel-image-capture-source-v1`, found by title through
//! `ext-foreign-toplevel-list-v1`). The compositor copies the window into our
//! buffers and answers only when it has new content, so WE paces us.
//!
//! With the private stage (stage.rs) there is no capture: WE's own buffers
//! arrive through [`Mirrors::show`] and are attached as they are; the
//! capture protocols are then not needed at all.
//!
//! The screensaver is the same surfaces raised to the Overlay layer (over
//! every window) with all input while the user is idle
//! (`ext-idle-notify-v1`), and lowered again on the first input
//! ([`Mirrors::set_screensaver`]).
//!
//! Buffers are GBM dmabufs on the compositor's GPU, attached to our surface
//! as-is: no GL context here, and no CPU copy. With wl_shm the compositor
//! read every frame back to RAM: one 1080p60 mirror took Hyprland from 2.8%
//! to 23% CPU (dmabuf: 3.1%; 2026-11-22, RTX 4060 Ti). shm stays as the
//! fallback when GBM cannot allocate.

use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};

use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_seat::WlSeat;
use wayland_client::protocol::{
    wl_buffer::{self, WlBuffer},
    wl_compositor::WlCompositor,
    wl_output::{self, WlOutput},
    wl_region::WlRegion,
    wl_registry::WlRegistry,
    wl_shm::{self, WlShm},
    wl_shm_pool::WlShmPool,
    wl_surface::WlSurface,
};
use wayland_client::{
    Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum, delegate_noop, event_created_child,
};
use wayland_protocols::ext::foreign_toplevel_list::v1::client::{
    ext_foreign_toplevel_handle_v1::{self, ExtForeignToplevelHandleV1},
    ext_foreign_toplevel_list_v1::{self, ExtForeignToplevelListV1},
};
use wayland_protocols::ext::idle_notify::v1::client::{
    ext_idle_notification_v1::{self, ExtIdleNotificationV1},
    ext_idle_notifier_v1::ExtIdleNotifierV1,
};
use wayland_protocols::ext::image_capture_source::v1::client::{
    ext_foreign_toplevel_image_capture_source_manager_v1::ExtForeignToplevelImageCaptureSourceManagerV1,
    ext_image_capture_source_v1::ExtImageCaptureSourceV1,
};
use wayland_protocols::ext::image_copy_capture::v1::client::{
    ext_image_copy_capture_frame_v1::{self, ExtImageCopyCaptureFrameV1},
    ext_image_copy_capture_manager_v1::{ExtImageCopyCaptureManagerV1, Options},
    ext_image_copy_capture_session_v1::{self, ExtImageCopyCaptureSessionV1},
};
use wayland_protocols::wp::linux_dmabuf::zv1::client::{
    zwp_linux_buffer_params_v1::{self, ZwpLinuxBufferParamsV1},
    zwp_linux_dmabuf_feedback_v1::{self, ZwpLinuxDmabufFeedbackV1},
    zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1,
};
use wayland_protocols::wp::viewporter::client::{
    wp_viewport::WpViewport, wp_viewporter::WpViewporter,
};
use wayland_protocols_wlr::layer_shell::v1::client::{
    zwlr_layer_shell_v1::{Layer, ZwlrLayerShellV1},
    zwlr_layer_surface_v1::{self, Anchor, KeyboardInteractivity, ZwlrLayerSurfaceV1},
};

use crate::gbm;
use crate::stage::{Frame, FramePixels, HostDmabuf};

/// Buffers per output: one shown, one being captured into, one spare (the
/// shown one is reusable only after the compositor releases it).
const BUFFERS: usize = 3;

const XR24: u32 = 0x3432_5258; // DRM_FORMAT_XRGB8888
const AR24: u32 = 0x3432_5241; // DRM_FORMAT_ARGB8888

struct Buf {
    /// Stable id (part of the wl_buffer user data): indices shift.
    id: u64,
    buffer: WlBuffer,
    size: (u32, u32),
    /// Attached to the surface and not yet released by the compositor.
    on_screen: bool,
    /// Handed to a capture frame that has not finished.
    capturing: bool,
    /// The GPU memory behind a dmabuf wl_buffer (None for shm).
    _bo: Option<gbm::Bo>,
}

struct Toplevel {
    handle: Option<ExtForeignToplevelHandleV1>,
    title: String,
}

/// One output's wallpaper.
pub struct Mirror {
    pub output: String,
    /// Title of the WE window shown here.
    pub title: String,
    /// While true no frames are captured (a fullscreen window covers it).
    pub paused: bool,
    /// The lock screen shows this monitor's frames (private stage): ours
    /// are hidden under it, so WE's buffers go to the lock only.
    locked: bool,
    /// Frames shown since the counter was last reset (stats).
    pub frames: u32,
    surface: WlSurface,
    layer: ZwlrLayerSurfaceV1,
    viewport: WpViewport,
    logical: (u32, u32),
    configured: bool,
    session: Option<ExtImageCopyCaptureSessionV1>,
    size: (u32, u32),
    shm_formats: Vec<wl_shm::Format>,
    dmabuf_device: Option<u64>,
    dmabuf_formats: Vec<(u32, Vec<u64>)>,
    buffers: Vec<Buf>,
    frame: Option<(ExtImageCopyCaptureFrameV1, u64)>,
    /// Frames come from the user's compositor (capture) or from the
    /// private stage (`show`).
    capture: bool,
    /// Buffers shown for the private stage.
    external: Vec<ExtBuf>,
}

/// A buffer of the private stage's frames on the user's compositor.
struct ExtBuf {
    id: u64,
    buffer: WlBuffer,
    size: (u32, u32),
    /// The stage's buffer key (dmabuf frames; None for our shm copies).
    key: Option<u64>,
    on_screen: bool,
    /// The stage's buffer was destroyed: drop this one once released.
    gone: bool,
    /// shm copies: our mapping (pointer, length, stride, wl_shm format).
    shm: Option<(*mut u8, usize, u32, u32)>,
}

impl ExtBuf {
    fn destroy(&self) {
        self.buffer.destroy();
        if let Some((ptr, len, ..)) = self.shm {
            // SAFETY: the mapping we created for this buffer, unmapped once.
            unsafe { libc::munmap(ptr.cast(), len) };
        }
    }
}

pub struct Mirrors {
    pub list: Vec<Mirror>,
    compositor: WlCompositor,
    shm: WlShm,
    layer_shell: ZwlrLayerShellV1,
    viewporter: WpViewporter,
    source_mgr: Option<ExtForeignToplevelImageCaptureSourceManagerV1>,
    copy_mgr: Option<ExtImageCopyCaptureManagerV1>,
    dmabuf: Option<ZwpLinuxDmabufV1>,
    /// What the compositor imports (its default dmabuf feedback).
    pub host_dmabuf: HostDmabuf,
    /// Format table of the feedback being received.
    feedback_table: Vec<(u32, u64)>,
    /// (mirror index, stage buffer key) the compositor released since the
    /// last call to `take_released`. Per mirror: one WE buffer can be on
    /// several monitors (shared rendering, main.rs).
    released: Vec<(usize, u64)>,
    gbm: Option<gbm::Device>,
    gbm_broken: bool,
    /// The compositor's outputs: object, name, current mode in pixels.
    outputs: Vec<(WlOutput, String, (u32, u32))>,
    toplevels: Vec<Toplevel>,
    next_id: u64,
    /// 1x1 black buffer the surfaces show while locked (see `set_locked`).
    park: Option<WlBuffer>,
    idle_notifier: Option<ExtIdleNotifierV1>,
    seat: Option<WlSeat>,
    /// The current idle notification and its timeout (seconds).
    idle_watch: Option<(u32, ExtIdleNotificationV1)>,
    /// No input for the timeout (until the next input).
    pub idle: bool,
    screensaver: bool,
}

impl Mirrors {
    /// Connects and binds everything; Err names what the compositor lacks.
    pub fn connect() -> Result<(Self, EventQueue<Self>, Connection), String> {
        let conn = Connection::connect_to_env().map_err(|e| format!("no Wayland display: {e}"))?;
        let (globals, mut queue) = registry_queue_init::<Self>(&conn).map_err(|e| e.to_string())?;
        let qh = queue.handle();
        let lacks = |what: &str| format!("the compositor lacks {what}");
        let mut m = Mirrors {
            list: Vec::new(),
            compositor: globals
                .bind(&qh, 4..=6, ())
                .map_err(|_| lacks("wl_compositor"))?,
            shm: globals.bind(&qh, 1..=1, ()).map_err(|_| lacks("wl_shm"))?,
            layer_shell: globals
                .bind(&qh, 1..=4, ())
                .map_err(|_| lacks("zwlr_layer_shell_v1"))?,
            viewporter: globals
                .bind(&qh, 1..=1, ())
                .map_err(|_| lacks("wp_viewporter"))?,
            // capture protocols: only needed by desktop stages
            source_mgr: globals.bind(&qh, 1..=1, ()).ok(),
            copy_mgr: globals.bind(&qh, 1..=1, ()).ok(),
            dmabuf: globals.bind(&qh, 4..=5, ()).ok(),
            host_dmabuf: HostDmabuf::default(),
            feedback_table: Vec::new(),
            released: Vec::new(),
            gbm: None,
            gbm_broken: false,
            outputs: Vec::new(),
            toplevels: Vec::new(),
            next_id: 0,
            park: None,
            idle_notifier: globals.bind(&qh, 1..=1, ()).ok(),
            seat: globals.bind(&qh, 1..=1, ()).ok(),
            idle_watch: None,
            idle: false,
            screensaver: false,
        };
        let _list: Option<ExtForeignToplevelListV1> = globals.bind(&qh, 1..=1, ()).ok();
        if let Some(d) = &m.dmabuf {
            d.get_default_feedback(&qh, ());
        }
        for g in globals.contents().clone_list() {
            if g.interface == WlOutput::interface().name {
                let idx = m.outputs.len();
                let o: WlOutput = globals.registry().bind(g.name, g.version.min(4), &qh, idx);
                m.outputs.push((o, String::new(), (0, 0)));
            }
        }
        // output names, then the toplevel list
        queue.roundtrip(&mut m).map_err(|e| e.to_string())?;
        queue.roundtrip(&mut m).map_err(|e| e.to_string())?;
        Ok((m, queue, conn))
    }

    /// The compositor's outputs as (name, pixel size): what the generic
    /// desktop adapter offers when no compositor-specific one exists.
    pub fn wl_outputs(&self) -> Vec<(String, (u32, u32))> {
        self.outputs
            .iter()
            .map(|(_, n, sz)| (n.clone(), *sz))
            .collect()
    }

    /// Starts the wallpaper surface of `output`, showing window `title`:
    /// captured from the user's compositor (`capture`), or fed by the
    /// private stage through `show`.
    pub fn add(
        &mut self,
        qh: &QueueHandle<Self>,
        output: &str,
        title: &str,
        capture: bool,
    ) -> Result<(), String> {
        if capture && (self.copy_mgr.is_none() || self.source_mgr.is_none()) {
            return Err("the compositor lacks ext-image-copy-capture (window capture)".into());
        }
        let wl_output = self
            .outputs
            .iter()
            .find(|(_, n, _)| n == output)
            .map(|(o, _, _)| o.clone())
            .ok_or_else(|| format!("output {output} not found"))?;
        let idx = self.list.len();
        let surface = self.compositor.create_surface(qh, ());
        let layer = self.layer_shell.get_layer_surface(
            &surface,
            Some(&wl_output),
            Layer::Bottom,
            "rustpaper".into(),
            qh,
            idx,
        );
        layer.set_anchor(Anchor::Top | Anchor::Bottom | Anchor::Left | Anchor::Right);
        layer.set_exclusive_zone(-1);
        layer.set_size(0, 0);
        // no input: clicks go to the desktop as before
        let region = self.compositor.create_region(qh, ());
        surface.set_input_region(Some(&region));
        region.destroy();
        let viewport = self.viewporter.get_viewport(&surface, qh, ());
        // (no buffer yet: the first captured frame is the first real commit)
        surface.commit();
        self.list.push(Mirror {
            output: output.into(),
            title: title.into(),
            paused: false,
            locked: false,
            frames: 0,
            surface,
            layer,
            viewport,
            logical: (0, 0),
            configured: false,
            session: None,
            size: (0, 0),
            shm_formats: Vec::new(),
            dmabuf_device: None,
            dmabuf_formats: Vec::new(),
            buffers: Vec::new(),
            frame: None,
            capture,
            external: Vec::new(),
        });
        Ok(())
    }

    /// Shows a frame of the private stage on its monitor. A dmabuf frame is
    /// attached as it is (its host buffer cached by key); an shm frame is
    /// copied into one of our buffers. Paused monitors drop the frame (the
    /// buffer goes straight back to WE).
    pub fn show(&mut self, qh: &QueueHandle<Self>, frame: Frame) {
        let Some(i) = self.list.iter().position(|m| m.output == frame.output) else {
            // (main.rs only sends frames to monitors it has a mirror for)
            return;
        };
        let release_now = |s: &mut Self, frame: &Frame| {
            if matches!(frame.pixels, FramePixels::Dmabuf { .. }) {
                s.released.push((i, frame.key));
            }
        };
        if self.list[i].paused || self.list[i].locked || !self.list[i].configured {
            return release_now(self, &frame);
        }
        let size = frame.size;
        let id = match frame.pixels {
            FramePixels::Dmabuf {
                fourcc,
                modifier,
                planes,
            } => {
                let cached = self.list[i]
                    .external
                    .iter()
                    .find(|e| e.key == Some(frame.key) && e.size == size && !e.gone)
                    .map(|e| e.id);
                match cached {
                    Some(id) => id,
                    None => {
                        let Some(dmabuf) = self.dmabuf.clone() else {
                            return self.released.push((i, frame.key));
                        };
                        let params = dmabuf.create_params(qh, ());
                        for (n, p) in planes.iter().enumerate() {
                            params.add(
                                p.fd.as_fd(),
                                n as u32,
                                p.offset,
                                p.stride,
                                (modifier >> 32) as u32,
                                modifier as u32,
                            );
                        }
                        let id = self.take_id();
                        let buffer = params.create_immed(
                            size.0 as i32,
                            size.1 as i32,
                            fourcc,
                            zwp_linux_buffer_params_v1::Flags::empty(),
                            qh,
                            (i, id),
                        );
                        params.destroy();
                        self.list[i].external.push(ExtBuf {
                            id,
                            buffer,
                            size,
                            key: Some(frame.key),
                            on_screen: false,
                            gone: false,
                            shm: None,
                        });
                        id
                    }
                }
            }
            FramePixels::Shm {
                format,
                stride,
                data,
            } => {
                let free = self.list[i].external.iter().find(|e| {
                    e.key.is_none()
                        && !e.on_screen
                        && e.size == size
                        && e.shm
                            .is_some_and(|(_, _, st, f)| st == stride && f == format)
                });
                let id = match free.map(|e| e.id) {
                    Some(id) => id,
                    None => {
                        let len = (stride * size.1) as usize;
                        let Some(fd) = memfd(len) else { return };
                        // SAFETY: mapping our own fresh memfd of `len` bytes.
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
                            return;
                        }
                        let Ok(wl_format) = wl_shm::Format::try_from(format) else {
                            return;
                        };
                        let pool: WlShmPool = self.shm.create_pool(fd.as_fd(), len as i32, qh, ());
                        let id = self.take_id();
                        let buffer = pool.create_buffer(
                            0,
                            size.0 as i32,
                            size.1 as i32,
                            stride as i32,
                            wl_format,
                            qh,
                            (i, id),
                        );
                        pool.destroy();
                        self.list[i].external.push(ExtBuf {
                            id,
                            buffer,
                            size,
                            key: None,
                            on_screen: false,
                            gone: false,
                            shm: Some((ptr.cast(), len, stride, format)),
                        });
                        id
                    }
                };
                if let Some(e) = self.list[i].external.iter().find(|e| e.id == id)
                    && let Some((ptr, len, ..)) = e.shm
                {
                    let n = len.min(data.len());
                    // SAFETY: copying into our own mapping of `len` bytes.
                    unsafe { std::ptr::copy_nonoverlapping(data.as_ptr(), ptr, n) };
                }
                id
            }
        };
        let m = &mut self.list[i];
        let Some(e) = m.external.iter_mut().find(|e| e.id == id) else {
            return;
        };
        m.surface.attach(Some(&e.buffer), 0, 0);
        m.viewport
            .set_destination(m.logical.0 as i32, m.logical.1 as i32);
        m.surface.damage_buffer(0, 0, size.0 as i32, size.1 as i32);
        m.surface.commit();
        e.on_screen = true;
        m.frames += 1;
    }

    /// While the lock screen shows the private stage's frames, our surfaces
    /// (hidden under the lock) stop taking them and show a 1x1 black buffer
    /// instead, so the compositor releases the WE buffer they still held:
    /// WE renders into a few buffers only, and one pinned here for the
    /// whole lock would leave it fewer to cycle through.
    pub fn set_locked(&mut self, qh: &QueueHandle<Self>, locked: bool) {
        if locked && self.park.is_none() {
            self.park = memfd(4).map(|fd| {
                let pool: WlShmPool = self.shm.create_pool(fd.as_fd(), 4, qh, ());
                // (memfd starts zeroed: black, and opaque as XRGB)
                let b = pool.create_buffer(0, 1, 1, 4, wl_shm::Format::Xrgb8888, qh, (0, u64::MAX));
                pool.destroy();
                b
            });
        }
        for m in &mut self.list {
            if m.capture || m.locked == locked {
                continue;
            }
            m.locked = locked;
            if locked
                && m.configured
                && let Some(park) = &self.park
            {
                m.surface.attach(Some(park), 0, 0);
                m.viewport
                    .set_destination(m.logical.0 as i32, m.logical.1 as i32);
                m.surface.damage_buffer(0, 0, 1, 1);
                m.surface.commit();
            }
        }
    }

    /// Watches for `secs` seconds without input (None: stop watching).
    /// Idle inhibitors (a playing video) hold it off, as they should.
    pub fn set_idle_timeout(&mut self, qh: &QueueHandle<Self>, secs: Option<u32>) {
        if self.idle_watch.as_ref().map(|w| w.0) == secs {
            return;
        }
        if let Some((_, n)) = self.idle_watch.take() {
            n.destroy();
        }
        self.idle = false;
        let (Some(secs), Some(notifier), Some(seat)) = (secs, &self.idle_notifier, &self.seat)
        else {
            if secs.is_some() {
                eprintln!("screensaver: the compositor lacks ext-idle-notify-v1");
            }
            return;
        };
        let n = notifier.get_idle_notification(secs.saturating_mul(1000), seat, qh, ());
        self.idle_watch = Some((secs, n));
    }

    /// Raises the wallpaper over every window (screensaver), or puts it
    /// back under them. While raised it takes all input, so the click or key
    /// that ends the screensaver does not reach the window under it.
    pub fn set_screensaver(&mut self, qh: &QueueHandle<Self>, on: bool) {
        if self.screensaver == on {
            return;
        }
        self.screensaver = on;
        for m in &self.list {
            if m.layer.version() < 2 {
                continue;
            }
            if on {
                m.layer.set_layer(Layer::Overlay);
                m.layer
                    .set_keyboard_interactivity(KeyboardInteractivity::Exclusive);
                m.surface.set_input_region(None);
            } else {
                m.layer.set_layer(Layer::Bottom);
                m.layer
                    .set_keyboard_interactivity(KeyboardInteractivity::None);
                let empty = self.compositor.create_region(qh, ());
                m.surface.set_input_region(Some(&empty));
                empty.destroy();
            }
            m.surface.commit();
        }
    }

    /// The private stage destroyed its buffer `key`.
    pub fn forget(&mut self, key: u64) {
        for m in &mut self.list {
            m.external.retain_mut(|e| {
                if e.key != Some(key) {
                    return true;
                }
                if e.on_screen {
                    e.gone = true;
                    return true;
                }
                e.destroy();
                false
            });
        }
    }

    /// (mirror index, stage buffer key) the compositor released.
    pub fn take_released(&mut self) -> Vec<(usize, u64)> {
        std::mem::take(&mut self.released)
    }

    /// The index of `output`'s mirror.
    pub fn index_of(&self, output: &str) -> Option<usize> {
        self.list.iter().position(|m| m.output == output)
    }

    /// Whether the window of mirror `i` is being captured.
    pub fn capturing(&self, i: usize) -> bool {
        let m = &self.list[i];
        if m.capture {
            m.session.is_some()
        } else {
            !m.external.is_empty()
        }
    }

    /// Starts sessions for windows that appeared and the next capture of
    /// every idle mirror. Call after each dispatch.
    pub fn pump(&mut self, qh: &QueueHandle<Self>) {
        let (Some(source_mgr), Some(copy_mgr)) = (self.source_mgr.clone(), self.copy_mgr.clone())
        else {
            return;
        };
        for i in 0..self.list.len() {
            if !self.list[i].capture {
                continue;
            }
            if self.list[i].session.is_none() {
                let handle = self
                    .toplevels
                    .iter()
                    .find(|t| t.title == self.list[i].title)
                    .and_then(|t| t.handle.clone());
                if let Some(handle) = handle {
                    eprintln!(
                        "{}: capturing window {}",
                        self.list[i].output, self.list[i].title
                    );
                    let source = source_mgr.create_source(&handle, qh, ());
                    // no cursor painted into the frames: it is the user's own cursor
                    self.list[i].session =
                        Some(copy_mgr.create_session(&source, Options::empty(), qh, i));
                    source.destroy();
                }
            }
            let m = &mut self.list[i];
            let Some(session) = &m.session else { continue };
            if m.paused || m.frame.is_some() || !m.configured || m.size == (0, 0) {
                continue;
            }
            let Some(b) = m
                .buffers
                .iter_mut()
                .find(|b| !b.on_screen && !b.capturing && b.size == m.size)
            else {
                continue;
            };
            let frame = session.create_frame(qh, i);
            frame.attach_buffer(&b.buffer);
            frame.damage_buffer(0, 0, m.size.0 as i32, m.size.1 as i32);
            frame.capture();
            b.capturing = true;
            m.frame = Some((frame, b.id));
        }
    }

    /// (Re)allocates the capture buffers of mirror `i` for its constraints.
    fn allocate(&mut self, qh: &QueueHandle<Self>, i: usize) {
        let size = self.list[i].size;
        // drop idle buffers of another size; busy ones go when they come back
        self.list[i].buffers.retain(|b| {
            let keep = b.size == size || b.on_screen || b.capturing;
            if !keep {
                b.buffer.destroy();
            }
            keep
        });
        let have = self.list[i]
            .buffers
            .iter()
            .filter(|b| b.size == size)
            .count();
        if have >= BUFFERS || size == (0, 0) {
            return;
        }
        let kind = if self.alloc_dmabuf(qh, i, BUFFERS - have) {
            "dmabuf"
        } else if self.alloc_shm(qh, i, BUFFERS - have) {
            "shm"
        } else {
            "none"
        };
        eprintln!(
            "{}: capture buffers {}x{} ({kind})",
            self.list[i].output, size.0, size.1
        );
    }

    fn take_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    fn alloc_dmabuf(&mut self, qh: &QueueHandle<Self>, i: usize, n: usize) -> bool {
        let m = &self.list[i];
        let (Some(dmabuf), Some(dev)) = (self.dmabuf.clone(), m.dmabuf_device) else {
            return false;
        };
        if self.gbm_broken {
            return false;
        }
        let Some((fourcc, mods)) = [XR24, AR24]
            .iter()
            .find_map(|f| m.dmabuf_formats.iter().find(|(g, _)| g == f).cloned())
        else {
            return false;
        };
        if self.gbm.is_none() {
            match gbm::Device::open(dev) {
                Ok(d) => self.gbm = Some(d),
                Err(e) => {
                    eprintln!("dmabuf unavailable, using shm: {e}");
                    self.gbm_broken = true;
                    return false;
                }
            }
        }
        let (w, h) = self.list[i].size;
        for _ in 0..n {
            // seen on odd sizes while a window animated (3649x2051): shm for
            // this size only, dmabuf again on the next constraints
            let Some(bo) = self.gbm.as_ref().and_then(|g| g.alloc(w, h, fourcc, &mods)) else {
                eprintln!("gbm could not allocate {w}x{h} {fourcc:#x}");
                return false;
            };
            let params = dmabuf.create_params(qh, ());
            for (p, plane) in bo.planes.iter().enumerate() {
                params.add(
                    plane.fd.as_fd(),
                    p as u32,
                    plane.offset,
                    plane.stride,
                    (bo.modifier >> 32) as u32,
                    bo.modifier as u32,
                );
            }
            let id = self.take_id();
            let buffer = params.create_immed(
                w as i32,
                h as i32,
                fourcc,
                zwp_linux_buffer_params_v1::Flags::empty(),
                qh,
                (i, id),
            );
            params.destroy();
            self.list[i].buffers.push(Buf {
                id,
                buffer,
                size: (w, h),
                on_screen: false,
                capturing: false,
                _bo: Some(bo),
            });
        }
        true
    }

    fn alloc_shm(&mut self, qh: &QueueHandle<Self>, i: usize, n: usize) -> bool {
        let Some(format) = [wl_shm::Format::Xrgb8888, wl_shm::Format::Argb8888]
            .into_iter()
            .find(|f| self.list[i].shm_formats.contains(f))
        else {
            return false;
        };
        let (w, h) = self.list[i].size;
        let stride = w * 4;
        let len = (stride * h) as usize;
        for _ in 0..n {
            let Some(fd) = memfd(len) else { return false };
            let pool: WlShmPool = self.shm.create_pool(fd.as_fd(), len as i32, qh, ());
            let id = self.take_id();
            let buffer =
                pool.create_buffer(0, w as i32, h as i32, stride as i32, format, qh, (i, id));
            pool.destroy();
            self.list[i].buffers.push(Buf {
                id,
                buffer,
                size: (w, h),
                on_screen: false,
                capturing: false,
                _bo: None,
            });
        }
        true
    }

    fn present(&mut self, i: usize, id: u64) {
        let m = &mut self.list[i];
        let Some(b) = m.buffers.iter_mut().find(|b| b.id == id) else {
            return;
        };
        m.surface.attach(Some(&b.buffer), 0, 0);
        // buffer at the window's pixel size, shown at the output's logical size
        m.viewport
            .set_destination(m.logical.0 as i32, m.logical.1 as i32);
        m.surface
            .damage_buffer(0, 0, b.size.0 as i32, b.size.1 as i32);
        m.surface.commit();
        b.on_screen = true;
        m.frames += 1;
    }

    /// A buffer came back (released, or its capture ended): drop it when the
    /// session moved on to another size.
    fn reap(&mut self, i: usize, id: u64) {
        let m = &mut self.list[i];
        if let Some(pos) = m.buffers.iter().position(|b| b.id == id) {
            let b = &m.buffers[pos];
            if !b.on_screen && !b.capturing && b.size != m.size {
                b.buffer.destroy();
                m.buffers.remove(pos);
            }
        }
    }

    fn end_frame(&mut self, i: usize) -> Option<u64> {
        let (frame, id) = self.list[i].frame.take()?;
        frame.destroy();
        if let Some(b) = self.list[i].buffers.iter_mut().find(|b| b.id == id) {
            b.capturing = false;
        }
        Some(id)
    }
}

fn memfd(len: usize) -> Option<OwnedFd> {
    // SAFETY: plain syscalls on a fresh fd that we own.
    unsafe {
        let fd = libc::memfd_create(c"rustpaper".as_ptr(), libc::MFD_CLOEXEC);
        if fd < 0 {
            return None;
        }
        let fd = OwnedFd::from_raw_fd(fd);
        (libc::ftruncate(fd.as_raw_fd(), len as libc::off_t) == 0).then_some(fd)
    }
}

// ---- event handlers --------------------------------------------------------

impl Dispatch<WlRegistry, GlobalListContents> for Mirrors {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: <WlRegistry as Proxy>::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ExtIdleNotificationV1, ()> for Mirrors {
    fn event(
        s: &mut Self,
        _: &ExtIdleNotificationV1,
        e: ext_idle_notification_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match e {
            ext_idle_notification_v1::Event::Idled => s.idle = true,
            ext_idle_notification_v1::Event::Resumed => s.idle = false,
            _ => {}
        }
    }
}

delegate_noop!(Mirrors: ignore ExtIdleNotifierV1);
delegate_noop!(Mirrors: ignore WlSeat);
delegate_noop!(Mirrors: WlCompositor);
delegate_noop!(Mirrors: WlShmPool);
delegate_noop!(Mirrors: WlRegion);
delegate_noop!(Mirrors: ZwlrLayerShellV1);
delegate_noop!(Mirrors: WpViewporter);
delegate_noop!(Mirrors: WpViewport);
delegate_noop!(Mirrors: ExtForeignToplevelImageCaptureSourceManagerV1);
delegate_noop!(Mirrors: ExtImageCaptureSourceV1);
delegate_noop!(Mirrors: ExtImageCopyCaptureManagerV1);
delegate_noop!(Mirrors: ignore WlShm);
delegate_noop!(Mirrors: ignore WlSurface);
delegate_noop!(Mirrors: ignore ZwpLinuxDmabufV1);
delegate_noop!(Mirrors: ignore ZwpLinuxBufferParamsV1);

impl Dispatch<WlOutput, usize> for Mirrors {
    fn event(
        s: &mut Self,
        _: &WlOutput,
        e: wl_output::Event,
        idx: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match e {
            wl_output::Event::Name { name } => s.outputs[*idx].1 = name,
            wl_output::Event::Mode {
                flags: WEnum::Value(f),
                width,
                height,
                ..
            } if f.contains(wl_output::Mode::Current) => {
                s.outputs[*idx].2 = (width as u32, height as u32);
            }
            _ => {}
        }
    }
}

impl Dispatch<WlBuffer, (usize, u64)> for Mirrors {
    fn event(
        s: &mut Self,
        _: &WlBuffer,
        e: wl_buffer::Event,
        &(i, id): &(usize, u64),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_buffer::Event::Release = e {
            if i >= s.list.len() {
                return;
            }
            if let Some(b) = s.list[i].buffers.iter_mut().find(|b| b.id == id) {
                b.on_screen = false;
                s.reap(i, id);
                return;
            }
            let m = &mut s.list[i];
            if let Some(pos) = m.external.iter().position(|e| e.id == id) {
                let e = &mut m.external[pos];
                e.on_screen = false;
                if let Some(key) = e.key {
                    s.released.push((i, key));
                }
                let m = &mut s.list[i];
                if m.external[pos].gone {
                    m.external[pos].destroy();
                    m.external.remove(pos);
                }
            }
        }
    }
}

impl Dispatch<ZwpLinuxDmabufFeedbackV1, ()> for Mirrors {
    fn event(
        s: &mut Self,
        _: &ZwpLinuxDmabufFeedbackV1,
        e: zwp_linux_dmabuf_feedback_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use zwp_linux_dmabuf_feedback_v1::Event;
        match e {
            // 16-byte entries: u32 format, u32 padding, u64 modifier
            Event::FormatTable { fd, size } => {
                let len = size as usize;
                // SAFETY: read-only private mapping of the table fd we were sent.
                let ptr = unsafe {
                    libc::mmap(
                        std::ptr::null_mut(),
                        len,
                        libc::PROT_READ,
                        libc::MAP_PRIVATE,
                        fd.as_raw_fd(),
                        0,
                    )
                };
                if ptr == libc::MAP_FAILED {
                    return;
                }
                // SAFETY: the mapping is `len` bytes long.
                let bytes = unsafe { std::slice::from_raw_parts(ptr.cast::<u8>(), len) };
                s.feedback_table = bytes
                    .as_chunks::<16>()
                    .0
                    .iter()
                    .map(|c| {
                        let f = u32::from_ne_bytes([c[0], c[1], c[2], c[3]]);
                        let m = u64::from_ne_bytes([
                            c[8], c[9], c[10], c[11], c[12], c[13], c[14], c[15],
                        ]);
                        (f, m)
                    })
                    .collect();
                // SAFETY: unmapping what we mapped above.
                unsafe { libc::munmap(ptr, len) };
            }
            Event::MainDevice { device } => {
                s.host_dmabuf.main_device = device.try_into().map(u64::from_ne_bytes).unwrap_or(0);
            }
            Event::TrancheFormats { indices } => {
                for idx in indices.as_chunks::<2>().0 {
                    let i = u16::from_ne_bytes(*idx) as usize;
                    if let Some(&pair) = s.feedback_table.get(i)
                        && !s.host_dmabuf.formats.contains(&pair)
                    {
                        s.host_dmabuf.formats.push(pair);
                    }
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<ZwlrLayerSurfaceV1, usize> for Mirrors {
    fn event(
        s: &mut Self,
        l: &ZwlrLayerSurfaceV1,
        e: zwlr_layer_surface_v1::Event,
        &i: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match e {
            zwlr_layer_surface_v1::Event::Configure {
                serial,
                width,
                height,
            } => {
                l.ack_configure(serial);
                let m = &mut s.list[i];
                m.logical = (width, height);
                m.configured = true;
                m.surface.commit();
            }
            zwlr_layer_surface_v1::Event::Closed => {
                // output unplugged: nothing to show it on any more
                eprintln!("{}: layer surface closed", s.list[i].output);
                s.list[i].paused = true;
            }
            _ => {}
        }
    }
}

impl Dispatch<ExtForeignToplevelListV1, ()> for Mirrors {
    fn event(
        s: &mut Self,
        _: &ExtForeignToplevelListV1,
        e: ext_foreign_toplevel_list_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let ext_foreign_toplevel_list_v1::Event::Toplevel { toplevel } = e {
            s.toplevels.push(Toplevel {
                handle: Some(toplevel),
                title: String::new(),
            });
        }
    }
    event_created_child!(Mirrors, ExtForeignToplevelListV1, [
        ext_foreign_toplevel_list_v1::EVT_TOPLEVEL_OPCODE => (ExtForeignToplevelHandleV1, ()),
    ]);
}

impl Dispatch<ExtForeignToplevelHandleV1, ()> for Mirrors {
    fn event(
        s: &mut Self,
        h: &ExtForeignToplevelHandleV1,
        e: ext_foreign_toplevel_handle_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match e {
            ext_foreign_toplevel_handle_v1::Event::Title { title } => {
                if let Some(t) = s
                    .toplevels
                    .iter_mut()
                    .find(|t| t.handle.as_ref() == Some(h))
                {
                    t.title = title;
                }
            }
            ext_foreign_toplevel_handle_v1::Event::Closed => {
                h.destroy();
                s.toplevels.retain(|t| t.handle.as_ref() != Some(h));
            }
            _ => {}
        }
    }
}

impl Dispatch<ExtImageCopyCaptureSessionV1, usize> for Mirrors {
    fn event(
        s: &mut Self,
        session: &ExtImageCopyCaptureSessionV1,
        e: ext_image_copy_capture_session_v1::Event,
        &i: &usize,
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        use ext_image_copy_capture_session_v1::Event;
        let m = &mut s.list[i];
        match e {
            // constraint events come in any order; a batch ends with done
            Event::BufferSize { width, height } => m.size = (width, height),
            Event::ShmFormat {
                format: WEnum::Value(f),
            } => m.shm_formats.push(f),
            // a dev_t in native byte order
            Event::DmabufDevice { device } => {
                m.dmabuf_device = device.try_into().ok().map(u64::from_ne_bytes)
            }
            Event::DmabufFormat { format, modifiers } => {
                let mods = modifiers
                    .as_chunks::<8>()
                    .0
                    .iter()
                    .map(|c| u64::from_ne_bytes(*c))
                    .collect();
                m.dmabuf_formats.push((format, mods));
            }
            Event::Done => {
                s.allocate(qh, i);
                let m = &mut s.list[i];
                m.shm_formats.clear();
                m.dmabuf_formats.clear();
            }
            Event::Stopped => {
                // the window closed (WE crashed or was restarted): keep the
                // last frame on screen until a new window shows up
                eprintln!("{}: window {} went away", m.output, m.title);
                session.destroy();
                m.session = None;
                s.end_frame(i);
            }
            _ => {}
        }
    }
}

impl Dispatch<ExtImageCopyCaptureFrameV1, usize> for Mirrors {
    fn event(
        s: &mut Self,
        _: &ExtImageCopyCaptureFrameV1,
        e: ext_image_copy_capture_frame_v1::Event,
        &i: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use ext_image_copy_capture_frame_v1::Event;
        match e {
            Event::Ready => {
                if let Some(id) = s.end_frame(i) {
                    s.present(i, id);
                    s.reap(i, id);
                }
            }
            // buffer_constraints: the session re-sends sizes + done;
            // stopped: the session's own `stopped` follows
            Event::Failed { .. } => {
                if let Some(id) = s.end_frame(i) {
                    s.reap(i, id);
                }
            }
            _ => {}
        }
    }
}
