// Módulo `particles`: simula na CPU os sistemas de partículas da cena e os
// renderiza como sprites (quads) via INSTANCING. CADA sistema tem sua PRÓPRIA
// textura (sprite) e seu blend (additive/translucent). O buffer de instâncias é
// compartilhado; cada sistema ocupa um range contíguo e é desenhado com seu
// bind group + pipeline (blend) próprios.

use bytemuck::{Pod, Zeroable};

use we_core::particle::ParticleSystem;

// Dados que o engine passa pra criar um sistema: os parâmetros + o sprite decodificado.
pub struct ParticleInit {
    pub system: ParticleSystem,
    pub additive: bool,
    pub sprite_rgba: Vec<u8>, // conteúdo já recortado (sem padding)
    pub sprite_w: u32,
    pub sprite_h: u32,
    pub origin: [f32; 3], // posição do objeto na cena (emitter é local a ela)
    pub sheet: Option<we_core::tex::SpriteSheet>, // flipbook (None = sprite único)
}

// PRNG minúsculo (xorshift64*), determinístico, sem crate externa.
struct Rng(u64);
impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed | 1)
    }
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }
    fn unit(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }
    fn range(&mut self, a: f32, b: f32) -> f32 {
        a + (b - a) * self.unit()
    }
    fn range3(&mut self, a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
        [self.range(a[0], b[0]), self.range(a[1], b[1]), self.range(a[2], b[2])]
    }
}

struct Particle {
    pos: [f32; 3],
    vel: [f32; 3],
    age: f32,
    life: f32,
    size: f32,
    color: [f32; 3], // cor base (colorrandom, 0-255)
    osc_freq: f32,
    osc_phase: f32,
    osc_scale: f32,
}

// Simulação de um sistema (parâmetros + partículas vivas + acumulador de spawn).
struct Sim {
    sys: ParticleSystem,
    additive: bool,
    origin: [f32; 3], // posição do objeto na cena (somada ao emitter local)
    sheet: Option<we_core::tex::SpriteSheet>,
    particles: Vec<Particle>,
    spawn_accum: f32,
}

impl Sim {
    fn new(sys: ParticleSystem, additive: bool, origin: [f32; 3], sheet: Option<we_core::tex::SpriteSheet>) -> Self {
        Sim { sys, additive, origin, sheet, particles: Vec::new(), spawn_accum: 0.0 }
    }

    fn update(&mut self, dt: f32, rng: &mut Rng) {
        for p in &mut self.particles {
            p.age += dt;
            for i in 0..3 {
                p.vel[i] += self.sys.gravity[i] * dt;
                p.vel[i] -= p.vel[i] * self.sys.drag * dt;
                p.pos[i] += p.vel[i] * dt;
            }
        }
        self.particles.retain(|p| p.age < p.life);

        self.spawn_accum += self.sys.rate * dt;
        while self.spawn_accum >= 1.0 {
            self.spawn_accum -= 1.0;
            if self.particles.len() >= self.sys.max_count as usize {
                break;
            }
            let dist = rng.range(self.sys.distance_min, self.sys.distance_max).max(0.0);
            let dir = [rng.range(-1.0, 1.0), rng.range(-1.0, 1.0), rng.range(-1.0, 1.0)];
            let len = (dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2]).sqrt().max(1e-3);
            let pos = [
                self.origin[0] + self.sys.origin[0] + dir[0] / len * dist,
                self.origin[1] + self.sys.origin[1] + dir[1] / len * dist,
                self.origin[2] + self.sys.origin[2] + dir[2] / len * dist,
            ];
            let ct = rng.unit();
            let color = [
                self.sys.color_min[0] + ct * (self.sys.color_max[0] - self.sys.color_min[0]),
                self.sys.color_min[1] + ct * (self.sys.color_max[1] - self.sys.color_min[1]),
                self.sys.color_min[2] + ct * (self.sys.color_max[2] - self.sys.color_min[2]),
            ];
            let (osc_freq, osc_phase, osc_scale) = match &self.sys.oscillate {
                Some(o) => (
                    rng.range(o.frequency.0, o.frequency.1),
                    rng.range(o.phase.0, o.phase.1),
                    rng.range(o.scale.0, o.scale.1),
                ),
                None => (0.0, 0.0, 0.0),
            };
            // velocidade inicial: turbulenta (direção aleatória * módulo) se o sistema
            // usar turbulentvelocityrandom; senão a faixa velocityrandom.
            let vel = match self.sys.turbulent_speed {
                Some((smin, smax)) => {
                    let d = [rng.range(-1.0, 1.0), rng.range(-1.0, 1.0), rng.range(-1.0, 1.0)];
                    let l = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().max(1e-3);
                    let sp = rng.range(smin, smax);
                    [d[0] / l * sp, d[1] / l * sp, d[2] / l * sp]
                }
                None => rng.range3(self.sys.velocity_min, self.sys.velocity_max),
            };
            self.particles.push(Particle {
                pos,
                vel,
                age: 0.0,
                life: rng.range(self.sys.lifetime.0, self.sys.lifetime.1).max(0.1),
                size: rng.range(self.sys.size.0, self.sys.size.1),
                color,
                osc_freq,
                osc_phase,
                osc_scale,
            });
        }
    }

    // Alpha da partícula: fade-in no começo, fade-out no fim da vida.
    fn alpha(&self, p: &Particle) -> f32 {
        let fade_in = if self.sys.fade_in_time > 0.0 {
            (p.age / self.sys.fade_in_time).min(1.0)
        } else {
            1.0
        };
        let remaining = 1.0 - (p.age / p.life);
        let fade_out = (remaining / 0.2).min(1.0);
        fade_in * fade_out
    }
}

