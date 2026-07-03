// Verifica que compilar vertex + fragment LINKADOS unifica o bloco de uniforms:
// os dois SPIR-V devem ter o MESMO tamanho de UBO e os mesmos offsets pros membros
// compartilhados. É o que permite usar o vertex do WE junto do fragment com um UBO
// único. Pulado se faltar assets do WE ou glslang.

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
fn linked_ubo_e_unificado() {
    let Some(dir) = shaders_dir() else {
        eprintln!("SKIP: sem assets do WE");
        return;
    };
    let vert = std::fs::read_to_string(dir.join("genericimage2.vert")).unwrap();
    let frag = std::fs::read_to_string(dir.join("genericimage2.frag")).unwrap();

    let (v_spv, f_spv) = match rustpaper_engine::shader_compile::compile_linked(&vert, &frag, &[], &dir) {
        Ok(x) => x,
        Err(e) if e.contains("não encontrado") => {
            eprintln!("SKIP: glslang ausente ({e})");
            return;
        }
        Err(e) => panic!("compile_linked: {e}"),
    };

    let vr = rustpaper_engine::shader_compile::reflect(&v_spv).unwrap();
    let fr = rustpaper_engine::shader_compile::reflect(&f_spv).unwrap();

    // mesmo tamanho de bloco nos dois estágios (unificado)
    assert_eq!(vr.uniform_size, fr.uniform_size, "UBO deve ter o mesmo tamanho nos dois estágios");
    // membros compartilhados no mesmo offset (g_Brightness existe no bloco unificado)
    for shared in ["g_Brightness", "g_ModelViewProjectionMatrix"] {
        if let (Some(a), Some(b)) = (vr.uniform_offsets.get(shared), fr.uniform_offsets.get(shared)) {
            assert_eq!(a, b, "offset de {shared} deve bater entre vert e frag");
        }
    }
    // o bloco unificado tem tanto o membro do vertex quanto o do fragment
    assert!(fr.uniform_offsets.contains_key("g_ModelViewProjectionMatrix"));
    assert!(fr.uniform_offsets.contains_key("g_Brightness"));
    eprintln!("UBO unificado: {} bytes, membros do frag: {:?}", fr.uniform_size, fr.uniform_offsets);
}
