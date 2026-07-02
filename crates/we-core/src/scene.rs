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
    // texturas podem ter nulos no array (slots vazios) -> Option.
    #[serde(default)]
    textures: Vec<Option<String>>,
    #[serde(default)]
    blending: Option<String>, // "translucent" | "additive" | ...
}

#[derive(serde::Deserialize)]
struct MaterialRaw {
    #[serde(default)]
    passes: Vec<MaterialPass>,
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
    mat.passes.into_iter().find_map(|p| p.textures.into_iter().flatten().next())
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

// Um sistema de partículas da cena, já com o nome da textura do sprite resolvido.
pub struct SceneParticles {
    pub system: ParticleSystem,
    pub texture: String,    // ex.: "particle/halo" (o engine resolve pro .tex)
    pub additive: bool,     // blend do material: additive (luz) vs translucent
}

// Extrai todos os sistemas de partículas da cena (objetos com "particle").
pub fn particle_systems(pkg: &Pkg) -> Vec<SceneParticles> {
    let Some(scene_json) = pkg.read("scene.json").and_then(|b| std::str::from_utf8(b).ok()) else {
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
        let Ok(system) = ParticleSystem::parse(pjson) else { continue };
        // material -> nome da textura do sprite + modo de blend
        let material_json = pkg.read(&system.material).and_then(|b| std::str::from_utf8(b).ok());
        let texture = material_json.and_then(first_texture).unwrap_or_default();
        let additive = material_json.map(first_blending).as_deref() == Some("additive");
        out.push(SceneParticles { system, texture, additive });
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
        assert_eq!(material_of_model(model), Some("materials/bg.json".to_string()));
    }

    #[test]
    fn reads_first_texture_skipping_nulls() {
        let material = r#"{ "passes": [ { "shader": "genericimage2",
            "textures": [ null, "the-texture" ] } ] }"#;
        assert_eq!(first_texture(material), Some("the-texture".to_string()));
    }

    #[test]
    fn missing_pieces_return_none() {
        assert_eq!(first_image_model(r#"{ "objects": [] }"#), None);
        assert_eq!(material_of_model(r#"{}"#), None);
        assert_eq!(first_texture(r#"{ "passes": [] }"#), None);
    }
}
