// Módulo `gpu`: Renderer compartilhado. A fonte da textura pode ser um VÍDEO
// (atualizada a cada frame) ou uma IMAGEM estática (subida uma vez) — o caso da
// cena. O recorte de conteúdo (`content`) ignora o padding do buffer da textura.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::compositor::Compositor;
use crate::particles::{ParticleInit, Particles};
use crate::video::Video;

// De onde vêm os pixels da textura.
pub enum Source {
    Video(String), // caminho do arquivo
    Scene {
        rgba: Vec<u8>,
        width: u32,      // dims do buffer (pode ter padding)
        height: u32,
        real_width: u32, // região de conteúdo
        real_height: u32,
        particles: Vec<ParticleInit>, // cada sistema traz seu próprio sprite
    },
    // Cena renderizada pelo COMPOSITOR multi-camada (todas as camadas + efeitos).
    // A cada frame o compositor desenha na textura de conteúdo; o resto do pipeline
    // (cover-scale por monitor) segue igual.
    SceneComposite(PathBuf),
}

// 16 bytes: scale@0 (vec2), content@8 (vec2). Casa com o WGSL.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    scale: [f32; 2],
    content: [f32; 2],
}

// Fator de cover: preenche a tela mantendo a proporção do conteúdo.
fn cover_scale(screen_w: f32, screen_h: f32, img_w: f32, img_h: f32) -> [f32; 2] {
    let screen_aspect = screen_w / screen_h;
    let image_aspect = img_w / img_h;
    if screen_aspect > image_aspect {
        [1.0, image_aspect / screen_aspect]
    } else {
        [screen_aspect / image_aspect, 1.0]
    }
}

pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    adapter: wgpu::Adapter,
    pipeline: wgpu::RenderPipeline,
    uniform_buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    format: wgpu::TextureFormat,
    content_size: [f32; 2],  // dims do conteúdo, pra o cover (aspect)
    content_norm: [f32; 2],  // conteúdo/buffer, pra o recorte no shader
    // Vídeo: presente só quando a fonte é vídeo (textura atualizada por frame).
    video: Option<Video>,
    texture: wgpu::Texture,
    extent: wgpu::Extent3d,
    last_gen: AtomicU64,
    // Partículas da cena (simulação + render instanciado). None p/ vídeo/imagem.
    particles: Option<Particles>,
    last_frame: std::time::Instant,
    // Compositor multi-camada (cenas). Redesenha a textura de conteúdo a cada frame.
    compositor: Option<Compositor>,
    start: std::time::Instant,
}

