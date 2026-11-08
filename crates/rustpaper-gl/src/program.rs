// A compiled WE material shader (vertex + fragment) for one set of combos.
//
// This is where the GL choice pays off: we compile the WE source almost as-is
// (rustpaper_core::shader::translate_gl only adds the dialect macros), link, and
// then ask the driver which uniforms are ACTIVE. Each draw just sets the ones the
// shader actually uses, by name. No uniform blocks, no binding tables, no
// reflection of our own.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use glow::HasContext;
use rustpaper_core::shader::{self, SamplerAnnotation, Stage, UniformParam};

/// Raw shader sources of a material, before combos are applied.
#[derive(Clone)]
pub struct ShaderSource {
    pub vert: String,
    pub frag: String,
}

impl ShaderSource {
    /// Annotated material parameters (`uniform float g_Speed; // {"material":"speed",...}`).
    pub fn params(&self) -> Vec<UniformParam> {
        let mut p = shader::parse_params(&self.vert);
        p.extend(shader::parse_params(&self.frag));
        p
    }

    /// `// [COMBO] {"combo":"X","default":N}` declarations of both stages.
    pub fn combo_defaults(&self) -> Vec<(String, i64)> {
        let mut c = shader::parse_combo_defaults(&self.vert);
        c.extend(shader::parse_combo_defaults(&self.frag));
        c
    }

    /// `uniform sampler2D` declarations (with their annotations) of both stages.
    pub fn samplers(&self) -> Vec<SamplerAnnotation> {
        let mut s = shader::parse_samplers(&self.frag);
        for v in shader::parse_samplers(&self.vert) {
            if !s.iter().any(|x| x.name == v.name) {
                s.push(v);
            }
        }
        s
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ActiveUniform {
    pub location: glow::UniformLocation,
    pub kind: u32, // GL type enum (FLOAT_VEC4, SAMPLER_2D, ...)
    pub size: i32, // > 1 for arrays
}

pub struct Program {
    pub id: glow::Program,
    pub uniforms: HashMap<String, ActiveUniform>,
    pub params: Vec<UniformParam>,
    pub samplers: Vec<SamplerAnnotation>,
}

impl Program {
    pub fn build(
        gl: &glow::Context,
        src: &ShaderSource,
        combos: &BTreeMap<String, i64>,
        shaders_dir: &Path,
    ) -> Result<Program, String> {
        let combos: Vec<(String, i64)> = combos.iter().map(|(k, v)| (k.clone(), *v)).collect();
        let vs = shader::translate_gl(Stage::Vertex, &src.vert, &combos, shaders_dir)?;
        let fs = shader::translate_gl(Stage::Fragment, &src.frag, &combos, shaders_dir)?;
        unsafe {
            let program = gl.create_program()?;
            let mut stages = Vec::new();
            for (kind, code, label) in [
                (glow::VERTEX_SHADER, &vs, "vertex"),
                (glow::FRAGMENT_SHADER, &fs, "fragment"),
            ] {
                let s = gl.create_shader(kind)?;
                gl.shader_source(s, code);
                gl.compile_shader(s);
                if !gl.get_shader_compile_status(s) {
                    let log = gl.get_shader_info_log(s);
                    gl.delete_shader(s);
                    gl.delete_program(program);
                    return Err(format!("{label} shader: {}", first_lines(&log, 6)));
                }
                gl.attach_shader(program, s);
                stages.push(s);
            }
            // Fixed attribute slots, matching QuadMesh.
            gl.bind_attrib_location(program, 0, "a_Position");
            gl.bind_attrib_location(program, 1, "a_TexCoord");
            gl.link_program(program);
            for s in stages {
                gl.detach_shader(program, s);
                gl.delete_shader(s);
            }
            if !gl.get_program_link_status(program) {
                let log = gl.get_program_info_log(program);
                gl.delete_program(program);
                return Err(format!("link: {}", first_lines(&log, 6)));
            }

            let mut uniforms = HashMap::new();
            for i in 0..gl.get_active_uniforms(program) {
                let Some(u) = gl.get_active_uniform(program, i) else {
                    continue;
                };
                // arrays come back as "name[0]"
                let name = u.name.trim_end_matches("[0]").to_string();
                if let Some(location) = gl.get_uniform_location(program, &u.name) {
                    uniforms.insert(
                        name,
                        ActiveUniform {
                            location,
                            kind: u.utype,
                            size: u.size,
                        },
                    );
                }
            }
            Ok(Program {
                id: program,
                uniforms,
                params: src.params(),
                samplers: src.samplers(),
            })
        }
    }

    /// Sets one uniform from floats, converting to whatever type it really is.
    /// Missing components are 0; extra ones are ignored.
    pub fn set(&self, gl: &glow::Context, name: &str, v: &[f32]) {
        let Some(u) = self.uniforms.get(name) else {
            return;
        };
        let f = |i: usize| v.get(i).copied().unwrap_or(0.0);
        let loc = Some(&u.location);
        unsafe {
            match u.kind {
                glow::FLOAT if u.size > 1 => {
                    let mut a = v.to_vec();
                    a.resize(u.size as usize, 0.0);
                    gl.uniform_1_f32_slice(loc, &a);
                }
                glow::FLOAT => gl.uniform_1_f32(loc, f(0)),
                glow::FLOAT_VEC2 => gl.uniform_2_f32(loc, f(0), f(1)),
                glow::FLOAT_VEC3 => gl.uniform_3_f32(loc, f(0), f(1), f(2)),
                glow::FLOAT_VEC4 => gl.uniform_4_f32(loc, f(0), f(1), f(2), f(3)),
                // samplers are ints too: the texture unit they read from
                glow::INT | glow::BOOL | glow::SAMPLER_2D => gl.uniform_1_i32(loc, f(0) as i32),
                glow::FLOAT_MAT4 if v.len() >= 16 => {
                    gl.uniform_matrix_4_f32_slice(loc, false, &v[..16])
                }
                glow::FLOAT_MAT3 if v.len() >= 9 => {
                    gl.uniform_matrix_3_f32_slice(loc, false, &v[..9])
                }
                _ => {}
            }
        }
    }
}

fn first_lines(s: &str, n: usize) -> String {
    s.lines().take(n).collect::<Vec<_>>().join(" | ")
}
