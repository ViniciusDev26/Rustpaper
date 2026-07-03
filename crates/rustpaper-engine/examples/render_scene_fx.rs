// Effect runner OFFSCREEN: renderiza o fundo de uma cena e depois a cadeia de
// efeitos SIMPLES (1 pass, sem FBO extra) usando os shaders REAIS do WE
// (vertex+fragment LINKADOS, UBO unificado). Salva PNG.
//
//   fundo (genericimage2) -> texA
//   cada efeito simples (filmgrain, ...) amostra o quadro anterior -> próximo tex
//
// Monta o UBO com defaults das anotações + constants (material/cena) + builtins
// (MVP identidade, g_Time, g_TextureNResolution) e resolve texturas (quadro anterior
// no slot 0; texturas nomeadas tipo util/noise nos outros slots).
//
// Uso: cargo run -p rustpaper-engine --example render_scene_fx -- <pasta-do-wallpaper> [saida.png] [tempo]

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rustpaper_engine::shader_compile as sc;
use rustpaper_core::pkg::Pkg;
use rustpaper_core::shader::{self, UniformParam};
use rustpaper_core::{effects, scene, tex};

const BASE: &str = "/home/vscode/we-assets";

fn align256(n: u32) -> u32 {
    (n + 255) & !255
}

// Lê um arquivo do pkg; se não achar, cai no diretório base dos assets.
fn resolve(pkg: &Pkg, path: &str) -> Option<Vec<u8>> {
    if let Some(b) = pkg.read(path) {
        return Some(b.to_vec());
    }
    std::fs::read(PathBuf::from(BASE).join(path)).ok()
}

// Constante do material/cena -> componentes float. Número ou string "a b c".
fn value_to_floats(v: &serde_json::Value) -> Vec<f32> {
    match v {
        serde_json::Value::Number(n) => vec![n.as_f64().unwrap_or(0.0) as f32],
        serde_json::Value::String(s) => s.split_whitespace().filter_map(|t| t.parse().ok()).collect(),
        _ => Vec::new(),
    }
}

struct Program {
    pipeline: wgpu::RenderPipeline,
    bgl: wgpu::BindGroupLayout,
    ubo_size: u64,
    offsets: HashMap<String, u32>,
    params: Vec<UniformParam>,
    sampler_defaults: Vec<(String, String)>,
    tex_bindings: Vec<u32>,
    smp_bindings: Vec<u32>,
}

