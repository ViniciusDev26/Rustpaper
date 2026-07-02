// Módulo `compositor`: renderiza uma cena INTEIRA do WE (todas as camadas-imagem,
// em ordem, com transform/blend/alpha, mais os efeitos de cada camada) numa textura
// de conteúdo. É a versão "engine" do que o example render_composite prova offscreen,
// pronta pra ser chamada a cada frame pelo renderer ao vivo (com `time` avançando).
//
// Cada camada é pré-construída em `new` (textura, programa, quad, constants). Camadas
// com efeitos são renderizadas num scratch, passam pelos efeitos e são compostas.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use we_core::effects::{self, EffectInstance};
use we_core::layout::{self, Layer};
use we_core::particle::ParticleSystem;
use we_core::pkg::Pkg;
use we_core::tex;

use crate::particles::{ParticleInit, Particles};
use crate::program::{ortho, Blend, Program};

const BASE: &str = "/home/vscode/we-assets";
pub const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
// Partículas ligadas? Desligadas até o simulador reproduzir os sistemas do WE com
// fidelidade (hoje saem como "quadrados de bolhas"). Ver nota em Compositor::new.
const PARTICLES_ENABLED: bool = false;

fn resolve(pkg: &Pkg, path: &str) -> Option<Vec<u8>> {
    pkg.read(path).map(|b| b.to_vec()).or_else(|| std::fs::read(PathBuf::from(BASE).join(path)).ok())
}
fn value_to_floats(v: &serde_json::Value) -> Vec<f32> {
    match v {
        serde_json::Value::Number(n) => vec![n.as_f64().unwrap_or(0.0) as f32],
        serde_json::Value::String(s) => s.split_whitespace().filter_map(|t| t.parse().ok()).collect(),
        _ => Vec::new(),
    }
}

fn upload_tex(device: &wgpu::Device, queue: &wgpu::Queue, bytes: &[u8]) -> Option<wgpu::Texture> {
    let d = tex::parse(bytes).ok()?;
    let (cw, ch) = (d.real_width.max(1), d.real_height.max(1));
    let mut content = vec![0u8; (cw * ch * 4) as usize];
    for y in 0..ch {
        let s = (y * d.width * 4) as usize;
        let dd = (y * cw * 4) as usize;
        if s + (cw * 4) as usize <= d.rgba.len() {
            content[dd..dd + (cw * 4) as usize].copy_from_slice(&d.rgba[s..s + (cw * 4) as usize]);
        }
    }
    let ext = wgpu::Extent3d { width: cw, height: ch, depth_or_array_layers: 1 };
    let t = device.create_texture(&wgpu::TextureDescriptor {
        label: None, size: ext, mip_level_count: 1, sample_count: 1,
        dimension: wgpu::TextureDimension::D2, format: FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST, view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo { texture: &t, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        &content,
        wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4 * cw), rows_per_image: Some(ch) },
        ext,
    );
    Some(t)
}

fn layer_quad(l: &Layer) -> [f32; 20] {
    let (hx, hy) = (l.size[0] * 0.5 * l.scale[0], l.size[1] * 0.5 * l.scale[1]);
    let (c, s) = (l.angles[2].cos(), l.angles[2].sin());
    let corner = |lx: f32, ly: f32| (l.origin[0] + lx * c - ly * s, l.origin[1] + lx * s + ly * c);
    let (a, b, cc, d) = (corner(-hx, -hy), corner(hx, -hy), corner(hx, hy), corner(-hx, hy));
    #[rustfmt::skip]
    let v = [a.0,a.1,0.0, 0.0,0.0,  b.0,b.1,0.0, 1.0,0.0,  cc.0,cc.1,0.0, 1.0,1.0,  d.0,d.1,0.0, 0.0,1.0];
    v
}

// Um efeito resolvido de uma camada: shader + combos + constants (já mesclados).
#[allow(dead_code)]
struct EffectPassGpu {
    key: String,
    constants: HashMap<String, Vec<f32>>,
    noise: Option<wgpu::Texture>, // textura extra (ex.: util/noise) já carregada
    noise_view: Option<wgpu::TextureView>,
}

