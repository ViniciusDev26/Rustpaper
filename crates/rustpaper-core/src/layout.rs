// Módulo `layout`: lê a cena INTEIRA como uma pilha de camadas (não só o fundo).
// Uma cena do WE é uma composição de vários objetos-imagem, cada um com sua textura,
// transform (origin/scale/angles/size), material (shader+combos+constants), blend,
// alpha/cor/brilho e efeitos — desenhados em ordem com alpha. É o que faz o
// personagem principal (Ashe, etc.) aparecer, em vez de só o fundo.

use crate::effects::{self, EffectInstance};
use crate::pkg::Pkg;
use crate::scene::{self, MaterialInfo};

/// Uma camada-imagem da cena, pronta pra desenhar.
#[derive(Debug, Clone)]
pub struct Layer {
    pub name: String,
    pub texture: Option<String>, // caminho do .tex dentro do pkg
    pub material: MaterialInfo,
    pub origin: [f32; 3], // posição do CENTRO no espaço da cena
    pub scale: [f32; 3],
    pub angles: [f32; 3], // rotação (radianos? o WE usa graus? -> ver nota abaixo)
    pub size: [f32; 2],   // tamanho do quad em unidades de cena
    pub color: [f32; 3],
    pub alpha: f32,
    pub brightness: f32,
    pub blend: String, // "normal" | "translucent" | "additive" | ...
    pub effects: Vec<EffectInstance>,
}

/// A cena como um todo: projeção ortográfica + camadas em ordem de desenho.
#[derive(Debug, Clone)]
pub struct SceneLayout {
    pub width: f32,
    pub height: f32,
    pub clear_color: [f32; 3],
    pub layers: Vec<Layer>,
}

fn parse_floats(s: &str) -> Vec<f32> {
    s.split_whitespace()
        .filter_map(|t| t.parse().ok())
        .collect()
}
fn vec3(v: &serde_json::Value, key: &str, default: [f32; 3]) -> [f32; 3] {
    v.get(key)
        .and_then(|x| x.as_str())
        .map(parse_floats)
        .filter(|f| f.len() >= 3)
        .map(|f| [f[0], f[1], f[2]])
        .unwrap_or(default)
}
fn vec2(v: &serde_json::Value, key: &str, default: [f32; 2]) -> [f32; 2] {
    v.get(key)
        .and_then(|x| x.as_str())
        .map(parse_floats)
        .filter(|f| f.len() >= 2)
        .map(|f| [f[0], f[1]])
        .unwrap_or(default)
}
fn scalar(v: &serde_json::Value, key: &str, default: f32) -> f32 {
    v.get(key)
        .and_then(|x| x.as_f64())
        .map(|n| n as f32)
        .unwrap_or(default)
}

// texturas do primeiro pass de um material (mesmo critério do scene::first_texture,
// mas devolve o caminho completo).
fn first_texture_path(mat: &MaterialInfo) -> Option<String> {
    mat.textures
        .iter()
        .flatten()
        .next()
        .map(|t| format!("materials/{t}.tex"))
}

/// Lê a cena inteira do pkg como uma lista de camadas.
pub fn parse_layout(pkg: &Pkg) -> Option<SceneLayout> {
    let scene_json = std::str::from_utf8(pkg.read("scene.json")?).ok()?;
    let scene: serde_json::Value = serde_json::from_str(scene_json).ok()?;

    let general = scene.get("general").cloned().unwrap_or_default();
    let proj = general
        .get("orthogonalprojection")
        .cloned()
        .unwrap_or_default();
    let width = proj.get("width").and_then(|w| w.as_f64()).unwrap_or(1920.0) as f32;
    let height = proj
        .get("height")
        .and_then(|h| h.as_f64())
        .unwrap_or(1080.0) as f32;
    let clear_color = vec3(&general, "clearcolor", [0.0, 0.0, 0.0]);

    let mut layers = Vec::new();
    let empty = Vec::new();
    let objects = scene
        .get("objects")
        .and_then(|o| o.as_array())
        .unwrap_or(&empty);
    for obj in objects {
        // só objetos-imagem visíveis
        let Some(image_path) = obj.get("image").and_then(|i| i.as_str()) else {
            continue;
        };
        if obj
            .get("visible")
            .map(|v| v == &serde_json::Value::Bool(false))
            .unwrap_or(false)
        {
            continue;
        }
        // resolve model -> material -> textura
        let Some(model) = pkg
            .read(image_path)
            .and_then(|b| std::str::from_utf8(b).ok().map(String::from))
        else {
            continue;
        };
        let Some(mat_path) = model_material(&model) else {
            continue;
        };
        let Some(mat_json) = pkg
            .read(&mat_path)
            .and_then(|b| std::str::from_utf8(b).ok().map(String::from))
        else {
            continue;
        };
        let Some(material) = scene::material_info_str(&mat_json) else {
            continue;
        };

        let texture = first_texture_path(&material);
        let effects = obj
            .get("effects")
            .map(effects::effects_from_json)
            .unwrap_or_default();

        layers.push(Layer {
            name: obj
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or("")
                .to_string(),
            texture,
            blend: obj
                .get("blending")
                .and_then(|b| b.as_str())
                .map(String::from)
                .unwrap_or_else(|| material.blending.clone()),
            origin: vec3(obj, "origin", [width / 2.0, height / 2.0, 0.0]),
            scale: vec3(obj, "scale", [1.0, 1.0, 1.0]),
            angles: vec3(obj, "angles", [0.0, 0.0, 0.0]),
            size: vec2(obj, "size", [width, height]),
            color: vec3(obj, "color", [1.0, 1.0, 1.0]),
            alpha: scalar(obj, "alpha", 1.0),
            brightness: scalar(obj, "brightness", 1.0),
            material,
            effects,
        });
    }

    Some(SceneLayout {
        width,
        height,
        clear_color,
        layers,
    })
}

// caminho do material de um model json.
fn model_material(model_json: &str) -> Option<String> {
    #[derive(serde::Deserialize)]
    struct M {
        material: Option<String>,
    }
    serde_json::from_str::<M>(model_json).ok()?.material
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_floats_e_vecs() {
        assert_eq!(parse_floats("1.0 2.0 3.0"), vec![1.0, 2.0, 3.0]);
        let v = serde_json::json!({ "origin": "10 20 0", "alpha": 0.5 });
        assert_eq!(vec3(&v, "origin", [0.0; 3]), [10.0, 20.0, 0.0]);
        assert_eq!(vec3(&v, "missing", [1.0, 1.0, 1.0]), [1.0, 1.0, 1.0]);
        assert_eq!(scalar(&v, "alpha", 1.0), 0.5);
        assert_eq!(scalar(&v, "brightness", 1.0), 1.0);
    }

    #[test]
    fn model_material_extrai() {
        assert_eq!(
            model_material(r#"{"material":"materials/x.json"}"#).as_deref(),
            Some("materials/x.json")
        );
        assert_eq!(model_material(r#"{}"#), None);
    }
}
