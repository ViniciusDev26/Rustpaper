// Módulo `wallpaper`: cria uma layer surface na camada de FUNDO para CADA
// monitor conectado, e desenha em todas usando um Renderer compartilhado.

use std::ptr::NonNull;

use raw_window_handle::{
    RawDisplayHandle, RawWindowHandle, WaylandDisplayHandle, WaylandWindowHandle,
};
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    delegate_compositor, delegate_layer, delegate_output, delegate_registry,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    shell::{
        wlr_layer::{
            Anchor, Layer, LayerShell, LayerShellHandler, LayerSurface, LayerSurfaceConfigure,
        },
        WaylandSurface,
    },
};
use wayland_client::{
    globals::registry_queue_init,
    protocol::{wl_output, wl_surface},
    Connection, Proxy, QueueHandle,
};

use std::path::Path;

use crate::gpu::Renderer;
use crate::project::{Project, WallpaperKind};

// Uma tela: sua layer surface, a wl_surface, a surface do wgpu e a config (tamanho).
struct Monitor {
    layer: LayerSurface,
    wl_surface: wl_surface::WlSurface,
    surface: wgpu::Surface<'static>,
    config: Option<wgpu::SurfaceConfiguration>, // definida no primeiro configure
    configured: bool,
}

struct Wallpaper {
    registry_state: RegistryState,
    output_state: OutputState,
    compositor: CompositorState,
    layer_shell: LayerShell,
    conn: Connection,
    instance: wgpu::Instance,
    renderer: Option<Renderer>, // compartilhado; criado no 1º monitor
    monitors: Vec<Monitor>,
    // Índice do monitor que "dá o ritmo": só os frame callbacks dele disparam o
    // redesenho de TODAS as telas. Evita que streams de callback concorrentes se
    // atrapalhem (o que congelava um monitor).
    driver: Option<usize>,
    // Caminho do vídeo a tocar (resolvido do project.json).
    video_path: String,
}

pub fn run(dir: &Path) {
    // Lê o project.json e decide o que tocar.
    let project = Project::load(dir).expect("falha ao ler project.json da pasta");
    println!("Wallpaper: {:?} (tipo {:?})", project.title, project.kind);

    let video_path = match project.kind {
        WallpaperKind::Video => project.file_path(dir).to_string_lossy().into_owned(),
        other => {
            eprintln!("tipo {other:?} ainda não suportado — por enquanto só 'video'.");
            std::process::exit(1);
        }
    };

    let conn = Connection::connect_to_env().expect("falha ao conectar no Wayland");
    let (globals, mut event_queue) = registry_queue_init(&conn).unwrap();
    let qh = event_queue.handle();

    let compositor = CompositorState::bind(&globals, &qh).expect("wl_compositor ausente");
    let layer_shell = LayerShell::bind(&globals, &qh).expect("wlr-layer-shell ausente");

    let mut state = Wallpaper {
        registry_state: RegistryState::new(&globals),
        output_state: OutputState::new(&globals, &qh),
        compositor,
        layer_shell,
        conn,
        instance: wgpu::Instance::default(),
        renderer: None,
        monitors: Vec::new(),
        driver: None,
        video_path,
    };

    // O primeiro dispatch entrega os outputs já existentes -> new_output cria uma
    // layer surface por monitor. Depois cada tela roda seu próprio loop de frames.
    loop {
        event_queue.blocking_dispatch(&mut state).unwrap();
    }
}

impl Wallpaper {
    // Desenha o monitor de índice `idx` (só se já tem renderer e config).
    fn render_monitor(&self, idx: usize) {
        if let (Some(renderer), Some(config)) =
            (self.renderer.as_ref(), self.monitors[idx].config.as_ref())
        {
            renderer.render(&self.monitors[idx].surface, config);
        }
    }

    // Agenda o próximo frame callback do monitor `idx` e desenha (forma o loop).
    fn draw(&self, idx: usize, qh: &QueueHandle<Self>) {
        let wl = self.monitors[idx].wl_surface.clone();
        wl.frame(qh, wl.clone());
        self.render_monitor(idx);
    }

