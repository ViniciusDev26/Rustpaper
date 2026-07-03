// Módulo `scene`: entende o suficiente do scene.json (e dos JSONs de modelo e
// material) pra achar a textura da imagem de FUNDO. A cadeia no Wallpaper Engine:
//   scene.json  -> objeto com "image": "models/x.json"
//   model json  -> "material": "materials/x.json"
//   material    -> passes[0].textures[0]: "nome-da-textura"
//   arquivo real: "materials/nome-da-textura.tex"

use crate::particle::ParticleSystem;
use crate::pkg::Pkg;

#[derive(serde::Deserialize)]
struct SceneObject {
    #[serde(default)]
    image: Option<String>,
    #[serde(default)]
    particle: Option<String>,
    // posição do objeto na cena ("x y z"); o emitter da partícula é LOCAL a ela.
    #[serde(default)]
    origin: Option<String>,
    // escala do objeto ("x y z"); escala o sistema de partículas (tamanho/velocidade).
    #[serde(default)]
    scale: Option<String>,
}

#[derive(serde::Deserialize)]
struct SceneRaw {
    #[serde(default)]
    objects: Vec<SceneObject>,
}

#[derive(serde::Deserialize)]
struct ModelRaw {
    #[serde(default)]
    material: Option<String>,
}

#[derive(serde::Deserialize)]
struct MaterialPass {
    #[serde(default)]
    shader: Option<String>, // ex.: "genericimage2", "effects/nitro"
    // texturas podem ter nulos no array (slots vazios) -> Option.
    #[serde(default)]
    textures: Vec<Option<String>>,
    #[serde(default)]
    blending: Option<String>, // "translucent" | "additive" | "normal" | ...
    // combos: feature flags do shader ({} quando nenhuma). Valores são inteiros.
    #[serde(default)]
    combos: serde_json::Map<String, serde_json::Value>,
    // constantshadervalues: overrides de parâmetros de material (g_Brightness, etc).
    #[serde(default)]
    constantshadervalues: serde_json::Map<String, serde_json::Value>,
}

#[derive(serde::Deserialize)]
struct MaterialRaw {
    #[serde(default)]
    passes: Vec<MaterialPass>,
}

/// Tudo que precisamos de um pass de material pra compilar+desenhar: o shader, seus
/// combos, as texturas e os overrides de parâmetro. Reutilizável tanto pro material
/// de fundo quanto pros materiais de efeito.
#[derive(Debug, Clone, PartialEq)]
pub struct MaterialInfo {
    pub shader: String,
    pub combos: Vec<(String, i64)>,
    pub textures: Vec<Option<String>>,
    pub blending: String,
    pub constants: Vec<(String, serde_json::Value)>,
}

/// Converte o primeiro pass de um material json (string) em MaterialInfo.
pub fn material_info_str(material_json: &str) -> Option<MaterialInfo> {
    material_info(material_json)
}

// Converte o primeiro pass de um material json em MaterialInfo.
fn material_info(material_json: &str) -> Option<MaterialInfo> {
    let mat: MaterialRaw = serde_json::from_str(material_json).ok()?;
    let pass = mat.passes.into_iter().next()?;
    let shader = pass.shader?;
    // combos: só os valores inteiros (é o que o WE usa nos #if).
    let combos = pass
        .combos
        .into_iter()
        .filter_map(|(k, v)| v.as_i64().map(|n| (k, n)))
        .collect();
    let constants = pass.constantshadervalues.into_iter().collect();
    Some(MaterialInfo {
        shader,
        combos,
        textures: pass.textures,
        blending: pass.blending.unwrap_or_else(|| "translucent".to_string()),
        constants,
    })
}

// O caminho (dentro do pkg) do model do primeiro objeto que tem imagem.
fn first_image_model(scene_json: &str) -> Option<String> {
    let scene: SceneRaw = serde_json::from_str(scene_json).ok()?;
    scene.objects.into_iter().find_map(|o| o.image)
}

// O caminho do material referenciado por um model json.
fn material_of_model(model_json: &str) -> Option<String> {
    let model: ModelRaw = serde_json::from_str(model_json).ok()?;
    model.material
}

// O nome da primeira textura de um material json.
fn first_texture(material_json: &str) -> Option<String> {
    let mat: MaterialRaw = serde_json::from_str(material_json).ok()?;
    mat.passes
        .into_iter()
        .find_map(|p| p.textures.into_iter().flatten().next())
}

// O modo de blending do primeiro pass (default "translucent").
fn first_blending(material_json: &str) -> String {
    serde_json::from_str::<MaterialRaw>(material_json)
        .ok()
        .and_then(|m| m.passes.into_iter().next())
        .and_then(|p| p.blending)
        .unwrap_or_else(|| "translucent".to_string())
}

// Integra tudo: dado um pkg de cena, resolve o caminho do .tex de fundo.
pub fn background_texture(pkg: &Pkg) -> Option<String> {
    let scene = std::str::from_utf8(pkg.read("scene.json")?).ok()?;
    let model_path = first_image_model(scene)?;

    let model = std::str::from_utf8(pkg.read(&model_path)?).ok()?;
    let material_path = material_of_model(model)?;

    let material = std::str::from_utf8(pkg.read(&material_path)?).ok()?;
    let tex_name = first_texture(material)?;

    Some(format!("materials/{tex_name}.tex"))
}

