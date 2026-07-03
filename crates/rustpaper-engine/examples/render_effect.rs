// Prova da CADEIA DE EFEITOS (offscreen): renderiza o fundo de uma cena e depois
// aplica um efeito do WE (tint) por cima, cada etapa como um `postprocess::Pass`
// que amostra a textura anterior. Salva PNG pra eu verificar.
//
//   fundo (genericimage2) -> texA
//   tint (BLENDMODE=2 multiply, cor vermelha) amostra texA -> texB -> PNG
//
// O tint multiplica a imagem (cinza do Zoro) pela cor -> deve sair NITIDAMENTE
// avermelhado, provando que o passe de efeito amostra o quadro anterior e aplica o
// fragment shader real do efeito.
//
// Uso: cargo run -p rustpaper-engine --example render_effect -- <pasta-do-wallpaper> [saida.png]

use std::path::Path;

use rustpaper_engine::postprocess::Pass;
use rustpaper_engine::shader_compile::{self, Reflection};
use rustpaper_core::pkg::Pkg;
use rustpaper_core::scene;
use rustpaper_core::shader::Stage;
use rustpaper_core::tex;

fn align256(n: u32) -> u32 {
    (n + 255) & !255
}

// escreve um f32 no offset (se o membro existir na reflexão)
fn put_f32(buf: &mut [u8], refl: &Reflection, name: &str, v: f32) {
    if let Some(&o) = refl.uniform_offsets.get(name) {
        buf[o as usize..o as usize + 4].copy_from_slice(&v.to_le_bytes());
    }
}
fn put_vec3(buf: &mut [u8], refl: &Reflection, name: &str, v: [f32; 3]) {
    if let Some(&o) = refl.uniform_offsets.get(name) {
        for (i, c) in v.iter().enumerate() {
            let p = o as usize + i * 4;
            buf[p..p + 4].copy_from_slice(&c.to_le_bytes());
        }
    }
}

fn make_target(device: &wgpu::Device, w: u32, h: u32) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    })
}

fn main() {
    let dir = std::env::args().nth(1).expect("uso: render_effect <pasta-do-wallpaper> [saida.png]");
    let out_png = std::env::args().nth(2).unwrap_or_else(|| "/tmp/render_effect.png".into());
    let shaders_dir = std::env::var("WE_SHADERS_DIR").unwrap_or_else(|_| "/home/vscode/we-assets/shaders".into());
    let sd = Path::new(&shaders_dir);

    // --- fundo da cena ---
    let pkg = Pkg::open(Path::new(&dir).join("scene.pkg").as_path()).expect("scene.pkg");
    let material = scene::background_material(&pkg).expect("material de fundo");
    let tex_path = scene::background_texture(&pkg).expect("textura de fundo");
    let tex_bytes = pkg.read(&tex_path).expect("ler .tex");
    let decoded = tex::parse(tex_bytes).expect("decodificar .tex");
    let (cw, ch) = (decoded.real_width, decoded.real_height);
    let mut content = vec![0u8; (cw * ch * 4) as usize];
    for y in 0..ch {
        let s = (y * decoded.width * 4) as usize;
        let d = (y * cw * 4) as usize;
        content[d..d + (cw * 4) as usize].copy_from_slice(&decoded.rgba[s..s + (cw * 4) as usize]);
    }

    // --- wgpu headless ---
    let instance = wgpu::Instance::default();
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        force_fallback_adapter: false,
        compatible_surface: None,
    })).expect("adapter");
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).unwrap();

    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });
    let fmt = wgpu::TextureFormat::Rgba8Unorm;

    // textura de entrada = conteúdo decodificado
    let in_tex = make_target(&device, cw, ch);
    queue.write_texture(
        wgpu::TexelCopyTextureInfo { texture: &in_tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        &content,
        wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4 * cw), rows_per_image: Some(ch) },
        wgpu::Extent3d { width: cw, height: ch, depth_or_array_layers: 1 },
    );
    let in_view = in_tex.create_view(&Default::default());

    // ---- Passe 1: material de fundo (genericimage2, v_TexCoord vec2) ----
    let bg_frag = std::fs::read_to_string(format!("{shaders_dir}/{}.frag", material.shader)).expect("bg .frag");
    let bg_spirv = shader_compile::compile(Stage::Fragment, &bg_frag, &material.combos, sd).expect("compilar fundo");
    let bg_refl = shader_compile::reflect(&bg_spirv).expect("refletir fundo");
    let bg_pass = Pass::new(&device, &bg_spirv, &bg_refl, fmt, false);
    let mut bg_ubo = vec![0u8; bg_pass.ubo_size() as usize];
    put_f32(&mut bg_ubo, &bg_refl, "g_Brightness", 1.0);
    put_f32(&mut bg_ubo, &bg_refl, "g_UserAlpha", 1.0);

    let tex_a = make_target(&device, cw, ch);
    let view_a = tex_a.create_view(&Default::default());
    bg_pass.render(&device, &queue, &in_view, &sampler, &bg_ubo, &view_a);

    // ---- Passe 2: efeito tint (BLENDMODE=2 multiply; v_TexCoord vec4) ----
    let tint_path = "/home/vscode/we-assets/effects/tint/shaders/effects/tint.frag";
    let tint_frag = std::fs::read_to_string(tint_path).expect("tint.frag");
    let tint_combos = vec![("BLENDMODE".to_string(), 2i64)]; // multiply
    let tint_spirv = shader_compile::compile(Stage::Fragment, &tint_frag, &tint_combos, sd).expect("compilar tint");
    let tint_refl = shader_compile::reflect(&tint_spirv).expect("refletir tint");
    println!("tint reflection: ubo={} membros={:?} tex={:?}", tint_refl.uniform_size, tint_refl.uniform_offsets, tint_refl.texture_bindings);
    let tint_pass = Pass::new(&device, &tint_spirv, &tint_refl, fmt, true);
    let mut tint_ubo = vec![0u8; tint_pass.ubo_size() as usize];
    put_f32(&mut tint_ubo, &tint_refl, "g_BlendAlpha", 1.0);
    put_vec3(&mut tint_ubo, &tint_refl, "g_TintColor", [1.0, 0.35, 0.35]); // vermelho

    let tex_b = make_target(&device, cw, ch);
    let view_b = tex_b.create_view(&Default::default());
    tint_pass.render(&device, &queue, &view_a, &sampler, &tint_ubo, &view_b);

    // ---- readback texB -> PNG ----
    let bpr = align256(4 * cw);
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("rb"),
        size: (bpr * ch) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = device.create_command_encoder(&Default::default());
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo { texture: &tex_b, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        wgpu::TexelCopyBufferInfo { buffer: &readback, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(bpr), rows_per_image: Some(ch) } },
        wgpu::Extent3d { width: cw, height: ch, depth_or_array_layers: 1 },
    );
    queue.submit(Some(enc.finish()));
    let slice = readback.slice(..);
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
    println!("PNG salvo em {out_png} ({cw}x{ch}) — fundo + tint");
}
