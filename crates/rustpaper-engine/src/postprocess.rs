// `postprocess` module: the "pass" primitive - renders a WE FRAGMENT shader
// (already compiled to SPIR-V) over an input texture, writing into an output
// texture. It is the building block of both the background material and the effect chain (each
// effect is a pass that samples the previous framebuffer as g_Texture0).
//
// The vertex shader is ours (fullscreen). WE shaders declare v_TexCoord as either vec2 OR
// vec4 (effects use .zw for the mask) - we generate the vertex with the right arity to
// match the interface with the fragment.

use crate::shader_compile::Reflection;

fn vertex_wgsl(uv_vec4: bool) -> String {
    // uv at location 0; vec4 replicates xy into zw (masks sample at the same uv).
    let (ty, build) = if uv_vec4 {
        ("vec4<f32>", "vec4(u, u.x, u.y)")
    } else {
        ("vec2<f32>", "u")
    };
    format!(
        r#"
struct VsOut {{ @builtin(position) pos: vec4<f32>, @location(0) uv: {ty} }};
@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> VsOut {{
    var p = array<vec2<f32>, 3>(vec2(-1.0, -1.0), vec2(3.0, -1.0), vec2(-1.0, 3.0));
    let xy = p[i];
    let u = vec2((xy.x + 1.0) * 0.5, 1.0 - (xy.y + 1.0) * 0.5);
    var o: VsOut;
    o.pos = vec4(xy, 0.0, 1.0);
    o.uv = {build};
    return o;
}}
"#
    )
}

/// A compiled pass: pipeline (our vertex + the WE frag) + bind group layout.
pub struct Pass {
    pipeline: wgpu::RenderPipeline,
    bgl: wgpu::BindGroupLayout,
    tex_binding: u32,
    smp_binding: u32,
    ubo_size: u64,
}

impl Pass {
    /// Creates the pass. `uv_vec4` = the frag declares v_TexCoord as vec4.
    pub fn new(
        device: &wgpu::Device,
        frag_spirv: &[u32],
        refl: &Reflection,
        target_format: wgpu::TextureFormat,
        uv_vec4: bool,
    ) -> Self {
        let vs = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("pp-vs"),
            source: wgpu::ShaderSource::Wgsl(vertex_wgsl(uv_vec4).into()),
        });
        let fs = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("pp-we-frag"),
            source: wgpu::ShaderSource::SpirV(frag_spirv.to_vec().into()),
        });

        let tex_binding = refl.texture_bindings.first().copied().unwrap_or(1);
        let smp_binding = refl.sampler_bindings.first().copied().unwrap_or(2);
        // UBO with at least 16 bytes; uses the reflected size, rounded up.
        let ubo_size = refl.uniform_size.max(16).next_multiple_of(16) as u64;

        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("pp-bgl"),
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
                    binding: tex_binding,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: smp_binding,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("pp-layout"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("pp-pipeline"),
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
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });

        Self {
            pipeline,
            bgl,
            tex_binding,
            smp_binding,
            ubo_size,
        }
    }

    pub fn ubo_size(&self) -> u64 {
        self.ubo_size
    }

    /// Renders: samples `input` through the WE frag (with `ubo_bytes` in the uniform
    /// block) and writes into `output`.
    pub fn render(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        input: &wgpu::TextureView,
        sampler: &wgpu::Sampler,
        ubo_bytes: &[u8],
        output: &wgpu::TextureView,
    ) {
        let ubo = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("pp-ubo"),
            size: self.ubo_size,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut data = vec![0u8; self.ubo_size as usize];
        let n = ubo_bytes.len().min(data.len());
        data[..n].copy_from_slice(&ubo_bytes[..n]);
        queue.write_buffer(&ubo, 0, &data);

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("pp-bg"),
            layout: &self.bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: ubo.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: self.tex_binding,
                    resource: wgpu::BindingResource::TextureView(input),
                },
                wgpu::BindGroupEntry {
                    binding: self.smp_binding,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
            ],
        });

        let mut enc = device.create_command_encoder(&Default::default());
        {
            let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("pp-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: output,
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
            rp.set_pipeline(&self.pipeline);
            rp.set_bind_group(0, &bind_group, &[]);
            rp.draw(0..3, 0..1);
        }
        queue.submit(Some(enc.finish()));
    }
}
