// `effects` module: understands the effect graph of a Wallpaper Engine scene.
//
// Model (two levels):
//   1. The scene OBJECT has `effects: [ { file, visible, passes: [override...] } ]`.
//      Each `file` points to an effect.json; the `passes` carry OVERRIDES (combos,
//      constantshadervalues, textures) per pass index.
//   2. The effect.json has `passes: [ { material, bind } ]` e `fbos: [ {name,scale,format} ]`.
//      The `material` (json) has shader + combos + base constants; the `bind` maps
//      where each input texture of the pass comes from (a named FBO or the previous frame).
//
// Here we only PARSE and resolve (merging the overrides onto the base). The engine
// is what renders the passes.

use serde_json::{Map, Value};

/// Override that the scene applies to an effect pass (by index).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PassOverride {
    pub combos: Vec<(String, i64)>,
    pub constants: Vec<(String, Value)>,
    pub textures: Vec<Option<String>>,
}

/// Use of an effect by a scene object.
#[derive(Debug, Clone, PartialEq)]
pub struct EffectInstance {
    pub file: String, // path of the effect.json inside the pkg
    pub visible: bool,
    pub passes: Vec<PassOverride>, // overrides per pass index
}

// ---- deserialization structs (scene.json: object -> effects) ----

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

/// Builds the effect instances from the JSON value of an object's `effects` field
/// (an array). Used by the compositor to get the effects of EACH layer.
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

/// Effects applied to the FIRST image object (the background). Empty if there are none.
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

/// Where an input texture of a pass comes from: a named FBO (or "previous").
#[derive(Debug, Clone, PartialEq)]
pub struct Bind {
    pub name: String,
    pub index: u32, // texture slot (g_TextureN)
}

/// An effect pass, as defined in effect.json.
#[derive(Debug, Clone, PartialEq)]
pub struct EffectPass {
    pub material: String, // path of the material.json
    pub binds: Vec<Bind>,
    pub target: Option<String>, // output FBO (None = the effect's default output)
}

/// A named render target of the effect.
#[derive(Debug, Clone, PartialEq)]
pub struct Fbo {
    pub name: String,
    pub scale: f32,     // 1 = full resolution, 2 = half, ...
    pub format: String, // e.g. "rgba_backbuffer"
}

/// Parsed effect.json.
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

/// Parses an effect.json. Passes without `material` are ignored.
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

/// True if the effect is the SIMPLE case we already know how to run: a single pass, no
/// extra named FBOs (samples only the previous frame). The complex ones (godrays, blur
/// with downsample) come later.
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
        // godrays layout (abridged)
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