// Como background_texture, mas devolve o MaterialInfo completo do fundo (shader,
// combos, texturas, constantes) — a base pra renderizar o fundo pelo material real.
pub fn background_material(pkg: &Pkg) -> Option<MaterialInfo> {
    let scene = std::str::from_utf8(pkg.read("scene.json")?).ok()?;
    let model_path = first_image_model(scene)?;
    let model = std::str::from_utf8(pkg.read(&model_path)?).ok()?;
    let material_path = material_of_model(model)?;
    let material = std::str::from_utf8(pkg.read(&material_path)?).ok()?;
    material_info(material)
}

// Um sistema de partículas da cena, já com o nome da textura do sprite resolvido.
pub struct SceneParticles {
    pub system: ParticleSystem,
    pub texture: String,  // ex.: "particle/halo" (o engine resolve pro .tex)
    pub additive: bool,   // blend do material: additive (luz) vs translucent
    pub origin: [f32; 3], // posição do objeto na cena (soma-se ao emitter local)
    pub scale: f32,       // escala do objeto (multiplica tamanho/velocidade/distância)
}

// Extrai todos os sistemas de partículas da cena (objetos com "particle").
pub fn particle_systems(pkg: &Pkg) -> Vec<SceneParticles> {
    let Some(scene_json) = pkg
        .read("scene.json")
        .and_then(|b| std::str::from_utf8(b).ok())
    else {
        return Vec::new();
    };
    let Ok(scene) = serde_json::from_str::<SceneRaw>(scene_json) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for obj in scene.objects {
        let Some(ppath) = obj.particle else { continue };
        let Some(pjson) = pkg.read(&ppath).and_then(|b| std::str::from_utf8(b).ok()) else {
            continue;
        };
        let Ok(system) = ParticleSystem::parse(pjson) else {
            continue;
        };
        // material -> nome da textura do sprite + modo de blend
        let material_json = pkg
            .read(&system.material)
            .and_then(|b| std::str::from_utf8(b).ok());
        let texture = material_json.and_then(first_texture).unwrap_or_default();
        let additive = material_json.map(first_blending).as_deref() == Some("additive");
        // origin do OBJETO (o emitter da partícula é local a ele).
        let origin = obj
            .origin
            .as_deref()
            .map(|s| {
                let f: Vec<f32> = s
                    .split_whitespace()
                    .filter_map(|t| t.parse().ok())
                    .collect();
                [
                    *f.first().unwrap_or(&0.0),
                    *f.get(1).unwrap_or(&0.0),
                    *f.get(2).unwrap_or(&0.0),
                ]
            })
            .unwrap_or([0.0; 3]);
        // escala do objeto (usa o X; a maioria é uniforme).
        let scale = obj
            .scale
            .as_deref()
            .and_then(|s| {
                s.split_whitespace()
                    .next()
                    .and_then(|t| t.parse::<f32>().ok())
            })
            .filter(|s| *s > 0.0)
            .unwrap_or(1.0);
        out.push(SceneParticles {
            system,
            texture,
            additive,
            origin,
            scale,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_first_image_model() {
        let scene = r#"{ "objects": [
            { "name": "particles", "particle": "p.json" },
            { "name": "bg", "image": "models/bg.json" }
        ] }"#;
        assert_eq!(first_image_model(scene), Some("models/bg.json".to_string()));
    }

    #[test]
    fn reads_material_from_model() {
        let model = r#"{ "autosize": true, "material": "materials/bg.json" }"#;
        assert_eq!(
            material_of_model(model),
            Some("materials/bg.json".to_string())
        );
    }

    #[test]
    fn reads_first_texture_skipping_nulls() {
        let material = r#"{ "passes": [ { "shader": "genericimage2",
            "textures": [ null, "the-texture" ] } ] }"#;
        assert_eq!(first_texture(material), Some("the-texture".to_string()));
    }

    #[test]
    fn material_info_extrai_shader_combos_constantes() {
        let material = r#"{ "passes": [ {
            "shader": "genericimage2",
            "blending": "translucent",
            "combos": { "LIGHTING": 1, "BLENDMODE": 3 },
            "constantshadervalues": { "brightness": 0.5 },
            "textures": [ "bg", null ]
        } ] }"#;
        let info = material_info(material).unwrap();
        assert_eq!(info.shader, "genericimage2");
        assert_eq!(info.blending, "translucent");
        // combos ordenados? não garantimos ordem; checa como conjunto
        assert!(info.combos.contains(&("LIGHTING".into(), 1)));
        assert!(info.combos.contains(&("BLENDMODE".into(), 3)));
        assert_eq!(info.textures, vec![Some("bg".to_string()), None]);
        assert_eq!(info.constants.len(), 1);
        assert_eq!(info.constants[0].0, "brightness");
    }

    #[test]
    fn material_info_sem_shader_e_none() {
        assert_eq!(
            material_info(r#"{ "passes": [ { "textures": ["x"] } ] }"#),
            None
        );
        assert_eq!(material_info(r#"{ "passes": [] }"#), None);
    }

    #[test]
    fn missing_pieces_return_none() {
        assert_eq!(first_image_model(r#"{ "objects": [] }"#), None);
        assert_eq!(material_of_model(r#"{}"#), None);
        assert_eq!(first_texture(r#"{ "passes": [] }"#), None);
    }
}
