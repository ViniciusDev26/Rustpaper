// The scene render graph: image layers + their effect chains, composited into
// one scene-sized target.
//
// How WE draws one image object (and how we mirror it):
//
//   no effects:   material shader --(object transform, blending)--> scene
//
//   with effects: material shader --> layer FBO "a"            (object size)
//                 effect pass 1   --> FBO "b"  (samples "a" as `previous`)
//                 effect pass 2   --> FBO "a"  (samples "b")      ... ping-pong
//                 named FBOs (`_rt_QuarterCompoBuffer1`...) for multi-pass effects
//                 result --(object transform, blending)--> scene
//
// The important part is that NOTHING here knows about specific effects. Every
// effect is data: an effect.json (passes, binds, FBOs) + materials + the WE
// shaders. If the runtime below fills uniforms, textures and combos the way WE
// does, all effects work at once; if it gets one of those wrong, they all break
// the same way — which is exactly what the reference harness is for.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use glow::HasContext;
use serde_json::Value;

use crate::assets::Assets;
use crate::gpu::{Blend, ParticleMesh, QuadMesh, Target, Tex, solid_texture, upload_texture};
use crate::math::{self, Mat4};
use crate::particles::{self, ChildEvent, Overrides, ParticleDef, ParticleSim, RendererKind};
use crate::program::{Program, ShaderSource};
use crate::value;

/// Inputs that change every frame.
#[derive(Clone, Copy, Debug)]
pub struct FrameInput {
    /// Seconds since the wallpaper started (`g_Time`).
    pub time: f32,
    /// Cursor in [0,1]x[0,1], (0,0) = top-left (`g_PointerPosition`).
    pub pointer: [f32; 2],
    /// Fraction of the day in [0,1) (`g_Daytime`).
    pub daytime: f32,
}

impl Default for FrameInput {
    fn default() -> Self {
        FrameInput {
            time: 0.0,
            pointer: [0.5, 0.5],
            daytime: 0.5,
        }
    }
}

/// Where a texture slot gets its pixels from.
#[derive(Clone, Debug, PartialEq)]
enum TexSource {
    /// The output of the previous pass (effect passes only).
    Previous,
    /// A file texture ("util/noise", "masks/x") or a render target ("_rt_...").
    Named(String),
}

/// One fully-resolved draw: which program variant, which textures in which
/// unit, and the material constants.
struct Pass {
    program_key: String,
    slots: Vec<(u32, TexSource)>, // (texture unit N of g_TextureN, source)
    constants: HashMap<String, Value>,
    blend: Blend,
}

struct EffectPass {
    pass: Pass,
    target: Option<String>,
}

struct Effect {
    file: String,
    passes: Vec<EffectPass>,
    fbos: Vec<(String, f32)>, // (name, downscale factor)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LayerKind {
    /// Regular image: its quad lives in object space.
    Image,
    /// `passthrough` util models (compose/fullscreen layers): they draw what is
    /// already on screen behind them, then run effects on it.
    Passthrough { fullscreen: bool },
}

struct Layer {
    id: i64,
    name: String,
    visible: bool,
    kind: LayerKind,
    /// parent transforms * own transform (objects can be parented).
    world: Mat4,
    size: [f32; 2],
    color: [f32; 3],
    alpha: f32,
    brightness: f32,
    material: Pass,
    effects: Vec<Effect>,
}

/// Scene objects in draw order (WE draws objects in scene.json order).
#[derive(Clone, Copy)]
enum ObjRef {
    Layer(usize),
    Particles(usize),
}

/// One simulated system of a particle object (the root or a child).
struct SystemRt {
    sim: ParticleSim,
    /// Index of the parent system (children react to its spawn/death events
    /// or follow its particles).
    parent: Option<(usize, ChildEvent, [f32; 3])>,
    pass: Pass,
    /// g_RenderVar0: trail (length scale, max, min, 0)
    render_var0: [f32; 4],
    /// g_RenderVar1: sprite sheet (frame w, frame h in uv, frames, h/w ratio)
    render_var1: [f32; 4],
}

struct ParticleObject {
    id: i64,
    name: String,
    visible: bool,
    /// parent transforms * own transform (objects can be parented).
    world: Mat4,
    systems: Vec<SystemRt>,
    /// Seconds simulated before t = 0 (`starttime`, so it starts "warm").
    prewarm: f32,
}

/// Scene-level bloom (`general.bloom`), LDR or HDR (`general.hdr`), drawn with
/// WE's own post-processing shaders.
struct Bloom {
    hdr: bool,
    /// material constants for the shaders' annotated params (bloomstrength...)
    constants: HashMap<String, Value>,
    /// LDR: [downsample_quarter, blur_v, blur_h, combine]
    /// HDR: [downsample (BLOOM), downsample, upsample, combine]
    keys: [String; 4],
    iterations: u32,
    chain: Vec<Target>,
    output: Option<Target>,
}

pub struct SceneRenderer {
    assets: Assets,
    pub width: u32,
    pub height: u32,
    clear: [f32; 4],
    ambient: [f32; 3],
    skylight: [f32; 3],
    layers: Vec<Layer>,
    particle_objs: Vec<ParticleObject>,
    order: Vec<ObjRef>,
    /// Layers some other layer samples as `_rt_imageLayerComposite_<id>_a`.
    referenced: HashSet<i64>,

    sources: HashMap<String, Option<ShaderSource>>,
    combos_by_key: HashMap<String, (String, BTreeMap<String, i64>)>,
    programs: HashMap<String, Option<Program>>,
    textures: HashMap<String, Tex>,
    white: Tex,

    quad: QuadMesh,
    pmesh: ParticleMesh,
    rope_mesh: ParticleMesh,
    pverts: Vec<f32>,
    blit: Program,
    scene: Target,
    fullframe: Target,
    bloom: Option<Bloom>,
    composites: HashMap<i64, Target>,
    pool: HashMap<(u32, u32), Vec<Target>>,

    /// Every object's world matrix by id (parent chains resolved).
    worlds: HashMap<i64, Mat4>,

    /// Human-readable problems found while loading/rendering (deduplicated).
    pub warnings: Vec<String>,
    /// Debug: when set, every intermediate layer/effect target of the next
    /// `render` is written here as PNG (see `rustpaper-render --dump-passes`).
    pub dump_dir: Option<std::path::PathBuf>,
    /// Debug: when set, only objects with these ids are drawn
    /// (`rustpaper-render --only`).
    pub only: Option<HashSet<i64>>,
}

const MAX_TARGET: u32 = 8192;

impl SceneRenderer {
    pub fn load(
        gl: &glow::Context,
        wallpaper_dir: &Path,
        base_assets: &Path,
    ) -> Result<Self, String> {
        Self::load_with_properties(gl, wallpaper_dir, base_assets, &serde_json::Map::new())
    }

