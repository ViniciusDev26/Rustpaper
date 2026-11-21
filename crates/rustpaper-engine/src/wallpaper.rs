// `wallpaper` module: creates a layer surface on the BACKGROUND layer for EACH
// connected monitor, and draws on all of them using a shared Renderer.

use std::ptr::NonNull;
use std::time::Duration;

use calloop::EventLoop;
use calloop::timer::{TimeoutAction, Timer};
use calloop_wayland_source::WaylandSource;
use raw_window_handle::{
    RawDisplayHandle, RawWindowHandle, WaylandDisplayHandle, WaylandWindowHandle,
};
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState, Region},
    delegate_compositor, delegate_layer, delegate_output, delegate_registry,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    shell::{
        WaylandSurface,
        wlr_layer::{
            Anchor, Layer, LayerShell, LayerShellHandler, LayerSurface, LayerSurfaceConfigure,
        },
    },
};
use wayland_client::{
    Connection, Proxy, QueueHandle,
    globals::registry_queue_init,
    protocol::{wl_output, wl_surface},
};

use std::path::Path;

use crate::gpu::{Renderer, Source};
use crate::particles::ParticleInit;
use rustpaper_core::particle::ParticleSystem;
use rustpaper_core::pkg::Pkg;
use rustpaper_core::project::{Project, WallpaperKind};
use rustpaper_core::{scene, tex};

// We render systems with the "sprite" renderer and moderate size. We skip:
// - "spritetrail" (trails) and the like: they would come out wrong in our render.
// - GIANT sprites (fog/beams/rays of 800-2200px): they are atmospheric
//   additive effects that, without WE's HDR/tonemap/bloom pipeline, just ADD light and
//   wash the screen out to white, covering the background. Better to skip than blow everything out.
// This way scenes keep the background visible + small particles (dust, embers,
// bubbles). Unimplemented operators (turbulence...) are simply ignored.
const MAX_PARTICLE_SIZE: f32 = 400.0;
fn is_supported(sys: &ParticleSystem) -> bool {
    sys.renderer == "sprite" && sys.size.1 <= MAX_PARTICLE_SIZE
}

/// Layer the wallpaper lives on. `Bottom` (above the Background layer, below
/// every window) because desktop shells such as Omarchy's draw their own
/// background on `Background` and would cover ours. Our surface takes no input,
/// so desktop clicks still reach the shell background underneath.
pub(crate) const LAYER: Layer = Layer::Bottom;

// A screen: its layer surface, the wl_surface, the wgpu surface and the config (size).
struct Monitor {
    // The LayerSurface keeps the wl_surface alive (its pointer feeds the
    // wgpu surface), so we don't need to store the wl_surface separately.
    layer: LayerSurface,
    surface: wgpu::Surface<'static>,
    config: Option<wgpu::SurfaceConfiguration>, // set on the first configure
}

struct Wallpaper {
    registry_state: RegistryState,
    output_state: OutputState,
    compositor: CompositorState,
    layer_shell: LayerShell,
    conn: Connection,
    instance: wgpu::Instance,
    renderer: Option<Renderer>, // shared; created on the 1st monitor
    monitors: Vec<Monitor>,
    // Texture source (video or scene image); moved into the Renderer on the 1st monitor.
    source: Option<Source>,
}

// WE base assets, mounted in the container (particle sprites etc.).
const ASSETS_DIR: &str = "/home/vscode/we-assets";

// Loads and decodes a texture by name ("particle/halo"), looking first
// in the scene's pkg and, if not found, in the WE base assets. Returns the cropped
// CONTENT (without the power-of-two padding) + its dimensions.
fn load_texture(pkg: &Pkg, name: &str) -> Option<(Vec<u8>, u32, u32)> {
    if name.is_empty() {
        return None;
    }
    let rel = format!("materials/{name}.tex");
    let bytes: Vec<u8> = match pkg.read(&rel) {
        Some(b) => b.to_vec(),
        None => std::fs::read(Path::new(ASSETS_DIR).join(&rel)).ok()?,
    };
    let t = tex::parse(&bytes).ok()?;

    // Crop to the real content (the buffer may have padding on the right/bottom).
    if t.width == t.real_width && t.height == t.real_height {
        return Some((t.rgba, t.width, t.height));
    }
    let (bw, rw, rh) = (
        t.width as usize,
        t.real_width as usize,
        t.real_height as usize,
    );
    let mut out = Vec::with_capacity(rw * rh * 4);
    for y in 0..rh {
        let row = &t.rgba[y * bw * 4..y * bw * 4 + rw * 4];
        out.extend_from_slice(row);
    }
    Some((out, t.real_width, t.real_height))
}

