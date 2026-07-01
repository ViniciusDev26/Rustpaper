// Módulo `wallpaper`: cria uma "layer surface" na camada de FUNDO do desktop
// (protocolo wlr-layer-shell, via smithay-client-toolkit) e entrega a surface
// pro módulo `gpu` desenhar. É o que substitui a janela do winit pra virar
// wallpaper de verdade.

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

use crate::gpu::GpuState;

// Estado do app Wayland. Os handlers (traits do SCTK) são implementados pra ele.
struct Wallpaper {
    registry_state: RegistryState,
    output_state: OutputState,
    layer: LayerSurface,
    gpu: GpuState,
    configured: bool, // já recebemos o primeiro configure? (pra iniciar o loop 1x)
}

pub fn run() {
    // 1) Conecta ao compositor (via socket em WAYLAND_DISPLAY) e lê os globais.
    let conn = Connection::connect_to_env().expect("falha ao conectar no Wayland");
    let (globals, mut event_queue) = registry_queue_init(&conn).unwrap();
    let qh = event_queue.handle();

    // 2) Pega os protocolos que precisamos.
    let compositor = CompositorState::bind(&globals, &qh).expect("wl_compositor ausente");
    let layer_shell = LayerShell::bind(&globals, &qh).expect("wlr-layer-shell ausente");

    // 3) Cria a superfície e a transforma numa LAYER surface no FUNDO do desktop.
    let surface = compositor.create_surface(&qh);
    let layer = layer_shell.create_layer_surface(
        &qh,
        surface,
        Layer::Background, // atrás das janelas/ícones = wallpaper
        Some("wallpaper-engine-rs"),
        None, // output: None => o compositor escolhe (1 monitor por ora)
    );
    // Ancora nos 4 lados => o compositor dimensiona pra tela inteira.
    layer.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
    // -1 = ignora zonas exclusivas (painéis) e ocupa tudo.
    layer.set_exclusive_zone(-1);
    // (0,0) = deixa o compositor decidir o tamanho (= tamanho do output).
    layer.set_size(0, 0);
    // Commit inicial SEM buffer: o compositor responde com um `configure`.
    layer.commit();

    // 4) Cria a surface do wgpu a partir dos PONTEIROS crus do Wayland. Como o
    //    SCTK não implementa os traits do raw-window-handle diretamente, montamos
    //    os handles na mão (o display do conn + o wl_surface da layer).
    let instance = wgpu::Instance::default();
    let raw_display = RawDisplayHandle::Wayland(WaylandDisplayHandle::new(
        NonNull::new(conn.backend().display_ptr() as *mut _).unwrap(),
    ));
    let raw_window = RawWindowHandle::Wayland(WaylandWindowHandle::new(
        NonNull::new(layer.wl_surface().id().as_ptr() as *mut _).unwrap(),
    ));
    // unsafe: nós garantimos que conn e layer vivem mais que a surface (ambos
    // ficam vivos até o fim de run()).
    let wgpu_surface = unsafe {
        instance
            .create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
                raw_display_handle: Some(raw_display),
                raw_window_handle: raw_window,
            })
            .unwrap()
    };

    // Tamanho inicial provisório (1x1): o `configure` logo abaixo nos dá o real.
    let gpu = GpuState::new(&instance, wgpu_surface, 1, 1);

    let mut state = Wallpaper {
        registry_state: RegistryState::new(&globals),
        output_state: OutputState::new(&globals, &qh),
        layer,
        gpu,
        configured: false,
    };

    // 5) Loop de eventos. O primeiro `configure` inicia o loop de render, e cada
    //    frame callback agenda o próximo (ver CompositorHandler::frame).
    loop {
        event_queue.blocking_dispatch(&mut state).unwrap();
    }
}

impl Wallpaper {
    // Agenda o próximo frame callback e desenha. Chamar isto forma o loop:
    // frame -> agenda próximo + render -> compositor chama frame de novo -> ...
    fn draw(&mut self, qh: &QueueHandle<Self>) {
        // Pede um frame callback: o compositor vai chamar CompositorHandler::frame
        // quando estiver pronto pro próximo quadro. Fica pendente até o commit que
        // o present() do wgpu faz dentro de render().
        let wl = self.layer.wl_surface();
        wl.frame(qh, wl.clone());
        self.gpu.render();
    }
}

// === Handlers exigidos pelo SCTK ===

impl CompositorHandler for Wallpaper {
    fn scale_factor_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: i32) {}
    fn transform_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: wl_output::Transform) {}
    fn surface_enter(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: &wl_output::WlOutput) {}
    fn surface_leave(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: &wl_output::WlOutput) {}

    // Chamado quando o compositor está pronto pro próximo frame -> desenha.
    fn frame(&mut self, _: &Connection, qh: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: u32) {
        self.draw(qh);
    }
}

impl LayerShellHandler for Wallpaper {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &LayerSurface) {}

    // Compositor informa o tamanho real da layer surface (a tela).
    fn configure(&mut self, _: &Connection, qh: &QueueHandle<Self>, _: &LayerSurface, configure: LayerSurfaceConfigure, _: u32) {
        let (w, h) = configure.new_size;
        if w != 0 && h != 0 {
            self.gpu.resize(w, h);
        }
        // Inicia o loop de render só uma vez (no primeiro configure).
        if !self.configured {
            self.configured = true;
            self.draw(qh);
        }
    }
}

impl OutputHandler for Wallpaper {
    fn output_state(&mut self) -> &mut OutputState { &mut self.output_state }
    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
}

impl ProvidesRegistryState for Wallpaper {
    fn registry(&mut self) -> &mut RegistryState { &mut self.registry_state }
    registry_handlers![OutputState];
}

delegate_compositor!(Wallpaper);
delegate_output!(Wallpaper);
delegate_layer!(Wallpaper);
delegate_registry!(Wallpaper);