    /// Like `load`, with user property values (name -> value) overriding the
    /// defaults in project.json `general.properties` — what the selector's
    /// customization panel produces.
    pub fn load_with_properties(
        gl: &glow::Context,
        wallpaper_dir: &Path,
        base_assets: &Path,
        overrides: &serde_json::Map<String, Value>,
    ) -> Result<Self, String> {
        let assets = Assets::open(wallpaper_dir, base_assets).map_err(|e| e.to_string())?;
        let project = assets.read_json("project.json").unwrap_or_default();
        let scene_file = project
            .get("file")
            .and_then(|f| f.as_str())
            .unwrap_or("scene.json")
            .to_string();
        let mut scene = assets
            .read_json(&scene_file)
            .ok_or_else(|| format!("{scene_file} not found or invalid"))?;
        let mut props = project
            .pointer("/general/properties")
            .and_then(|p| p.as_object())
            .cloned()
            .unwrap_or_default();
        for (name, v) in overrides {
            if let Some(Value::Object(p)) = props.get_mut(name) {
                p.insert("value".into(), v.clone());
            }
        }
        value::apply_user_properties(&mut scene, &props);

        let general = scene.get("general").cloned().unwrap_or_default();
        let proj = general
            .get("orthogonalprojection")
            .cloned()
            .unwrap_or_default();
        let width = proj.get("width").and_then(|v| v.as_u64()).unwrap_or(1920) as u32;
        let height = proj.get("height").and_then(|v| v.as_u64()).unwrap_or(1080) as u32;
        let cc = value::get_vec3(&general, "clearcolor", [0.0, 0.0, 0.0]);
        let hdr = value::get_bool(&general, "hdr", false);

        let blit_src = ShaderSource {
            vert: BLIT_VERT.into(),
            frag: BLIT_FRAG.into(),
        };
        let blit = Program::build(gl, &blit_src, &BTreeMap::new(), &assets.shaders_dir())
            .map_err(|e| format!("internal blit shader: {e}"))?;

        let mut r = SceneRenderer {
            width,
            height,
            clear: [cc[0], cc[1], cc[2], 1.0],
            ambient: value::get_vec3(&general, "ambientcolor", [0.3; 3]),
            skylight: value::get_vec3(&general, "skylightcolor", [0.3; 3]),
            layers: Vec::new(),
            particle_objs: Vec::new(),
            order: Vec::new(),
            referenced: HashSet::new(),
            sources: HashMap::new(),
            combos_by_key: HashMap::new(),
            programs: HashMap::new(),
            textures: HashMap::new(),
            white: solid_texture(gl, [255, 255, 255, 255]),
            quad: QuadMesh::new(gl),
            pmesh: ParticleMesh::new(gl, crate::gpu::SPRITE_LAYOUT, particles::VERTEX_FLOATS),
            rope_mesh: ParticleMesh::new(gl, crate::gpu::ROPE_LAYOUT, particles::ROPE_FLOATS),
            pverts: Vec::new(),
            blit,
            scene: Target::with_format(gl, width, height, hdr),
            bloom: None,
            fullframe: Target::with_format(gl, width, height, hdr),
            composites: HashMap::new(),
            pool: HashMap::new(),
            warnings: Vec::new(),
            worlds: HashMap::new(),
            dump_dir: None,
            only: None,
            assets,
        };

        let empty = Vec::new();
        let objects = scene
            .get("objects")
            .and_then(|o| o.as_array())
            .unwrap_or(&empty);
        r.worlds = world_matrices(objects);
        for obj in objects {
            // `"image": null` is common on non-image objects: test for a string
            if obj.get("image").and_then(|i| i.as_str()).is_some() {
                match r.load_layer(gl, obj) {
                    Ok(layer) => {
                        r.order.push(ObjRef::Layer(r.layers.len()));
                        r.layers.push(layer);
                    }
                    Err(e) => r.warn(format!(
                        "layer {:?} skipped: {e}",
                        obj.get("name").and_then(|n| n.as_str()).unwrap_or("?")
                    )),
                }
            } else if obj.get("particle").and_then(|p| p.as_str()).is_some() {
                match r.load_particles(gl, obj) {
                    Ok(p) => {
                        r.order.push(ObjRef::Particles(r.particle_objs.len()));
                        r.particle_objs.push(p);
                    }
                    Err(e) => r.warn(format!(
                        "particles {:?} skipped: {e}",
                        obj.get("name").and_then(|n| n.as_str()).unwrap_or("?")
                    )),
                }
            } else if obj.get("text").is_some() {
                r.warn("text objects are not rendered yet".into());
            }
        }

        if value::get_bool(&general, "bloom", false) {
            match r.setup_bloom(gl, &general, hdr) {
                Ok(b) => r.bloom = Some(b),
                Err(e) => r.warn(format!("bloom disabled: {e}")),
            }
        }

        // Layers referenced by other layers' textures must always be rendered
        // into their own target, even if they are hidden themselves.
        let mut referenced = HashSet::new();
        let all_slots = r.layers.iter().flat_map(|l| {
            l.effects
                .iter()
                .flat_map(|e| e.passes.iter().map(|p| &p.pass))
                .chain(std::iter::once(&l.material))
                .flat_map(|p| p.slots.iter())
        });
        for (_, src) in all_slots {
            if let TexSource::Named(n) = src
                && let Some(id) = composite_ref(n)
            {
                referenced.insert(id);
            }
        }
        r.referenced = referenced;

        // Compile everything up-front so the first frame doesn't stall and
        // compile errors show up in `warnings` right away.
        let keys: Vec<String> = r.combos_by_key.keys().cloned().collect();
        for k in keys {
            r.program(gl, &k);
        }
        Ok(r)
    }

    fn warn(&mut self, msg: String) {
        if !self.warnings.contains(&msg) {
            self.warnings.push(msg);
        }
    }

    // ---------------------------------------------------------------- loading

    fn load_layer(&mut self, gl: &glow::Context, obj: &Value) -> Result<Layer, String> {
        let image = obj["image"].as_str().ok_or("image is not a string")?;
        let model = self
            .assets
            .read_json(image)
            .ok_or_else(|| format!("model {image} not found"))?;
        let mat_path = model
            .get("material")
            .and_then(|m| m.as_str())
            .ok_or("model has no material")?;
        let mat = self
            .assets
            .read_json(mat_path)
            .ok_or_else(|| format!("material {mat_path} not found"))?;
        let pass_json = mat
            .get("passes")
            .and_then(|p| p.get(0))
            .ok_or("material has no passes")?;
        if model.get("puppet").is_some() {
            self.warn(format!(
                "{image}: puppet (skeletal) animation not supported, drawn static"
            ));
        }

        let kind = if model.get("passthrough").and_then(|v| v.as_bool()) == Some(true) {
            LayerKind::Passthrough {
                fullscreen: model.get("fullscreen").and_then(|v| v.as_bool()) == Some(true),
            }
        } else {
            LayerKind::Image
        };

        // Object-level combo: colorBlendMode N selects the material's BLENDMODE
        // code path (multiply, screen, ... against what is behind the object).
        let mut extra_combos = BTreeMap::new();
        let blend_mode = obj
            .get("colorBlendMode")
            .and_then(|v| value::plain(v).as_i64())
            .unwrap_or(0);
        if blend_mode != 0 {
            extra_combos.insert("BLENDMODE".to_string(), blend_mode);
        }
        let material = self.resolve_pass(gl, pass_json, None, &[], &extra_combos, false)?;

        let (w, h) = (self.width as f32, self.height as f32);
        let first_tex = material.slots.iter().find_map(|(n, s)| match s {
            TexSource::Named(name) if *n == 0 && !name.starts_with("_rt_") => {
                self.textures.get(name).copied()
            }
            _ => None,
        });
        let size = match kind {
            LayerKind::Passthrough { fullscreen: true } => [w, h],
            _ => value::get_floats(obj, "size")
                .filter(|s| s.len() >= 2 && s[0] > 0.0 && s[1] > 0.0)
                .map(|s| [s[0], s[1]])
                .or_else(|| first_tex.map(|t| [t.width as f32, t.height as f32]))
                .unwrap_or([w, h]),
        };

        let mut effects = Vec::new();
        for fx in obj
            .get("effects")
            .and_then(|e| e.as_array())
            .into_iter()
            .flatten()
        {
            if !value::get_bool(fx, "visible", true) {
                continue;
            }
            match self.load_effect(gl, fx) {
                Ok(e) => effects.push(e),
                Err(e) => self.warn(format!(
                    "effect {} skipped: {e}",
                    fx.get("file").and_then(|f| f.as_str()).unwrap_or("?")
                )),
            }
        }

        Ok(Layer {
            id: obj.get("id").and_then(|i| i.as_i64()).unwrap_or(-1),
            name: obj
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or("")
                .to_string(),
            visible: value::get_bool(obj, "visible", true),
            kind,
            world: match kind {
                LayerKind::Passthrough { fullscreen: true } => {
                    math::model([w / 2.0, h / 2.0, 0.0], 0.0, [1.0; 3])
                }
                _ => self.world_of(obj),
            },
            size,
            color: value::get_vec3(obj, "color", [1.0; 3]),
            alpha: value::get_f32(obj, "alpha", 1.0),
            brightness: value::get_f32(obj, "brightness", 1.0),
            material,
            effects,
        })
    }

