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
    /// Includes are expanded first: headers declare annotated uniforms too
    /// (`g_CompositeColor` in common_composite.h: left at GL's 0 it turned
    /// every blur/composite result black).
    pub fn params(&self, shaders_dir: &Path) -> Vec<UniformParam> {
        let mut p = shader::parse_params(&shader::expand_includes(&self.vert, shaders_dir));
        p.extend(shader::parse_params(&shader::expand_includes(
            &self.frag,
            shaders_dir,
        )));
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

/// Fixed attribute slots shared by every program (see QuadMesh/ParticleMesh).
const ATTRIBUTES: &[(u32, &str)] = &[
    (0, "a_Position"),
    (1, "a_TexCoord"),
    // genericparticle's vertex stream (gpu::SPRITE_LAYOUT)
    (2, "a_TexCoordVec4"),
    (3, "a_Color"),
    (4, "a_TexCoordC2"),
    (5, "a_TexCoordVec4C1"),
    // genericropeparticle's extra attributes (gpu::ROPE_LAYOUT)
    (6, "a_PositionVec4"),
    (7, "a_TexCoordVec3C2"),
    (8, "a_TexCoordC3"),
];

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
        let mut vs = shader::translate_gl(Stage::Vertex, &src.vert, &combos, shaders_dir)?;
        let mut fs = shader::translate_gl(Stage::Fragment, &src.frag, &combos, shaders_dir)?;
        // HLSL doesn't care if a varying is vec4 in one stage and vec2 in the
        // other; GLSL refuses to link. Promote the smaller declaration to the
        // other stage's (bigger) type; the fixer then adapts the uses.
        (vs, fs) = shader::match_varyings(&vs, &fs);
        let no_fix = std::env::var_os("RUSTPAPER_NO_FIX").is_some();
        let mut last_err = String::new();
        unsafe {
            // Compile + link; on failure the HLSL fixer patches the lines the
            // driver complained about (compile or link log) and we retry.
            for _attempt in 0..10 {
                let program = gl.create_program()?;
                let mut stages = Vec::new();
                let mut failed: Option<(&str, String)> = None;
                for (kind, code, label) in [
                    (glow::VERTEX_SHADER, &vs, "vertex"),
                    (glow::FRAGMENT_SHADER, &fs, "fragment"),
                ] {
                    let s = gl.create_shader(kind)?;
                    gl.shader_source(s, code);
                    gl.compile_shader(s);
                    stages.push(s);
                    if !gl.get_shader_compile_status(s) {
                        failed = Some((label, gl.get_shader_info_log(s)));
                        break;
                    }
                    gl.attach_shader(program, s);
                }
                if failed.is_none() {
                    // Fixed attribute slots, matching QuadMesh / ParticleMesh.
                    for (loc, name) in ATTRIBUTES {
                        gl.bind_attrib_location(program, *loc, name);
                    }
                    gl.link_program(program);
                    if !gl.get_program_link_status(program) {
                        let log = gl.get_program_info_log(program);
                        // the link log has per-stage sections
                        let stage = if log.contains("Fragment info") {
                            "fragment"
                        } else {
                            "vertex"
                        };
                        failed = Some((stage, log));
                    }
                }
                for s in stages {
                    gl.detach_shader(program, s);
                    gl.delete_shader(s);
                }
                let Some((stage, log)) = failed else {
                    return Ok(Self::finish(gl, program, src, shaders_dir));
                };
                gl.delete_program(program);
                last_err = format!("{stage} shader: {}", first_lines(&log, 6));
                let code = if stage == "fragment" {
                    &mut fs
                } else {
                    &mut vs
                };
                match (!no_fix)
                    .then(|| shader::fix_glsl_from_log(code, &log))
                    .flatten()
                {
                    Some(fixed) => *code = fixed,
                    None => {
                        dump_failed(stage, code, &log);
                        break;
                    }
                }
            }
        }
        Err(last_err)
    }

    unsafe fn finish(
        gl: &glow::Context,
        program: glow::Program,
        src: &ShaderSource,
        shaders_dir: &Path,
    ) -> Program {
        unsafe {
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
            Program {
                id: program,
                uniforms,
                params: src.params(shaders_dir),
                samplers: src.samplers(),
            }
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

/// With RUSTPAPER_DUMP_SHADERS=<dir>, keep the translated source + log of
/// every shader that fails to compile (for debugging translations).
fn dump_failed(label: &str, code: &str, log: &str) {
    let Some(dir) = std::env::var_os("RUSTPAPER_DUMP_SHADERS") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    let _ = std::fs::create_dir_all(&dir);
    let n = std::fs::read_dir(&dir).map(|d| d.count()).unwrap_or(0);
    let _ = std::fs::write(
        dir.join(format!("{n:04}_{label}.glsl")),
        format!("{code}\n/* LOG:\n{log}\n*/\n"),
    );
}

fn first_lines(s: &str, n: usize) -> String {
    s.lines().take(n).collect::<Vec<_>>().join(" | ")
}