struct LayerGpu {
    key: String,
    _tex: wgpu::Texture,
    tex_view: wgpu::TextureView,
    extra: Vec<(u32, wgpu::Texture, wgpu::TextureView)>, // (binding, tex, view) pros slots >0
    vbuf: wgpu::Buffer,
    constants: HashMap<String, Vec<f32>>,
    brightness: f32,
    alpha: f32,
    color: [f32; 3],
    blend: Blend,
    effects: Vec<EffectPassGpu>,
}

pub struct Compositor {
    pub width: u32,
    pub height: u32,
    clear: [f32; 3],
    mvp: [f32; 16],
    programs: HashMap<String, Program>,
    layers: Vec<LayerGpu>,
    ibuf: wgpu::Buffer,
    clamp: wgpu::Sampler,
    repeat: wgpu::Sampler,
    // ping-pong pros efeitos (tamanho da cena)
    scratch_a: wgpu::Texture,
    scratch_b: wgpu::Texture,
    // partículas da cena (simulação + render instanciado), em espaço de cena.
    particles: Option<Particles>,
}

// Sprite de partícula suportado: renderer "sprite" e tamanho moderado (os gigantes
// additive lavam a tela sem HDR — ver nota no wallpaper.rs).
fn particle_supported(sys: &ParticleSystem) -> bool {
    sys.renderer == "sprite" && sys.size.1 <= 400.0
}

// carrega o sprite de uma partícula (rgba recortado) do pkg/base.
fn load_sprite(pkg: &Pkg, name: &str) -> Option<(Vec<u8>, u32, u32)> {
    if name.is_empty() {
        return None;
    }
    let bytes = resolve(pkg, &format!("materials/{name}.tex"))?;
    let t = tex::parse(&bytes).ok()?;
    if t.width == t.real_width && t.height == t.real_height {
        return Some((t.rgba, t.width, t.height));
    }
    let (bw, rw, rh) = (t.width as usize, t.real_width as usize, t.real_height as usize);
    let mut out = Vec::with_capacity(rw * rh * 4);
    for y in 0..rh {
        out.extend_from_slice(&t.rgba[y * bw * 4..y * bw * 4 + rw * 4]);
    }
    Some((out, t.real_width, t.real_height))
}

impl Compositor {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, dir: &Path) -> Option<Compositor> {
        let pkg = Pkg::open(&dir.join("scene.pkg")).ok()?;
        let lay = layout::parse_layout(&pkg)?;
        let (w, h) = (lay.width as u32, lay.height as u32);
        let shaders_dir = PathBuf::from(BASE).join("shaders");