    fn load_effect(&mut self, gl: &glow::Context, fx: &Value) -> Result<Effect, String> {
        let file = fx["file"].as_str().ok_or("no file")?;
        let def = self
            .assets
            .read_json(file)
            .ok_or_else(|| format!("{file} not found"))?;
        let overrides = fx
            .get("passes")
            .and_then(|p| p.as_array())
            .cloned()
            .unwrap_or_default();
        let mut passes = Vec::new();
        for (i, p) in def
            .get("passes")
            .and_then(|p| p.as_array())
            .into_iter()
            .flatten()
            .enumerate()
        {
            let Some(mat_path) = p.get("material").and_then(|m| m.as_str()) else {
                continue; // e.g. "command" passes (copy/swap) — not supported yet
            };
            let mat = self
                .assets
                .read_json(mat_path)
                .ok_or_else(|| format!("material {mat_path} not found"))?;
            let pass_json = mat
                .get("passes")
                .and_then(|p| p.get(0))
                .ok_or_else(|| format!("{mat_path} has no passes"))?;
            let binds: Vec<(u32, String)> = p
                .get("bind")
                .and_then(|b| b.as_array())
                .into_iter()
                .flatten()
                .filter_map(|b| {
                    Some((
                        b.get("index")?.as_u64()? as u32,
                        b.get("name")?.as_str()?.to_string(),
                    ))
                })
                .collect();
            let pass = self.resolve_pass(
                gl,
                pass_json,
                overrides.get(i),
                &binds,
                &BTreeMap::new(),
                true,
            )?;
            passes.push(EffectPass {
                pass,
                target: p.get("target").and_then(|t| t.as_str()).map(String::from),
            });
        }
        let fbos = def
            .get("fbos")
            .and_then(|f| f.as_array())
            .into_iter()
            .flatten()
            .filter_map(|f| {
                Some((
                    f.get("name")?.as_str()?.to_string(),
                    f.get("scale")
                        .and_then(|s| s.as_f64())
                        .unwrap_or(1.0)
                        .max(1.0) as f32,
                ))
            })
            .collect();
        Ok(Effect {
            file: file.to_string(),
            passes,
            fbos,
        })
    }

    /// Merges material pass + scene override + effect binds into a `Pass`, works
    /// out the combos (which decide the program variant) and preloads textures.
    fn resolve_pass(
        &mut self,
        gl: &glow::Context,
        pass_json: &Value,
        scene_override: Option<&Value>,
        binds: &[(u32, String)],
        extra_combos: &BTreeMap<String, i64>,
        is_effect: bool,
    ) -> Result<Pass, String> {
        let shader = pass_json
            .get("shader")
            .and_then(|s| s.as_str())
            .ok_or("pass has no shader")?
            .to_string();
        let src = self
            .shader_source(&shader)
            .ok_or_else(|| format!("shader {shader} not found"))?;

        let mut combos: BTreeMap<String, i64> = BTreeMap::new();
        let mut constants: HashMap<String, Value> = HashMap::new();
        let mut textures: Vec<Option<String>> = Vec::new();
        let mut merge = |j: &Value, combos: &mut BTreeMap<String, i64>| {
            if let Some(c) = j.get("combos").and_then(|c| c.as_object()) {
                for (k, v) in c {
                    if let Some(n) = value::plain(v).as_i64() {
                        combos.insert(k.to_uppercase(), n);
                    }
                }
            }
            if let Some(c) = j.get("constantshadervalues").and_then(|c| c.as_object()) {
                for (k, v) in c {
                    constants.insert(k.clone(), value::plain(v).clone());
                }
            }
            if let Some(t) = j.get("textures").and_then(|t| t.as_array()) {
                if textures.len() < t.len() {
                    textures.resize(t.len(), None);
                }
                for (i, v) in t.iter().enumerate() {
                    if let Some(s) = v.as_str() {
                        textures[i] = Some(s.to_string());
                    }
                }
            }
        };
        merge(pass_json, &mut combos);
        if let Some(o) = scene_override {
            merge(o, &mut combos);
        }
        for (k, v) in extra_combos {
            combos.insert(k.clone(), *v);
        }

        // Texture slots. Precedence: effect bind > explicit texture (scene /
        // material) > the sampler's annotated default > `previous` for slot 0.
        // Binds win because the editor also serializes the bound render target
        // into the scene (`_rt_imageLayerComposite_<self>_b`), which is just its
        // name for `previous`.
        let mut slots = Vec::new();
        for s in src.samplers() {
            let Some(unit) = texture_unit(&s.name) else {
                continue;
            };
            let explicit = textures.get(unit as usize).cloned().flatten();
            let bound = binds
                .iter()
                .find(|(i, _)| *i == unit)
                .map(|(_, n)| n.clone());
            let source = match (bound, explicit) {
                (Some(b), _) if b == "previous" => Some(TexSource::Previous),
                (Some(b), _) => Some(TexSource::Named(b)),
                (None, Some(t)) => Some(TexSource::Named(t)),
                (None, None) => None,
            };
            // A texture the user/material actually provided switches the
            // sampler's combo on (e.g. `MASK` when an opacity mask is painted).
            if source.is_some()
                && let Some(c) = &s.combo
            {
                combos.entry(c.to_uppercase()).or_insert(1);
            }
            let source = source
                .or_else(|| s.default.clone().map(TexSource::Named))
                .or_else(|| (is_effect && unit == 0).then_some(TexSource::Previous));
            if let Some(src) = source {
                if let TexSource::Named(n) = &src
                    && !n.starts_with("_rt_")
                    && !n.starts_with("_alias_")
                {
                    // WE tells shaders each texture's storage format through
                    // TEXnFORMAT (e.g. R8 sprites become white + alpha in
                    // ConvertTexture0Format).
                    let t = self.texture(gl, n);
                    combos
                        .entry(format!("TEX{unit}FORMAT"))
                        .or_insert(t.format as i64);
                }
                slots.push((unit, src));
            }
        }

        for (name, default) in src.combo_defaults() {
            combos.entry(name.to_uppercase()).or_insert(default);
        }

        let key = format!(
            "{shader}|{}",
            combos
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join(",")
        );
        self.combos_by_key
            .insert(key.clone(), (shader.clone(), combos));
        Ok(Pass {
            program_key: key,
            slots,
            constants,
            blend: Blend::from_we(
                pass_json
                    .get("blending")
                    .and_then(|b| b.as_str())
                    .unwrap_or("normal"),
            ),
        })
    }

    fn shader_source(&mut self, name: &str) -> Option<ShaderSource> {
        if let Some(s) = self.sources.get(name) {
            return s.clone();
        }
        let src = (|| {
            Some(ShaderSource {
                vert: self.assets.read_string(&format!("shaders/{name}.vert"))?,
                frag: self.assets.read_string(&format!("shaders/{name}.frag"))?,
            })
        })();
        self.sources.insert(name.to_string(), src.clone());
        src
    }

    fn texture(&mut self, gl: &glow::Context, name: &str) -> Tex {
        if let Some(t) = self.textures.get(name) {
            return *t;
        }
        let path = format!("materials/{name}.tex");
        let tex = match self.assets.read(&path) {
            Some(bytes) => match rustpaper_core::tex::parse(&bytes) {
                Ok(d) => upload_texture(gl, &d),
                Err(e) => {
                    self.warn(format!("{path}: {e}"));
                    self.white
                }
            },
            None => {
                self.warn(format!("texture {path} not found"));
                self.white
            }
        };
        self.textures.insert(name.to_string(), tex);
        tex
    }

    fn program(&mut self, gl: &glow::Context, key: &str) -> Option<&Program> {
        if !self.programs.contains_key(key) {
            let built = match self.combos_by_key.get(key).cloned() {
                Some((shader, combos)) => match self.shader_source(&shader) {
                    Some(src) => Program::build(gl, &src, &combos, &self.assets.shaders_dir())
                        .map_err(|e| format!("shader {key}: {e}")),
                    None => Err(format!("shader {shader} not found")),
                },
                None => Err(format!("unknown program {key}")),
            };
            let built = match built {
                Ok(p) => Some(p),
                Err(e) => {
                    self.warn(e);
                    None
                }
            };
            self.programs.insert(key.to_string(), built);
        }
        self.programs.get(key).and_then(|p| p.as_ref())
    }

    // -------------------------------------------------------------- rendering

