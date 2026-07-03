// Módulo `wallpaper`: cria uma layer surface na camada de FUNDO para CADA
// monitor conectado, e desenha em todas usando um Renderer compartilhado.

use std::ptr::NonNull;
use std::time::Duration;

use calloop::timer::{TimeoutAction, Timer};
use calloop::EventLoop;
use calloop_wayland_source::WaylandSource;
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

use crate::gpu::{Renderer, Source};
use crate::particles::ParticleInit;
use rustpaper_core::particle::ParticleSystem;
use rustpaper_core::pkg::Pkg;
use rustpaper_core::project::{Project, WallpaperKind};
use rustpaper_core::{scene, tex};

// Renderizamos sistemas com o renderer "sprite" e tamanho moderado. Pulamos:
// - "spritetrail" (trilhas) e afins: ficariam errados no nosso render.
// - sprites GIGANTES (fog/feixes/raios de 800-2200px): são efeitos atmosféricos
//   additive que, sem o pipeline HDR/tonemap/bloom do WE, apenas SOMAM luz e
//   lavam a tela em branco, cobrindo o fundo. Melhor pular do que estourar tudo.
// Assim as cenas ficam com o fundo visível + partículas pequenas (poeira, brasas,
// bolhas). Operadores não implementados (turbulence...) são só ignorados.
const MAX_PARTICLE_SIZE: f32 = 400.0;
fn is_supported(sys: &ParticleSystem) -> bool {
    sys.renderer == "sprite" && sys.size.1 <= MAX_PARTICLE_SIZE
}

// Uma tela: sua layer surface, a wl_surface, a surface do wgpu e a config (tamanho).
struct Monitor {
    // O LayerSurface segura a wl_surface viva (o ponteiro dela alimenta a surface
    // do wgpu), então não precisamos guardar a wl_surface separada.
    layer: LayerSurface,
    surface: wgpu::Surface<'static>,
    config: Option<wgpu::SurfaceConfiguration>, // definida no primeiro configure
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
    // Fonte da textura (vídeo ou imagem da cena); movida pro Renderer no 1º monitor.
    source: Option<Source>,
}

// Assets base do WE, montados no container (sprites de partícula etc.).
const ASSETS_DIR: &str = "/home/vscode/we-assets";

// Carrega e decodifica uma textura por nome ("particle/halo"), buscando primeiro
// no pkg da cena e, se não achar, nos assets base do WE. Devolve o CONTEÚDO
// recortado (sem o padding pra potência de 2) + suas dimensões.
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

    // Recorta pro conteúdo real (o buffer pode ter padding à direita/embaixo).
    if t.width == t.real_width && t.height == t.real_height {
        return Some((t.rgba, t.width, t.height));
    }
    let (bw, rw, rh) = (t.width as usize, t.real_width as usize, t.real_height as usize);
    let mut out = Vec::with_capacity(rw * rh * 4);
    for y in 0..rh {
        let row = &t.rgba[y * bw * 4..y * bw * 4 + rw * 4];
        out.extend_from_slice(row);
    }
    Some((out, t.real_width, t.real_height))
}