        let idx: [u16; 6] = [0, 1, 2, 0, 2, 3];
        let ibuf = device.create_buffer(&wgpu::BufferDescriptor { label: Some("ib"), size: 12, usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        queue.write_buffer(&ibuf, 0, bytemuck::cast_slice(&idx));

        let clamp = device.create_sampler(&wgpu::SamplerDescriptor { mag_filter: wgpu::FilterMode::Linear, min_filter: wgpu::FilterMode::Linear, ..Default::default() });
        let repeat = device.create_sampler(&wgpu::SamplerDescriptor { address_mode_u: wgpu::AddressMode::Repeat, address_mode_v: wgpu::AddressMode::Repeat, mag_filter: wgpu::FilterMode::Linear, min_filter: wgpu::FilterMode::Linear, ..Default::default() });

        let mut programs: HashMap<String, Program> = HashMap::new();
        let mut layers: Vec<LayerGpu> = Vec::new();

        let ensure_prog = |programs: &mut HashMap<String, Program>, device: &wgpu::Device, pkg: &Pkg, shader: &str, combos: &[(String, i64)], blend: Blend| -> Option<String> {
            let key = format!("{shader}|{combos:?}|{blend:?}", blend = blend as u8);
            if !programs.contains_key(&key) {
                let vsrc = resolve(pkg, &format!("shaders/{shader}.vert")).and_then(|b| String::from_utf8(b).ok())?;
                let fsrc = resolve(pkg, &format!("shaders/{shader}.frag")).and_then(|b| String::from_utf8(b).ok())?;
                match Program::build(device, &vsrc, &fsrc, combos, blend, FORMAT, &shaders_dir) {
                    Ok(p) => { programs.insert(key.clone(), p); }
                    Err(e) => { eprintln!("  (shader {shader}: {})", e.lines().next().unwrap_or("")); return None; }
                }
            }
            Some(key)
        };

        for layer in &lay.layers {
            let Some(tex_path) = &layer.texture else { continue };
            let Some(tex_bytes) = resolve(&pkg, tex_path) else { continue };
            let Some(ltex) = upload_tex(device, queue, &tex_bytes) else { continue };
            let blend = Blend::from_we(&layer.blend);
            let Some(key) = ensure_prog(&mut programs, device, &pkg, &layer.material.shader, &layer.material.combos, blend) else { continue };

            // texturas extras do material (slots > 0)
            let prog = &programs[&key];
            let mut extra = Vec::new();
            for &b in &prog.tex_bindings {
                let n = (b - 1) / 2;
                if n == 0 { continue; }
                let name = layer.material.textures.get(n as usize).and_then(|o| o.clone())
                    .or_else(|| prog.sampler_defaults.iter().find(|(u, _)| *u == format!("g_Texture{n}")).map(|(_, d)| d.clone()));
                if let Some(name) = name {
                    if let Some(bytes) = resolve(&pkg, &format!("materials/{name}.tex")) {
                        if let Some(t) = upload_tex(device, queue, &bytes) {
                            let v = t.create_view(&Default::default());
                            extra.push((b, t, v));
                        }
                    }
                }
            }

            let constants: HashMap<String, Vec<f32>> = layer.material.constants.iter().map(|(k, v)| (k.clone(), value_to_floats(v))).collect();
            let tex_view = ltex.create_view(&Default::default());
            let verts = layer_quad(layer);
            let vbuf = device.create_buffer(&wgpu::BufferDescriptor { label: Some("vb"), size: 80, usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
            queue.write_buffer(&vbuf, 0, bytemuck::cast_slice(&verts));

            // efeitos por camada desligados por ora (ver render). Não compilamos os
            // shaders de efeito no startup (economiza dezenas de chamadas ao glslang).
            let _ = &layer.effects;
            let fx: Vec<EffectPassGpu> = Vec::new();

            layers.push(LayerGpu {
                key, _tex: ltex, tex_view, extra, vbuf,
                constants, brightness: layer.brightness, alpha: layer.alpha, color: layer.color,
                blend, effects: fx,
            });
        }

        let mk_scratch = |label| device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label), size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC, view_formats: &[],
        });

        // partículas da cena (em espaço de cena — usa a projeção, não a textura de
        // fundo; era esse o bug do "quadrado no meio").
        // DESLIGADO por ora (ver PARTICLES_ENABLED): nosso simulador é simples demais
        // pros sistemas do WE (múltiplos emissores, operadores, alpha/blend) e o
        // resultado vira "quadrados de bolhas" — pior que sem partícula. Religar quando
        // o sistema estiver fiel. O código fica gated pra reativar fácil.
        let particles = if PARTICLES_ENABLED {
            let mut inits: Vec<ParticleInit> = Vec::new();
            for sp in we_core::scene::particle_systems(&pkg) {
                if !particle_supported(&sp.system) {
                    continue;
                }
                let Some((rgba, sw, sh)) = load_sprite(&pkg, &sp.texture) else { continue };
                inits.push(ParticleInit { system: sp.system, additive: sp.additive, sprite_rgba: rgba, sprite_w: sw, sprite_h: sh, origin: sp.origin });
            }
            (!inits.is_empty()).then(|| Particles::new(device, queue, FORMAT, inits, [lay.width, lay.height]))
        } else {
            None
        };

        Some(Compositor {
            width: w, height: h, clear: lay.clear_color, mvp: ortho(lay.width, lay.height),
            programs, layers, ibuf, clamp, repeat,
            scratch_a: mk_scratch("scratch_a"), scratch_b: mk_scratch("scratch_b"),
            particles,
        })
    }