fn build_program(
    device: &wgpu::Device,
    pkg: &Pkg,
    shaders_dir: &Path,
    shader_name: &str,
    combos: &[(String, i64)],
    fmt: wgpu::TextureFormat,
) -> Program {
    let vert_src = String::from_utf8(resolve(pkg, &format!("shaders/{shader_name}.vert")).expect("ler .vert")).unwrap();
    let frag_src = String::from_utf8(resolve(pkg, &format!("shaders/{shader_name}.frag")).expect("ler .frag")).unwrap();

    // combos efetivos: defaults das anotações // [COMBO], sobrescritos por material/cena.
    let mut eff_combos = shader::parse_combo_defaults(&vert_src);
    eff_combos.extend(shader::parse_combo_defaults(&frag_src));
    for (k, v) in combos {
        eff_combos.retain(|(ck, _)| ck != k);
        eff_combos.push((k.clone(), *v));
    }

    let (v_spv, f_spv) = sc::compile_linked(&vert_src, &frag_src, &eff_combos, shaders_dir).expect("compile_linked");
    let refl = sc::reflect(&f_spv).expect("reflect frag");
    let vinputs = sc::reflect_vertex_inputs(&v_spv).expect("reflect vert inputs");

    let mut params = shader::parse_params(&vert_src);
    params.extend(shader::parse_params(&frag_src));
    let mut sampler_defaults = shader::parse_sampler_defaults(&vert_src);
    sampler_defaults.extend(shader::parse_sampler_defaults(&frag_src));

    let vs = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("we-vert"),
        source: wgpu::ShaderSource::SpirV(v_spv.into()),
    });
    let fs = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("we-frag"),
        source: wgpu::ShaderSource::SpirV(f_spv.into()),
    });

    // layout do vertex buffer: casa pela dimensão (vec3 = a_Position@0, vec2 = a_TexCoord@12)
    let mut attrs = Vec::new();
    for vi in &vinputs {
        let (offset, format) = match vi.components {
            3 => (0u64, wgpu::VertexFormat::Float32x3),
            2 => (12u64, wgpu::VertexFormat::Float32x2),
            _ => continue,
        };
        attrs.push(wgpu::VertexAttribute { format, offset, shader_location: vi.location });
    }
    let vbl = wgpu::VertexBufferLayout {
        array_stride: 20, // vec3 (12) + vec2 (8)
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &attrs,
    };

    // bind group layout: UBO@0 (vertex+fragment), e cada textura+sampler (fragment)
    let mut entries = vec![wgpu::BindGroupLayoutEntry {
        binding: 0,
        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
        ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
        count: None,
    }];
    for &b in &refl.texture_bindings {
        entries.push(wgpu::BindGroupLayoutEntry {
            binding: b,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
            count: None,
        });
    }
    for &b in &refl.sampler_bindings {
        entries.push(wgpu::BindGroupLayoutEntry {
            binding: b,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        });
    }
    let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("prog-bgl"), entries: &entries });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: None, bind_group_layouts: &[Some(&bgl)], immediate_size: 0 });

    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("prog"),
        layout: Some(&layout),
        vertex: wgpu::VertexState { module: &vs, entry_point: Some("main"), compilation_options: Default::default(), buffers: &[vbl] },
        fragment: Some(wgpu::FragmentState { module: &fs, entry_point: Some("main"), compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState { format: fmt, blend: None, write_mask: wgpu::ColorWrites::ALL })] }),
        primitive: Default::default(), depth_stencil: None, multisample: Default::default(), multiview_mask: None, cache: None,
    });

    Program {
        pipeline, bgl,
        ubo_size: refl.uniform_size.max(16).next_multiple_of(16) as u64,
        offsets: refl.uniform_offsets,
        params,
        sampler_defaults,
        tex_bindings: refl.texture_bindings,
        smp_bindings: refl.sampler_bindings,
    }
}

impl Program {
    // Monta os bytes do UBO: builtins + params (default ou constant do material/cena).
    fn build_ubo(&self, w: u32, h: u32, time: f32, constants: &HashMap<String, Vec<f32>>) -> Vec<u8> {
        let mut buf = vec![0u8; self.ubo_size as usize];
        let write = |buf: &mut [u8], off: u32, vals: &[f32]| {
            for (i, v) in vals.iter().enumerate() {
                let p = off as usize + i * 4;
                if p + 4 <= buf.len() {
                    buf[p..p + 4].copy_from_slice(&v.to_le_bytes());
                }
            }
        };
        // builtins
        if let Some(&o) = self.offsets.get("g_ModelViewProjectionMatrix") {
            let ident = [1.0f32, 0., 0., 0.,  0., 1., 0., 0.,  0., 0., 1., 0.,  0., 0., 0., 1.];
            write(&mut buf, o, &ident);
        }
        if let Some(&o) = self.offsets.get("g_Time") {
            write(&mut buf, o, &[time]);
        }
        for res in ["g_Texture0Resolution", "g_Texture1Resolution", "g_Texture2Resolution"] {
            if let Some(&o) = self.offsets.get(res) {
                write(&mut buf, o, &[w as f32, h as f32, w as f32, h as f32]);
            }
        }
        // params: default, sobrescrito por constant (material/cena) via nome "material"
        for p in &self.params {
            let Some(&o) = self.offsets.get(&p.uniform) else { continue };
            let vals = p
                .material
                .as_ref()
                .and_then(|m| constants.get(m))
                .cloned()
                .unwrap_or_else(|| p.default.clone());
            let mut vals = vals;
            vals.resize(p.components, 0.0);
            write(&mut buf, o, &vals);
        }
        buf
    }
}