// Resolves a scene's source: background (image) + particle systems + sprite.
// (kept for the old path/particles; the default is now the compositor.)
#[allow(dead_code)]
fn scene_source(dir: &Path) -> Source {
    let pkg = Pkg::open(&dir.join("scene.pkg")).expect("failed to open scene.pkg");

    // Background.
    let tex_path =
        scene::background_texture(&pkg).expect("couldn't find the scene's background texture");
    let bytes = pkg.read(&tex_path).expect("texture is not in the pkg");
    let t = tex::parse(bytes).unwrap_or_else(|e| {
        eprintln!("failed to decode {tex_path}: {e}");
        std::process::exit(1);
    });

    // Particles: we only render systems with the "sprite" renderer (spritetrail
    // etc. would come out wrong). EACH system loads ITS OWN sprite; if the
    // sprite fails to load/decode, the system is skipped.
    let scene_particles = scene::particle_systems(&pkg);
    let total = scene_particles.len();
    let mut particles: Vec<ParticleInit> = Vec::new();
    for sp in scene_particles {
        if !is_supported(&sp.system) {
            continue;
        }
        let Some((sprite_rgba, sprite_w, sprite_h)) = load_texture(&pkg, &sp.texture) else {
            continue;
        };
        particles.push(ParticleInit {
            system: sp.system,
            additive: sp.additive,
            sprite_rgba,
            sprite_w,
            sprite_h,
            origin: sp.origin,
            scale: sp.scale,
            sheet: None,
        });
    }
    if total > 0 {
        println!(
            "  particles: {} rendered of {} ({} skipped)",
            particles.len(),
            total,
            total - particles.len()
        );
    }

    Source::Scene {
        rgba: t.rgba,
        width: t.width,
        height: t.height,
        real_width: t.real_width,
        real_height: t.real_height,
        particles,
    }
}

pub fn run(dir: &Path) {
    // Reads project.json and decides what to play.
    let project = Project::load(dir).expect("failed to read project.json from the folder");
    println!("Wallpaper: {:?} (kind {:?})", project.title, project.kind);

    let source = match project.kind {
        WallpaperKind::Video => {
            Source::Video(project.file_path(dir).to_string_lossy().into_owned())
        }
        // Scene: uses the multi-layer COMPOSITOR (all layers + per-layer effects),
        // instead of the old single-texture path (scene_source).
        WallpaperKind::Scene => Source::SceneComposite(dir.to_path_buf()),
        other => {
            eprintln!("kind {other:?} not supported yet.");
            std::process::exit(1);
        }
    };

    let conn = Connection::connect_to_env().expect("failed to connect to Wayland");
    let (globals, event_queue) = registry_queue_init(&conn).unwrap();
    let qh = event_queue.handle();

    let compositor = CompositorState::bind(&globals, &qh).expect("wl_compositor missing");
    let layer_shell = LayerShell::bind(&globals, &qh).expect("wlr-layer-shell missing");

    let mut state = Wallpaper {
        registry_state: RegistryState::new(&globals),
        output_state: OutputState::new(&globals, &qh),
        compositor,
        layer_shell,
        conn: conn.clone(),
        instance: wgpu::Instance::default(),
        renderer: None,
        monitors: Vec::new(),
        source: Some(source),
    };

    // Event loop via calloop. Rendering is driven by a TIMER (~30fps), not by Wayland
    // frame callbacks - this way the wallpaper never "freezes" when the
    // compositor stops sending callbacks (e.g. surface covered by a window).
    let mut event_loop: EventLoop<Wallpaper> = EventLoop::try_new().expect("create event loop");
    let handle = event_loop.handle();

    // Wayland source: delivers the events (outputs, configure...) to the handlers.
    WaylandSource::new(conn, event_queue)
        .insert(handle.clone())
        .expect("insert WaylandSource");

    // ~30fps timer: redraws all the screens. While the renderer/config don't exist yet
    // (before the 1st configure), render_monitor simply skips.
    let frame_interval = Duration::from_millis(33);
    handle
        .insert_source(Timer::from_duration(frame_interval), move |_, _, state| {
            state.render_all();
            TimeoutAction::ToDuration(frame_interval)
        })
        .expect("insert timer");

    event_loop
        .run(None, &mut state, |_| {})
        .expect("run event loop");
}