    /// Renderiza a cena inteira na textura `target` (deve ter o tamanho da cena e
    /// usage RENDER_ATTACHMENT). `time` alimenta g_Time; `dt` avança as partículas.
    pub fn render(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, time: f32, dt: f32, target: &wgpu::TextureView) {
        let mut first = true;
        for layer in &self.layers {
            // Desenha a camada DIRETO no target. (Efeitos por camada estão desligados
            // por ora: a maioria das cenas tem cadeias enormes e frágeis que, aplicadas
            // parcialmente, destroem a imagem — sai pior que sem efeito. O caminho de
            // efeitos existe em draw_effect/composite e será religado por-efeito quando
            // cada um estiver confiável.)
            let load = if first {
                wgpu::LoadOp::Clear(wgpu::Color { r: self.clear[0] as f64, g: self.clear[1] as f64, b: self.clear[2] as f64, a: 1.0 })
            } else {
                wgpu::LoadOp::Load
            };
            self.draw_layer(device, queue, layer, time, target, load, layer.blend);
            first = false;
        }

        // partículas por cima, em espaço de cena (avança a simulação com dt)
        if let Some(p) = self.particles.as_mut() {
            p.update(dt, queue);
        }
        if let Some(p) = self.particles.as_ref() {
            let mut enc = device.create_command_encoder(&Default::default());
            {
                let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("particles"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: target, resolve_target: None, depth_slice: None, ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store } })],
                    depth_stencil_attachment: None, timestamp_writes: None, occlusion_query_set: None, multiview_mask: None,
                });
                p.draw(&mut rp);
            }
            queue.submit(Some(enc.finish()));
        }
    }

    fn draw_layer(&self, device: &wgpu::Device, queue: &wgpu::Queue, layer: &LayerGpu, time: f32, target: &wgpu::TextureView, load: wgpu::LoadOp<wgpu::Color>, blend: Blend) {
        let prog = &self.programs[&layer.key];
        let ubo_bytes = prog.build_ubo(&self.mvp, self.width, self.height, time, layer.brightness, layer.alpha, layer.color, &layer.constants);
        let ubo = device.create_buffer(&wgpu::BufferDescriptor { label: Some("ubo"), size: prog.ubo_size, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        queue.write_buffer(&ubo, 0, &ubo_bytes);
        let _ = blend; // o pipeline já foi criado com o blend certo (via key)

        let mut entries: Vec<wgpu::BindGroupEntry> = vec![wgpu::BindGroupEntry { binding: 0, resource: ubo.as_entire_binding() }];
        for &b in &prog.tex_bindings {
            let n = (b - 1) / 2;
            let view = if n == 0 { &layer.tex_view } else {
                layer.extra.iter().find(|(bb, _, _)| *bb == b).map(|(_, _, v)| v).unwrap_or(&layer.tex_view)
            };
            entries.push(wgpu::BindGroupEntry { binding: b, resource: wgpu::BindingResource::TextureView(view) });
        }
        for &b in &prog.smp_bindings {
            let n = (b - 2) / 2;
            entries.push(wgpu::BindGroupEntry { binding: b, resource: wgpu::BindingResource::Sampler(if n == 0 { &self.clamp } else { &self.repeat }) });
        }
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout: &prog.bgl, entries: &entries });

        let mut enc = device.create_command_encoder(&Default::default());
        {
            let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("layer"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: target, resolve_target: None, depth_slice: None, ops: wgpu::Operations { load, store: wgpu::StoreOp::Store } })],
                depth_stencil_attachment: None, timestamp_writes: None, occlusion_query_set: None, multiview_mask: None,
            });
            rp.set_pipeline(&prog.pipeline);
            rp.set_bind_group(0, &bg, &[]);
            rp.set_vertex_buffer(0, layer.vbuf.slice(..));
            rp.set_index_buffer(self.ibuf.slice(..), wgpu::IndexFormat::Uint16);
            rp.draw_indexed(0..6, 0, 0..1);
        }
        queue.submit(Some(enc.finish()));
    }

    // desenha um efeito fullscreen amostrando `src` -> `dst`
    #[allow(dead_code)]
    fn draw_effect(&self, device: &wgpu::Device, queue: &wgpu::Queue, fx: &EffectPassGpu, time: f32, w: u32, h: u32, src: &wgpu::TextureView, dst: &wgpu::TextureView) {
        let prog = &self.programs[&fx.key];
        // quad fullscreen em espaço de cena (a cena inteira)
        let verts: [f32; 20] = [0.0, 0.0, 0.0, 0.0, 0.0,  w as f32, 0.0, 0.0, 1.0, 0.0,  w as f32, h as f32, 0.0, 1.0, 1.0,  0.0, h as f32, 0.0, 0.0, 1.0];
        let vbuf = device.create_buffer(&wgpu::BufferDescriptor { label: None, size: 80, usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        queue.write_buffer(&vbuf, 0, bytemuck::cast_slice(&verts));
        let ubo_bytes = prog.build_ubo(&self.mvp, w, h, time, 1.0, 1.0, [1.0, 1.0, 1.0], &fx.constants);
        let ubo = device.create_buffer(&wgpu::BufferDescriptor { label: None, size: prog.ubo_size, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        queue.write_buffer(&ubo, 0, &ubo_bytes);

        let mut entries: Vec<wgpu::BindGroupEntry> = vec![wgpu::BindGroupEntry { binding: 0, resource: ubo.as_entire_binding() }];
        for &b in &prog.tex_bindings {
            let n = (b - 1) / 2;
            let view = if n == 0 { src } else { fx.noise_view.as_ref().unwrap_or(src) };
            entries.push(wgpu::BindGroupEntry { binding: b, resource: wgpu::BindingResource::TextureView(view) });
        }
        for &b in &prog.smp_bindings {
            let n = (b - 2) / 2;
            entries.push(wgpu::BindGroupEntry { binding: b, resource: wgpu::BindingResource::Sampler(if n == 0 { &self.clamp } else { &self.repeat }) });
        }
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout: &prog.bgl, entries: &entries });
        let mut enc = device.create_command_encoder(&Default::default());
        {
            let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("effect"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: dst, resolve_target: None, depth_slice: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store } })],
                depth_stencil_attachment: None, timestamp_writes: None, occlusion_query_set: None, multiview_mask: None,
            });
            rp.set_pipeline(&prog.pipeline);
            rp.set_bind_group(0, &bg, &[]);
            rp.set_vertex_buffer(0, vbuf.slice(..));
            rp.set_index_buffer(self.ibuf.slice(..), wgpu::IndexFormat::Uint16);
            rp.draw_indexed(0..6, 0, 0..1);
        }
        queue.submit(Some(enc.finish()));
    }

    // compõe uma textura fullscreen no target, com blend (usa o programa de composição)
    #[allow(dead_code)]
    fn composite(&self, device: &wgpu::Device, queue: &wgpu::Queue, src: &wgpu::TextureView, target: &wgpu::TextureView, first: bool, blend: Blend) {
        let prog = self.programs.get(COMPOSITE_KEY).expect("composite program");
        let w = self.width;
        let h = self.height;
        let verts: [f32; 20] = [0.0, 0.0, 0.0, 0.0, 0.0,  w as f32, 0.0, 0.0, 1.0, 0.0,  w as f32, h as f32, 0.0, 1.0, 1.0,  0.0, h as f32, 0.0, 0.0, 1.0];
        let vbuf = device.create_buffer(&wgpu::BufferDescriptor { label: None, size: 80, usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        queue.write_buffer(&vbuf, 0, bytemuck::cast_slice(&verts));
        let _ = blend;
        let ubo_bytes = prog.build_ubo(&self.mvp, w, h, 0.0, 1.0, 1.0, [1.0, 1.0, 1.0], &HashMap::new());
        let ubo = device.create_buffer(&wgpu::BufferDescriptor { label: None, size: prog.ubo_size, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        queue.write_buffer(&ubo, 0, &ubo_bytes);
        let mut entries: Vec<wgpu::BindGroupEntry> = vec![wgpu::BindGroupEntry { binding: 0, resource: ubo.as_entire_binding() }];
        for &b in &prog.tex_bindings {
            entries.push(wgpu::BindGroupEntry { binding: b, resource: wgpu::BindingResource::TextureView(src) });
        }
        for &b in &prog.smp_bindings {
            entries.push(wgpu::BindGroupEntry { binding: b, resource: wgpu::BindingResource::Sampler(&self.clamp) });
        }
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout: &prog.bgl, entries: &entries });
        let load = if first { wgpu::LoadOp::Clear(wgpu::Color { r: self.clear[0] as f64, g: self.clear[1] as f64, b: self.clear[2] as f64, a: 1.0 }) } else { wgpu::LoadOp::Load };
        let mut enc = device.create_command_encoder(&Default::default());
        {
            let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("composite"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: target, resolve_target: None, depth_slice: None, ops: wgpu::Operations { load, store: wgpu::StoreOp::Store } })],
                depth_stencil_attachment: None, timestamp_writes: None, occlusion_query_set: None, multiview_mask: None,
            });
            rp.set_pipeline(&prog.pipeline);
            rp.set_bind_group(0, &bg, &[]);
            rp.set_vertex_buffer(0, vbuf.slice(..));
            rp.set_index_buffer(self.ibuf.slice(..), wgpu::IndexFormat::Uint16);
            rp.draw_indexed(0..6, 0, 0..1);
        }
        queue.submit(Some(enc.finish()));
    }
}