    /// Renders one frame into the internal scene target.
    pub fn render(&mut self, gl: &glow::Context, frame: FrameInput) {
        unsafe {
            gl.disable(glow::DEPTH_TEST);
            gl.disable(glow::CULL_FACE);
        }
        self.scene.clear(gl, self.clear);

        // Hidden-but-referenced layers first, so others can sample them.
        for i in 0..self.layers.len() {
            let l = &self.layers[i];
            if self.referenced.contains(&l.id)
                && !l.visible
                && let Some(t) = self.composite_layer(gl, i, frame)
            {
                self.keep_composite(gl, i, t);
            }
        }

        for k in 0..self.order.len() {
            let i = match self.order[k] {
                ObjRef::Layer(i) => i,
                ObjRef::Particles(i) => {
                    if let Some(only) = &self.only
                        && !only.contains(&self.particle_objs[i].id)
                    {
                        continue;
                    }
                    self.simulate_particles(i, frame);
                    if self.particle_objs[i].visible {
                        self.draw_particles(gl, i);
                    }
                    continue;
                }
            };
            if !self.layers[i].visible
                || self
                    .only
                    .as_ref()
                    .is_some_and(|o| !o.contains(&self.layers[i].id))
            {
                continue;
            }
            let needs_target = !self.layers[i].effects.is_empty()
                || matches!(self.layers[i].kind, LayerKind::Passthrough { .. })
                || self.referenced.contains(&self.layers[i].id);
            if needs_target {
                if let Some(t) = self.composite_layer(gl, i, frame) {
                    self.draw_composite(gl, i, &t);
                    if self.referenced.contains(&self.layers[i].id) {
                        self.keep_composite(gl, i, t);
                    } else {
                        self.give(t);
                    }
                }
            } else {
                self.draw_direct(gl, i, frame);
            }
        }
        self.post_process(gl);
    }

    /// The finished frame (scene resolution).
    pub fn output(&self) -> &Target {
        self.bloom
            .as_ref()
            .and_then(|b| b.output.as_ref())
            .unwrap_or(&self.scene)
    }

    // ------------------------------------------------------------------ bloom

    fn setup_bloom(
        &mut self,
        gl: &glow::Context,
        general: &Value,
        hdr: bool,
    ) -> Result<Bloom, String> {
        let pass = |r: &mut Self, shader: &str, combos: Value| -> Result<String, String> {
            let j = serde_json::json!({ "shader": shader, "combos": combos });
            Ok(r.resolve_pass(gl, &j, None, &[], &BTreeMap::new(), false)?
                .program_key)
        };
        let tint = value::get_vec3(general, "bloomtint", [1.0; 3]);
        let mut constants: HashMap<String, Value> = HashMap::new();
        let num = |v: f32| serde_json::json!(v);
        constants.insert(
            "bloomtint".into(),
            serde_json::json!(format!("{} {} {}", tint[0], tint[1], tint[2])),
        );
        let keys = if hdr {
            let t = value::get_f32(general, "bloomhdrthreshold", 1.0);
            let knee = (t * value::get_f32(general, "bloomhdrfeather", 0.1)).max(1e-4);
            constants.insert(
                "bloomstrength".into(),
                num(value::get_f32(general, "bloomhdrstrength", 2.0)),
            );
            // soft-knee threshold: (threshold, threshold - knee, 2 knee, 0.25 / knee)
            constants.insert(
                "blend".into(),
                serde_json::json!(format!("{} {} {} {}", t, t - knee, 2.0 * knee, 0.25 / knee)),
            );
            constants.insert(
                "scatter".into(),
                num(value::get_f32(general, "bloomhdrscatter", 1.619)),
            );
            [
                pass(self, "hdr_downsample", serde_json::json!({ "BLOOM": 1 }))?,
                pass(self, "hdr_downsample", serde_json::json!({}))?,
                pass(self, "hdr_downsample", serde_json::json!({ "UPSAMPLE": 1 }))?,
                pass(self, "combine_hdr", serde_json::json!({ "LINEAR": 1 }))?,
            ]
        } else {
            constants.insert(
                "bloomstrength".into(),
                num(value::get_f32(general, "bloomstrength", 2.0)),
            );
            constants.insert(
                "bloomthreshold".into(),
                num(value::get_f32(general, "bloomthreshold", 0.65)),
            );
            [
                pass(self, "downsample_quarter_bloom", serde_json::json!({}))?,
                pass(self, "downsample_eighth_blur_v", serde_json::json!({}))?,
                pass(self, "blur_h_bloom", serde_json::json!({}))?,
                pass(self, "combine", serde_json::json!({}))?,
            ]
        };
        Ok(Bloom {
            hdr,
            constants,
            keys,
            iterations: value::get_f32(general, "bloomhdriterations", 8.0).clamp(1.0, 12.0) as u32,
            chain: Vec::new(),
            output: None,
        })
    }

    /// Fullscreen post pass: `inputs` bound to g_Texture0.., `vars` set by name.
    #[allow(clippy::too_many_arguments)]
    fn post_pass(
        &mut self,
        gl: &glow::Context,
        key: &str,
        target: Target,
        inputs: &[Tex],
        vars: &[(&str, [f32; 4])],
        constants: &HashMap<String, Value>,
        blend: Blend,
    ) {
        if self.program(gl, key).is_none() {
            return;
        }
        let prog = self.programs[key].as_ref().expect("checked above");
        target.bind(gl);
        blend.apply(gl);
        unsafe {
            gl.use_program(Some(prog.id));
            for (unit, tex) in inputs.iter().enumerate() {
                gl.active_texture(glow::TEXTURE0 + unit as u32);
                gl.bind_texture(glow::TEXTURE_2D, Some(tex.id));
            }
        }
        for (unit, tex) in inputs.iter().enumerate() {
            prog.set(gl, &format!("g_Texture{unit}"), &[unit as f32]);
            prog.set(gl, &format!("g_Texture{unit}Resolution"), &tex.resolution());
        }
        set_params(gl, prog, constants);
        for (name, v) in vars {
            prog.set(gl, name, v);
        }
        self.quad.draw(gl, &CLIP_QUAD);
    }

    /// Runs the bloom chain over the finished scene.
    fn post_process(&mut self, gl: &glow::Context) {
        let Some(mut b) = self.bloom.take() else {
            return;
        };
        let (w, h) = (self.width, self.height);
        let texel = [1.0 / w as f32, 1.0 / h as f32, 0.0, 0.0];
        let scene = self.scene.tex;
        let out = *b.output.get_or_insert_with(|| Target::new(gl, w, h));
        if b.hdr {
            if b.chain.is_empty() {
                let (mut cw, mut ch) = (w / 2, h / 2);
                for _ in 0..b.iterations {
                    if cw < 8 || ch < 8 {
                        break;
                    }
                    b.chain.push(Target::with_format(gl, cw, ch, true));
                    (cw, ch) = (cw / 2, ch / 2);
                }
            }
            let offsets = |t: &Tex| {
                let (tx, ty) = (1.0 / t.width as f32, 1.0 / t.height as f32);
                [-tx, -ty, tx, ty]
            };
            let n = b.chain.len();
            for i in 0..n {
                let src = if i == 0 { scene } else { b.chain[i - 1].tex };
                let key = if i == 0 { &b.keys[0] } else { &b.keys[1] };
                b.chain[i].clear(gl, [0.0, 0.0, 0.0, 1.0]);
                self.post_pass(
                    gl,
                    key,
                    b.chain[i],
                    &[src],
                    &[("g_RenderVar0", offsets(&src))],
                    &b.constants,
                    Blend::Opaque,
                );
            }
            for i in (0..n.saturating_sub(1)).rev() {
                let src = b.chain[i + 1].tex;
                self.post_pass(
                    gl,
                    &b.keys[2],
                    b.chain[i],
                    &[src],
                    &[("g_RenderVar0", offsets(&src))],
                    &b.constants,
                    Blend::Add,
                );
            }
            if let Some(first) = b.chain.first().map(|t| t.tex) {
                let t = [
                    1.0 / first.width as f32,
                    1.0 / first.height as f32,
                    0.0,
                    0.0,
                ];
                self.post_pass(
                    gl,
                    &b.keys[3],
                    out,
                    &[scene, first],
                    &[("g_TexelSize", t), ("g_RenderVar0", [1.0, 0.0, 0.0, 0.0])],
                    &b.constants,
                    Blend::Opaque,
                );
            }
        } else {
            if b.chain.is_empty() {
                b.chain.push(Target::new(gl, w / 4, h / 4));
                b.chain.push(Target::new(gl, w / 8, h / 8));
                b.chain.push(Target::new(gl, w / 8, h / 8));
            }
            let (q, e1, e2) = (b.chain[0], b.chain[1], b.chain[2]);
            let v = [("g_TexelSize", texel)];
            self.post_pass(gl, &b.keys[0], q, &[scene], &v, &b.constants, Blend::Opaque);
            self.post_pass(
                gl,
                &b.keys[1],
                e1,
                &[q.tex],
                &v,
                &b.constants,
                Blend::Opaque,
            );
            self.post_pass(
                gl,
                &b.keys[2],
                e2,
                &[e1.tex],
                &v,
                &b.constants,
                Blend::Opaque,
            );
            self.post_pass(
                gl,
                &b.keys[3],
                out,
                &[scene, e2.tex],
                &v,
                &b.constants,
                Blend::Opaque,
            );
        }
        self.bloom = Some(b);
    }

