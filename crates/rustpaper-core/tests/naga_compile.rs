// Prova do "boss final": o pipeline WE→GLSL→SPIR-V→(naga, o que o wgpu usa)
// funciona ponta-a-ponta com os shaders REAIS do WE?
//
// Pipeline:
//   1. shader::translate  -> GLSL 330 (dialeto WE resolvido + samplers SEPARADOS)
//   2. glslangValidator   -> SPIR-V Vulkan
//      -V (Vulkan) -R (regras relaxadas: permite uniforms livres estilo GL)
//      --amb --aml (auto-bind/locate) --sdub WeGlobals 0 0 (junta os uniforms
//      livres num bloco no set 0, binding 0)
//   3. naga spv-in        -> Module + validação (é o que o wgpu faz internamente)
//
// Descobertas que moldaram esse caminho (ver shader.rs / SHADER_TRANSLATION.md):
//   - naga glsl-in é fraco demais (rejeita uniforms livres e qualifiers de sampler)
//   - naga spv-in rejeita samplers COMBINADOS -> por isso separamos em translate()
//
// Precisa dos assets do WE ($WE_SHADERS_DIR ou /home/vscode/we-assets/shaders) e do
// glslangValidator no PATH. Se faltar, o teste é PULADO (não falha).

use std::path::{Path, PathBuf};
use std::process::Command;
use rustpaper_core::shader::{translate, Stage};

fn shaders_dir() -> Option<PathBuf> {
    if let Ok(d) = std::env::var("WE_SHADERS_DIR") {
        let p = PathBuf::from(d);
        if p.is_dir() {
            return Some(p);
        }
    }
    let default = PathBuf::from("/home/vscode/we-assets/shaders");
    default.is_dir().then_some(default)
}

fn have_glslang() -> bool {
    Command::new("glslangValidator")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

// Traduz + compila pra SPIR-V via glslang. Retorna as palavras SPIR-V.
fn to_spirv(stage: Stage, src: &str, dir: &Path) -> Result<Vec<u32>, String> {
    let glsl = translate(stage, src, &[], dir)?;
    let tmp = std::env::temp_dir();
    let (ext, sflag) = match stage {
        Stage::Vertex => ("vert", "vert"),
        Stage::Fragment => ("frag", "frag"),
    };
    let in_path = tmp.join(format!("we_test.{ext}"));
    let out_path = tmp.join(format!("we_test.{ext}.spv"));
    std::fs::write(&in_path, &glsl).map_err(|e| e.to_string())?;

    let out = Command::new("glslangValidator")
        .args(["-V", "-R", "--amb", "--aml", "--sdub", "WeGlobals", "0", "0", "-S", sflag])
        .arg(&in_path)
        .arg("-o")
        .arg(&out_path)
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        // dump numerado pra depurar o que o glslang rejeitou
        for (i, l) in glsl.lines().enumerate() {
            eprintln!("{:4} | {}", i + 1, l);
        }
        return Err(format!(
            "glslang falhou:\n{}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let bytes = std::fs::read(&out_path).map_err(|e| e.to_string())?;
    Ok(bytes
        .chunks_exact(4)
        .map(|c| u32::from_le_bytes(c.try_into().unwrap()))
        .collect())
}

#[test]
fn genericimage2_pipeline_completo() {
    let Some(dir) = shaders_dir() else {
        eprintln!("SKIP: assets do WE não encontrados (defina WE_SHADERS_DIR)");
        return;
    };
    if !have_glslang() {
        eprintln!("SKIP: glslangValidator não está no PATH");
        return;
    }

    for (stage, ext) in [(Stage::Vertex, "vert"), (Stage::Fragment, "frag")] {
        let src = std::fs::read_to_string(dir.join(format!("genericimage2.{ext}"))).unwrap();
        let spirv = to_spirv(stage, &src, &dir).expect("translate+glslang");

        // naga spv-in + validação: exatamente o que o wgpu faz.
        let module = naga::front::spv::Frontend::new(
            spirv.iter().copied(),
            &naga::front::spv::Options::default(),
        )
        .parse()
        .unwrap_or_else(|e| panic!("naga rejeitou o SPIR-V do {ext}: {e:?}"));

        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap_or_else(|e| panic!("validação naga falhou ({ext}): {e:?}"));

        eprintln!("--- {ext}: {} globais ---", module.global_variables.len());
        for (_, gv) in module.global_variables.iter() {
            eprintln!("  {:?} space={:?} binding={:?}", gv.name, gv.space, gv.binding);
        }
    }
}
