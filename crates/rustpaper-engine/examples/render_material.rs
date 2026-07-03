// Prova OFFSCREEN do caminho de render por material do WE.
//
// Renderiza uma textura de teste (gradiente) através do FRAGMENT shader REAL do WE
// (genericimage2), compilado em runtime (translate -> glslang -> SPIR-V -> wgpu), e
// grava um PNG. O vertex é um triângulo fullscreen simples nosso (em WGSL) — só o
// FRAGMENT é do WE, que é a parte interessante (cor/brilho/blend). Isso contorna,
// por ora, a unificação do bloco de uniforms entre estágios e a matriz MVP.
//
// Pra PROVAR que o shader do WE realmente roda, usamos g_Brightness = 0.4: o PNG de
// saída deve sair NITIDAMENTE mais escuro que o gradiente de entrada.
//
// Uso: cargo run -p rustpaper-engine --example render_material
// Saída: /tmp/render_material.png

use rustpaper_core::shader::{Stage, translate};
use std::process::Command;

const SIZE: u32 = 256;

// Vertex fullscreen (triângulo cobrindo a tela) que emite v_TexCoord no location 0,
// que é onde o frag do WE espera `in vec2 v_TexCoord`.
const VERTEX_WGSL: &str = r#"
struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};
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

fn glslang() -> &'static str {
    for c in [
        "glslangValidator",
        "/usr/sbin/glslangValidator",
        "/usr/bin/glslangValidator",
    ] {
        if Command::new(c)
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
        {
            return Box::leak(c.to_string().into_boxed_str());
        }
    }
    panic!("glslangValidator não encontrado");
}

fn compile_frag_spirv(shaders_dir: &str) -> Vec<u32> {
    let src = std::fs::read_to_string(format!("{shaders_dir}/genericimage2.frag")).unwrap();
    let glsl = translate(
        Stage::Fragment,
        &src,
        &[],
        std::path::Path::new(shaders_dir),
    )
    .unwrap();
    let inp = std::env::temp_dir().join("render_material.frag");
    let outp = std::env::temp_dir().join("render_material.frag.spv");
    std::fs::write(&inp, &glsl).unwrap();
    let o = Command::new(glslang())
        .args([
            "-V",
            "-R",
            "--amb",
            "--aml",
            "--sdub",
            "WeGlobals",
            "0",
            "0",
            "-S",
            "frag",
        ])
        .arg(&inp)
        .arg("-o")
        .arg(&outp)
        .output()
        .unwrap();
    assert!(
        o.status.success(),
        "glslang: {}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
    let bytes = std::fs::read(&outp).unwrap();
    bytes
        .chunks_exact(4)
        .map(|c| u32::from_le_bytes(c.try_into().unwrap()))
        .collect()
}

fn main() {
    let shaders_dir =
        std::env::var("WE_SHADERS_DIR").unwrap_or_else(|_| "/home/vscode/we-assets/shaders".into());
    let frag_spirv = compile_frag_spirv(&shaders_dir);

    // --- wgpu headless (sem surface) ---
    let instance = wgpu::Instance::default();
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        force_fallback_adapter: false,
        compatible_surface: None,
    }))
    .expect("adapter");
    println!("adapter: {:?}", adapter.get_info());
    let (device, queue) =
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).unwrap();

    // --- textura de entrada: gradiente (X=vermelho, Y=verde) ---
    let mut rgba = vec![0u8; (SIZE * SIZE * 4) as usize];
    for y in 0..SIZE {
        for x in 0..SIZE {
            let i = ((y * SIZE + x) * 4) as usize;
            rgba[i] = (x * 255 / SIZE) as u8;
            rgba[i + 1] = (y * 255 / SIZE) as u8;
            rgba[i + 2] = 200;
            rgba[i + 3] = 255;
        }
    }
    let extent = wgpu::Extent3d {
        width: SIZE,
        height: SIZE,
        depth_or_array_layers: 1,
    };
    let in_tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("input"),
        size: extent,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &in_tex,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &rgba,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(4 * SIZE),
            rows_per_image: Some(SIZE),
        },
        extent,
    );
    let in_view = in_tex.create_view(&wgpu::TextureViewDescriptor::default());
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });

    // --- WeGlobals UBO (genericimage2 frag, sem combos): g_Brightness@0, g_UserAlpha@4 ---
    // 0.4 de brilho pra o efeito ser óbvio no PNG.
    let mut ubo_data = [0u8; 16];
    ubo_data[0..4].copy_from_slice(&0.4f32.to_le_bytes()); // g_Brightness
    ubo_data[4..8].copy_from_slice(&1.0f32.to_le_bytes()); // g_UserAlpha
    let ubo = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("WeGlobals"),
        size: 16,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&ubo, 0, &ubo_data);

    // --- módulos: vertex (nosso WGSL) + fragment (SPIR-V do WE) ---
    let vs = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("vs"),
        source: wgpu::ShaderSource::Wgsl(VERTEX_WGSL.into()),
    });
    let fs = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("we-frag"),
        source: wgpu::ShaderSource::SpirV(frag_spirv.into()),
    });

    // bind group layout: WeGlobals@0, textura@1, sampler@2 (tudo no fragment)
    let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("bgl"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    });
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("bg"),
        layout: &bgl,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: ubo.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(&in_view),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Sampler(&sampler),
            },
        ],
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("layout"),
        bind_group_layouts: &[Some(&bgl)],
        immediate_size: 0,
    });

    let target_format = wgpu::TextureFormat::Rgba8Unorm;
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("material"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &vs,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module: &fs,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: target_format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    });

    // --- alvo offscreen + render ---
    let out_tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("output"),
        size: extent,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: target_format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let out_view = out_tex.create_view(&wgpu::TextureViewDescriptor::default());

    let mut encoder =
        device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    {
        let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &out_view,
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
        rp.set_pipeline(&pipeline);
        rp.set_bind_group(0, &bind_group, &[]);
        rp.draw(0..3, 0..1);
    }

    // copiar textura -> buffer (bytes_per_row múltiplo de 256; 256*4=1024 ok)
    let bpr = 4 * SIZE;
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: (bpr * SIZE) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &out_tex,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bpr),
                rows_per_image: Some(SIZE),
            },
        },
        extent,
    );
    queue.submit(Some(encoder.finish()));

    let slice = readback.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    let data = slice.get_mapped_range();

    let out_path = "/tmp/render_material.png";
    image::save_buffer(out_path, &data, SIZE, SIZE, image::ColorType::Rgba8).unwrap();

    // sanidade: pixel do centro deve estar escurecido (~0.4x) vs a entrada
    let center = ((SIZE / 2 * SIZE + SIZE / 2) * 4) as usize;
    println!("centro saída RGBA = {:?}", &data[center..center + 4]);
    println!("PNG salvo em {out_path}");
}