    // Redesenha TODAS as telas (chamado a cada tick do monitor-relógio).
    fn render_all(&self) {
        for idx in 0..self.monitors.len() {
            self.render_monitor(idx);
        }
    }
}

impl OutputHandler for Wallpaper {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    // Chamado para cada monitor (na inicialização e em hotplug).
    fn new_output(&mut self, _: &Connection, qh: &QueueHandle<Self>, output: wl_output::WlOutput) {
        // Cria a layer surface ancorada a ESTE output específico.
        let surface = self.compositor.create_surface(qh);
        let layer = self.layer_shell.create_layer_surface(
            qh,
            surface,
            Layer::Background,
            Some("wallpaper-engine-rs"),
            Some(&output),
        );
        layer.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
        layer.set_exclusive_zone(-1);
        layer.set_size(0, 0);
        layer.commit();

        let wl_surface = layer.wl_surface().clone();

        // Cria a surface do wgpu a partir dos ponteiros crus do Wayland.
        let raw_display = RawDisplayHandle::Wayland(WaylandDisplayHandle::new(
            NonNull::new(self.conn.backend().display_ptr() as *mut _).unwrap(),
        ));
        let raw_window = RawWindowHandle::Wayland(WaylandWindowHandle::new(
            NonNull::new(wl_surface.id().as_ptr() as *mut _).unwrap(),
        ));
        let wgpu_surface = unsafe {
            self.instance
                .create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
                    raw_display_handle: Some(raw_display),
                    raw_window_handle: raw_window,
                })
                .unwrap()
        };

        // O Renderer (device/pipeline) é criado uma vez, a partir da 1ª surface.
        if self.renderer.is_none() {
            self.renderer = Some(Renderer::new(&self.instance, &wgpu_surface, &self.video_path));
        }

        self.monitors.push(Monitor {
            layer,
            wl_surface,
            surface: wgpu_surface,
            config: None,
            configured: false,
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
        qh: &QueueHandle<Self>,
        layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _: u32,
    ) {
        // Descobre QUAL monitor é este configure comparando a wl_surface.
        let Some(idx) = self
            .monitors
            .iter()
            .position(|m| m.layer.wl_surface() == layer.wl_surface())
        else {
            return;
        };

        let (w, h) = configure.new_size;
        let (w, h) = if w == 0 || h == 0 { (1920, 1080) } else { (w, h) };

        // Configura a surface do wgpu pro tamanho desta tela.
        let config = self
            .renderer
            .as_ref()
            .map(|r| r.configure(&self.monitors[idx].surface, w, h));
        if let Some(config) = config {
            self.monitors[idx].config = Some(config);
        }

        // Inicia o loop de render desta tela uma única vez.
        if !self.monitors[idx].configured {
            self.monitors[idx].configured = true;
            self.draw(idx, qh);
        }
    }
}

impl CompositorHandler for Wallpaper {
    fn scale_factor_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: i32) {}
    fn transform_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: wl_output::Transform) {}
    fn surface_enter(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: &wl_output::WlOutput) {}
    fn surface_leave(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: &wl_output::WlOutput) {}

    // Frame callback. Só o monitor-relógio (driver) dirige o redesenho de todas
    // as telas; os callbacks dos outros são ignorados (e morrem naturalmente,
    // pois não os re-agendamos).
    fn frame(&mut self, _: &Connection, qh: &QueueHandle<Self>, surface: &wl_surface::WlSurface, _: u32) {
        let Some(idx) = self.monitors.iter().position(|m| &m.wl_surface == surface) else {
            return;
        };
        // O primeiro callback a chegar elege o driver.
        if self.driver.is_none() {
            self.driver = Some(idx);
        }
        if self.driver != Some(idx) {
            return; // callback de um não-driver: ignora
        }
        // Re-agenda o próximo callback do driver (o present abaixo o envia)...
        let wl = self.monitors[idx].wl_surface.clone();
        wl.frame(qh, wl.clone());
        // ...e redesenha todas as telas neste tick.
        self.render_all();
    }
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
