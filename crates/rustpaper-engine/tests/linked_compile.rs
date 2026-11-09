// Checks that compiling vertex + fragment LINKED unifies the uniform block:
// both SPIR-V outputs must have the SAME UBO size and the same offsets for the
// shared members. This is what allows using the WE vertex together with the fragment
// with a single UBO. Skipped if the WE assets or glslang are missing.

use std::path::PathBuf;

fn shaders_dir() -> Option<PathBuf> {
    if let Ok(d) = std::env::var("WE_SHADERS_DIR") {
        let p = PathBuf::from(d);
        if p.is_dir() {
            return Some(p);
        }
    }
    let d = PathBuf::from("/home/vscode/we-assets/shaders");
    d.is_dir().then_some(d)
}

#[test]
fn linked_ubo_is_unified() {
    let Some(dir) = shaders_dir() else {
        eprintln!("SKIP: no WE assets");
        return;
    };
    let vert = std::fs::read_to_string(dir.join("genericimage2.vert")).unwrap();
    let frag = std::fs::read_to_string(dir.join("genericimage2.frag")).unwrap();

    let (v_spv, f_spv) =
        match rustpaper_engine::shader_compile::compile_linked(&vert, &frag, &[], &dir) {
            Ok(x) => x,
            Err(e) if e.contains("not found") => {
                eprintln!("SKIP: glslang missing ({e})");
                return;
            }
            Err(e) => panic!("compile_linked: {e}"),
        };

    let vr = rustpaper_engine::shader_compile::reflect(&v_spv).unwrap();
    let fr = rustpaper_engine::shader_compile::reflect(&f_spv).unwrap();

    // same block size in both stages (unified)
    assert_eq!(
        vr.uniform_size, fr.uniform_size,
        "UBO must have the same size in both stages"
    );
    // shared members at the same offset (g_Brightness exists in the unified block)
    for shared in ["g_Brightness", "g_ModelViewProjectionMatrix"] {
        if let (Some(a), Some(b)) = (
            vr.uniform_offsets.get(shared),
            fr.uniform_offsets.get(shared),
        ) {
            assert_eq!(a, b, "offset of {shared} must match between vert and frag");
        }
    }
    // the unified block has both the vertex member and the fragment member
    assert!(
        fr.uniform_offsets
            .contains_key("g_ModelViewProjectionMatrix")
    );
    assert!(fr.uniform_offsets.contains_key("g_Brightness"));
    eprintln!(
        "Unified UBO: {} bytes, frag members: {:?}",
        fr.uniform_size, fr.uniform_offsets
    );
}
