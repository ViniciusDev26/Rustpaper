// `shader_compile` module: takes a WE shader, translates it to GLSL (via rustpaper-core),
// compiles it to Vulkan SPIR-V by calling glslangValidator, and reflects the layout
// (uniform block + texture bindings) of the resulting module.
//
// Why glslang as a subprocess: naga's GLSL frontend is too weak for the
// WE shaders (see docs/SHADER_TRANSLATION.md). glslang already ships in the container.

use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

use rustpaper_core::shader::{Stage, translate};

// Finds glslangValidator (it is usually not on the process's PATH).
fn glslang_bin() -> &'static str {
    use std::sync::OnceLock;
    static BIN: OnceLock<String> = OnceLock::new();
    BIN.get_or_init(|| {
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
                return c.to_string();
            }
        }
        panic!("glslangValidator not found (install glslang in the container)");
    })
}

/// Compiles (already translated) GLSL to Vulkan SPIR-V. `stage` = "vert" | "frag".
pub fn glsl_to_spirv(stage: &str, glsl: &str) -> Result<Vec<u32>, String> {
    let dir = std::env::temp_dir();
    // name unique per process+stage to avoid collisions between concurrent calls
    let base = format!("we_{}_{stage}", std::process::id());
    let inp = dir.join(format!("{base}.{stage}"));
    let outp = dir.join(format!("{base}.{stage}.spv"));
    std::fs::write(&inp, glsl).map_err(|e| e.to_string())?;

    let out = Command::new(glslang_bin())
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
            stage,
        ])
        .arg(&inp)
        .arg("-o")
        .arg(&outp)
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!(
            "glslang failed ({stage}):\n{}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let bytes = std::fs::read(&outp).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(&inp);
    let _ = std::fs::remove_file(&outp);
    Ok(bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| u32::from_le_bytes(*c))
        .collect())
}

/// Compiles vertex + fragment TOGETHER (linked) to SPIR-V. This makes glslang
/// UNIFY the default uniform block (WeGlobals) across the two stages: the two resulting
/// SPIR-V modules have the SAME struct, with the same offsets. This is what allows using
/// WE's vertex shader (which most effects need, for animated coords) together with the
/// fragment, sharing a single UBO at binding 0. Returns (vert_spirv, frag_spirv).
pub fn glsl_to_spirv_linked(
    vert_glsl: &str,
    frag_glsl: &str,
) -> Result<(Vec<u32>, Vec<u32>), String> {
    // unique subdirectory: glslang with -l writes vert.spv/frag.spv into the CWD.
    let dir = std::env::temp_dir().join(format!("we_link_{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("s.vert"), vert_glsl).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("s.frag"), frag_glsl).map_err(|e| e.to_string())?;

    let out = Command::new(glslang_bin())
        .current_dir(&dir)
        .args([
            "-V",
            "-R",
            "--amb",
            "--aml",
            "--sdub",
            "WeGlobals",
            "0",
            "0",
            "-l",
            "s.vert",
            "s.frag",
        ])
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!(
            "glslang -l failed:\n{}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let read = |name: &str| -> Result<Vec<u32>, String> {
        let bytes = std::fs::read(dir.join(name)).map_err(|e| format!("{name}: {e}"))?;
        Ok(bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| u32::from_le_bytes(*c))
            .collect())
    };
    let vert = read("vert.spv")?;
    let frag = read("frag.spv")?;
    let _ = std::fs::remove_dir_all(&dir);
    Ok((vert, frag))
}

/// Translates the WE vertex + fragment and compiles them linked (unified UBO).
pub fn compile_linked(
    vert_src: &str,
    frag_src: &str,
    combos: &[(String, i64)],
    shaders_dir: &Path,
) -> Result<(Vec<u32>, Vec<u32>), String> {
    let v = translate(Stage::Vertex, vert_src, combos, shaders_dir)?;
    let f = translate(Stage::Fragment, frag_src, combos, shaders_dir)?;
    glsl_to_spirv_linked(&v, &f)
}

/// Translates a WE shader and compiles it to SPIR-V.
pub fn compile(
    stage: Stage,
    source: &str,
    combos: &[(String, i64)],
    shaders_dir: &Path,
) -> Result<Vec<u32>, String> {
    let glsl = translate(stage, source, combos, shaders_dir)?;
    let sflag = match stage {
        Stage::Vertex => "vert",
        Stage::Fragment => "frag",
    };
    glsl_to_spirv(sflag, &glsl)
}

/// Reflected layout of a compiled shader: the uniform block (WeGlobals) and the
/// texture/sampler bindings. Used to build the correct UBO and bind group.
#[derive(Debug, Default, Clone)]
pub struct Reflection {
    /// Size (bytes) of the WeGlobals uniform block (0 if there is none).
    pub uniform_size: u32,
    /// Offset of each member of the block, by name (e.g. "g_Brightness" -> 0).
    pub uniform_offsets: HashMap<String, u32>,
    /// Texture bindings (global `Handle`s of image type).
    pub texture_bindings: Vec<u32>,
    /// Sampler bindings (global `Handle`s of sampler type).
    pub sampler_bindings: Vec<u32>,
}

/// A vertex entry (attribute): location and number of components.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VertexInput {
    pub location: u32,
    pub components: u32,
}

/// Reflects the vertex's input attributes (a_Position, a_TexCoord, ...) - location
/// and dimension of each one. Used to build the vertex buffer layout by matching the
/// locations that glslang assigned.
pub fn reflect_vertex_inputs(spirv: &[u32]) -> Result<Vec<VertexInput>, String> {
    let module = naga::front::spv::Frontend::new(spirv.iter().copied(), &Default::default())
        .parse()
        .map_err(|e| format!("naga spv-in: {e:?}"))?;
    let ep = module.entry_points.first().ok_or("no entry point")?;
    let mut out = Vec::new();
    for arg in &ep.function.arguments {
        if let Some(naga::Binding::Location { location, .. }) = arg.binding {
            let components = match module.types[arg.ty].inner {
                naga::TypeInner::Scalar(_) => 1,
                naga::TypeInner::Vector { size, .. } => size as u32,
                _ => continue,
            };
            out.push(VertexInput {
                location,
                components,
            });
        }
    }
    out.sort_by_key(|v| v.location);
    Ok(out)
}

/// Reflects the SPIR-V using naga (the same parser wgpu uses).
pub fn reflect(spirv: &[u32]) -> Result<Reflection, String> {
    let module = naga::front::spv::Frontend::new(spirv.iter().copied(), &Default::default())
        .parse()
        .map_err(|e| format!("naga spv-in: {e:?}"))?;

    let mut r = Reflection::default();
    for (_, gv) in module.global_variables.iter() {
        match gv.space {
            naga::AddressSpace::Uniform => {
                if let naga::TypeInner::Struct { members, span } = &module.types[gv.ty].inner {
                    r.uniform_size = *span;
                    for m in members {
                        if let Some(name) = &m.name {
                            r.uniform_offsets.insert(name.clone(), m.offset);
                        }
                    }
                }
            }
            naga::AddressSpace::Handle => {
                let binding = gv.binding.as_ref().map(|b| b.binding);
                if let Some(b) = binding {
                    match module.types[gv.ty].inner {
                        naga::TypeInner::Image { .. } => r.texture_bindings.push(b),
                        naga::TypeInner::Sampler { .. } => r.sampler_bindings.push(b),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    r.texture_bindings.sort_unstable();
    r.sampler_bindings.sort_unstable();
    Ok(r)
}
