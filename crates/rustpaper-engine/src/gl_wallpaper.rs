// Live scene wallpaper on OpenGL (rustpaper-gl).
//
// Same shape as `wallpaper.rs` (one wlr-layer-shell surface per output on the
// Background layer, a calloop timer drives rendering so a covered surface never
// freezes it), but scenes are drawn by `rustpaper_gl::SceneRenderer`:
//
//   timer tick -> scene.render() into its offscreen target (once)
//              -> for each output: make its EGL surface current,
//                 cover-blit the target, swap (no vsync wait)
//
// HiDPI: with wp_viewporter, each output gets a buffer at its PHYSICAL mode size
// (e.g. 3840x2160 on a 1.5x-scaled 4K screen) shown at the logical size, so the
// wallpaper stays sharp under fractional scaling.

use std::ffi::c_void;
use std::path::Path;
use std::time::{Duration, Instant};

use calloop::EventLoop;
use calloop::timer::{TimeoutAction, Timer};
use calloop_wayland_source::WaylandSource;
use rustpaper_gl::context::{GlContext, Surface};
use rustpaper_gl::gpu::present_cover;
use rustpaper_gl::scene::{FrameInput, SceneRenderer};
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState, Region},
    delegate_compositor, delegate_layer, delegate_output, delegate_registry,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    shell::{
        WaylandSurface,
        wlr_layer::{Anchor, LayerShell, LayerShellHandler, LayerSurface, LayerSurfaceConfigure},
    },
};
use wayland_client::{
    Connection, Proxy, QueueHandle, delegate_noop,
    globals::registry_queue_init,
    protocol::{wl_output, wl_surface},
};
use wayland_egl::WlEglSurface;
use wayland_protocols::wp::viewporter::client::{
    wp_viewport::WpViewport, wp_viewporter::WpViewporter,
};

/// Runtime options of the live wallpaper (CLI flags of `rustpaper`).
pub struct Options {
    /// Frames per second (the scene animates in real time regardless).
    pub fps: u32,
    /// User property values (`general.properties` of project.json) chosen in
    /// the selector, overriding the wallpaper defaults.
    pub properties: serde_json::Map<String, serde_json::Value>,
    /// Capture system audio for audio-reactive wallpapers.
    pub audio: bool,
    /// Play the scene's own sound objects (music); `--mute` turns it off.
    pub sound: bool,
    /// Run the scene's SceneScripts (sandboxed); `--no-scripts` turns it off.
    pub scripts: bool,
}

/// How often (in frames) fullscreen state is re-read from the compositor.
const MONITOR_POLL_FRAMES: u32 = 30;

/// One output: its layer surface and the EGL window drawn into it.
struct Screen {
    layer: LayerSurface,
    output: wl_output::WlOutput,
    viewport: Option<WpViewport>,
    // `surface` renders into `window`; drop order matters (surface first).
    surface: Option<Surface>,
    window: Option<WlEglSurface>,
    buffer_size: (u32, u32),
    /// Output name ("DP-1"), to match compositor IPC data.
    name: Option<String>,
}

struct GlWallpaper {
    registry_state: RegistryState,
    output_state: OutputState,
    compositor: CompositorState,
    layer_shell: LayerShell,
    viewporter: Option<WpViewporter>,
    gl: GlContext,
    scene: SceneRenderer,
    screens: Vec<Screen>,
    vsync_off: bool,
    /// SceneScripts start with the first frame (they need the screen size).
    scripts_pending: bool,
    /// Animation clock: advances with real time except while every output is
    /// covered by a fullscreen window (then the wallpaper is fully paused).
    clock: f32,
    last_tick: Instant,
    frame: u32,
    hypr: Vec<crate::hyprland::Monitor>,
    pointer: [f32; 2],
    audio: Option<crate::audio_capture::AudioCapture>,
    sound: Option<crate::sound::SoundPlayer>,
}

