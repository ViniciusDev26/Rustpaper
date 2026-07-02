// Módulo `shader`: traduz o "dialeto" de shader do Wallpaper Engine para GLSL 330
// que o naga (frontend GLSL do wgpu) consegue compilar.
//
// Os shaders do WE (`.vert`/`.frag` nos assets base) NÃO são GLSL puro: são um
// dialeto cross-compile (HLSL+GLSL) com:
//   - `#include "common_*.h"`  — includes próprios (resolvidos a partir de shaders/)
//   - macros estilo HLSL: `mul()`, `frac`, `saturate`, `lerp`, `CAST3`, `texSample2D`...
//   - GLSL legado: `attribute`, `varying`, `gl_FragColor`
//   - `#if COMBO` — feature flags ("combos") setadas por material
//
// A estratégia (mesma do linux-wallpaperengine, ver ShaderUnit.cpp) é montar um
// PRELÚDIO de `#define`s que mapeia o dialeto pra GLSL 330 e prependê-lo ao código
// com os includes resolvidos e os combos definidos. O resultado é GLSL que o
// preprocessador+frontend do naga entende. Aqui só produzimos o TEXTO GLSL; quem
// compila pra GPU é o engine (via `wgpu::ShaderSource::Glsl`).

use std::collections::HashSet;
use std::path::Path;

/// Estágio do pipeline — muda quais `#define`s de compatibilidade entram e como
/// `varying` é traduzido (saída no vertex, entrada no fragment).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Vertex,
    Fragment,
}

// Prelúdio comum a todos os estágios: mapeia o dialeto WE (HLSL-ish) pra GLSL.
// Copiado 1:1 do SHADER_HEADER do linux-wallpaperengine (ground truth) — cada
// linha resolve uma construção que o naga não engoliria crua.
// #version 450 (não 330 como o lwe): precisamos de `layout(binding=)` explícito
// nos samplers separados, que só existe a partir do 420.
const PRELUDE: &str = "#version 450\n\
    precision highp float;\n\
    #define mul(x, y) ((y) * (x))\n\
    #define max(x, y) max(y, x)\n\
    #define lerp mix\n\
    #define frac fract\n\
    #define CAST2(x) (vec2(x))\n\
    #define CAST3(x) (vec3(x))\n\
    #define CAST4(x) (vec4(x))\n\
    #define CAST3X3(x) (mat3(x))\n\
    #define float2 vec2\n\
    #define float3 vec3\n\
    #define float4 vec4\n\
    #define int2 ivec2\n\
    #define int3 ivec3\n\
    #define int4 ivec4\n\
    #define saturate(x) (clamp(x, 0.0, 1.0))\n\
    #define texSample2D(tex, coord) texture(sampler2D(tex, _smp_##tex), coord)\n\
    #define texSample2DLod(tex, coord, lod) textureLod(sampler2D(tex, _smp_##tex), coord, lod)\n\
    #define log10(x) (log2(x) * 0.301029995663981)\n\
    #define atan2 atan\n\
    #define fmod(x, y) ((x)-(y)*trunc((x)/(y)))\n\
    #define ddx dFdx\n\
    #define ddy(x) dFdy(-(x))\n\
    #define GLSL 1\n";

// No fragment, `varying` é ENTRADA e existe um destino de cor explícito.
const FRAGMENT_DEFINES: &str = "out vec4 out_FragColor;\n#define varying in\n";
// No vertex, `attribute` é entrada e `varying` é SAÍDA.
const VERTEX_DEFINES: &str = "#define attribute in\n#define varying out\n";

/// Traduz um shader do WE pra GLSL 330.
///
/// - `stage`: vertex ou fragment.
/// - `source`: conteúdo bruto do `.vert`/`.frag`.
/// - `combos`: pares (NOME, valor) das feature flags do material. Viram
///   `#define NOME valor` (nome em maiúsculas, como o WE espera nos `#if`).
/// - `shaders_dir`: pasta dos shaders base (ex.: `we-assets/shaders`) pra
///   resolver os `#include`.
pub fn translate(
    stage: Stage,
    source: &str,
    combos: &[(String, i64)],
    shaders_dir: &Path,
) -> Result<String, String> {
    // 1) resolve os includes recursivamente (o naga não tem #include).
    let mut seen = HashSet::new();
    let with_includes = resolve_includes(source, shaders_dir, &mut seen)?;

    // 1b) resolve `#require` (módulos gerados pelo WE, ex.: LightingV1).
    let with_includes = resolve_requires(&with_includes);

    // 2) gl_FragColor -> out_FragColor (destino declarado no prelúdio do fragment).
    let body = with_includes.replace("gl_FragColor", "out_FragColor");

    // 3) separa samplers combinados (`sampler2D`, estilo GL) em texture2D +
    // sampler (estilo Vulkan/wgpu). Ver `separate_samplers`.
    let body = separate_samplers(&body);

    // 4) monta o resultado: prelúdio + defines do estágio + combos + corpo.
    let mut out = String::with_capacity(PRELUDE.len() + body.len() + 512);
    out.push_str(PRELUDE);
    out.push_str(match stage {
        Stage::Vertex => VERTEX_DEFINES,
        Stage::Fragment => FRAGMENT_DEFINES,
    });
    for (name, value) in combos {
        out.push_str(&format!("#define {} {}\n", name.to_uppercase(), value));
    }
    out.push('\n');
    out.push_str(&body);
    Ok(out)
}

