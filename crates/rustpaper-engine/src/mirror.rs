//! Mirrors WE's wallpaper windows onto the desktop, one per output.
//!
//! For each output: a `zwlr-layer-shell` surface (layer Bottom, namespace
//! `rustpaper`, no input) shows frames captured from WE's window with
//! `ext-image-copy-capture-v1` (window source from
//! `ext-foreign-toplevel-image-capture-source-v1`, found by title through
//! `ext-foreign-toplevel-list-v1`). The compositor copies the window into our
//! buffers and answers only when it has new content, so WE paces us.
//!
//! Buffers are GBM dmabufs on the compositor's GPU, attached to our surface
//! as-is: no GL context here, and no CPU copy. With wl_shm the compositor
//! read every frame back to RAM: one 1080p60 mirror took Hyprland from 2.8%
//! to 23% CPU (dmabuf: 3.1%; 2026-11-22, RTX 4060 Ti). shm stays as the
//! fallback when GBM cannot allocate.

use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};

use wayland_client::globals::{GlobalListContents, registry_queue_init};
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
    zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1,
};
use wayland_protocols::wp::viewporter::client::{
    wp_viewport::WpViewport, wp_viewporter::WpViewporter,
};
use wayland_protocols_wlr::layer_shell::v1::client::{
    zwlr_layer_shell_v1::{Layer, ZwlrLayerShellV1},
    zwlr_layer_surface_v1::{self, Anchor, ZwlrLayerSurfaceV1},
};

use crate::gbm;

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
    /// Frames shown since the counter was last reset (stats).
    pub frames: u32,
    surface: WlSurface,
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
}

pub struct Mirrors {
    pub list: Vec<Mirror>,
    compositor: WlCompositor,
    shm: WlShm,
    layer_shell: ZwlrLayerShellV1,
    viewporter: WpViewporter,
    source_mgr: ExtForeignToplevelImageCaptureSourceManagerV1,
    copy_mgr: ExtImageCopyCaptureManagerV1,
    dmabuf: Option<ZwpLinuxDmabufV1>,
    gbm: Option<gbm::Device>,
    gbm_broken: bool,
    outputs: Vec<(WlOutput, String)>,
    toplevels: Vec<Toplevel>,
    next_id: u64,
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
            source_mgr: globals
                .bind(&qh, 1..=1, ())
                .map_err(|_| lacks("ext_foreign_toplevel_image_capture_source_manager_v1"))?,
            copy_mgr: globals
                .bind(&qh, 1..=1, ())
                .map_err(|_| lacks("ext_image_copy_capture_manager_v1"))?,
            dmabuf: globals.bind(&qh, 4..=5, ()).ok(),
            gbm: None,
            gbm_broken: false,
            outputs: Vec::new(),
            toplevels: Vec::new(),
            next_id: 0,
        };
        let _list: ExtForeignToplevelListV1 = globals
            .bind(&qh, 1..=1, ())
            .map_err(|_| lacks("ext_foreign_toplevel_list_v1"))?;
        for g in globals.contents().clone_list() {
            if g.interface == WlOutput::interface().name {
                let idx = m.outputs.len();
                let o: WlOutput = globals.registry().bind(g.name, g.version.min(4), &qh, idx);
                m.outputs.push((o, String::new()));
            }
        }
        // output names, then the toplevel list
        queue.roundtrip(&mut m).map_err(|e| e.to_string())?;
        queue.roundtrip(&mut m).map_err(|e| e.to_string())?;
        Ok((m, queue, conn))
    }

    /// Starts the wallpaper surface of `output`, showing window `title`.
    pub fn add(&mut self, qh: &QueueHandle<Self>, output: &str, title: &str) -> Result<(), String> {
        let wl_output = self
            .outputs
            .iter()
            .find(|(_, n)| n == output)
            .map(|(o, _)| o.clone())
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
            frames: 0,
            surface,
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
        });
        Ok(())
    }

    /// Whether the window of mirror `i` is being captured.
    pub fn capturing(&self, i: usize) -> bool {
        self.list[i].session.is_some()
    }

    /// Starts sessions for windows that appeared and the next capture of
    /// every idle mirror. Call after each dispatch.
    pub fn pump(&mut self, qh: &QueueHandle<Self>) {
        for i in 0..self.list.len() {
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
                    let source = self.source_mgr.create_source(&handle, qh, ());
                    // no cursor painted into the frames: it is the user's own cursor
                    self.list[i].session =
                        Some(
                            self.copy_mgr
                                .create_session(&source, Options::empty(), qh, i),
                        );
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
        if let wl_output::Event::Name { name } = e {
            s.outputs[*idx].1 = name;
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
            if let Some(b) = s.list[i].buffers.iter_mut().find(|b| b.id == id) {
                b.on_screen = false;
            }
            s.reap(i, id);
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