// Dados por instância (32 bytes). Bate com o layout no shader.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Instance {
    center: [f32; 2],
    half: [f32; 2],
    color: [f32; 4],
    uv_offset: [f32; 2], // canto do frame no sprite sheet
    uv_scale: [f32; 2],  // tamanho do frame (1,1 se não for sheet)
}

// Um sistema já pronto na GPU: a simulação + o bind group do seu sprite + o range
// que ele ocupa no buffer de instâncias neste frame.
struct SystemGpu {
    sim: Sim,
    bind_group: wgpu::BindGroup,
    range: std::ops::Range<u32>,
}

pub struct Particles {
    systems: Vec<SystemGpu>,
    scene_size: [f32; 2],
    alpha_pipeline: wgpu::RenderPipeline,
    additive_pipeline: wgpu::RenderPipeline,
    instance_buffer: wgpu::Buffer,
    capacity: usize,
    rng: Rng,
}

impl Particles {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        inits: Vec<ParticleInit>,
        scene_size: [f32; 2],
    ) -> Self {
        let capacity: usize =
            inits.iter().map(|i| i.system.max_count as usize).sum::<usize>().max(1);

        // Bind group layout explícito (textura + sampler), compartilhado por todos
        // os sistemas e pelos 2 pipelines.
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("particle bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("particle pl"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });

        let shader = device.create_shader_module(wgpu::include_wgsl!("particle.wgsl"));
        let instance_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Instance>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &[
                wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x2, offset: 0, shader_location: 0 },
                wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x2, offset: 8, shader_location: 1 },
                wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: 16, shader_location: 2 },
                wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x2, offset: 32, shader_location: 3 },
                wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x2, offset: 40, shader_location: 4 },
            ],
        };
        let make_pipeline = |blend: wgpu::BlendState, label: &str| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: std::slice::from_ref(&instance_layout),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_main"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(blend),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let alpha_pipeline = make_pipeline(wgpu::BlendState::ALPHA_BLENDING, "particle alpha");
        let additive_pipeline = make_pipeline(
            wgpu::BlendState {
                color: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::SrcAlpha,
                    dst_factor: wgpu::BlendFactor::One,
                    operation: wgpu::BlendOperation::Add,
                },
                alpha: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::One,
                    dst_factor: wgpu::BlendFactor::One,
                    operation: wgpu::BlendOperation::Add,
                },
            },
            "particle additive",
        );

        // Sampler compartilhado (clamp + linear).
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("particle sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        // Um sistema por textura própria.
        let mut systems = Vec::new();
        for init in inits {
            let extent = wgpu::Extent3d {
                width: init.sprite_w,
                height: init.sprite_h,
                depth_or_array_layers: 1,
            };
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("particle sprite"),
                size: extent,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &init.sprite_rgba,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(4 * init.sprite_w),
                    rows_per_image: Some(init.sprite_h),
                },
                extent,
            );
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("particle bind group"),
                layout: &bgl,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&sampler) },
                ],
            });
            systems.push(SystemGpu {
                sim: Sim::new(init.system, init.additive, init.origin, init.sheet),
                bind_group,
                range: 0..0,
            });
        }

        let instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("particle instances"),
            size: (capacity * std::mem::size_of::<Instance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Particles {
            systems,
            scene_size,
            alpha_pipeline,
            additive_pipeline,
            instance_buffer,
            capacity,
            rng: Rng::new(0x9E3779B97F4A7C15),
        }
    }

    // Avança a simulação por `dt` e sobe as instâncias vivas (por sistema) pra GPU.
    pub fn update(&mut self, dt: f32, queue: &wgpu::Queue) {
        for s in &mut self.systems {
            s.sim.update(dt, &mut self.rng);
        }

        let scene_aspect = self.scene_size[0] / self.scene_size[1];
        let scene = self.scene_size;
        let cap = self.capacity;

        let mut instances: Vec<Instance> = Vec::new();
        for s in &mut self.systems {
            let start = instances.len() as u32;
            Self::emit(&s.sim, &mut instances, scene, scene_aspect, cap);
            s.range = start..instances.len() as u32;
        }
        queue.write_buffer(&self.instance_buffer, 0, bytemuck::cast_slice(&instances));
    }

    fn emit(sim: &Sim, out: &mut Vec<Instance>, scene: [f32; 2], scene_aspect: f32, cap: usize) {
        for p in &sim.particles {
            if out.len() >= cap {
                break;
            }
            let a = sim.alpha(p);
            if a <= 0.0 {
                continue;
            }
            let (mut px, mut py) = (p.pos[0], p.pos[1]);
            if let Some(osc) = &sim.sys.oscillate {
                use std::f32::consts::TAU;
                let s = p.osc_scale * (TAU * p.osc_freq * p.age + p.osc_phase * TAU).sin();
                px += osc.mask[0] * s;
                py += osc.mask[1] * s;
            }
            let rgb = match &sim.sys.color_change {
                Some(cc) => {
                    let denom = (cc.end_time - cc.start_time).max(1e-3);
                    let f = ((p.age / p.life - cc.start_time) / denom).clamp(0.0, 1.0);
                    [
                        cc.start[0] + f * (cc.end[0] - cc.start[0]),
                        cc.start[1] + f * (cc.end[1] - cc.start[1]),
                        cc.start[2] + f * (cc.end[2] - cc.start[2]),
                    ]
                }
                None => [p.color[0] / 255.0, p.color[1] / 255.0, p.color[2] / 255.0],
            };
            // Aditivo sem HDR/tonemap satura em branco quando sprites se sobrepõem.
            // O WE compõe em HDR e faz tonemap; nós amortecemos a contribuição pra
            // manter o brilho sutil (aproxima o resultado tonemapeado).
            let rgb = if sim.additive { [rgb[0] * 0.35, rgb[1] * 0.35, rgb[2] * 0.35] } else { rgb };
            let hx = p.size / (scene[0] * 0.5);
            // Sprite sheet: escolhe a célula do frame pela idade (toca a animação uma
            // vez ao longo da vida). Sem sheet, usa a textura inteira (0..1).
            let (uv_offset, uv_scale) = match sim.sheet {
                Some(s) => {
                    let f = ((p.age / p.life) * s.frames as f32).floor() as u32;
                    let f = f.min(s.frames.saturating_sub(1));
                    let (col, row) = (f % s.cols, f / s.cols);
                    (
                        [col as f32 / s.cols as f32, row as f32 / s.rows as f32],
                        [1.0 / s.cols as f32, 1.0 / s.rows as f32],
                    )
                }
                None => ([0.0, 0.0], [1.0, 1.0]),
            };
            // Posição da cena (0..scene, y pra baixo) -> clip space [-1,1] com Y
            // invertido. Sem o -1/flip as partículas ficavam presas num quadrado.
            out.push(Instance {
                center: [px / (scene[0] * 0.5) - 1.0, 1.0 - py / (scene[1] * 0.5)],
                half: [hx, hx * scene_aspect],
                color: [rgb[0], rgb[1], rgb[2], a],
                uv_offset,
                uv_scale,
            });
        }
    }

    // Desenha cada sistema com seu sprite (bind group) e blend (pipeline).
    pub fn draw(&self, pass: &mut wgpu::RenderPass) {
        pass.set_vertex_buffer(0, self.instance_buffer.slice(..));
        for s in &self.systems {
            if s.range.is_empty() {
                continue;
            }
            let pipe = if s.sim.additive { &self.additive_pipeline } else { &self.alpha_pipeline };
            pass.set_pipeline(pipe);
            pass.set_bind_group(0, &s.bind_group, &[]);
            pass.draw(0..6, s.range.clone());
        }
    }
}