pub fn run(dir: &Path, opts: Options) {
    let conn = Connection::connect_to_env().expect("failed to connect to Wayland");
    let (globals, event_queue) = registry_queue_init(&conn).expect("wayland registry");
    let qh = event_queue.handle();

    let compositor = CompositorState::bind(&globals, &qh).expect("wl_compositor missing");
    let layer_shell = LayerShell::bind(&globals, &qh).expect("wlr-layer-shell missing");
    let viewporter = globals.bind::<WpViewporter, _, _>(&qh, 1..=1, ()).ok();

    // SAFETY: the connection (and its wl_display) lives as long as `state`,
    // which owns the context.
    let gl = unsafe { GlContext::new_wayland(conn.backend().display_ptr() as *mut c_void) }
        .unwrap_or_else(|e| {
            eprintln!("error: OpenGL on Wayland: {e}");
            std::process::exit(1);
        });
    println!("GL: {}", gl.describe());

    let base = rustpaper_core::paths::assets_dir().unwrap_or_else(|| {
        eprintln!("error: WE base assets not found (set WE_ASSETS_DIR)");
        std::process::exit(1);
    });
    let scene = SceneRenderer::load_with_properties(&gl.gl, dir, &base, &opts.properties)
        .unwrap_or_else(|e| {
            eprintln!("error: {}: {e}", dir.display());
            std::process::exit(1);
        });
    println!("scene {}x{}", scene.width, scene.height);
    for w in &scene.warnings {
        eprintln!("warning: {w}");
    }

    let mut state = GlWallpaper {
        registry_state: RegistryState::new(&globals),
        output_state: OutputState::new(&globals, &qh),
        compositor,
        layer_shell,
        viewporter,
        gl,
        scene,
        screens: Vec::new(),
        vsync_off: false,
        scripts_pending: opts.scripts,
        clock: 0.0,
        last_tick: Instant::now(),
        frame: 0,
        hypr: Vec::new(),
        pointer: [0.5, 0.5],
        audio: if opts.audio {
            crate::audio_capture::AudioCapture::start()
        } else {
            None
        },
        sound: None,
    };
    if opts.sound {
        state.sound = crate::sound::SoundPlayer::start(&state.scene.sounds);
    }
    let interval = Duration::from_secs_f64(1.0 / opts.fps.clamp(1, 240) as f64);

    let mut event_loop: EventLoop<GlWallpaper> = EventLoop::try_new().expect("event loop");
    let handle = event_loop.handle();
    WaylandSource::new(conn, event_queue)
        .insert(handle.clone())
        .expect("insert WaylandSource");
    handle
        .insert_source(Timer::from_duration(interval), move |_, _, state| {
            state.render_all();
            TimeoutAction::ToDuration(interval)
        })
        .expect("insert timer");
    event_loop
        .run(None, &mut state, |_| {})
        .expect("run event loop");
}