const COMPOSITE_KEY: &str = "__composite__";

// resolve os efeitos simples de uma camada em EffectPassGpu (compilando programas).
#[allow(dead_code)]
fn build_effects(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pkg: &Pkg,
    programs: &mut HashMap<String, Program>,
    instances: &[EffectInstance],
) -> Vec<EffectPassGpu> {
    let shaders_dir = PathBuf::from(BASE).join("shaders");
    // garante o programa de composição (passthrough alpha) — usa genericimage2.
    if !programs.contains_key(COMPOSITE_KEY) {
        if let (Some(v), Some(f)) = (
            resolve(pkg, "shaders/genericimage2.vert").and_then(|b| String::from_utf8(b).ok()),
            resolve(pkg, "shaders/genericimage2.frag").and_then(|b| String::from_utf8(b).ok()),
        ) {
            if let Ok(p) = Program::build(device, &v, &f, &[], Blend::Alpha, FORMAT, &shaders_dir) {
                programs.insert(COMPOSITE_KEY.to_string(), p);
            }
        }
    }

    let mut out = Vec::new();
    for inst in instances {
        if !inst.visible {
            continue;
        }
        let Some(def) = resolve(pkg, &inst.file).and_then(|b| String::from_utf8(b).ok()).and_then(|s| effects::parse_effect(&s)) else { continue };
        if !effects::is_simple(&def) {
            continue; // multi-passe/FBO fica pra depois
        }
        let epass = &def.passes[0];
        let Some(mat_json) = resolve(pkg, &epass.material).and_then(|b| String::from_utf8(b).ok()) else { continue };
        let Some(info) = we_core::scene::material_info_str(&mat_json) else { continue };
        let mut combos = info.combos.clone();
        let mut constants: HashMap<String, Vec<f32>> = info.constants.iter().map(|(k, v)| (k.clone(), value_to_floats(v))).collect();
        if let Some(ov) = inst.passes.first() {
            for (k, val) in &ov.combos {
                combos.retain(|(ck, _)| ck != k);
                combos.push((k.clone(), *val));
            }
            for (k, v) in &ov.constants {
                constants.insert(k.clone(), value_to_floats(v));
            }
        }
        let key = format!("{}|{combos:?}|fx", info.shader);
        if !programs.contains_key(&key) {
            let (Some(v), Some(f)) = (
                resolve(pkg, &format!("shaders/{}.vert", info.shader)).and_then(|b| String::from_utf8(b).ok()),
                resolve(pkg, &format!("shaders/{}.frag", info.shader)).and_then(|b| String::from_utf8(b).ok()),
            ) else { continue };
            match Program::build(device, &v, &f, &combos, Blend::Opaque, FORMAT, &shaders_dir) {
                Ok(p) => { programs.insert(key.clone(), p); }
                Err(_) => continue,
            }
        }
        // textura de ruído (slot >0) do efeito
        let prog = &programs[&key];
        let mut noise = None;
        let mut noise_view = None;
        for &b in &prog.tex_bindings {
            let n = (b - 1) / 2;
            if n == 0 { continue; }
            if let Some(name) = prog.sampler_defaults.iter().find(|(u, _)| *u == format!("g_Texture{n}")).map(|(_, d)| d.clone()) {
                if let Some(bytes) = resolve(pkg, &format!("materials/{name}.tex")) {
                    if let Some(t) = upload_tex(device, queue, &bytes) {
                        noise_view = Some(t.create_view(&Default::default()));
                        noise = Some(t);
                    }
                }
            }
        }
        out.push(EffectPassGpu { key, constants, noise, noise_view });
    }
    out
}