    fn keep_composite(&mut self, gl: &glow::Context, i: usize, t: Target) {
        let id = self.layers[i].id;
        let (w, h) = (t.tex.width, t.tex.height);
        let keep = match self.composites.get(&id) {
            Some(k) if k.tex.width == w && k.tex.height == h => *k,
            _ => {
                let k = Target::new(gl, w, h);
                self.composites.insert(id, k);
                k
            }
        };
        t.blit_to(gl, &keep);
        self.dump(gl, i, "z-kept", &keep);
        self.give(t);
    }

    /// Layer without effects: material straight onto the scene.
    fn draw_direct(&mut self, gl: &glow::Context, i: usize, frame: FrameInput) {
        let l = &self.layers[i];
        let model = l.world;
        let proj = math::scene_ortho(self.width as f32, self.height as f32);
        let mvp = math::mul(&proj, &model);
        let verts = math::quad(l.size[0], l.size[1], 1.0, 1.0);
        let blend = l.material.blend;
        let scene = self.scene;
        self.draw_pass(
            gl,
            DrawCall {
                layer: i,
                which: PassRef::Material,
                target: scene,
                previous: None,
                effect_targets: &HashMap::new(),
                mvp,
                model,
                proj,
                verts,
                blend,
                frame,
            },
        );
    }

    /// Renders the layer (and its effect chain) into a pooled target of the
    /// layer's size. Returns that target; caller draws or keeps it.
    fn composite_layer(
        &mut self,
        gl: &glow::Context,
        i: usize,
        frame: FrameInput,
    ) -> Option<Target> {
        let (fw, fh) = target_size(self.layers[i].size);
        let mut current = self.take(gl, fw, fh);
        current.clear(gl, [0.0, 0.0, 0.0, 0.0]);

        // 1) the layer itself
        let (mvp, model, proj, verts) = match self.layers[i].kind {
            LayerKind::Image => (
                math::centered_ortho(fw as f32, fh as f32),
                math::IDENTITY,
                math::centered_ortho(fw as f32, fh as f32),
                math::quad(fw as f32, fh as f32, 1.0, 1.0),
            ),
            // compose layers get the SCENE transform: their shader turns it into
            // "which part of the screen is behind me".
            LayerKind::Passthrough { fullscreen } => {
                let l = &self.layers[i];
                let model = l.world;
                let proj = math::scene_ortho(self.width as f32, self.height as f32);
                let verts = if fullscreen {
                    math::quad(2.0, -2.0, 1.0, 1.0) // already in clip space
                } else {
                    math::quad(l.size[0], l.size[1], 1.0, 1.0)
                };
                (math::mul(&proj, &model), model, proj, verts)
            }
        };
        self.draw_pass(
            gl,
            DrawCall {
                layer: i,
                which: PassRef::Material,
                target: current,
                previous: None,
                effect_targets: &HashMap::new(),
                mvp,
                model,
                proj,
                verts,
                blend: Blend::Opaque,
                frame,
            },
        );

        self.dump(gl, i, "0-layer", &current);

        // 2) its effects, ping-ponging between `current` and `other`
        let mut other = self.take(gl, fw, fh);
        for e in 0..self.layers[i].effects.len() {
            let mut named: HashMap<String, Target> = HashMap::new();
            for (name, scale) in self.layers[i].effects[e].fbos.clone() {
                let (w, h) = ((fw as f32 / scale) as u32, (fh as f32 / scale) as u32);
                named.insert(name, self.take(gl, w.max(1), h.max(1)));
            }
            for p in 0..self.layers[i].effects[e].passes.len() {
                // A pass whose shader failed to compile is skipped: the image
                // flows through unchanged instead of the whole layer vanishing.
                let key = self.layers[i].effects[e].passes[p].pass.program_key.clone();
                if self.program(gl, &key).is_none() {
                    continue;
                }
                let target_name = self.layers[i].effects[e].passes[p].target.clone();
                let target = match &target_name {
                    Some(n) => match named.get(n) {
                        Some(t) => *t,
                        None => {
                            self.warn(format!("effect target {n} is not declared"));
                            continue;
                        }
                    },
                    None => other,
                };
                target.clear(gl, [0.0, 0.0, 0.0, 0.0]);
                // WE effect passes draw a quad that is ALREADY in clip space with
                // an identity MVP: some effect vertex shaders skip the matrix
                // entirely (`gl_Position = vec4(a_Position, 1.0)`).
                let blend = self.layers[i].effects[e].passes[p].pass.blend;
                self.draw_pass(
                    gl,
                    DrawCall {
                        layer: i,
                        which: PassRef::Effect(e, p),
                        target,
                        previous: Some(current.tex),
                        effect_targets: &named,
                        mvp: math::IDENTITY,
                        model: math::IDENTITY,
                        proj: math::IDENTITY,
                        verts: CLIP_QUAD,
                        blend,
                        frame,
                    },
                );
                let fx = &self.layers[i].effects[e];
                let label = format!(
                    "{}-{}-p{p}-{}",
                    e + 1,
                    fx.file
                        .trim_start_matches("effects/")
                        .replace(['/', '.'], "_"),
                    target_name.as_deref().unwrap_or("pingpong")
                );
                self.dump(gl, i, &label, &target);
                if target_name.is_none() {
                    std::mem::swap(&mut current, &mut other);
                }
            }
            for (_, t) in named {
                self.give(t);
            }
        }
        self.give(other);
        Some(current)
    }