// carrega uma textura resolvida (name -> materials/<name>.tex), recortada ao conteúdo
fn load_named_texture(device: &wgpu::Device, queue: &wgpu::Queue, pkg: &Pkg, name: &str) -> Option<wgpu::Texture> {
    let bytes = resolve(pkg, &format!("materials/{name}.tex"))?;
    let d = tex::parse(&bytes).ok()?;
    let (cw, ch) = (d.real_width, d.real_height);
    let mut content = vec![0u8; (cw * ch * 4) as usize];
    for y in 0..ch {
        let s = (y * d.width * 4) as usize;
        let dd = (y * cw * 4) as usize;
        content[dd..dd + (cw * 4) as usize].copy_from_slice(&d.rgba[s..s + (cw * 4) as usize]);
    }
    Some(upload(device, queue, &content, cw, ch))
}

fn upload(device: &wgpu::Device, queue: &wgpu::Queue, rgba: &[u8], w: u32, h: u32) -> wgpu::Texture {
    let ext = wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 };
    let t = device.create_texture(&wgpu::TextureDescriptor {
        label: None, size: ext, mip_level_count: 1, sample_count: 1,
        dimension: wgpu::TextureDimension::D2, format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST, view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo { texture: &t, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        rgba,
        wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4 * w), rows_per_image: Some(h) },
        ext,
    );
    t
}

fn target(device: &wgpu::Device, w: u32, h: u32) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}