// Expande `#include "arquivo.h"` recursivamente, inserindo o conteúdo no lugar da
// diretiva. Cada arquivo é incluído no MÁXIMO uma vez (dedupe por nome) — funciona
// como include guard, evitando redefinições (GLSL não tem `#pragma once`).
fn resolve_includes(
    source: &str,
    shaders_dir: &Path,
    seen: &mut HashSet<String>,
) -> Result<String, String> {
    let mut out = String::with_capacity(source.len());
    for line in source.lines() {
        if let Some(name) = parse_include(line) {
            if seen.insert(name.clone()) {
                let path = shaders_dir.join(&name);
                let content = std::fs::read_to_string(&path)
                    .map_err(|e| format!("include {name:?} não encontrado ({}): {e}", path.display()))?;
                out.push_str(&format!("// begin include {name}\n"));
                out.push_str(&resolve_includes(&content, shaders_dir, seen)?);
                out.push_str(&format!("\n// end include {name}\n"));
            }
            // já incluído antes: omite (dedupe)
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    Ok(out)
}

// Stub do módulo LightingV1: no WE, `PerformLighting_V1` é gerado dinamicamente a
// partir das luzes da cena. Como (assim como o linux-wallpaperengine) ainda não há
// suporte a objetos de luz, geramos um stub que não contribui luz dinâmica. Sem
// isso os shaders que dão `#require LightingV1` (ex.: genericimage4) não compilam.
const LIGHTING_V1_STUB: &str = "// generated module LightingV1\n\
    vec3 PerformLighting_V1(vec3 worldPos, vec3 albedo, vec3 normal, vec3 viewDir,\n\
    vec3 specularTint, vec3 baseReflectance, float roughness, float metallic) {\n\
    return vec3(0.0);\n\
    }\n";

// Substitui diretivas `#require <módulo>` pelo código do módulo. Só conhecemos
// LightingV1; outras viram comentário (com aviso inline) pra não quebrar o parse.
fn resolve_requires(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    for line in source.lines() {
        if let Some(module) = line.trim_start().strip_prefix("#require") {
            match module.trim() {
                "LightingV1" => out.push_str(LIGHTING_V1_STUB),
                other => out.push_str(&format!("// #require {other} (módulo não suportado)\n")),
            }
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// Binding do bloco de uniforms livres (WeGlobals) no descriptor set 0.
pub const UNIFORM_BLOCK_BINDING: u32 = 0;

/// Bindings de uma textura `g_TextureN`: a imagem e o sampler que a acompanha.
/// Reservamos o binding 0 pro bloco de uniforms, então N=0 → (1, 2), N=1 → (3, 4)...
/// Determinístico (deriva do índice, não da ordem) pra bater entre vertex e fragment.
pub fn texture_bindings(index: u32) -> (u32, u32) {
    (2 * index + 1, 2 * index + 2)
}

// Reescreve declarações `uniform sampler2D g_TextureN;` (samplers COMBINADOS,
// estilo OpenGL) em duas declarações separadas estilo Vulkan/wgpu:
//   layout(binding = t) uniform texture2D g_TextureN;
//   layout(binding = s) uniform sampler   _smp_g_TextureN;
// As macros texSample2D/Lod no prelúdio remontam `sampler2D(tex, _smp_tex)` na hora
// de amostrar. Isso é necessário porque o wgpu (WebGPU) usa textura e sampler como
// bindings distintos — não existe sampler combinado como no GL.
fn separate_samplers(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    for line in source.lines() {
        if let Some((name, comment)) = parse_sampler_decl(line) {
            let (tb, sb) = match name.strip_prefix("g_Texture").and_then(|s| {
                let digits: String = s.chars().take_while(|c| c.is_ascii_digit()).collect();
                digits.parse::<u32>().ok()
            }) {
                Some(idx) => texture_bindings(idx),
                // Sampler sem o padrão g_TextureN: joga em bindings altos pra não
                // colidir com os das texturas numeradas nem com o bloco.
                None => (900, 901),
            };
            out.push_str(&format!(
                "layout(binding = {tb}) uniform texture2D {name}; // {comment}\n\
                 layout(binding = {sb}) uniform sampler _smp_{name};\n"
            ));
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

// Reconhece `uniform sampler2D NOME;` (com espaços variáveis e comentário opcional).
// Retorna (nome, comentário_sem_barras). None se a linha não for isso.
fn parse_sampler_decl(line: &str) -> Option<(String, String)> {
    let t = line.trim();
    let rest = t.strip_prefix("uniform")?;
    let rest = rest.trim_start().strip_prefix("sampler2D")?;
    // precisa de um espaço após o tipo (senão "sampler2DArray" casaria)
    if !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let rest = rest.trim_start();
    let semi = rest.find(';')?;
    let name = rest[..semi].trim();
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    let comment = rest[semi + 1..].trim().trim_start_matches('/').trim().to_string();
    Some((name.to_string(), comment))
}

// Extrai o nome de um `#include "x.h"` (ignorando espaços). Retorna None se a
// linha não for um include.
fn parse_include(line: &str) -> Option<String> {
    let t = line.trim_start();
    let rest = t.strip_prefix("#include")?;
    let start = rest.find('"')? + 1;
    let end = rest[start..].find('"')? + start;
    Some(rest[start..end].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_include_extrai_nome() {
        assert_eq!(parse_include("#include \"common_pbr.h\"").as_deref(), Some("common_pbr.h"));
        assert_eq!(parse_include("  #include   \"a.h\"  ").as_deref(), Some("a.h"));
        assert_eq!(parse_include("uniform float x;"), None);
        assert_eq!(parse_include("// #include comentado"), None);
    }

    #[test]
    fn translate_prepende_prelude_e_combos() {
        let dir = std::env::temp_dir();
        let src = "varying vec2 v_TexCoord;\nvoid main() { gl_FragColor = vec4(1.0); }\n";
        let out = translate(Stage::Fragment, src, &[("lighting".into(), 1)], &dir).unwrap();
        // prelúdio presente
        assert!(out.starts_with("#version 450"));
        assert!(out.contains("#define mul(x, y) ((y) * (x))"));
        // define do estágio fragment
        assert!(out.contains("out vec4 out_FragColor;"));
        assert!(out.contains("#define varying in"));
        // combo em maiúsculas
        assert!(out.contains("#define LIGHTING 1"));
        // gl_FragColor reescrito
        assert!(out.contains("out_FragColor = vec4(1.0)"));
        assert!(!out.contains("gl_FragColor"));
    }

    #[test]
    fn separa_sampler_combinado() {
        assert_eq!(
            parse_sampler_decl("uniform sampler2D g_Texture0; // {\"label\":\"x\"}"),
            Some(("g_Texture0".into(), "{\"label\":\"x\"}".into()))
        );
        assert_eq!(parse_sampler_decl("uniform float g_Brightness;"), None);
        assert_eq!(parse_sampler_decl("uniform sampler2DArray g_X;"), None);

        let out = separate_samplers("uniform sampler2D g_Texture1; // nota\n");
        // g_Texture1 -> texture binding 3, sampler binding 4
        assert!(out.contains("layout(binding = 3) uniform texture2D g_Texture1;"));
        assert!(out.contains("layout(binding = 4) uniform sampler _smp_g_Texture1;"));
    }

    #[test]
    fn resolve_requires_gera_stub_lighting() {
        let out = resolve_requires("a\n#require LightingV1\nb\n");
        assert!(out.contains("vec3 PerformLighting_V1("));
        assert!(!out.contains("#require LightingV1"));
        // módulo desconhecido vira comentário, não some nem quebra
        let unk = resolve_requires("#require Foo\n");
        assert!(unk.contains("// #require Foo"));
    }

    #[test]
    fn texture_bindings_reserva_binding_0_pro_bloco() {
        assert_eq!(UNIFORM_BLOCK_BINDING, 0);
        assert_eq!(texture_bindings(0), (1, 2));
        assert_eq!(texture_bindings(3), (7, 8));
    }

    #[test]
    fn resolve_includes_expande_e_dedup() {
        let dir = std::env::temp_dir().join("we_shader_test_inc");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.h"), "float helper() { return 1.0; }\n").unwrap();
        let src = "#include \"a.h\"\n#include \"a.h\"\nvoid main() {}\n";
        let mut seen = HashSet::new();
        let out = resolve_includes(src, &dir, &mut seen).unwrap();
        // incluído uma vez só
        assert_eq!(out.matches("float helper()").count(), 1);
        assert!(out.contains("void main()"));
    }
}
