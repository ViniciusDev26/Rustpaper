// Módulo `effects`: entende o grafo de efeitos de uma cena do Wallpaper Engine.
//
// Modelo (dois níveis):
//   1. O OBJETO da cena tem `effects: [ { file, visible, passes: [override...] } ]`.
//      Cada `file` aponta pra um effect.json; os `passes` trazem OVERRIDES (combos,
//      constantshadervalues, textures) por índice de pass.
//   2. O effect.json tem `passes: [ { material, bind } ]` e `fbos: [ {name,scale,format} ]`.
//      O `material` (json) tem shader + combos + constants base; o `bind` mapeia de
//      onde vem cada textura de entrada do pass (um FBO nomeado ou o quadro anterior).
//
// Aqui só PARSEAMOS e resolvemos (merge dos overrides sobre a base). Quem renderiza
// os passes é o engine.

use serde_json::{Map, Value};

/// Override que a cena aplica sobre um pass do efeito (por índice).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PassOverride {
    pub combos: Vec<(String, i64)>,
    pub constants: Vec<(String, Value)>,
    pub textures: Vec<Option<String>>,
}

/// Uso de um efeito por um objeto da cena.
#[derive(Debug, Clone, PartialEq)]
pub struct EffectInstance {
    pub file: String, // caminho do effect.json dentro do pkg
    pub visible: bool,
    pub passes: Vec<PassOverride>, // overrides por índice de pass
}

// ---- structs de desserialização (scene.json: objeto -> effects) ----

#[derive(serde::Deserialize)]
struct RawPassOverride {
    #[serde(default)]
    combos: Map<String, Value>,
    #[serde(default)]
    constantshadervalues: Map<String, Value>,
    #[serde(default)]
    textures: Vec<Option<String>>,
}

#[derive(serde::Deserialize)]
struct RawEffect {
    file: String,
    #[serde(default = "default_true")]
    visible: bool,
    #[serde(default)]
    passes: Vec<RawPassOverride>,
}

fn default_true() -> bool {
    true
}

#[derive(serde::Deserialize)]
struct RawObject {
    #[serde(default)]
    image: Option<String>,
    #[serde(default)]
    effects: Vec<RawEffect>,
}

#[derive(serde::Deserialize)]
struct RawScene {
    #[serde(default)]
    objects: Vec<RawObject>,
}

fn combos_of(m: Map<String, Value>) -> Vec<(String, i64)> {
    m.into_iter()
        .filter_map(|(k, v)| v.as_i64().map(|n| (k, n)))
        .collect()
}

fn to_override(r: RawPassOverride) -> PassOverride {
    PassOverride {
        combos: combos_of(r.combos),
        constants: r.constantshadervalues.into_iter().collect(),
        textures: r.textures,
    }
}