impl Renderer {
    pub fn new(instance: &wgpu::Instance, first_surface: &wgpu::Surface, source: Source) -> Self {
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: Some(first_surface),
        }))
        .unwrap();

        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).unwrap();

        let format = first_surface.get_capabilities(&adapter).formats[0];

        // Resolve as dimensões e a fonte (vídeo x cena x compositor).
        let (buf_w, buf_h, content_w, content_h, initial_rgba, video, part_inits, mut compositor) = match source {
            Source::Video(path) => {
                let v = Video::open(&path);
                let (w, h) = (v.width, v.height);
                // vídeo: buffer == conteúdo, sem padding, sem partículas
                (w, h, w, h, None, Some(v), Vec::new(), None)
            }
            Source::Scene { rgba, width, height, real_width, real_height, particles } => {
                (width, height, real_width, real_height, Some(rgba), None, particles, None)
            }
            Source::SceneComposite(dir) => {
                let comp = Compositor::new(&device, &queue, &dir)
                    .expect("falha ao montar o compositor da cena");
                let (w, h) = (comp.width, comp.height);
                // buffer == conteúdo (sem padding); sem vídeo/partículas; o compositor
                // preenche a textura de conteúdo a cada frame.
                (w, h, w, h, None, None, Vec::new(), Some(comp))
            }
        };

        let extent = wgpu::Extent3d {
            width: buf_w,
            height: buf_h,
            depth_or_array_layers: 1,
        };

        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("uniforms"),
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("content texture"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::RENDER_ATTACHMENT, // compositor desenha aqui
            view_formats: &[],
        });

        // Compositor: renderiza o primeiro frame já, pra não piscar preto.
        if let Some(comp) = &mut compositor {
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            comp.render(&device, &queue, 0.0, 0.0, &view);
        }

        // Imagem estática: sobe os pixels uma vez agora.
        if let Some(rgba) = initial_rgba {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &rgba,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(4 * buf_w),
                    rows_per_image: Some(buf_h),
                },
                extent,
            );
        }

        let texture_view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });

        let shader = device.create_shader_module(wgpu::include_wgsl!("shader.wgsl"));

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("wallpaper pipeline"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let bind_group_layout = pipeline.get_bind_group_layout(0);
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("bind group"),
            layout: &bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: uniform_buffer.as_entire_binding() },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&texture_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });

        // Partículas da cena (cada sistema com seu sprite).
        let particles = if part_inits.is_empty() {
            None
        } else {
            Some(Particles::new(
                &device,
                &queue,
                format,
                part_inits,
                [content_w as f32, content_h as f32],
            ))
        };

        Self {
            device,
            queue,
            adapter,
            pipeline,
            uniform_buffer,
            bind_group,
            format,
            content_size: [content_w as f32, content_h as f32],
            content_norm: [content_w as f32 / buf_w as f32, content_h as f32 / buf_h as f32],
            video,
            texture,
            extent,
            last_gen: AtomicU64::new(0),
            particles,
            last_frame: std::time::Instant::now(),
            compositor,
            start: std::time::Instant::now(),
        }
    }

    // Avança a simulação de partículas uma vez por frame (dt medido aqui). Chamar
    // ANTES de renderizar os monitores.
    pub fn tick(&mut self) {
        let now = std::time::Instant::now();
        let dt = (now - self.last_frame).as_secs_f32().min(0.1); // clampa picos
        self.last_frame = now;
        // Compositor: redesenha a cena inteira na textura de conteúdo (g_Time avança;
        // dt anima as partículas).
        let t = self.start.elapsed().as_secs_f32();
        let view = self.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let (dev, q) = (&self.device, &self.queue);
        if let Some(c) = self.compositor.as_mut() {
            c.render(dev, q, t, dt, &view);
        }
        if let Some(p) = self.particles.as_mut() {
            p.update(dt, &self.queue);
        }
    }

    pub fn configure(
        &self,
        surface: &wgpu::Surface,
        width: u32,
        height: u32,
    ) -> wgpu::SurfaceConfiguration {
        let caps = surface.get_capabilities(&self.adapter);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: self.format,
            width: width.max(1),
            height: height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&self.device, &config);
        config
    }

    // Só faz algo quando a fonte é vídeo: sobe o frame mais recente, se houver.
    fn update_video_texture(&self) {
        let Some(video) = &self.video else { return };
        let last = self.last_gen.load(Ordering::Relaxed);
        let new_gen = video.upload_if_newer(last, |data| {
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                data,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(4 * self.extent.width),
                    rows_per_image: Some(self.extent.height),
                },
                self.extent,
            );
        });
        if let Some(generation) = new_gen {
            self.last_gen.store(generation, Ordering::Relaxed);
        }
    }

    pub fn render(&self, surface: &wgpu::Surface, config: &wgpu::SurfaceConfiguration) {
        self.update_video_texture();

        let frame = match surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f) | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                surface.configure(&self.device, config);
                return;
            }
            _ => return,
        };
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());

        let uniforms = Uniforms {
            scale: cover_scale(
                config.width as f32,
                config.height as f32,
                self.content_size[0],
                self.content_size[1],
            ),
            content: self.content_norm,
        };
        self.queue.write_buffer(&self.uniform_buffer, 0, bytemuck::cast_slice(&[uniforms]));

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("encoder") });
        {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("main pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            // Fundo (imagem/vídeo da cena).
            render_pass.set_pipeline(&self.pipeline);
            render_pass.set_bind_group(0, &self.bind_group, &[]);
            render_pass.draw(0..3, 0..1);

            // Partículas por cima (instanciadas, blend por sistema).
            if let Some(p) = self.particles.as_ref() {
                p.draw(&mut render_pass);
            }
        }

        self.queue.submit(Some(encoder.finish()));
        frame.present();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-6
    }

    #[test]
    fn cover_same_aspect_no_scale() {
        let s = cover_scale(1920.0, 1080.0, 1920.0, 1080.0);
        assert!(approx(s[0], 1.0) && approx(s[1], 1.0));
    }

    #[test]
    fn cover_square_image_on_wide_screen() {
        let s = cover_scale(1920.0, 1080.0, 1000.0, 1000.0);
        assert!(approx(s[0], 1.0));
        assert!(approx(s[1], 0.5625));
    }

    #[test]
    fn uniforms_layout_matches_shader() {
        assert_eq!(std::mem::size_of::<Uniforms>(), 16);
        assert_eq!(std::mem::offset_of!(Uniforms, scale), 0);
        assert_eq!(std::mem::offset_of!(Uniforms, content), 8);
    }
}