impl Wallpaper {
    // Draws the monitor at index `idx` (only if it already has a renderer and config).
    fn render_monitor(&self, idx: usize) {
        if let (Some(renderer), Some(config)) =
            (self.renderer.as_ref(), self.monitors[idx].config.as_ref())
        {
            renderer.render(&self.monitors[idx].surface, config);
        }
    }

    // Redraws ALL the screens (called on every timer tick).
    fn render_all(&mut self) {
        // Advances the particle simulation once per frame (before drawing).
        if let Some(r) = self.renderer.as_mut() {
            r.tick();
        }
        for idx in 0..self.monitors.len() {
            self.render_monitor(idx);
        }
    }
}

impl OutputHandler for Wallpaper {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    // Called for each monitor (at startup and on hotplug).
    fn new_output(&mut self, _: &Connection, qh: &QueueHandle<Self>, output: wl_output::WlOutput) {
        // Creates the layer surface anchored to THIS specific output.
        let surface = self.compositor.create_surface(qh);
        if let Ok(region) = Region::new(&self.compositor) {
            surface.set_input_region(Some(region.wl_region()));
        }
        let layer = self.layer_shell.create_layer_surface(
            qh,
            surface,
            LAYER,
            Some("rustpaper"),
            Some(&output),
        );
        layer.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
        layer.set_exclusive_zone(-1);
        layer.set_size(0, 0);
        layer.commit();

        // Creates the wgpu surface from the raw Wayland pointers. The wl_surface
        // pointer comes from the layer (which keeps it alive as long as the Monitor exists).
        let raw_display = RawDisplayHandle::Wayland(WaylandDisplayHandle::new(
            NonNull::new(self.conn.backend().display_ptr() as *mut _).unwrap(),
        ));
        let raw_window = RawWindowHandle::Wayland(WaylandWindowHandle::new(
            NonNull::new(layer.wl_surface().id().as_ptr() as *mut _).unwrap(),
        ));
        let wgpu_surface = unsafe {
            self.instance
                .create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
                    raw_display_handle: Some(raw_display),
                    raw_window_handle: raw_window,
                })
                .unwrap()
        };

        // The Renderer (device/pipeline) is created once, from the 1st surface.
        if self.renderer.is_none() {
            let source = self.source.take().expect("source already consumed");
            self.renderer = Some(Renderer::new(&self.instance, &wgpu_surface, source));
        }

        self.monitors.push(Monitor {
            layer,
            surface: wgpu_surface,
            config: None,
        });
    }

    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
}

impl LayerShellHandler for Wallpaper {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &LayerSurface) {}

    fn configure(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _: u32,
    ) {
        // Finds out WHICH monitor this configure is for by comparing the wl_surface.
        let Some(idx) = self
            .monitors
            .iter()
            .position(|m| m.layer.wl_surface() == layer.wl_surface())
        else {
            return;
        };

        let (w, h) = configure.new_size;
        let (w, h) = if w == 0 || h == 0 {
            (1920, 1080)
        } else {
            (w, h)
        };

        // Configures the wgpu surface for this screen's size.
        let config = self
            .renderer
            .as_ref()
            .map(|r| r.configure(&self.monitors[idx].surface, w, h));
        if let Some(config) = config {
            self.monitors[idx].config = Some(config);
        }
        // The timer takes care of drawing; we don't need to trigger a render here.
    }
}

impl CompositorHandler for Wallpaper {
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

    // We don't use frame callbacks to drive rendering (the timer does that), so
    // this handler stays empty.
    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: u32) {}
}

impl ProvidesRegistryState for Wallpaper {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState];
}

delegate_compositor!(Wallpaper);
delegate_output!(Wallpaper);
delegate_layer!(Wallpaper);
delegate_registry!(Wallpaper);