/// Constrói as instâncias de efeito a partir do valor JSON do campo `effects` de um
/// objeto (um array). Usado pelo compositor pra pegar os efeitos de CADA camada.
pub fn effects_from_json(effects_array: &Value) -> Vec<EffectInstance> {
    serde_json::from_value::<Vec<RawEffect>>(effects_array.clone())
        .ok()
        .map(|es| {
            es.into_iter()
                .map(|e| EffectInstance {
                    file: e.file,
                    visible: e.visible,
                    passes: e.passes.into_iter().map(to_override).collect(),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Efeitos aplicados ao PRIMEIRO objeto-imagem (o fundo). Vazio se não houver.
pub fn background_effects(scene_json: &str) -> Vec<EffectInstance> {
    let Ok(scene) = serde_json::from_str::<RawScene>(scene_json) else {
        return Vec::new();
    };
    scene
        .objects
        .into_iter()
        .find(|o| o.image.is_some())
        .map(|o| {
            o.effects
                .into_iter()
                .map(|e| EffectInstance {
                    file: e.file,
                    visible: e.visible,
                    passes: e.passes.into_iter().map(to_override).collect(),
                })
                .collect()
        })
        .unwrap_or_default()
}

// ---- effect.json ----

/// De onde vem uma textura de entrada de um pass: um FBO nomeado (ou "previous").
#[derive(Debug, Clone, PartialEq)]
pub struct Bind {
    pub name: String,
    pub index: u32, // slot de textura (g_TextureN)
}

/// Um pass do efeito, como definido no effect.json.
#[derive(Debug, Clone, PartialEq)]
pub struct EffectPass {
    pub material: String, // caminho do material.json
    pub binds: Vec<Bind>,
    pub target: Option<String>, // FBO de saída (None = a saída padrão do efeito)
}

/// Um render target nomeado do efeito.
#[derive(Debug, Clone, PartialEq)]
pub struct Fbo {
    pub name: String,
    pub scale: f32,     // 1 = resolução cheia, 2 = metade, ...
    pub format: String, // ex.: "rgba_backbuffer"
}

/// effect.json parseado.
#[derive(Debug, Clone, PartialEq)]
pub struct EffectDef {
    pub passes: Vec<EffectPass>,
    pub fbos: Vec<Fbo>,
}

#[derive(serde::Deserialize)]
struct RawBind {
    name: String,
    #[serde(default)]
    index: u32,
}

#[derive(serde::Deserialize)]
struct RawEffectPass {
    #[serde(default)]
    material: Option<String>,
    #[serde(default)]
    bind: Vec<RawBind>,
    #[serde(default)]
    target: Option<String>,
}

#[derive(serde::Deserialize)]
struct RawFbo {
    name: String,
    #[serde(default = "default_scale")]
    scale: f32,
    #[serde(default)]
    format: String,
}

fn default_scale() -> f32 {
    1.0
}

#[derive(serde::Deserialize)]
struct RawEffectDef {
    #[serde(default)]
    passes: Vec<RawEffectPass>,
    #[serde(default)]
    fbos: Vec<RawFbo>,
}

/// Parseia um effect.json. Passes sem `material` são ignorados.
pub fn parse_effect(effect_json: &str) -> Option<EffectDef> {
    let raw: RawEffectDef = serde_json::from_str(effect_json).ok()?;
    let passes = raw
        .passes
        .into_iter()
        .filter_map(|p| {
            Some(EffectPass {
                material: p.material?,
                binds: p
                    .bind
                    .into_iter()
                    .map(|b| Bind {
                        name: b.name,
                        index: b.index,
                    })
                    .collect(),
                target: p.target,
            })
        })
        .collect();
    let fbos = raw
        .fbos
        .into_iter()
        .map(|f| Fbo {
            name: f.name,
            scale: f.scale,
            format: f.format,
        })
        .collect();
    Some(EffectDef { passes, fbos })
}

/// True se o efeito é do caso SIMPLES que já sabemos rodar: um único pass, sem FBOs
/// nomeados extras (amostra só o quadro anterior). Os complexos (godrays, blur com
/// downsample) ficam pra depois.
pub fn is_simple(def: &EffectDef) -> bool {
    def.passes.len() == 1 && def.fbos.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn background_effects_le_overrides() {
        let scene = r#"{ "objects": [
            { "name": "bg", "image": "models/bg.json", "effects": [
                { "file": "effects/tint/effect.json", "passes": [
                    { "combos": { "BLENDMODE": 2 }, "constantshadervalues": { "alpha": 0.5 } }
                ] }
            ] }
        ] }"#;
        let fx = background_effects(scene);
        assert_eq!(fx.len(), 1);
        assert_eq!(fx[0].file, "effects/tint/effect.json");
        assert!(fx[0].visible);
        assert_eq!(fx[0].passes[0].combos, vec![("BLENDMODE".to_string(), 2)]);
        assert_eq!(fx[0].passes[0].constants[0].0, "alpha");
    }

    #[test]
    fn parse_effect_single_pass() {
        let ej = r#"{ "passes": [ { "material": "materials/effects/tint.json" } ] }"#;
        let def = parse_effect(ej).unwrap();
        assert_eq!(def.passes.len(), 1);
        assert_eq!(def.passes[0].material, "materials/effects/tint.json");
        assert!(is_simple(&def));
    }

    #[test]
    fn parse_effect_multipass_with_fbos_e_binds() {
        // formato do godrays (resumido)
        let ej = r#"{
            "passes": [
                { "material": "m/down.json", "target": "_rt_Half1" },
                { "material": "m/combine.json", "bind": [
                    { "name": "_rt_Half1", "index": 0 },
                    { "name": "previous", "index": 1 }
                ] }
            ],
            "fbos": [ { "name": "_rt_Half1", "scale": 2, "format": "rgba_backbuffer" } ]
        }"#;
        let def = parse_effect(ej).unwrap();
        assert_eq!(def.passes.len(), 2);
        assert_eq!(def.passes[0].target.as_deref(), Some("_rt_Half1"));
        assert_eq!(
            def.passes[1].binds,
            vec![
                Bind {
                    name: "_rt_Half1".into(),
                    index: 0
                },
                Bind {
                    name: "previous".into(),
                    index: 1
                },
            ]
        );
        assert_eq!(def.fbos.len(), 1);
        assert_eq!(def.fbos[0].scale, 2.0);
        assert!(!is_simple(&def)); // multi-pass + fbos
    }

    #[test]
    fn sem_efeitos_vazio() {
        assert!(background_effects(r#"{ "objects": [ { "image": "m.json" } ] }"#).is_empty());
        assert!(background_effects(r#"{ "objects": [] }"#).is_empty());
    }
}
