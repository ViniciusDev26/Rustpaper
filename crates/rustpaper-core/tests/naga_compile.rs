// Proof of the "final boss": does the WE→GLSL→SPIR-V→(naga, what wgpu uses)
// pipeline work end to end with the REAL WE shaders?
//
// Pipeline:
//   1. shader::translate  -> GLSL 330 (WE dialect resolved + SEPARATE samplers)
//   2. glslangValidator   -> SPIR-V Vulkan
//      -V (Vulkan) -R (relaxed rules: allows GL-style free uniforms)
//      --amb --aml (auto-bind/locate) --sdub WeGlobals 0 0 (gathers the free
//      uniforms into one block at set 0, binding 0)
//   3. naga spv-in        -> Module + validation (this is what wgpu does internally)
//
// Findings that shaped this path (see shader.rs / SHADER_TRANSLATION.md):
//   - naga glsl-in is too weak (rejects free uniforms and sampler qualifiers)
//   - naga spv-in rejects COMBINED samplers -> hence we split them in translate()
//
// Needs the WE assets ($WE_SHADERS_DIR or /home/vscode/we-assets/shaders) and
// glslangValidator on the PATH. If either is missing, the test is SKIPPED (does not fail).

use rustpaper_core::shader::{Stage, translate};
use std::path::{Path, PathBuf};
use std::process::Command;

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

// Translates + compiles to SPIR-V via glslang. Returns the SPIR-V words.
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
            sflag,
        ])
        .arg(&in_path)
        .arg("-o")
        .arg(&out_path)
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        // numbered dump to debug what glslang rejected
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
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| u32::from_le_bytes(*c))
        .collect())
}

#[test]
fn genericimage2_pipeline_completo() {
    let Some(dir) = shaders_dir() else {
        eprintln!("SKIP: WE assets not found (set WE_SHADERS_DIR)");
        return;
    };
    if !have_glslang() {
        eprintln!("SKIP: glslangValidator is not on the PATH");
        return;
    }

    for (stage, ext) in [(Stage::Vertex, "vert"), (Stage::Fragment, "frag")] {
        let src = std::fs::read_to_string(dir.join(format!("genericimage2.{ext}"))).unwrap();
        let spirv = to_spirv(stage, &src, &dir).expect("translate+glslang");

        // naga spv-in + validation: exactly what wgpu does.
        let module = naga::front::spv::Frontend::new(
            spirv.iter().copied(),
            &naga::front::spv::Options::default(),
        )
        .parse()
        .unwrap_or_else(|e| panic!("naga rejected the SPIR-V for {ext}: {e:?}"));

        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap_or_else(|e| panic!("naga validation failed ({ext}): {e:?}"));

        eprintln!("--- {ext}: {} globais ---", module.global_variables.len());
        for (_, gv) in module.global_variables.iter() {
            eprintln!(
                "  {:?} space={:?} binding={:?}",
                gv.name, gv.space, gv.binding
            );
        }
    }
}