    /// Draws a finished layer target onto the scene with the object transform.
    fn draw_composite(&mut self, gl: &glow::Context, i: usize, t: &Target) {
        let l = &self.layers[i];
        let (model, verts) = match l.kind {
            LayerKind::Passthrough { fullscreen: true } => (
                math::model(
                    [self.width as f32 / 2.0, self.height as f32 / 2.0, 0.0],
                    0.0,
                    [1.0; 3],
                ),
                math::quad(self.width as f32, self.height as f32, 1.0, 1.0),
            ),
            _ => (l.world, math::quad(l.size[0], l.size[1], 1.0, 1.0)),
        };
        let mvp = math::mul(
            &math::scene_ortho(self.width as f32, self.height as f32),
            &model,
        );
        self.scene.bind(gl);
        l.material.blend.apply(gl);
        unsafe {
            gl.use_program(Some(self.blit.id));
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(t.tex.id));
        }
        self.blit.set(gl, "g_ModelViewProjectionMatrix", &mvp);
        self.blit.set(gl, "g_Texture0", &[0.0]);
        self.quad.draw(gl, &verts);
    }

    fn draw_pass(&mut self, gl: &glow::Context, call: DrawCall) {
        let pass = match call.which {
            PassRef::Material => &self.layers[call.layer].material,
            PassRef::Effect(e, p) => &self.layers[call.layer].effects[e].passes[p].pass,
        };
        let key = pass.program_key.clone();
        let slots = pass.slots.clone();
        let constants = pass.constants.clone();
        let is_material = matches!(call.which, PassRef::Material);

        // Resolve every texture slot to a GL texture.
        let mut bound: Vec<(u32, Tex)> = Vec::new();
        for (unit, src) in &slots {
            let own_id = self.layers[call.layer].id;
            let tex = match src {
                TexSource::Previous => call.previous.unwrap_or(self.white),
                // a layer's own ping-pong buffers, by name, inside its own chain
                TexSource::Named(n)
                    if call.previous.is_some() && composite_ref(n) == Some(own_id) =>
                {
                    call.previous.unwrap_or(self.white)
                }
                TexSource::Named(n) => self.named_texture(gl, n, call.effect_targets),
            };
            bound.push((*unit, tex));
        }

        if self.program(gl, &key).is_none() {
            return;
        }
        let prog = self.programs[&key].as_ref().expect("checked above");
        if std::env::var_os("RUSTPAPER_TRACE").is_some() {
            eprintln!(
                "draw layer#{} {} -> {}x{} {key} slots={:?}",
                self.layers[call.layer].id,
                match call.which {
                    PassRef::Material => "material".to_string(),
                    PassRef::Effect(e, p) => format!("fx{e}.p{p}"),
                },
                call.target.tex.width,
                call.target.tex.height,
                slots
                    .iter()
                    .zip(&bound)
                    .map(|((u, s), (_, t))| format!("{u}:{s:?}={}x{}", t.width, t.height))
                    .collect::<Vec<_>>()
            );
        }
        let l = &self.layers[call.layer];

        call.target.bind(gl);
        call.blend.apply(gl);
        unsafe {
            gl.use_program(Some(prog.id));
            for (unit, tex) in &bound {
                gl.active_texture(glow::TEXTURE0 + unit);
                gl.bind_texture(glow::TEXTURE_2D, Some(tex.id));
            }
        }

        // 1) engine globals
        let (tw, th) = (call.target.tex.width as f32, call.target.tex.height as f32);
        let (sw, sh) = (self.width as f32, self.height as f32);
        let f = &call.frame;
        prog.set(gl, "g_ModelViewProjectionMatrix", &call.mvp);
        prog.set(gl, "g_ModelMatrix", &call.model);
        prog.set(gl, "g_ViewProjectionMatrix", &call.proj);
        prog.set(gl, "g_EffectTextureProjectionMatrix", &math::IDENTITY);
        prog.set(
            gl,
            "g_EffectTextureProjectionMatrixInverse",
            &math::IDENTITY,
        );
        prog.set(gl, "g_EffectModelViewProjectionMatrix", &call.mvp);
        prog.set(gl, "g_Time", &[f.time]);
        prog.set(gl, "g_Daytime", &[f.daytime]);
        prog.set(gl, "g_Frametime", &[1.0 / 30.0]);
        prog.set(gl, "g_PointerPosition", &f.pointer);
        prog.set(gl, "g_PointerPositionLast", &f.pointer);
        prog.set(gl, "g_ParallaxPosition", &[0.5, 0.5]);
        prog.set(gl, "g_TexelSize", &[1.0 / tw, 1.0 / th]);
        prog.set(gl, "g_TexelSizeHalf", &[0.5 / tw, 0.5 / th]);
        prog.set(gl, "g_Screen", &[sw, sh, sw / sh]);
        prog.set(gl, "g_EyePosition", &[sw / 2.0, sh / 2.0, 1.0]);
        prog.set(gl, "g_LightAmbientColor", &self.ambient);
        prog.set(gl, "g_LightSkylightColor", &self.skylight);
        prog.set(gl, "g_Texture0Rotation", &[1.0, 0.0, 0.0, 1.0]);
        prog.set(gl, "g_Texture0Translation", &[0.0, 0.0]);
        prog.set(gl, "g_Alpha", &[1.0]);
        prog.set(gl, "g_UserAlpha", &[1.0]);
        prog.set(gl, "g_Brightness", &[1.0]);
        prog.set(gl, "g_Color", &[1.0, 1.0, 1.0]);
        prog.set(gl, "g_Color4", &[1.0, 1.0, 1.0, 1.0]);
        for (unit, tex) in &bound {
            prog.set(gl, &format!("g_Texture{unit}"), &[*unit as f32]);
            prog.set(gl, &format!("g_Texture{unit}Resolution"), &tex.resolution());
            prog.set(gl, &format!("g_Texture{unit}MipMapInfo"), &[0.0]);
        }

        // 2) annotated material parameters: default, overridden by constants
        set_params(gl, prog, &constants);

        // 3) the object's own properties (image pass only)
        if is_material {
            prog.set(gl, "g_Brightness", &[l.brightness]);
            prog.set(gl, "g_UserAlpha", &[l.alpha]);
            prog.set(gl, "g_Alpha", &[l.alpha]);
            prog.set(gl, "g_Color", &l.color);
            prog.set(
                gl,
                "g_Color4",
                &[l.color[0], l.color[1], l.color[2], l.alpha],
            );
        }

        self.quad.draw(gl, &call.verts);
    }

    fn named_texture(
        &mut self,
        gl: &glow::Context,
        name: &str,
        effect_targets: &HashMap<String, Target>,
    ) -> Tex {
        if let Some(t) = effect_targets.get(name) {
            return t.tex;
        }
        if let Some(id) = composite_ref(name) {
            return match self.composites.get(&id) {
                Some(t) => t.tex,
                None => {
                    self.warn(format!("{name} requested before it was rendered"));
                    self.white
                }
            };
        }
        match name {
            // Snapshot of everything drawn so far (blend modes, compose layers).
            "_rt_FullFrameBuffer" | "_rt_MipMappedFrameBuffer" => {
                self.scene.blit_to(gl, &self.fullframe);
                self.fullframe.tex
            }
            // engine-provided aliases (light cookies, shadow atlases...) only
            // exist with scene lighting, which we don't render
            n if n.starts_with("_alias_") => self.white,
            n if n.starts_with("_rt_") => {
                self.warn(format!("render target {n} not supported"));
                self.white
            }
            n => self.texture(gl, n),
        }
    }

    fn dump(&self, gl: &glow::Context, layer: usize, label: &str, t: &Target) {
        let Some(dir) = &self.dump_dir else {
            return;
        };
        let l = &self.layers[layer];
        let path = dir.join(format!("layer{}_{label}.png", l.id));
        let px = t.read_rgba(gl);
        let _ = image::save_buffer(
            &path,
            &px,
            t.tex.width,
            t.tex.height,
            image::ExtendedColorType::Rgba8,
        );
    }

    fn take(&mut self, gl: &glow::Context, w: u32, h: u32) -> Target {
        self.pool
            .get_mut(&(w, h))
            .and_then(|v| v.pop())
            .unwrap_or_else(|| Target::new(gl, w, h))
    }

    fn give(&mut self, t: Target) {
        self.pool
            .entry((t.tex.width, t.tex.height))
            .or_default()
            .push(t);
    }

    fn world_of(&self, obj: &Value) -> Mat4 {
        obj.get("id")
            .and_then(|i| i.as_i64())
            .and_then(|id| self.worlds.get(&id).copied())
            .unwrap_or_else(|| local_matrix(obj))
    }

    // -------------------------------------------------------------- particles

    fn load_particles(
        &mut self,
        gl: &glow::Context,
        obj: &Value,
    ) -> Result<ParticleObject, String> {
        let file = obj["particle"].as_str().ok_or("particle is not a string")?;
        let id = obj.get("id").and_then(|i| i.as_i64()).unwrap_or(0);
        let ov = Overrides::parse(obj.get("instanceoverride"));
        let mut systems = Vec::new();
        self.load_system(gl, file, None, ov, id as u64, &mut systems, 0)?;
        let prewarm = systems.first().map(|s| s.sim.def.start_time).unwrap_or(0.0);
        Ok(ParticleObject {
            id,
            name: obj
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or("")
                .to_string(),
            visible: value::get_bool(obj, "visible", true),
            world: self.world_of(obj),
            systems,
            prewarm: prewarm.min(60.0),
        })
    }

    /// Loads one system and, recursively, its children (flattened, parents
    /// first so a single forward pass can feed children their parent events).
    #[allow(clippy::too_many_arguments)]
    fn load_system(
        &mut self,
        gl: &glow::Context,
        file: &str,
        parent: Option<(usize, ChildEvent, [f32; 3], Option<u32>)>,
        ov: Overrides,
        seed: u64,
        out: &mut Vec<SystemRt>,
        depth: u32,
    ) -> Result<(), String> {
        let json = self
            .assets
            .read_json(file)
            .ok_or_else(|| format!("{file} not found"))?;
        let mut def = ParticleDef::parse(&json);
        for u in def.unsupported.clone() {
            self.warn(format!("particles: {u} not supported"));
        }
        if let Some((_, _, _, Some(max))) = parent {
            def.max_count = max;
        }
        let mat = self
            .assets
            .read_json(&def.material)
            .ok_or_else(|| format!("material {} not found", def.material))?;
        let mut pass_json = mat
            .get("passes")
            .and_then(|p| p.get(0))
            .cloned()
            .ok_or("particle material has no passes")?;
        // Rope renderers draw with WE's rope shader, whatever the material says.
        let rope = matches!(
            def.renderer,
            RendererKind::Rope | RendererKind::RopeTrail { .. }
        );
        if rope {
            pass_json["shader"] = Value::from("genericropeparticle");
        }

        // Sprite sheet: embedded in the .tex (Workshop sprites) or described by
        // a companion .tex-json (base assets).
        let sprite = pass_json
            .get("textures")
            .and_then(|t| t.get(0))
            .and_then(|t| t.as_str())
            .map(String::from);
        let mut render_var1 = [0.0; 4];
        let mut sheet = false;
        if let Some(name) = &sprite {
            let tex = self.texture(gl, name);
            let frames = self
                .assets
                .read(&format!("materials/{name}.tex"))
                .and_then(|b| rustpaper_core::tex::embedded_sheet(&b))
                .map(|e| (e.frames, e.frame_w, e.frame_h))
                .or_else(|| {
                    let j = self
                        .assets
                        .read_json(&format!("materials/{name}.tex-json"))?;
                    let seq = j.get("spritesheetsequences")?.get(0)?;
                    Some((
                        seq.get("frames")?.as_u64()? as u32,
                        seq.get("width")?.as_u64()? as u32,
                        seq.get("height")?.as_u64()? as u32,
                    ))
                });
            if let Some((frames, fw, fh)) = frames
                && frames > 1
                && fw > 0
                && fh > 0
            {
                sheet = true;
                render_var1 = [
                    fw as f32 / tex.width as f32,
                    fh as f32 / tex.height as f32,
                    frames as f32,
                    fh as f32 / fw as f32,
                ];
            }
        }
        let trail = matches!(def.renderer, RendererKind::Trail { .. });
        let render_var0 = match def.renderer {
            RendererKind::Trail { length, max, min } => [length, max, min, 0.0],
            // rope: (segment count unscaled, -, segment uv time offset, count)
            RendererKind::Rope | RendererKind::RopeTrail { .. } => {
                let n = def.max_count as f32;
                [n, 0.0, 1.0, n]
            }
            _ => [0.0; 4],
        };
        let sheet = sheet && !rope;
        let mut combos = BTreeMap::new();
        if (sheet || trail) && !rope {
            combos.insert("THICKFORMAT".to_string(), 1);
        }
        if trail {
            combos.insert("TRAILRENDERER".to_string(), 1);
        }
        if sheet {
            combos.insert("SPRITESHEET".to_string(), 1);
        }
        let pass = self.resolve_pass(gl, &pass_json, None, &[], &combos, false)?;

        let children = def.children.clone();
        let index = out.len();
        out.push(SystemRt {
            sim: ParticleSim::new(def, ov, seed.wrapping_mul(31).wrapping_add(index as u64)),
            parent: parent.map(|(i, e, o, _)| (i, e, o)),
            pass,
            render_var0,
            render_var1,
        });
        if depth < 4 {
            for c in children {
                if let Err(e) = self.load_system(
                    gl,
                    &c.file,
                    Some((index, c.event, c.origin, c.max_count)),
                    Overrides::NONE,
                    seed.wrapping_add(1000 + out.len() as u64),
                    out,
                    depth + 1,
                ) {
                    self.warn(format!("particle child {}: {e}", c.file));
                }
            }
        }
        Ok(())
    }

    /// Advances an object's systems to `frame.time` (+ prewarm) in fixed steps.
    /// Going back in time restarts the simulation (deterministic replay).
    fn simulate_particles(&mut self, i: usize, frame: FrameInput) {
        let (w, h) = (self.width as f32, self.height as f32);
        let o = &mut self.particle_objs[i];
        let target = o.prewarm + frame.time.max(0.0);
        let Some(first) = o.systems.first() else {
            return;
        };
        if target + particles::STEP < first.sim.time {
            o.systems.iter_mut().for_each(|s| s.sim.reset());
        }
        // cursor (scene space, y-up) -> object space, for control points that
        // follow the mouse
        let inv = math::invert(&o.world);
        let (px, py) = (frame.pointer[0] * w, (1.0 - frame.pointer[1]) * h);
        let pointer = [
            inv[0] * px + inv[4] * py + inv[12],
            inv[1] * px + inv[5] * py + inv[13],
            0.0,
        ];
        let max_steps = ((o.prewarm + 1.0) / particles::STEP) as usize + 120;
        let mut steps = 0;
        while o.systems[0].sim.time + particles::STEP <= target + 1e-4 && steps < max_steps {
            steps += 1;
            for k in 0..o.systems.len() {
                let (before, rest) = o.systems.split_at_mut(k);
                let sys = &mut rest[0];
                let cps = particles::control_points(&sys.sim.def, pointer);
                match sys.parent {
                    None => sys.sim.step(particles::STEP, &cps, true),
                    Some((pi, event, origin)) => {
                        let parent = &mut before[pi].sim;
                        match event {
                            ChildEvent::Follow => {
                                let rate = sys.sim.rate();
                                let mut spawns = Vec::new();
                                for (pos, acc) in parent.positions_with_acc() {
                                    *acc += rate * particles::STEP;
                                    while *acc >= 1.0 {
                                        *acc -= 1.0;
                                        spawns.push(pos);
                                    }
                                }
                                for pos in spawns {
                                    sys.sim.burst_at(
                                        [
                                            pos[0] + origin[0],
                                            pos[1] + origin[1],
                                            pos[2] + origin[2],
                                        ],
                                        1,
                                        &cps,
                                    );
                                }
                            }
                            ChildEvent::Spawn | ChildEvent::Death => {
                                let n = sys.sim.burst_size();
                                let events: Vec<_> = parent
                                    .events
                                    .iter()
                                    .filter(|e| e.kind == event)
                                    .map(|e| e.pos)
                                    .collect();
                                for pos in events {
                                    sys.sim.burst_at(
                                        [
                                            pos[0] + origin[0],
                                            pos[1] + origin[1],
                                            pos[2] + origin[2],
                                        ],
                                        n,
                                        &cps,
                                    );
                                }
                            }
                        }
                        sys.sim.step(particles::STEP, &cps, false);
                    }
                }
            }
        }
    }

    fn draw_particles(&mut self, gl: &glow::Context, i: usize) {
        let (w, h) = (self.width as f32, self.height as f32);
        let o = &self.particle_objs[i];
        let model = o.world;
        let proj = math::scene_ortho(w, h);
        let mvp = math::mul(&proj, &model);
        let inverse = math::invert(&model);
        for k in 0..self.particle_objs[i].systems.len() {
            let sys = &self.particle_objs[i].systems[k];
            if sys.sim.is_empty() {
                continue;
            }
            let key = sys.pass.program_key.clone();
            let slots = sys.pass.slots.clone();
            let constants = sys.pass.constants.clone();
            let blend = sys.pass.blend;
            let (rv0, rv1) = (sys.render_var0, sys.render_var1);
            let rope = matches!(
                sys.sim.def.renderer,
                RendererKind::Rope | RendererKind::RopeTrail { .. }
            );
            let mut verts = std::mem::take(&mut self.pverts);
            if rope {
                sys.sim.rope_vertices(&mut verts);
            } else {
                sys.sim.vertices(&mut verts);
            }

            let mut bound = Vec::new();
            for (unit, src) in &slots {
                let tex = match src {
                    TexSource::Previous => self.white,
                    TexSource::Named(n) => self.named_texture(gl, n, &HashMap::new()),
                };
                bound.push((*unit, tex));
            }
            if self.program(gl, &key).is_none() {
                self.pverts = verts;
                continue;
            }
            let prog = self.programs[&key].as_ref().expect("checked above");
            self.scene.bind(gl);
            blend.apply(gl);
            unsafe {
                gl.use_program(Some(prog.id));
                for (unit, tex) in &bound {
                    gl.active_texture(glow::TEXTURE0 + unit);
                    gl.bind_texture(glow::TEXTURE_2D, Some(tex.id));
                }
            }
            prog.set(gl, "g_ModelViewProjectionMatrix", &mvp);
            prog.set(gl, "g_ModelMatrix", &model);
            prog.set(gl, "g_ModelMatrixInverse", &inverse);
            prog.set(gl, "g_ViewProjectionMatrix", &proj);
            prog.set(gl, "g_OrientationRight", &[1.0, 0.0, 0.0]);
            prog.set(gl, "g_OrientationUp", &[0.0, 1.0, 0.0]);
            prog.set(gl, "g_OrientationForward", &[0.0, 0.0, 1.0]);
            prog.set(gl, "g_ViewRight", &[1.0, 0.0, 0.0]);
            prog.set(gl, "g_ViewUp", &[0.0, 1.0, 0.0]);
            // far "in front" of the scene plane: trails get right = cross(view, velocity)
            prog.set(gl, "g_EyePosition", &[w / 2.0, h / 2.0, 100_000.0]);
            prog.set(gl, "g_RenderVar0", &rv0);
            prog.set(gl, "g_RenderVar1", &rv1);
            prog.set(gl, "g_LightAmbientColor", &self.ambient);
            prog.set(gl, "g_LightSkylightColor", &self.skylight);
            for (unit, tex) in &bound {
                prog.set(gl, &format!("g_Texture{unit}"), &[*unit as f32]);
                prog.set(gl, &format!("g_Texture{unit}Resolution"), &tex.resolution());
            }
            set_params(gl, prog, &constants);
            if rope {
                self.rope_mesh.draw(gl, &verts);
            } else {
                self.pmesh.draw(gl, &verts);
            }
            self.pverts = verts;
        }
    }

    /// One line per layer, for `--describe`.
    pub fn describe(&self) -> Vec<String> {
        self.layers
            .iter()
            .map(|l| {
                format!(
                    "#{:<5} {:<28} {:>5} {:?} size={:?} effects={} shader={}",
                    l.id,
                    l.name.chars().take(28).collect::<String>(),
                    if l.visible { "shown" } else { "HIDDEN" },
                    match l.kind {
                        LayerKind::Image => "image",
                        LayerKind::Passthrough { fullscreen: true } => "fullscreen",
                        LayerKind::Passthrough { .. } => "compose",
                    },
                    l.size,
                    l.effects.len(),
                    l.material.program_key
                )
            })
            .chain(self.particle_objs.iter().map(|p| {
                format!(
                    "#{:<5} {:<28} {:>5} particles prewarm={}s {}",
                    p.id,
                    p.name.chars().take(28).collect::<String>(),
                    if p.visible { "shown" } else { "HIDDEN" },
                    p.prewarm,
                    p.systems
                        .iter()
                        .map(|s| format!("[max={} {}]", s.sim.max_count(), s.pass.program_key))
                        .collect::<Vec<_>>()
                        .join(" ")
                )
            }))
            .collect()
    }
}