impl GlWallpaper {
    fn render_all(&mut self) {
        // compositor state: who is covered by a fullscreen window
        if self.frame.is_multiple_of(MONITOR_POLL_FRAMES) {
            self.hypr = crate::hyprland::monitors().unwrap_or_default();
        }
        self.frame = self.frame.wrapping_add(1);
        let covered = |name: &Option<String>| {
            name.as_ref()
                .and_then(|n| self.hypr.iter().find(|m| &m.name == n))
                .is_some_and(|m| m.covered)
        };
        let ready: Vec<(Surface, (u32, u32), Option<String>)> = self
            .screens
            .iter()
            .filter(|s| !covered(&s.name))
            .filter_map(|s| s.surface.map(|surf| (surf, s.buffer_size, s.name.clone())))
            .collect();

        let now = Instant::now();
        let dt = now.duration_since(self.last_tick).as_secs_f32().min(0.1);
        self.last_tick = now;
        if let Some(s) = &self.sound {
            s.set_paused(ready.is_empty());
        }
        let Some(&(first, _, _)) = ready.first() else {
            return; // nothing visible: paused (the clock doesn't advance)
        };
        self.clock += dt;
        if let Err(e) = self.gl.make_current(Some(first)) {
            eprintln!("warning: {e}");
            return;
        }
        if !self.vsync_off {
            self.gl.disable_vsync();
            self.vsync_off = true;
        }

        // The scene drawn at screen resolution, like WE (the largest visible
        // output; a no-op while it doesn't change).
        let (ow, oh) = ready
            .iter()
            .map(|r| r.1)
            .max_by_key(|&(w, h)| w as u64 * h as u64)
            .unwrap_or((1920, 1080));
        self.scene.set_output_size(&self.gl.gl, ow, oh);
        if std::mem::take(&mut self.scripts_pending) {
            let n = self.scene.enable_scripts([ow as f32, oh as f32]);
            if n > 0 {
                println!("{n} SceneScript(s), sandboxed");
            }
            for w in &self.scene.warnings {
                eprintln!("warning: {w}");
            }
        }

        self.update_pointer();
        let frame = FrameInput {
            time: self.clock,
            pointer: self.pointer,
            audio: self.audio.as_ref().map(|a| a.frame()).unwrap_or_default(),
            daytime: rustpaper_gl::text::daytime(),
        };
        self.scene.render(&self.gl.gl, frame);
        for (id, v) in self.scene.sound_volumes.drain(..) {
            if let Some(s) = &self.sound {
                s.set_volume(id, v);
            }
        }
        for (id, playing) in self.scene.sound_playing.drain(..) {
            if let Some(s) = &self.sound {
                s.set_playing(id, playing);
            }
        }

        for (surface, (w, h), _) in ready {
            if self.gl.make_current(Some(surface)).is_err() {
                continue;
            }
            present_cover(&self.gl.gl, self.scene.output(), w, h, self.scene.pixelated);
            if let Err(e) = self.gl.swap(surface) {
                eprintln!("warning: {e}");
            }
        }
    }

    /// Cursor -> the output under it -> scene uv (through that output's cover
    /// crop), (0,0) = top-left. Keeps the last value when the cursor is on an
    /// output we don't draw to.
    fn update_pointer(&mut self) {
        let Some((gx, gy)) = crate::hyprland::cursor() else {
            return;
        };
        let Some(m) = self.hypr.iter().find(|m| {
            let (x, y, w, h) = m.rect;
            gx >= x && gx < x + w && gy >= y && gy < y + h
        }) else {
            return;
        };
        let Some(screen) = self
            .screens
            .iter()
            .find(|s| s.name.as_deref() == Some(&m.name))
        else {
            return;
        };
        let (x, y, w, h) = m.rect;
        let (u, v) = (((gx - x) / w) as f32, ((gy - y) / h) as f32);
        let (sw, sh) = (self.scene.width, self.scene.height);
        let (bw, bh) = screen.buffer_size;
        if bw == 0 || bh == 0 {
            return;
        }
        let (x0, y0, x1, y1) = rustpaper_gl::gpu::cover_rect(sw, sh, bw, bh);
        self.pointer = [
            (x0 as f32 + u * (x1 - x0) as f32) / sw as f32,
            (y0 as f32 + v * (y1 - y0) as f32) / sh as f32,
        ];
    }

    /// Buffer size for an output: its physical mode when we can show it through
    /// a viewport, otherwise logical size x integer scale.
    fn buffer_size(&self, screen: &Screen, logical: (u32, u32)) -> (u32, u32, i32) {
        let info = self.output_state.info(&screen.output);
        if screen.viewport.is_some()
            && let Some(mode) = info
                .as_ref()
                .and_then(|i| i.modes.iter().find(|m| m.current))
        {
            let (w, h) = (mode.dimensions.0 as u32, mode.dimensions.1 as u32);
            // a rotated output reports its mode in the panel's orientation
            let rotated = (logical.0 > logical.1) != (w > h);
            return if rotated { (h, w, 1) } else { (w, h, 1) };
        }
        let scale = info.map(|i| i.scale_factor.max(1)).unwrap_or(1);
        (logical.0 * scale as u32, logical.1 * scale as u32, scale)
    }
}