// Resolve a fonte de uma cena: fundo (imagem) + sistemas de partículas + sprite.
// (mantido pro caminho antigo/partículas; o padrão agora é o compositor.)
#[allow(dead_code)]
fn scene_source(dir: &Path) -> Source {
    let pkg = Pkg::open(&dir.join("scene.pkg")).expect("falha ao abrir scene.pkg");

    // Fundo.
    let tex_path = scene::background_texture(&pkg).expect("não achei a textura de fundo da cena");
    let bytes = pkg.read(&tex_path).expect("textura não está no pkg");
    let t = tex::parse(bytes).unwrap_or_else(|e| {
        eprintln!("falha ao decodificar {tex_path}: {e}");
        std::process::exit(1);
    });

    // Partículas: só renderizamos sistemas com renderer "sprite" (os spritetrail
    // etc. ficariam errados). CADA sistema carrega o SEU próprio sprite; se o
    // sprite não carregar/decodificar, o sistema é pulado.
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
            "  partículas: {} renderizado(s) de {} ({} pulado(s))",
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
    // Lê o project.json e decide o que tocar.
    let project = Project::load(dir).expect("falha ao ler project.json da pasta");
    println!("Wallpaper: {:?} (tipo {:?})", project.title, project.kind);

    let source = match project.kind {
        WallpaperKind::Video => {
            Source::Video(project.file_path(dir).to_string_lossy().into_owned())
        }
        // Cena: usa o COMPOSITOR multi-camada (todas as camadas + efeitos por camada),
        // em vez do antigo caminho de uma textura só (scene_source).
        WallpaperKind::Scene => Source::SceneComposite(dir.to_path_buf()),
        other => {
            eprintln!("tipo {other:?} ainda não suportado.");
            std::process::exit(1);
        }
    };

    let conn = Connection::connect_to_env().expect("falha ao conectar no Wayland");
    let (globals, event_queue) = registry_queue_init(&conn).unwrap();
    let qh = event_queue.handle();

    let compositor = CompositorState::bind(&globals, &qh).expect("wl_compositor ausente");
    let layer_shell = LayerShell::bind(&globals, &qh).expect("wlr-layer-shell ausente");

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

    // Event loop via calloop. O render é dirigido por um TIMER (~30fps), não pelos
    // frame callbacks do Wayland — assim o wallpaper nunca "congela" quando o
    // compositor para de mandar callbacks (ex.: superfície coberta por uma janela).
    let mut event_loop: EventLoop<Wallpaper> = EventLoop::try_new().expect("criar event loop");
    let handle = event_loop.handle();

    // Fonte Wayland: entrega os eventos (outputs, configure...) pros handlers.
    WaylandSource::new(conn, event_queue)
        .insert(handle.clone())
        .expect("inserir WaylandSource");

    // Timer de ~30fps: redesenha todas as telas. Enquanto renderer/config ainda
    // não existem (antes do 1º configure), render_monitor simplesmente pula.
    let frame_interval = Duration::from_millis(33);
    handle
        .insert_source(Timer::from_duration(frame_interval), move |_, _, state| {
            state.render_all();
            TimeoutAction::ToDuration(frame_interval)
        })
        .expect("inserir timer");

    event_loop.run(None, &mut state, |_| {}).expect("rodar event loop");
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

    // Redesenha TODAS as telas (chamado a cada tick do timer).
    fn render_all(&mut self) {
        // Avança a simulação de partículas uma vez por frame (antes de desenhar).
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

    // Chamado para cada monitor (na inicialização e em hotplug).
    fn new_output(&mut self, _: &Connection, qh: &QueueHandle<Self>, output: wl_output::WlOutput) {
        // Cria a layer surface ancorada a ESTE output específico.
        let surface = self.compositor.create_surface(qh);
        let layer = self.layer_shell.create_layer_surface(
            qh,
            surface,
            Layer::Background,
            Some("rustpaper"),
            Some(&output),
        );
        layer.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
        layer.set_exclusive_zone(-1);
        layer.set_size(0, 0);
        layer.commit();

        // Cria a surface do wgpu a partir dos ponteiros crus do Wayland. O ponteiro
        // da wl_surface vem do layer (que a mantém viva enquanto o Monitor existir).
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

        // O Renderer (device/pipeline) é criado uma vez, a partir da 1ª surface.
        if self.renderer.is_none() {
            let source = self.source.take().expect("source já consumida");
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
        // O timer cuida de desenhar; não precisamos disparar render aqui.
    }
}

impl CompositorHandler for Wallpaper {
    fn scale_factor_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: i32) {}
    fn transform_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: wl_output::Transform) {}
    fn surface_enter(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: &wl_output::WlOutput) {}
    fn surface_leave(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: &wl_output::WlOutput) {}

    // Não usamos frame callbacks pra dirigir o render (o timer faz isso), então
    // este handler fica vazio.
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
