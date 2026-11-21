// `program` module: a WE material compiled into a pipeline ready to draw a
// layer/pass - LINKED vertex+fragment (unified UBO), with a vertex
// buffer layout (a_Position vec3 + a_TexCoord vec2), bind group (UBO + textures/samplers) and
// blend state. Brings together compilation, reflection and UBO assembly.

use std::collections::HashMap;

use crate::shader_compile as sc;
use rustpaper_core::shader::{self, UniformParam};

/// Blend mode of a layer/pass.
#[derive(Clone, Copy, PartialEq)]
pub enum Blend {
    /// alpha over (normal/translucent)
    Alpha,
    /// additive (adds light)
    Additive,
    /// replaces (no blend)
    Opaque,
}

impl Blend {
    pub fn from_we(s: &str) -> Blend {
        match s {
            "additive" | "add" => Blend::Additive,
            "normal" | "translucent" | "" => Blend::Alpha,
            _ => Blend::Alpha,
        }
    }
    fn state(self) -> Option<wgpu::BlendState> {
        match self {
            Blend::Opaque => None,
            Blend::Alpha => Some(wgpu::BlendState {
                color: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::SrcAlpha,
                    dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                    operation: wgpu::BlendOperation::Add,
                },
                alpha: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::One,
                    dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                    operation: wgpu::BlendOperation::Add,
                },
            }),
            Blend::Additive => Some(wgpu::BlendState {
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
            }),
        }
    }
}

pub struct Program {
    pub pipeline: wgpu::RenderPipeline,
    pub bgl: wgpu::BindGroupLayout,
    pub ubo_size: u64,
    pub offsets: HashMap<String, u32>,
    pub params: Vec<UniformParam>,
    pub sampler_defaults: Vec<(String, String)>,
    pub tex_bindings: Vec<u32>,
    pub smp_bindings: Vec<u32>,
}

impl Program {
    /// Compiles and assembles the pipeline from the WE vertex and fragment sources.
    /// `combos` must already include defaults + overrides. `target` is the target format.
    pub fn build(
        device: &wgpu::Device,
        vert_src: &str,
        frag_src: &str,
        combos: &[(String, i64)],
        blend: Blend,
        target: wgpu::TextureFormat,
        shaders_dir: &std::path::Path,
    ) -> Result<Program, String> {
        // effective combos: defaults from the // [COMBO] annotations, overridden by the data.
        let mut eff = shader::parse_combo_defaults(vert_src);
        eff.extend(shader::parse_combo_defaults(frag_src));
        for (k, v) in combos {
            eff.retain(|(ck, _)| ck != k);
            eff.push((k.clone(), *v));
        }

        let (v_spv, f_spv) = sc::compile_linked(vert_src, frag_src, &eff, shaders_dir)?;
        let refl = sc::reflect(&f_spv)?;
        let vinputs = sc::reflect_vertex_inputs(&v_spv)?;

        let mut params = shader::parse_params(vert_src);
        params.extend(shader::parse_params(frag_src));
        let mut sampler_defaults = shader::parse_sampler_defaults(vert_src);
        sampler_defaults.extend(shader::parse_sampler_defaults(frag_src));

        // Captures validation errors (e.g. incompatible vertex/fragment interface in
        // some WE shaders) instead of letting wgpu panic and take everything down.
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);

