// Módulo `shader_compile`: pega um shader do WE, traduz pra GLSL (via rustpaper-core),
// compila pra SPIR-V Vulkan chamando o glslangValidator, e reflete o layout
// (bloco de uniforms + bindings de textura) do módulo resultante.
//
// Por que glslang como subprocesso: o frontend GLSL do naga é fraco demais pros
// shaders do WE (ver docs/SHADER_TRANSLATION.md). O glslang já vem no container.

use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

use rustpaper_core::shader::{translate, Stage};

// Acha o glslangValidator (não costuma estar no PATH do processo).
fn glslang_bin() -> &'static str {
    use std::sync::OnceLock;
    static BIN: OnceLock<String> = OnceLock::new();
    BIN.get_or_init(|| {
        for c in ["glslangValidator", "/usr/sbin/glslangValidator", "/usr/bin/glslangValidator"] {
            if Command::new(c).arg("--version").output().map(|o| o.status.success()).unwrap_or(false) {
                return c.to_string();
            }
        }
        panic!("glslangValidator não encontrado (instale glslang no container)");
    })
}

/// Compila GLSL (já traduzido) pra SPIR-V Vulkan. `stage` = "vert" | "frag".
pub fn glsl_to_spirv(stage: &str, glsl: &str) -> Result<Vec<u32>, String> {
    let dir = std::env::temp_dir();
    // nome único por processo+estágio pra não colidir entre chamadas concorrentes
    let base = format!("we_{}_{stage}", std::process::id());
    let inp = dir.join(format!("{base}.{stage}"));
    let outp = dir.join(format!("{base}.{stage}.spv"));
    std::fs::write(&inp, glsl).map_err(|e| e.to_string())?;

    let out = Command::new(glslang_bin())
        .args(["-V", "-R", "--amb", "--aml", "--sdub", "WeGlobals", "0", "0", "-S", stage])
        .arg(&inp)
        .arg("-o")
        .arg(&outp)
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!(
            "glslang falhou ({stage}):\n{}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let bytes = std::fs::read(&outp).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(&inp);
    let _ = std::fs::remove_file(&outp);
    Ok(bytes.chunks_exact(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect())
}

/// Compila vertex + fragment JUNTOS (linkados) pra SPIR-V. Isso faz o glslang
/// UNIFICAR o bloco default de uniforms (WeGlobals) entre os dois estágios: os dois
/// SPIR-V resultantes têm o MESMO struct, com os mesmos offsets. É o que permite usar
/// o vertex do WE (que a maioria dos efeitos precisa, pra coords animadas) junto do
/// fragment, compartilhando um único UBO no binding 0. Retorna (vert_spirv, frag_spirv).
pub fn glsl_to_spirv_linked(vert_glsl: &str, frag_glsl: &str) -> Result<(Vec<u32>, Vec<u32>), String> {
    // subdiretório único: o glslang com -l escreve vert.spv/frag.spv no CWD.
    let dir = std::env::temp_dir().join(format!("we_link_{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("s.vert"), vert_glsl).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("s.frag"), frag_glsl).map_err(|e| e.to_string())?;

    let out = Command::new(glslang_bin())
        .current_dir(&dir)
        .args(["-V", "-R", "--amb", "--aml", "--sdub", "WeGlobals", "0", "0", "-l", "s.vert", "s.frag"])
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!(
            "glslang -l falhou:\n{}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let read = |name: &str| -> Result<Vec<u32>, String> {
        let bytes = std::fs::read(dir.join(name)).map_err(|e| format!("{name}: {e}"))?;
        Ok(bytes.chunks_exact(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect())
    };
    let vert = read("vert.spv")?;
    let frag = read("frag.spv")?;
    let _ = std::fs::remove_dir_all(&dir);
    Ok((vert, frag))
}

/// Traduz vertex + fragment do WE e compila linkados (UBO unificado).
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

/// Traduz um shader do WE e compila pra SPIR-V.
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

/// Layout refletido de um shader compilado: o bloco de uniforms (WeGlobals) e os
/// bindings de textura/sampler. Serve pra montar o UBO e o bind group corretos.
#[derive(Debug, Default, Clone)]
pub struct Reflection {
    /// Tamanho (bytes) do bloco de uniforms WeGlobals (0 se não houver).
    pub uniform_size: u32,
    /// Offset de cada membro do bloco, por nome (ex.: "g_Brightness" -> 0).
    pub uniform_offsets: HashMap<String, u32>,
    /// Bindings das texturas (globais `Handle` do tipo imagem).
    pub texture_bindings: Vec<u32>,
    /// Bindings dos samplers (globais `Handle` do tipo sampler).
    pub sampler_bindings: Vec<u32>,
}

/// Uma entrada (attribute) do vertex: local (location) e nº de componentes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VertexInput {
    pub location: u32,
    pub components: u32,
}

/// Reflete os attributes de entrada do vertex (a_Position, a_TexCoord, ...) — local
/// e dimensão de cada um. Usado pra montar o layout do vertex buffer casando as
/// locations que o glslang atribuiu.
pub fn reflect_vertex_inputs(spirv: &[u32]) -> Result<Vec<VertexInput>, String> {
    let module = naga::front::spv::Frontend::new(spirv.iter().copied(), &Default::default())
        .parse()
        .map_err(|e| format!("naga spv-in: {e:?}"))?;
    let ep = module.entry_points.first().ok_or("sem entry point")?;
    let mut out = Vec::new();
    for arg in &ep.function.arguments {
        if let Some(naga::Binding::Location { location, .. }) = arg.binding {
            let components = match module.types[arg.ty].inner {
                naga::TypeInner::Scalar(_) => 1,
                naga::TypeInner::Vector { size, .. } => size as u32,
                _ => continue,
            };
            out.push(VertexInput { location, components });
        }
    }
    out.sort_by_key(|v| v.location);
    Ok(out)
}

/// Reflete o SPIR-V usando o naga (o mesmo parser que o wgpu usa).
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
