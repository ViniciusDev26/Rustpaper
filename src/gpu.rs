// Módulo `gpu`: Renderer compartilhado. Agora a textura é DINÂMICA — recebe um
// novo frame do vídeo a cada quadro (via o módulo `video`).

use std::sync::atomic::{AtomicU64, Ordering};

use crate::video::Video;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    resolution: [f32; 2],
    image_size: [f32; 2],
    time: f32,
    _pad: [f32; 3],
}

pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    adapter: wgpu::Adapter,
    pipeline: wgpu::RenderPipeline,
    uniform_buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    format: wgpu::TextureFormat,
    start: std::time::Instant,
    image_size: [f32; 2],
    // Vídeo + a textura que recebe cada frame + a última geração já enviada.
    video: Video,
    texture: wgpu::Texture,
    extent: wgpu::Extent3d,
    last_gen: AtomicU64,
}

impl Renderer {
    pub fn new(instance: &wgpu::Instance, first_surface: &wgpu::Surface, video_path: &str) -> Self {
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: Some(first_surface),
        }))
        .unwrap();

        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).unwrap();

        let format = first_surface.get_capabilities(&adapter).formats[0];

        // Abre o vídeo (inicia o ffmpeg + thread de decodificação).
        let video = Video::open(video_path);
        let extent = wgpu::Extent3d {
            width: video.width,
            height: video.height,
            depth_or_array_layers: 1,
        };

        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("uniforms"),
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Textura do tamanho do vídeo; COPY_DST porque vamos reescrever todo frame.
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("video texture"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
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
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform_buffer.as_entire_binding(),
                },
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

        Self {
            device,
            queue,
            adapter,
            pipeline,
            uniform_buffer,
            bind_group,
            format,
            start: std::time::Instant::now(),
            image_size: [video.width as f32, video.height as f32],
            video,
            texture,
            extent,
            last_gen: AtomicU64::new(0),
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

    // Sobe o frame de vídeo mais recente pra textura, se houver um novo.
    fn update_video_texture(&self) {
        let last = self.last_gen.load(Ordering::Relaxed);
        let new_gen = self.video.upload_if_newer(last, |data| {
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
        // Atualiza a textura com o frame de vídeo mais recente (se houver).
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
            resolution: [config.width as f32, config.height as f32],
            image_size: self.image_size,
            time: self.start.elapsed().as_secs_f32(),
            _pad: [0.0; 3],
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
            render_pass.set_pipeline(&self.pipeline);
            render_pass.set_bind_group(0, &self.bind_group, &[]);
            render_pass.draw(0..3, 0..1);
        }

        self.queue.submit(Some(encoder.finish()));
        frame.present();
    }
}