        let vs = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("we-vert"),
            source: wgpu::ShaderSource::SpirV(v_spv.into()),
        });
        let fs = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("we-frag"),
            source: wgpu::ShaderSource::SpirV(f_spv.into()),
        });

        // vertex buffer layout: matches by dimension (vec3=a_Position@0, vec2=a_TexCoord@12)
        let mut attrs = Vec::new();
        for vi in &vinputs {
            let (offset, format) = match vi.components {
                3 => (0u64, wgpu::VertexFormat::Float32x3),
                2 => (12u64, wgpu::VertexFormat::Float32x2),
                _ => continue,
            };
            attrs.push(wgpu::VertexAttribute {
                format,
                offset,
                shader_location: vi.location,
            });
        }
        let vbl = wgpu::VertexBufferLayout {
            array_stride: 20,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &attrs,
        };

        let mut entries = vec![wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }];
        for &b in &refl.texture_bindings {
            entries.push(wgpu::BindGroupLayoutEntry {
                binding: b,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
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
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("prog-bgl"),
            entries: &entries,
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("prog"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &vs,
                entry_point: Some("main"),
                compilation_options: Default::default(),
                buffers: &[vbl],
            },
            fragment: Some(wgpu::FragmentState {
                module: &fs,
                entry_point: Some("main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target,
                    blend: blend.state(),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });

        if let Some(err) = pollster::block_on(scope.pop()) {
            return Err(format!("invalid pipeline: {err}"));
        }

        Ok(Program {
            pipeline,
            bgl,
            ubo_size: refl.uniform_size.max(16).next_multiple_of(16) as u64,
            offsets: refl.uniform_offsets,
            params,
            sampler_defaults,
            tex_bindings: refl.texture_bindings,
            smp_bindings: refl.sampler_bindings,
        })
    }

    /// Builds the UBO bytes: builtins (MVP, time, resolution, layer
    /// color/brightness/alpha) + params (default or material/scene constant by "material" name).
    #[allow(clippy::too_many_arguments)]
    pub fn build_ubo(
        &self,
        mvp: &[f32; 16],
        w: u32,
        h: u32,
        time: f32,
        brightness: f32,
        alpha: f32,
        color: [f32; 3],
        constants: &HashMap<String, Vec<f32>>,
    ) -> Vec<u8> {
        let mut buf = vec![0u8; self.ubo_size as usize];
        let write = |buf: &mut [u8], off: u32, vals: &[f32]| {
            for (i, v) in vals.iter().enumerate() {
                let p = off as usize + i * 4;
                if p + 4 <= buf.len() {
                    buf[p..p + 4].copy_from_slice(&v.to_le_bytes());
                }
            }
        };
        if let Some(&o) = self.offsets.get("g_ModelViewProjectionMatrix") {
            write(&mut buf, o, mvp);
        }
        if let Some(&o) = self.offsets.get("g_Time") {
            write(&mut buf, o, &[time]);
        }
        for res in [
            "g_Texture0Resolution",
            "g_Texture1Resolution",
            "g_Texture2Resolution",
        ] {
            if let Some(&o) = self.offsets.get(res) {
                write(&mut buf, o, &[w as f32, h as f32, w as f32, h as f32]);
            }
        }
        // layer color/brightness/alpha (usual names in genericimage*)
        if let Some(&o) = self.offsets.get("g_Brightness") {
            write(&mut buf, o, &[brightness]);
        }
        if let Some(&o) = self.offsets.get("g_UserAlpha") {
            write(&mut buf, o, &[alpha]);
        }
        if let Some(&o) = self.offsets.get("g_Color") {
            write(&mut buf, o, &color);
        }
        if let Some(&o) = self.offsets.get("g_Color4") {
            write(&mut buf, o, &[color[0], color[1], color[2], alpha]);
        }
        // annotated params: default, overridden by material/scene constant
        for p in &self.params {
            let Some(&o) = self.offsets.get(&p.uniform) else {
                continue;
            };
            let mut vals = p
                .material
                .as_ref()
                .and_then(|m| constants.get(m))
                .cloned()
                .unwrap_or_else(|| p.default.clone());
            vals.resize(p.components, 0.0);
            write(&mut buf, o, &vals);
        }
        buf
    }
}

/// WE orthographic projection: scene space [0,w]x[0,h] -> NDC, with Y flipped
/// (y=0 at the top). Column-major (as GLSL expects).
pub fn ortho(w: f32, h: f32) -> [f32; 16] {
    [
        2.0 / w,
        0.0,
        0.0,
        0.0,
        0.0,
        -2.0 / h,
        0.0,
        0.0,
        0.0,
        0.0,
        1.0,
        0.0,
        -1.0,
        1.0,
        0.0,
        1.0,
    ]
}