fn main() {
    let dir = std::env::args().nth(1).expect("uso: render_scene_fx <pasta> [saida.png] [tempo]");
    let out_png = std::env::args().nth(2).unwrap_or_else(|| "/tmp/render_scene_fx.png".into());
    let time: f32 = std::env::args().nth(3).and_then(|s| s.parse().ok()).unwrap_or(0.0);
    let shaders_dir = PathBuf::from(BASE).join("shaders");

    let pkg = Pkg::open(Path::new(&dir).join("scene.pkg").as_path()).expect("scene.pkg");
    let scene_json = std::str::from_utf8(pkg.read("scene.json").expect("scene.json")).unwrap().to_string();
    let bg_mat = scene::background_material(&pkg).expect("material de fundo");
    let bg_tex_path = scene::background_texture(&pkg).expect("textura de fundo");
    let bg_decoded = tex::parse(pkg.read(&bg_tex_path).expect("bg .tex")).expect("decode bg");
    let (cw, ch) = (bg_decoded.real_width, bg_decoded.real_height);

    let instance = wgpu::Instance::default();
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance, force_fallback_adapter: false, compatible_surface: None,
    })).expect("adapter");
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).unwrap();
    let fmt = wgpu::TextureFormat::Rgba8Unorm;

    // quad fullscreen: pos vec3 + texcoord vec2 (uv com flip Y). 4 verts + 6 índices.
    #[rustfmt::skip]
    let verts: [f32; 20] = [
        -1.0,-1.0,0.0,  0.0,1.0,
         1.0,-1.0,0.0,  1.0,1.0,
         1.0, 1.0,0.0,  1.0,0.0,
        -1.0, 1.0,0.0,  0.0,0.0,
    ];
    let idx: [u16; 6] = [0, 1, 2, 0, 2, 3];
    let vbuf = device.create_buffer(&wgpu::BufferDescriptor { label: Some("vb"), size: std::mem::size_of_val(&verts) as u64, usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
    queue.write_buffer(&vbuf, 0, bytemuck::cast_slice(&verts));
    let ibuf = device.create_buffer(&wgpu::BufferDescriptor { label: Some("ib"), size: std::mem::size_of_val(&idx) as u64, usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
    queue.write_buffer(&ibuf, 0, bytemuck::cast_slice(&idx));

    let clamp = device.create_sampler(&wgpu::SamplerDescriptor { mag_filter: wgpu::FilterMode::Linear, min_filter: wgpu::FilterMode::Linear, ..Default::default() });
    let repeat = device.create_sampler(&wgpu::SamplerDescriptor {
        address_mode_u: wgpu::AddressMode::Repeat, address_mode_v: wgpu::AddressMode::Repeat,
        mag_filter: wgpu::FilterMode::Linear, min_filter: wgpu::FilterMode::Linear, ..Default::default()
    });

    // input inicial = textura de fundo recortada
    let mut bg_content = vec![0u8; (cw * ch * 4) as usize];
    for y in 0..ch {
        let s = (y * bg_decoded.width * 4) as usize;
        let d = (y * cw * 4) as usize;
        bg_content[d..d + (cw * 4) as usize].copy_from_slice(&bg_decoded.rgba[s..s + (cw * 4) as usize]);
    }
    let mut current = upload(&device, &queue, &bg_content, cw, ch);

    // ---- lista de passes: (shader_name, combos, constants, material_textures) ----
    struct PassSpec {
        shader: String,
        combos: Vec<(String, i64)>,
        constants: HashMap<String, Vec<f32>>,
        textures: Vec<Option<String>>,
    }
    let mut passes: Vec<PassSpec> = Vec::new();

    // pass do fundo
    let mut bg_consts = HashMap::new();
    for (k, v) in &bg_mat.constants {
        bg_consts.insert(k.clone(), value_to_floats(v));
    }
    passes.push(PassSpec { shader: bg_mat.shader.clone(), combos: bg_mat.combos.clone(), constants: bg_consts, textures: bg_mat.textures.clone() });

    // passes dos efeitos simples
    for eff in effects::background_effects(&scene_json) {
        if !eff.visible {
            continue;
        }
        let Some(def) = resolve(&pkg, &eff.file).and_then(|b| String::from_utf8(b).ok()).and_then(|s| effects::parse_effect(&s)) else { continue };
        if !effects::is_simple(&def) {
            println!("(pulando efeito complexo: {})", eff.file);
            continue;
        }
        let epass = &def.passes[0];
        let Some(mat_json) = resolve(&pkg, &epass.material).and_then(|b| String::from_utf8(b).ok()) else { continue };
        let Some(info) = rustpaper_core::scene::material_info_str(&mat_json) else { continue };
        // combos: material + override do primeiro override da cena
        let mut combos = info.combos.clone();
        let mut constants: HashMap<String, Vec<f32>> = info.constants.iter().map(|(k, v)| (k.clone(), value_to_floats(v))).collect();
        if let Some(ov) = eff.passes.first() {
            for (k, val) in &ov.combos {
                combos.retain(|(ck, _)| ck != k);
                combos.push((k.clone(), *val));
            }
            for (k, v) in &ov.constants {
                constants.insert(k.clone(), value_to_floats(v));
            }
        }
        println!("efeito {}: shader={} combos={:?}", eff.file, info.shader, combos);
        passes.push(PassSpec { shader: info.shader, combos, constants, textures: info.textures });
    }

    // ---- roda os passes ----
    for (i, spec) in passes.iter().enumerate() {
        let prog = build_program(&device, &pkg, &shaders_dir, &spec.shader, &spec.combos, fmt);
        let ubo_bytes = prog.build_ubo(cw, ch, time, &spec.constants);
        let ubo = device.create_buffer(&wgpu::BufferDescriptor { label: Some("ubo"), size: prog.ubo_size, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        queue.write_buffer(&ubo, 0, &ubo_bytes);

        let input_view = current.create_view(&Default::default());
        // resolve texturas por slot; guarda as texturas vivas
        let mut extra_texs: Vec<wgpu::Texture> = Vec::new();
        let mut extra_views: HashMap<u32, wgpu::TextureView> = HashMap::new();
        for &b in &prog.tex_bindings {
            let n = (b - 1) / 2; // g_TextureN
            if n == 0 {
                continue; // slot 0 = input (view próprio)
            }
            // nome: material.textures[n] ou default do sampler
            let name = spec.textures.get(n as usize).and_then(|o| o.clone())
                .or_else(|| prog.sampler_defaults.iter().find(|(u, _)| *u == format!("g_Texture{n}")).map(|(_, d)| d.clone()));
            if let Some(name) = name {
                if let Some(t) = load_named_texture(&device, &queue, &pkg, &name) {
                    let v = t.create_view(&Default::default());
                    extra_texs.push(t);
                    extra_views.insert(b, v);
                }
            }
        }
        // dummy 1x1 branco pra slots não resolvidos
        let white = upload(&device, &queue, &[255, 255, 255, 255], 1, 1);
        let white_view = white.create_view(&Default::default());

        let mut entries: Vec<wgpu::BindGroupEntry> = vec![wgpu::BindGroupEntry { binding: 0, resource: ubo.as_entire_binding() }];
        for &b in &prog.tex_bindings {
            let n = (b - 1) / 2;
            let view = if n == 0 { &input_view } else { extra_views.get(&b).unwrap_or(&white_view) };
            entries.push(wgpu::BindGroupEntry { binding: b, resource: wgpu::BindingResource::TextureView(view) });
        }
        for &b in &prog.smp_bindings {
            let n = (b - 2) / 2;
            let smp = if n == 0 { &clamp } else { &repeat };
            entries.push(wgpu::BindGroupEntry { binding: b, resource: wgpu::BindingResource::Sampler(smp) });
        }
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("bg"), layout: &prog.bgl, entries: &entries });

        let out = target(&device, cw, ch);
        let out_view = out.create_view(&Default::default());
        let mut enc = device.create_command_encoder(&Default::default());
        {
            let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &out_view, resolve_target: None, depth_slice: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store } })],
                depth_stencil_attachment: None, timestamp_writes: None, occlusion_query_set: None, multiview_mask: None,
            });
            rp.set_pipeline(&prog.pipeline);
            rp.set_bind_group(0, &bind_group, &[]);
            rp.set_vertex_buffer(0, vbuf.slice(..));
            rp.set_index_buffer(ibuf.slice(..), wgpu::IndexFormat::Uint16);
            rp.draw_indexed(0..6, 0, 0..1);
        }
        queue.submit(Some(enc.finish()));
        println!("pass {i} ({}) ok", spec.shader);
        current = out;
    }

    // readback -> PNG
    let bpr = align256(4 * cw);
    let rb = device.create_buffer(&wgpu::BufferDescriptor { label: Some("rb"), size: (bpr * ch) as u64, usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
    let mut enc = device.create_command_encoder(&Default::default());
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo { texture: &current, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        wgpu::TexelCopyBufferInfo { buffer: &rb, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(bpr), rows_per_image: Some(ch) } },
        wgpu::Extent3d { width: cw, height: ch, depth_or_array_layers: 1 },
    );
    queue.submit(Some(enc.finish()));
    let slice = rb.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    let data = slice.get_mapped_range();
    let mut pixels = vec![0u8; (cw * ch * 4) as usize];
    for y in 0..ch {
        let s = (y * bpr) as usize;
        let d = (y * cw * 4) as usize;
        pixels[d..d + (cw * 4) as usize].copy_from_slice(&data[s..s + (cw * 4) as usize]);
    }
    image::save_buffer(&out_png, &pixels, cw, ch, image::ColorType::Rgba8).unwrap();
    println!("PNG salvo em {out_png} ({cw}x{ch}) — fundo + {} efeito(s)", passes.len() - 1);
}
