// Render OFFSCREEN de uma CENA REAL: carrega o material e a textura de fundo de um
// wallpaper do Workshop, compila o fragment shader REAL do WE (via glslang) e
// renderiza a imagem de fundo num PNG — pra eu verificar que o caminho
// parse -> decode .tex -> translate -> compile -> reflect -> render funciona em
// assets de verdade.
//
// Usa nosso vertex fullscreen (WGSL) + o FRAGMENT do WE. Isso vale pros shaders de
// fundo cujo frag só precisa de v_TexCoord (ex.: genericimage2).
//
// Uso: cargo run -p engine --example render_scene -- <pasta-do-wallpaper> [saida.png]

use std::path::Path;

use engine::shader_compile;
use we_core::pkg::Pkg;
use we_core::scene;
use we_core::shader::Stage;
use we_core::tex;

const VERTEX_WGSL: &str = r#"
struct VsOut { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32> };
@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> VsOut {
    var p = array<vec2<f32>, 3>(vec2(-1.0, -1.0), vec2(3.0, -1.0), vec2(-1.0, 3.0));
    let xy = p[i];
    var o: VsOut;
    o.pos = vec4(xy, 0.0, 1.0);
    o.uv = vec2((xy.x + 1.0) * 0.5, 1.0 - (xy.y + 1.0) * 0.5);
    return o;
}
"#;

fn align256(n: u32) -> u32 {
    (n + 255) & !255
}