#[derive(Clone, Copy)]
enum PassRef {
    Material,
    Effect(usize, usize),
}

struct DrawCall<'a> {
    layer: usize,
    which: PassRef,
    target: Target,
    previous: Option<Tex>,
    effect_targets: &'a HashMap<String, Target>,
    mvp: Mat4,
    model: Mat4,
    proj: Mat4,
    verts: [f32; 20],
    blend: Blend,
    frame: FrameInput,
}

/// Sets the shader's annotated material parameters (`uniform float g_Speed;
/// // {"material":"speed","default":1}`): the annotated default, overridden by
/// the pass constants (exact name first, then case-insensitive).
fn set_params(gl: &glow::Context, prog: &Program, constants: &HashMap<String, Value>) {
    for p in &prog.params {
        let v = p
            .material
            .as_deref()
            .and_then(|m| {
                constants.get(m).or_else(|| {
                    constants
                        .iter()
                        .find(|(k, _)| k.eq_ignore_ascii_case(m))
                        .map(|(_, v)| v)
                })
            })
            .map(value::floats)
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| p.default.clone());
        prog.set(gl, &p.uniform, &v);
    }
}

/// An object's own transform: translate(origin) * rotZ(angle) * scale.
fn local_matrix(obj: &Value) -> Mat4 {
    math::model(
        value::get_vec3(obj, "origin", [0.0; 3]),
        value::get_vec3(obj, "angles", [0.0; 3])[2],
        value::get_vec3(obj, "scale", [1.0; 3]),
    )
}