impl OutputHandler for GlWallpaper {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(&mut self, _: &Connection, qh: &QueueHandle<Self>, output: wl_output::WlOutput) {
        let surface = self.compositor.create_surface(qh);
        let viewport = self
            .viewporter
            .as_ref()
            .map(|v| v.get_viewport(&surface, qh, ()));
        // Input passes through (empty input region): clicks on the desktop
        // still reach whatever is below, e.g. the Omarchy background's menu.
        if let Ok(region) = Region::new(&self.compositor) {
            surface.set_input_region(Some(region.wl_region()));
        }
        let layer = self.layer_shell.create_layer_surface(
            qh,
            surface,
            crate::wallpaper::LAYER,
            Some("rustpaper"),
            Some(&output),
        );
        layer.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
        layer.set_exclusive_zone(-1);
        layer.set_size(0, 0);
        layer.commit();
        self.screens.push(Screen {
            layer,
            output,
            viewport,
            surface: None,
            window: None,
            buffer_size: (0, 0),
            name: None,
        });
    }

    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}

    fn output_destroyed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        output: wl_output::WlOutput,
    ) {
        if let Some(i) = self.screens.iter().position(|s| s.output == output) {
            let s = self.screens.remove(i);
            if let Some(surf) = s.surface {
                let _ = self.gl.make_current(None);
                self.gl.destroy_surface(surf);
            }
        }
    }
}

impl LayerShellHandler for GlWallpaper {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &LayerSurface) {}

    fn configure(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _: u32,
    ) {
        let Some(idx) = self
            .screens
            .iter()
            .position(|s| s.layer.wl_surface() == layer.wl_surface())
        else {
            return;
        };
        let (lw, lh) = match configure.new_size {
            (0, _) | (_, 0) => (1920, 1080),
            s => s,
        };
        let (bw, bh, int_scale) = self.buffer_size(&self.screens[idx], (lw, lh));

        let screen = &mut self.screens[idx];
        match &screen.viewport {
            Some(vp) => vp.set_destination(lw as i32, lh as i32),
            None => screen.layer.wl_surface().set_buffer_scale(int_scale),
        }
        match &screen.window {
            Some(win) => win.resize(bw as i32, bh as i32, 0, 0),
            None => {
                let win = WlEglSurface::new(screen.layer.wl_surface().id(), bw as i32, bh as i32)
                    .expect("wl_egl_window");
                // SAFETY: `win` is stored next to the surface and outlives it.
                match unsafe { self.gl.create_window_surface(win.ptr() as *mut c_void) } {
                    Ok(s) => screen.surface = Some(s),
                    Err(e) => eprintln!("warning: {e}"),
                }
                screen.window = Some(win);
            }
        }
        screen.buffer_size = (bw, bh);
        screen.name = self.output_state.info(&screen.output).and_then(|i| i.name);
        println!("output {idx}: {lw}x{lh} logical, buffer {bw}x{bh}");
    }
}

impl CompositorHandler for GlWallpaper {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: i32,
    ) {
    }
    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: wl_output::Transform,
    ) {
    }
    fn surface_enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
    fn surface_leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: u32) {}
}

impl ProvidesRegistryState for GlWallpaper {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState];
}

delegate_compositor!(GlWallpaper);
delegate_output!(GlWallpaper);
delegate_layer!(GlWallpaper);
delegate_registry!(GlWallpaper);
delegate_noop!(GlWallpaper: ignore WpViewporter);
delegate_noop!(GlWallpaper: ignore WpViewport);
