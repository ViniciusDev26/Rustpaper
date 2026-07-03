// Compositor OFFSCREEN: renderiza a cena INTEIRA como pilha de camadas (não só o
// fundo), cada uma pelo shader real do WE, com transform (ortho da cena), blend e
// alpha — em espaço sRGB. É o que faz o personagem principal (Ashe, etc.) aparecer.
//
// Uso: cargo run -p rustpaper-engine --example render_composite -- <pasta> [saida.png] [tempo]

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rustpaper_engine::program::{ortho, Blend, Program};
use rustpaper_core::layout::{self, Layer};
use rustpaper_core::pkg::Pkg;
use rustpaper_core::tex;

const BASE: &str = "/home/vscode/we-assets";

fn align256(n: u32) -> u32 {
    (n + 255) & !255
}
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

// carrega uma textura .tex (recortada ao conteúdo) como Rgba8UnormSrgb
fn load_texture(device: &wgpu::Device, queue: &wgpu::Queue, bytes: &[u8]) -> Option<(wgpu::Texture, u32, u32)> {
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
        dimension: wgpu::TextureDimension::D2, format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST, view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo { texture: &t, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        &content,
        wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4 * cw), rows_per_image: Some(ch) },
        ext,
    );
    Some((t, cw, ch))
}

// quad da camada em espaço de cena (origin centro, ±size/2 com scale + rotação Z).
fn layer_quad(l: &Layer) -> [f32; 20] {
    let (hx, hy) = (l.size[0] * 0.5 * l.scale[0], l.size[1] * 0.5 * l.scale[1]);
    let a = l.angles[2]; // rotação em torno de Z (radianos)
    let (c, s) = (a.cos(), a.sin());
    let corner = |lx: f32, ly: f32| -> (f32, f32) {
        (l.origin[0] + lx * c - ly * s, l.origin[1] + lx * s + ly * c)
    };
    let (tlx, tly) = corner(-hx, -hy);
    let (trx, tr_y) = corner(hx, -hy);
    let (brx, bry) = corner(hx, hy);
    let (blx, bly) = corner(-hx, hy);
    #[rustfmt::skip]
    let v = [
        tlx, tly, 0.0, 0.0, 0.0,
        trx, tr_y, 0.0, 1.0, 0.0,
        brx, bry, 0.0, 1.0, 1.0,
        blx, bly, 0.0, 0.0, 1.0,
    ];
    v
}