/// World matrix of every object: objects can have a `parent` (any object,
/// including plain groups) and their origin/angles/scale are relative to it.
fn world_matrices(objects: &[Value]) -> HashMap<i64, Mat4> {
    let by_id: HashMap<i64, &Value> = objects
        .iter()
        .filter_map(|o| Some((o.get("id")?.as_i64()?, o)))
        .collect();
    fn resolve(
        id: i64,
        by_id: &HashMap<i64, &Value>,
        out: &mut HashMap<i64, Mat4>,
        depth: u32,
    ) -> Mat4 {
        if let Some(m) = out.get(&id) {
            return *m;
        }
        let Some(obj) = by_id.get(&id) else {
            return math::IDENTITY;
        };
        let local = local_matrix(obj);
        let world = match obj.get("parent").and_then(|p| p.as_i64()) {
            Some(pid) if depth < 32 && pid != id => {
                math::mul(&resolve(pid, by_id, out, depth + 1), &local)
            }
            _ => local,
        };
        out.insert(id, world);
        world
    }
    let mut out = HashMap::new();
    for &id in by_id.keys() {
        resolve(id, &by_id, &mut out, 0);
    }
    out
}

/// "g_Texture3" -> 3
fn texture_unit(sampler: &str) -> Option<u32> {
    sampler.strip_prefix("g_Texture")?.parse().ok()
}

/// "_rt_imageLayerComposite_877_a" -> 877
fn composite_ref(name: &str) -> Option<i64> {
    let rest = name.strip_prefix("_rt_imageLayerComposite_")?;
    rest.split('_').next()?.parse().ok()
}

/// Full-target quad in clip space; top edge at y = -1 (row 0, see `math`).
const CLIP_QUAD: [f32; 20] = [
    -1.0, -1.0, 0.0, 0.0, 0.0, //
    1.0, -1.0, 0.0, 1.0, 0.0, //
    -1.0, 1.0, 0.0, 0.0, 1.0, //
    1.0, 1.0, 0.0, 1.0, 1.0,
];

fn target_size(size: [f32; 2]) -> (u32, u32) {
    let w = (size[0].round() as u32).clamp(1, MAX_TARGET);
    let h = (size[1].round() as u32).clamp(1, MAX_TARGET);
    (w, h)
}

// Our own tiny shader to put a finished layer target onto the scene.
const BLIT_VERT: &str = "uniform mat4 g_ModelViewProjectionMatrix;\n\
    attribute vec3 a_Position;\nattribute vec2 a_TexCoord;\nvarying vec2 v_TexCoord;\n\
    void main() {\n  gl_Position = mul(vec4(a_Position, 1.0), g_ModelViewProjectionMatrix);\n  v_TexCoord = a_TexCoord;\n}\n";
const BLIT_FRAG: &str = "uniform sampler2D g_Texture0;\nvarying vec2 v_TexCoord;\n\
    void main() {\n  gl_FragColor = texSample2D(g_Texture0, v_TexCoord);\n}\n";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_texture_units_and_composite_refs() {
        assert_eq!(texture_unit("g_Texture0"), Some(0));
        assert_eq!(texture_unit("g_Texture12"), Some(12));
        assert_eq!(texture_unit("g_Mask"), None);
        assert_eq!(composite_ref("_rt_imageLayerComposite_877_a"), Some(877));
        assert_eq!(composite_ref("_rt_FullFrameBuffer"), None);
    }
}
