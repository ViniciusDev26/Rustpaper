// Módulo `gpu`: tudo relacionado a desenhar com a GPU (wgpu). Não sabe nada sobre
// tratar eventos — só recebe uma janela e desenha nela.

use std::sync::Arc;
use winit::window::Window;

// `pub` = visível fora deste módulo (o app.rs precisa usar GpuState).
// Os CAMPOS continuam privados (sem `pub`): só o próprio gpu.rs mexe neles.
pub struct GpuState {
    // A janela vive num Arc (ponteiro contado por referência) porque DUAS coisas
    // precisam dela: nós e a surface do wgpu. Arc permite compartilhar posse.
    window: Arc<Window>,
    // Surface = a "ponte" entre a janela e a GPU: é onde a GPU escreve pra que o
    // resultado apareça na janela. 'static porque o Arc mantém a janela viva.
    surface: wgpu::Surface<'static>,
    device: wgpu::Device, // fábrica de recursos da GPU
    queue: wgpu::Queue,   // envia comandos pra GPU
    // Configuração da surface (formato de cor, tamanho, v-sync). Guardamos pra
    // poder reconfigurar quando a janela mudar de tamanho.
    config: wgpu::SurfaceConfiguration,
    // Momento em que o app começou. A cada frame medimos quanto tempo passou
    // desde aqui pra animar a cor. Instant = um relógio monotônico (só avança).
    start: std::time::Instant,
    // O "render pipeline": amarra os shaders + como rasterizar + formato de saída.
    // Criado uma vez (é caro) e reusado a cada frame.
    pipeline: wgpu::RenderPipeline,
}

impl GpuState {
    // Constrói todo o estado da GPU a partir de uma janela. (Convenção Rust:
    // `new` é o nome usual de construtor; não é palavra-chave.)
    pub fn new(window: Arc<Window>) -> Self {
        let instance = wgpu::Instance::default();

        // Cria a surface a partir da janela. Passamos window.clone() (clona o Arc,
        // não a janela — só incrementa o contador) pra surface ficar co-dona dela.
        let surface = instance.create_surface(window.clone()).unwrap();

        // Escolhe a GPU. compatible_surface garante que a GPU escolhida consegue
        // desenhar NESTA surface.
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: Some(&surface),
        }))
        .unwrap();

        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).unwrap();

        // Pergunta à surface o que ela suporta (formatos de cor, modos de present).
        let caps = surface.get_capabilities(&adapter);
        let size = window.inner_size();

        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT, // vamos desenhar nela
            format: caps.formats[0],                       // formato de cor preferido
            width: size.width.max(1),                      // nunca 0 (quebra a surface)
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::Fifo, // v-sync (sempre suportado)
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        // Carrega e compila o shader WGSL. include_wgsl! embute o arquivo no binário
        // (o caminho é relativo a ESTE arquivo, então resolve src/shader.wgsl).
        let shader = device.create_shader_module(wgpu::include_wgsl!("shader.wgsl"));

        // O render pipeline junta tudo que a GPU precisa pra desenhar:
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("triangle pipeline"),
            // layout: None => o wgpu deriva sozinho (não temos texturas/uniforms ainda).
            layout: None,
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"), // a função @vertex do shader
                compilation_options: Default::default(),
                buffers: &[], // sem buffer de vértices: as posições estão no shader
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"), // a função @fragment
                compilation_options: Default::default(),
                // O alvo de cor precisa ter o MESMO formato da surface.
                targets: &[Some(wgpu::ColorTargetState {
                    format: config.format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),   // lista de triângulos (padrão)
            depth_stencil: None,                          // sem profundidade (2D)
            multisample: wgpu::MultisampleState::default(), // sem antialiasing por ora
            multiview_mask: None,
            cache: None,
        });

        Self { window, surface, device, queue, config, start: std::time::Instant::now(), pipeline }
    }

    // Reconfigura a surface quando a janela muda de tamanho.
    pub fn resize(&mut self, width: u32, height: u32) {
        if width > 0 && height > 0 {
            self.config.width = width;
            self.config.height = height;
            self.surface.configure(&self.device, &self.config);
        }
    }

    // Pede à janela pra emitir outro RedrawRequested (mantém o loop de render).
    // Encapsula o acesso ao campo `window`, que é privado.
    pub fn request_redraw(&self) {
        self.window.request_redraw();
    }

    // Desenha UM frame: aqui, só limpa a tela com uma cor animada.
    pub fn render(&mut self) {
        // get_current_texture devolve um ENUM com todos os casos possíveis (não um
        // simples Result). Tratamos cada um: usamos o frame nos casos bons, e nos
        // ruins ou reconfiguramos ou pulamos o frame — sem crashar.
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f) | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                // Surface mudou/perdeu: reconfigura e pula este frame.
                self.surface.configure(&self.device, &self.config);
                return;
            }
            // Timeout / Occluded (minimizada) / Validation: só pula o frame.
            _ => return,
        };
        // Uma "view" é como a GPU enxerga essa textura pra desenhar nela.
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());

        // Quanto tempo (em segundos, com fração) passou desde o início.
        let t = self.start.elapsed().as_secs_f64();
        // Cada canal oscila com um seno de fase/velocidade diferente. sin() vai de
        // -1 a 1; a fórmula (0.5 + 0.5*sin) remapeia pra 0..1 (faixa de cor válida).
        let color = wgpu::Color {
            r: 0.5 + 0.5 * t.sin(),
            g: 0.5 + 0.5 * (t * 0.7 + 2.0).sin(),
            b: 0.5 + 0.5 * (t * 1.3 + 4.0).sin(),
            a: 1.0,
        };

        // O encoder grava uma lista de comandos pra GPU (nada roda ainda).
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("encoder") });

        // Bloco { } pra limitar o tempo de vida do render pass: ele empresta o
        // encoder de forma mutável, e precisa ser SOLTO antes de encoder.finish().
        {
            // Agora `mut`: vamos emitir comandos de desenho nele.
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("main pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        // Clear = pinta o FUNDO com a cor animada (roda antes do draw).
                        load: wgpu::LoadOp::Clear(color),
                        store: wgpu::StoreOp::Store, // guarda o resultado
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None, // multiview (VR/camadas) — não usamos
            });
            // Desenha o triângulo POR CIMA do fundo já limpo:
            render_pass.set_pipeline(&self.pipeline); // usa nossos shaders
            // draw(vertices, instances): 3 vértices (0..3), 1 instância (0..1).
            // A GPU chama vs_main com index=0,1,2; depois fs_main por pixel.
            render_pass.draw(0..3, 0..1);
            // Ao sair do bloco, render_pass é dropado e os comandos ficam gravados.
        }

        // Finaliza os comandos e envia pra GPU executar.
        self.queue.submit(Some(encoder.finish()));
        // Apresenta o frame na janela (é aqui que aparece na tela!).
        frame.present();
    }
}