fn main() {
    let dir = std::env::args().nth(1).expect("uso: render_composite <pasta> [saida.png] [tempo]");
    let out_png = std::env::args().nth(2).unwrap_or_else(|| "/tmp/composite.png".into());
    let time: f32 = std::env::args().nth(3).and_then(|s| s.parse().ok()).unwrap_or(0.0);
    let shaders_dir = PathBuf::from(BASE).join("shaders");

    let pkg = Pkg::open(Path::new(&dir).join("scene.pkg").as_path()).expect("scene.pkg");
    let layout = layout::parse_layout(&pkg).expect("layout da cena");
    let (w, h) = (layout.width as u32, layout.height as u32);
    println!("cena {w}x{h}, {} camadas", layout.layers.len());

    let instance = wgpu::Instance::default();
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance, force_fallback_adapter: false, compatible_surface: None,
    })).expect("adapter");
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).unwrap();
    let fmt = wgpu::TextureFormat::Rgba8UnormSrgb;

    let clamp = device.create_sampler(&wgpu::SamplerDescriptor { mag_filter: wgpu::FilterMode::Linear, min_filter: wgpu::FilterMode::Linear, ..Default::default() });
    let repeat = device.create_sampler(&wgpu::SamplerDescriptor { address_mode_u: wgpu::AddressMode::Repeat, address_mode_v: wgpu::AddressMode::Repeat, mag_filter: wgpu::FilterMode::Linear, min_filter: wgpu::FilterMode::Linear, ..Default::default() });
    let white = {
        let t = device.create_texture(&wgpu::TextureDescriptor { label: None, size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 }, mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: fmt, usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST, view_formats: &[] });
        queue.write_texture(wgpu::TexelCopyTextureInfo { texture: &t, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All }, &[255, 255, 255, 255], wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4), rows_per_image: Some(1) }, wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 });
        t.create_view(&Default::default())
    };

    // alvo de acumulação
    let acc = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("acc"), size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: fmt,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::TEXTURE_BINDING, view_formats: &[],
    });
    let acc_view = acc.create_view(&Default::default());
    let mvp = ortho(layout.width, layout.height);

    // cache de programas por (shader, combos, blend)
    let mut programs: HashMap<String, Program> = HashMap::new();
    let idx: [u16; 6] = [0, 1, 2, 0, 2, 3];
    let ibuf = device.create_buffer(&wgpu::BufferDescriptor { label: Some("ib"), size: 12, usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
    queue.write_buffer(&ibuf, 0, bytemuck::cast_slice(&idx));

    let mut first = true;
    let mut drawn = 0;
    for layer in &layout.layers {
        let Some(tex_path) = &layer.texture else { continue };
        let Some(tex_bytes) = resolve(&pkg, tex_path) else { continue };
        let Some((ltex, _tw, _th)) = load_texture(&device, &queue, &tex_bytes) else { continue };
        let ltex_view = ltex.create_view(&Default::default());

        let blend = Blend::from_we(&layer.blend);
        let key = format!("{}|{:?}|{}", layer.material.shader, layer.material.combos, layer.blend);
        if !programs.contains_key(&key) {
            let vsrc = resolve(&pkg, &format!("shaders/{}.vert", layer.material.shader)).and_then(|b| String::from_utf8(b).ok());
            let fsrc = resolve(&pkg, &format!("shaders/{}.frag", layer.material.shader)).and_then(|b| String::from_utf8(b).ok());
            let (Some(vsrc), Some(fsrc)) = (vsrc, fsrc) else {
                println!("  (pulando '{}': shader {} não encontrado)", layer.name, layer.material.shader);
                continue;
            };
            match Program::build(&device, &vsrc, &fsrc, &layer.material.combos, blend, fmt, &shaders_dir) {
                Ok(p) => { programs.insert(key.clone(), p); }
                Err(e) => { println!("  (pulando '{}': {})", layer.name, e.lines().next().unwrap_or("")); continue; }
            }
        }
        let prog = &programs[&key];

        // constants do material
        let constants: HashMap<String, Vec<f32>> = layer.material.constants.iter().map(|(k, v)| (k.clone(), value_to_floats(v))).collect();
        let ubo_bytes = prog.build_ubo(&mvp, w, h, time, layer.brightness, layer.alpha, layer.color, &constants);
        let ubo = device.create_buffer(&wgpu::BufferDescriptor { label: Some("ubo"), size: prog.ubo_size, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        queue.write_buffer(&ubo, 0, &ubo_bytes);

        // vertex buffer do quad
        let verts = layer_quad(layer);
        let vbuf = device.create_buffer(&wgpu::BufferDescriptor { label: Some("vb"), size: std::mem::size_of_val(&verts) as u64, usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        queue.write_buffer(&vbuf, 0, bytemuck::cast_slice(&verts));

        // texturas extras (slot 0 = a textura da camada)
        let mut extra_texs: Vec<wgpu::Texture> = Vec::new();
        let mut extra_views: HashMap<u32, wgpu::TextureView> = HashMap::new();
        for &b in &prog.tex_bindings {
            let n = (b - 1) / 2;
            if n == 0 { continue; }
            let name = layer.material.textures.get(n as usize).and_then(|o| o.clone())
                .or_else(|| prog.sampler_defaults.iter().find(|(u, _)| *u == format!("g_Texture{n}")).map(|(_, d)| d.clone()));
            if let Some(name) = name {
                if let Some(bytes) = resolve(&pkg, &format!("materials/{name}.tex")) {
                    if let Some((t, _, _)) = load_texture(&device, &queue, &bytes) {
                        let v = t.create_view(&Default::default());
                        extra_texs.push(t);
                        extra_views.insert(b, v);
                    }
                }
            }
        }
        let _keep = &extra_texs;

        let mut entries: Vec<wgpu::BindGroupEntry> = vec![wgpu::BindGroupEntry { binding: 0, resource: ubo.as_entire_binding() }];
        for &b in &prog.tex_bindings {
            let n = (b - 1) / 2;
            let view = if n == 0 { &ltex_view } else { extra_views.get(&b).unwrap_or(&white) };
            entries.push(wgpu::BindGroupEntry { binding: b, resource: wgpu::BindingResource::TextureView(view) });
        }
        for &b in &prog.smp_bindings {
            let n = (b - 2) / 2;
            let smp = if n == 0 { &clamp } else { &repeat };
            entries.push(wgpu::BindGroupEntry { binding: b, resource: wgpu::BindingResource::Sampler(smp) });
        }
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("bg"), layout: &prog.bgl, entries: &entries });

        let mut enc = device.create_command_encoder(&Default::default());
        {
            let load = if first {
                wgpu::LoadOp::Clear(wgpu::Color { r: layout.clear_color[0] as f64, g: layout.clear_color[1] as f64, b: layout.clear_color[2] as f64, a: 1.0 })
            } else {
                wgpu::LoadOp::Load
            };
            let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("layer"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &acc_view, resolve_target: None, depth_slice: None, ops: wgpu::Operations { load, store: wgpu::StoreOp::Store } })],
                depth_stencil_attachment: None, timestamp_writes: None, occlusion_query_set: None, multiview_mask: None,
            });
            rp.set_pipeline(&prog.pipeline);
            rp.set_bind_group(0, &bind_group, &[]);
            rp.set_vertex_buffer(0, vbuf.slice(..));
            rp.set_index_buffer(ibuf.slice(..), wgpu::IndexFormat::Uint16);
            rp.draw_indexed(0..6, 0, 0..1);
        }
        queue.submit(Some(enc.finish()));
        first = false;
        drawn += 1;
    }
    println!("{drawn} camadas desenhadas");

    // readback
    let bpr = align256(4 * w);
    let rb = device.create_buffer(&wgpu::BufferDescriptor { label: Some("rb"), size: (bpr * h) as u64, usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
    let mut enc = device.create_command_encoder(&Default::default());
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo { texture: &acc, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        wgpu::TexelCopyBufferInfo { buffer: &rb, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(bpr), rows_per_image: Some(h) } },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
    );
    queue.submit(Some(enc.finish()));
    let slice = rb.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    let data = slice.get_mapped_range();
    let mut pixels = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        let s = (y * bpr) as usize;
        let d = (y * w * 4) as usize;
        pixels[d..d + (w * 4) as usize].copy_from_slice(&data[s..s + (w * 4) as usize]);
    }
    image::save_buffer(&out_png, &pixels, w, h, image::ColorType::Rgba8).unwrap();
    println!("PNG salvo em {out_png} ({w}x{h})");
}