fn main() {
    let dir = std::env::args().nth(1).expect("uso: render_scene <pasta-do-wallpaper> [saida.png]");
    let out_png = std::env::args().nth(2).unwrap_or_else(|| "/tmp/render_scene.png".into());
    let shaders_dir = std::env::var("WE_SHADERS_DIR").unwrap_or_else(|_| "/home/vscode/we-assets/shaders".into());

    // --- material + textura de fundo ---
    let pkg = Pkg::open(Path::new(&dir).join("scene.pkg").as_path()).expect("abrir scene.pkg");
    let material = scene::background_material(&pkg).expect("material de fundo");
    let tex_path = scene::background_texture(&pkg).expect("textura de fundo");
    println!("shader={} combos={:?} tex={}", material.shader, material.combos, tex_path);

    let tex_bytes = pkg.read(&tex_path).expect("ler .tex do pkg");
    let decoded = tex::parse(tex_bytes).expect("decodificar .tex");
    println!("tex buffer={}x{} conteúdo={}x{}", decoded.width, decoded.height, decoded.real_width, decoded.real_height);

    // recorta o conteúdo (ignora o padding pra potência de 2 do buffer)
    let (cw, ch) = (decoded.real_width, decoded.real_height);
    let mut content = vec![0u8; (cw * ch * 4) as usize];
    for y in 0..ch {
        let src = (y * decoded.width * 4) as usize;
        let dst = (y * cw * 4) as usize;
        content[dst..dst + (cw * 4) as usize].copy_from_slice(&tex_bytes_row(&decoded.rgba, src, (cw * 4) as usize));
    }

    // --- compila o fragment do WE + reflete o layout ---
    let frag_src = std::fs::read_to_string(format!("{shaders_dir}/{}.frag", material.shader)).expect("ler .frag");
    let frag_spirv = shader_compile::compile(Stage::Fragment, &frag_src, &material.combos, Path::new(&shaders_dir))
        .expect("compilar frag");
    let refl = shader_compile::reflect(&frag_spirv).expect("refletir");
    println!("reflection: ubo_size={} membros={:?} tex_bindings={:?} smp_bindings={:?}",
        refl.uniform_size, refl.uniform_offsets, refl.texture_bindings, refl.sampler_bindings);

    // --- wgpu headless ---
    let instance = wgpu::Instance::default();
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        force_fallback_adapter: false,
        compatible_surface: None,
    })).expect("adapter");
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).unwrap();

    // textura de entrada (conteúdo recortado)
    let extent = wgpu::Extent3d { width: cw, height: ch, depth_or_array_layers: 1 };
    let in_tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("bg"), size: extent, mip_level_count: 1, sample_count: 1,
        dimension: wgpu::TextureDimension::D2, format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST, view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo { texture: &in_tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        &content,
        wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4 * cw), rows_per_image: Some(ch) },
        extent,
    );
    let in_view = in_tex.create_view(&Default::default());
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        mag_filter: wgpu::FilterMode::Linear, min_filter: wgpu::FilterMode::Linear, ..Default::default()
    });

    // UBO: aloca o tamanho refletido (min 16) e seta brilho/alpha = 1 (defaults).
    let ubo_size = align256(refl.uniform_size.max(16)) as u64; // 256 evita qualquer erro de min size
    let mut ubo_data = vec![0u8; ubo_size as usize];
    for (name, off) in &refl.uniform_offsets {
        if name == "g_Brightness" || name == "g_UserAlpha" {
            let o = *off as usize;
            ubo_data[o..o + 4].copy_from_slice(&1.0f32.to_le_bytes());
        }
    }
    let ubo = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("WeGlobals"), size: ubo_size,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false,
    });
    queue.write_buffer(&ubo, 0, &ubo_data);

    // módulos + pipeline
    let vs = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("vs"), source: wgpu::ShaderSource::Wgsl(VERTEX_WGSL.into()) });
    let fs = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("we-frag"), source: wgpu::ShaderSource::SpirV(frag_spirv.into()) });

    let tex_binding = refl.texture_bindings.first().copied().unwrap_or(1);
    let smp_binding = refl.sampler_bindings.first().copied().unwrap_or(2);
    let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("bgl"),
        entries: &[
            wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None },
            wgpu::BindGroupLayoutEntry { binding: tex_binding, visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None },
            wgpu::BindGroupLayoutEntry { binding: smp_binding, visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
        ],
    });
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("bg"), layout: &bgl,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: ubo.as_entire_binding() },
            wgpu::BindGroupEntry { binding: tex_binding, resource: wgpu::BindingResource::TextureView(&in_view) },
            wgpu::BindGroupEntry { binding: smp_binding, resource: wgpu::BindingResource::Sampler(&sampler) },
        ],
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: None, bind_group_layouts: &[Some(&bgl)], immediate_size: 0 });
    let fmt = wgpu::TextureFormat::Rgba8Unorm;
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("scene"), layout: Some(&layout),
        vertex: wgpu::VertexState { module: &vs, entry_point: Some("vs_main"), compilation_options: Default::default(), buffers: &[] },
        fragment: Some(wgpu::FragmentState { module: &fs, entry_point: Some("main"), compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState { format: fmt, blend: None, write_mask: wgpu::ColorWrites::ALL })] }),
        primitive: Default::default(), depth_stencil: None, multisample: Default::default(), multiview_mask: None, cache: None,
    });

    // alvo + render
    let out_tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("out"), size: extent, mip_level_count: 1, sample_count: 1,
        dimension: wgpu::TextureDimension::D2, format: fmt,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC, view_formats: &[],
    });
    let out_view = out_tex.create_view(&Default::default());
    let mut enc = device.create_command_encoder(&Default::default());
    {
        let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &out_view, resolve_target: None, depth_slice: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store } })],
            depth_stencil_attachment: None, timestamp_writes: None, occlusion_query_set: None, multiview_mask: None,
        });
        rp.set_pipeline(&pipeline);
        rp.set_bind_group(0, &bind_group, &[]);
        rp.draw(0..3, 0..1);
    }

    // readback com row alignment (256)
    let bpr = align256(4 * cw);
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("rb"), size: (bpr * ch) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false,
    });
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo { texture: &out_tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        wgpu::TexelCopyBufferInfo { buffer: &readback, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(bpr), rows_per_image: Some(ch) } },
        extent,
    );
    queue.submit(Some(enc.finish()));
    let slice = readback.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    let data = slice.get_mapped_range();

    // remove o padding de linha e salva
    let mut pixels = vec![0u8; (cw * ch * 4) as usize];
    for y in 0..ch {
        let s = (y * bpr) as usize;
        let d = (y * cw * 4) as usize;
        pixels[d..d + (cw * 4) as usize].copy_from_slice(&data[s..s + (cw * 4) as usize]);
    }
    image::save_buffer(&out_png, &pixels, cw, ch, image::ColorType::Rgba8).unwrap();
    println!("PNG salvo em {out_png} ({cw}x{ch})");
}

// Helper: fatia uma linha do buffer RGBA (evita panics de índice em código inline).
fn tex_bytes_row(rgba: &[u8], start: usize, len: usize) -> Vec<u8> {
    rgba[start..start + len].to_vec()
}
