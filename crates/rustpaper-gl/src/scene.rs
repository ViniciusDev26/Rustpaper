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
use crate::gpu::{Blend, QuadMesh, Target, Tex, solid_texture, upload_texture};
use crate::math::{self, Mat4};
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
    origin: [f32; 3],
    angle: f32,
    scale: [f32; 3],
    size: [f32; 2],
    color: [f32; 3],
    alpha: f32,
    brightness: f32,
    material: Pass,
    effects: Vec<Effect>,
}

pub struct SceneRenderer {
    assets: Assets,
    pub width: u32,
    pub height: u32,
    clear: [f32; 4],
    ambient: [f32; 3],
    skylight: [f32; 3],
    layers: Vec<Layer>,
    /// Layers some other layer samples as `_rt_imageLayerComposite_<id>_a`.
    referenced: HashSet<i64>,

    sources: HashMap<String, Option<ShaderSource>>,
    combos_by_key: HashMap<String, (String, BTreeMap<String, i64>)>,
    programs: HashMap<String, Option<Program>>,
    textures: HashMap<String, Tex>,
    white: Tex,

    quad: QuadMesh,
    blit: Program,
    scene: Target,
    fullframe: Target,
    composites: HashMap<i64, Target>,
    pool: HashMap<(u32, u32), Vec<Target>>,

    /// Human-readable problems found while loading/rendering (deduplicated).
    pub warnings: Vec<String>,
    /// Debug: when set, every intermediate layer/effect target of the next
    /// `render` is written here as PNG (see `rustpaper-render --dump-passes`).
    pub dump_dir: Option<std::path::PathBuf>,
}

const MAX_TARGET: u32 = 8192;

impl SceneRenderer {
    pub fn load(
        gl: &glow::Context,
        wallpaper_dir: &Path,
        base_assets: &Path,
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
        if let Some(props) = project
            .pointer("/general/properties")
            .and_then(|p| p.as_object())
        {
            value::apply_user_properties(&mut scene, props);
        }

        let general = scene.get("general").cloned().unwrap_or_default();
        let proj = general
            .get("orthogonalprojection")
            .cloned()
            .unwrap_or_default();
        let width = proj.get("width").and_then(|v| v.as_u64()).unwrap_or(1920) as u32;
        let height = proj.get("height").and_then(|v| v.as_u64()).unwrap_or(1080) as u32;
        let cc = value::get_vec3(&general, "clearcolor", [0.0, 0.0, 0.0]);

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
            referenced: HashSet::new(),
            sources: HashMap::new(),
            combos_by_key: HashMap::new(),
            programs: HashMap::new(),
            textures: HashMap::new(),
            white: solid_texture(gl, [255, 255, 255, 255]),
            quad: QuadMesh::new(gl),
            blit,
            scene: Target::new(gl, width, height),
            fullframe: Target::new(gl, width, height),
            composites: HashMap::new(),
            pool: HashMap::new(),
            warnings: Vec::new(),
            dump_dir: None,
            assets,
        };

        let empty = Vec::new();
        let objects = scene
            .get("objects")
            .and_then(|o| o.as_array())
            .unwrap_or(&empty);
        for obj in objects {
            if obj.get("image").is_some() {
                match r.load_layer(gl, obj) {
                    Ok(layer) => r.layers.push(layer),
                    Err(e) => r.warn(format!(
                        "layer {:?} skipped: {e}",
                        obj.get("name").and_then(|n| n.as_str()).unwrap_or("?")
                    )),
                }
            } else if obj.get("particle").is_some() {
                r.warn("particle systems are not rendered by rustpaper-gl yet".into());
            } else if obj.get("text").is_some() {
                r.warn("text objects are not rendered yet".into());
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
        let origin = match kind {
            LayerKind::Passthrough { fullscreen: true } => [w / 2.0, h / 2.0, 0.0],
            _ => value::get_vec3(obj, "origin", [w / 2.0, h / 2.0, 0.0]),
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
            origin,
            angle: value::get_vec3(obj, "angles", [0.0; 3])[2],
            scale: value::get_vec3(obj, "scale", [1.0; 3]),
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
                {
                    self.texture(gl, n);
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

        for i in 0..self.layers.len() {
            if !self.layers[i].visible {
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
    }

    /// The finished frame (scene resolution).
    pub fn output(&self) -> &Target {
        &self.scene
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
        let model = math::model(l.origin, l.angle, l.scale);
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
                let model = math::model(l.origin, l.angle, l.scale);
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
            _ => (
                math::model(l.origin, l.angle, l.scale),
                math::quad(l.size[0], l.size[1], 1.0, 1.0),
            ),
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
