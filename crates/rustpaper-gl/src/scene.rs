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
use crate::text;
use crate::value;

/// A scene `sound` object: audio files the wallpaper plays.
#[derive(Clone, Debug)]
pub struct SceneSound {
    pub name: String,
    /// (path inside the wallpaper, bytes), in playlist order.
    pub files: Vec<(String, Vec<u8>)>,
    /// `loop` (the playlist over and over), `single` (once) or `random`
    /// (a random file, then a pause of `min_time`..`max_time` seconds).
    pub mode: String,
    /// 0..1, after user properties.
    pub volume: f32,
    pub min_time: f32,
    pub max_time: f32,
    /// `startsilent`: WE keeps it muted until a script starts it.
    pub start_silent: bool,
}

/// Inputs that change every frame.
#[derive(Clone, Copy, Debug)]
pub struct FrameInput {
    /// Seconds since the wallpaper started (`g_Time`).
    pub time: f32,
    /// Cursor in [0,1]x[0,1], (0,0) = top-left (`g_PointerPosition`).
    pub pointer: [f32; 2],
    /// Fraction of the day in [0,1) (`g_Daytime`).
    pub daytime: f32,
    /// Spectrum of the system audio (zeros when not captured).
    pub audio: crate::audio::AudioFrame,
}

impl Default for FrameInput {
    fn default() -> Self {
        FrameInput {
            time: 0.0,
            pointer: [0.5, 0.5],
            daytime: 0.5,
            audio: crate::audio::AudioFrame::default(),
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
    /// `"command": "copy"` passes: (run before material pass n, source,
    /// target); n == passes.len() runs after the last one.
    copies: Vec<(usize, String, String)>,
    /// (name, downscale factor, unique). A `unique` FBO keeps its content
    /// from one frame to the next (motion blur's accumulator).
    fbos: Vec<(String, f32, bool)>,
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
    /// `parallaxDepth`: how much the object follows the camera parallax.
    parallax: [f32; 2],
    size: [f32; 2],
    color: [f32; 3],
    alpha: f32,
    brightness: f32,
    /// The object sets `brightness` itself: only then does it override the
    /// material's own Brightness constant (Water effect 13 820654165 has
    /// "Bright": 0.86 in the material and no object brightness).
    own_brightness: bool,
    /// Keyframed properties (timeline animations), applied every frame.
    anim: Option<LayerAnim>,
    /// Animated image (GIF scenes, `spritesheet` combo): frames over images.
    flipbook: Option<Flipbook>,
    /// `"attachment": name` with a puppet `parent`: (parent id, name, local).
    attach: Option<(i64, String, Mat4)>,
    material: Pass,
    /// Puppet warp: skeletal animation + its mesh (replaces the quad).
    puppet: Option<(crate::puppet::PuppetRt, crate::gpu::MeshGpu)>,
    /// Composite pass with the object's colorBlendMode (layers with a target).
    composite: Option<Pass>,
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
    parallax: [f32; 2],
    /// Timeline animations of origin / angles / scale.
    anim: Option<LayerAnim>,
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

/// A text object's content source and its rasterized texture.
struct TextRt {
    id: i64,
    source: text::TextSource,
    font: ab_glyph::FontVec,
    style: text::TextStyle,
    shown: Option<String>,
    tex: Option<Tex>,
    /// The layer's world matrix as loaded (alignment shifts start from it).
    world0: Option<Mat4>,
}

pub struct SceneRenderer {
    assets: Assets,
    pub width: u32,
    pub height: u32,
    clear: [f32; 4],
    ambient: [f32; 3],
    skylight: [f32; 3],
    /// `general.fov` (degrees), for perspective particle systems.
    fov: f32,
    /// `general.zoom`: the camera zooms in around the scene center.
    zoom: f32,
    /// Attached layers (index), parents before children.
    attach_order: Vec<usize>,
    /// `general.camerashake`: (amplitude, speed, roughness).
    shake: Option<(f32, f32, f32)>,
    /// Scene time of the frame being rendered (camera shake).
    time: f32,
    layers: Vec<Layer>,
    particle_objs: Vec<ParticleObject>,
    texts: Vec<TextRt>,
    order: Vec<ObjRef>,
    /// Layers some other layer samples as `_rt_imageLayerComposite_<id>_a`.
    referenced: HashSet<i64>,

    sources: HashMap<String, Option<ShaderSource>>,
    combos_by_key: HashMap<String, (String, BTreeMap<String, i64>)>,
    programs: HashMap<String, Option<Program>>,
    textures: HashMap<String, Tex>,
    /// Sound objects, for the live engine to play.
    pub sounds: Vec<SceneSound>,
    /// Padded uploads for image material passes (None: no padding).
    padded_textures: HashMap<String, Option<Tex>>,
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
    /// `unique` effect FBOs, kept across frames: (layer, effect, name).
    unique_fbos: HashMap<(usize, usize, String), Target>,
    /// Scene time of the last render (None before the first one).
    last_render: Option<f32>,

    /// Every object's world matrix by id (parent chains resolved).
    worlds: HashMap<i64, Mat4>,
    /// `general.cameraparallax*` and the smoothed pointer offset it follows.
    parallax: Option<Parallax>,

    /// Human-readable problems found while loading/rendering (deduplicated).
    pub warnings: Vec<String>,
    /// The scene's animated image asks for no interpolation (pixel art):
    /// scale the frame to the screen with nearest filtering.
    pub pixelated: bool,
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
        // `"auto": true` (GIF scenes): the scene is the size of its image,
        // which sits centered (origin = size / 2).
        let auto_size = scene
            .get("objects")
            .and_then(|o| o.as_array())
            .and_then(|objs| {
                objs.iter()
                    .find(|o| o.get("image").is_some_and(|i| i.is_string()))
            })
            .map(|o| value::get_vec3(o, "origin", [960.0, 540.0, 0.0]))
            .filter(|o| o[0] > 0.0 && o[1] > 0.0)
            .map(|o| ((o[0] * 2.0).round() as u64, (o[1] * 2.0).round() as u64));
        let auto = value::get_bool(&proj, "auto", false);
        let width = proj
            .get("width")
            .and_then(|v| v.as_u64())
            .or(auto_size.filter(|_| auto).map(|s| s.0))
            .unwrap_or(1920) as u32;
        let height = proj
            .get("height")
            .and_then(|v| v.as_u64())
            .or(auto_size.filter(|_| auto).map(|s| s.1))
            .unwrap_or(1080) as u32;
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
            fov: value::get_f32(&general, "fov", 50.0),
            zoom: value::get_f32(&general, "zoom", 1.0).max(0.01),
            shake: value::get_bool(&general, "camerashake", false).then(|| {
                (
                    value::get_f32(&general, "camerashakeamplitude", 0.5),
                    value::get_f32(&general, "camerashakespeed", 1.0),
                    value::get_f32(&general, "camerashakeroughness", 0.0),
                )
            }),
            attach_order: Vec::new(),
            time: 0.0,
            layers: Vec::new(),
            particle_objs: Vec::new(),
            texts: Vec::new(),
            order: Vec::new(),
            referenced: HashSet::new(),
            sources: HashMap::new(),
            combos_by_key: HashMap::new(),
            programs: HashMap::new(),
            textures: HashMap::new(),
            sounds: Vec::new(),
            padded_textures: HashMap::new(),
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
            unique_fbos: HashMap::new(),
            last_render: None,
            warnings: Vec::new(),
            pixelated: false,
            worlds: HashMap::new(),
            parallax: Parallax::from_general(&general),
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
            } else if let Some(files) = obj.get("sound").and_then(|s| s.as_array()) {
                // Sound objects: the live engine plays them (WE does, and its
                // audio-reactive layers then react to that music).
                if !value::get_bool(obj, "visible", true) {
                    continue;
                }
                let files: Vec<(String, Vec<u8>)> = files
                    .iter()
                    .filter_map(|f| f.as_str())
                    .filter_map(|f| r.assets.read(f).map(|b| (f.to_string(), b.into_owned())))
                    .collect();
                if !files.is_empty() {
                    r.sounds.push(SceneSound {
                        name: obj
                            .get("name")
                            .and_then(|n| n.as_str())
                            .unwrap_or("")
                            .to_string(),
                        files,
                        mode: obj
                            .get("playbackmode")
                            .and_then(|m| m.as_str())
                            .unwrap_or("loop")
                            .to_string(),
                        volume: value::get_f32(obj, "volume", 1.0).clamp(0.0, 1.0),
                        min_time: value::get_f32(obj, "mintime", 1.0).max(0.0),
                        max_time: value::get_f32(obj, "maxtime", 5.0).max(0.0),
                        start_silent: value::get_bool(obj, "startsilent", false),
                    });
                }
            } else if obj.get("text").is_some_and(|t| !t.is_null()) {
                match r.load_text(obj) {
                    Ok(t) => {
                        r.texts.push(t);
                        match r.load_layer(gl, obj) {
                            Ok(layer) => {
                                r.order.push(ObjRef::Layer(r.layers.len()));
                                r.layers.push(layer);
                            }
                            Err(e) => r.warn(format!("text layer skipped: {e}")),
                        }
                    }
                    Err(e) => r.warn(format!(
                        "text {:?} skipped: {e}",
                        obj.get("name").and_then(|n| n.as_str()).unwrap_or("?")
                    )),
                }
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
        // a hidden parent hides its descendants (unless `disablepropagation`)
        let hidden = hidden_by_ancestor(objects);
        for l in &mut r.layers {
            l.visible &= !hidden.contains(&l.id);
        }
        for p in &mut r.particle_objs {
            p.visible &= !hidden.contains(&p.id);
        }
        r.attach_order = attach_order(&r.layers);
        Ok(r)
    }

    fn warn(&mut self, msg: String) {
        if !self.warnings.contains(&msg) {
            self.warnings.push(msg);
        }
    }

    // ---------------------------------------------------------------- loading

    fn load_layer(&mut self, gl: &glow::Context, obj: &Value) -> Result<Layer, String> {
        let (image, model, pass_owned) = match obj.get("image").and_then(|i| i.as_str()) {
            Some(image) => {
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
                let pass = mat
                    .get("passes")
                    .and_then(|p| p.get(0))
                    .cloned()
                    .ok_or("material has no passes")?;
                (image.to_string(), model, pass)
            }
            // text objects: a plain image material over the rasterized text
            // (VERSION 2 = tinted by the object's color + alpha)
            None => {
                let id = obj.get("id").and_then(|i| i.as_i64()).unwrap_or(0);
                (
                    format!("text #{id}"),
                    Value::Null,
                    serde_json::json!({
                        "shader": "genericimage2",
                        "blending": "translucent",
                        "combos": { "VERSION": 2 },
                        "textures": [format!("_text_{id}")]
                    }),
                )
            }
        };
        let pass_json = &pass_owned;
        // puppet warp: the quad becomes a skinned mesh
        let puppet = match model.get("puppet").and_then(|p| p.as_str()) {
            Some(path) => match self
                .assets
                .read(path)
                .map(|b| rustpaper_core::mdl::Puppet::parse(&b))
            {
                Some(Ok(p)) => {
                    let mesh = crate::gpu::MeshGpu::new(gl, &p.mesh.indices);
                    Some((
                        crate::puppet::PuppetRt::new(p, obj.get("animationlayers")),
                        mesh,
                    ))
                }
                Some(Err(e)) => {
                    self.warn(format!("{image}: puppet {path}: {e}"));
                    None
                }
                None => {
                    self.warn(format!("{image}: puppet {path} not found"));
                    None
                }
            },
            None => None,
        };

        let kind = if model.get("passthrough").and_then(|v| v.as_bool()) == Some(true) {
            LayerKind::Passthrough {
                fullscreen: model.get("fullscreen").and_then(|v| v.as_bool()) == Some(true),
            }
        } else {
            LayerKind::Image
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

        // Object-level combo: colorBlendMode N selects the material's BLENDMODE
        // code path (multiply, screen, ... against what is behind the object).
        // A layer drawn through its own target (effects, compose layers) is
        // blended with the background when it is COMPOSITED, not inside its
        // target (whose screen coordinates aren't the scene's).
        let mut extra_combos = BTreeMap::new();
        let blend_mode = obj
            .get("colorBlendMode")
            .and_then(|v| value::plain(v).as_i64())
            .unwrap_or(0);
        let via_target = !effects.is_empty() || matches!(kind, LayerKind::Passthrough { .. });
        let mut composite = None;
        if blend_mode != 0 {
            if via_target {
                let j = serde_json::json!({
                    "shader": "genericimage2",
                    "blending": "translucent",
                    "combos": { "VERSION": 2, "BLENDMODE": blend_mode },
                    "textures": ["_composite_"]
                });
                composite = Some(self.resolve_pass(gl, &j, None, &[], &BTreeMap::new(), false)?);
            } else {
                extra_combos.insert("BLENDMODE".to_string(), blend_mode);
            }
        }
        let material = self.resolve_pass(gl, pass_json, None, &[], &extra_combos, false)?;
        let flipbook = self.load_flipbook(gl, pass_json);

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

        Ok(Layer {
            parallax: value::get_vec2(obj, "parallaxDepth", [1.0, 1.0]),
            id: obj.get("id").and_then(|i| i.as_i64()).unwrap_or(-1),
            name: obj
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or("")
                .to_string(),
            visible: object_visible(obj),
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
            own_brightness: obj.get("brightness").is_some()
                || obj.pointer("/brightness/animation").is_some(),
            anim: LayerAnim::parse(obj, self.world_of(obj)),
            flipbook,
            attach: obj
                .get("attachment")
                .and_then(|a| a.as_str())
                .zip(obj.get("parent").and_then(|p| p.as_i64()))
                .map(|(name, parent)| (parent, name.to_string(), local_matrix(obj))),
            material,
            puppet,
            composite,
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
        let mut copies = Vec::new();
        for (i, p) in def
            .get("passes")
            .and_then(|p| p.as_array())
            .into_iter()
            .flatten()
            .enumerate()
        {
            let Some(mat_path) = p.get("material").and_then(|m| m.as_str()) else {
                let field = |k: &str| p.get(k).and_then(|v| v.as_str()).map(String::from);
                match (
                    field("command").as_deref(),
                    field("source"),
                    field("target"),
                ) {
                    (Some("copy"), Some(src), Some(dst)) => copies.push((passes.len(), src, dst)),
                    _ => self.warn(format!("{file}: effect pass {i} has no material")),
                }
                continue;
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
                    f.get("unique").and_then(|u| u.as_bool()).unwrap_or(false),
                ))
            })
            .collect();
        Ok(Effect {
            file: file.to_string(),
            passes,
            copies,
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
                    // keep timelines whole (evaluated per frame in set_params)
                    let keep = v.get("animation").is_some_and(|a| a.is_object());
                    constants.insert(
                        k.clone(),
                        if keep {
                            v.clone()
                        } else {
                            value::plain(v).clone()
                        },
                    );
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
                    && !n.starts_with("_text_")
                    && n != "_composite_"
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

    /// Flipbook of an image material with the `spritesheet` combo: every image
    /// of its .tex plus the frame list of the embedded `TEXS` block.
    fn load_flipbook(&mut self, gl: &glow::Context, pass_json: &Value) -> Option<Flipbook> {
        let combos = pass_json.get("combos")?;
        let on = ["spritesheet", "SPRITESHEET"]
            .iter()
            .any(|k| combos.get(k).and_then(|v| v.as_i64()).unwrap_or(0) != 0);
        if !on {
            return None;
        }
        let name = pass_json.get("textures")?.get(0)?.as_str()?.to_string();
        let bytes = self.assets.read(&format!("materials/{name}.tex"))?;
        let frames = rustpaper_core::tex::sheet_frames(&bytes)?;
        let mut images = Vec::new();
        for i in 0..rustpaper_core::tex::image_count(&bytes).max(1) {
            match rustpaper_core::tex::parse_image(&bytes, i) {
                // the header size is one FRAME; each image is a whole atlas
                Ok(mut d) => {
                    self.pixelated |= d.flags & 1 != 0;
                    d.real_width = d.width;
                    d.real_height = d.height;
                    images.push(upload_texture(gl, &d));
                }
                Err(e) => {
                    self.warn(format!("materials/{name}.tex image {i}: {e}"));
                    break;
                }
            }
        }
        (frames.len() > 1 && !images.is_empty()).then_some(Flipbook { images, frames })
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
        // Effects with `unique` FBOs (motion blur) accumulate over frames: a
        // render that starts cold, or jumps in time, first runs 1 s of frames
        // so it doesn't begin from a black accumulator (WE has been running
        // for a while when the harness captures it).
        let has_unique = self
            .layers
            .iter()
            .any(|l| l.effects.iter().any(|e| e.fbos.iter().any(|f| f.2)));
        let jumped = self
            .last_render
            .is_none_or(|t| frame.time < t || frame.time - t > 0.5);
        if has_unique && jumped {
            for k in (1..=30).rev() {
                self.render_frame(
                    gl,
                    FrameInput {
                        time: frame.time - k as f32 / 30.0,
                        ..frame
                    },
                );
            }
        }
        self.last_render = Some(frame.time);
        self.render_frame(gl, frame);
    }

    fn render_frame(&mut self, gl: &glow::Context, frame: FrameInput) {
        unsafe {
            gl.disable(glow::DEPTH_TEST);
            gl.disable(glow::CULL_FACE);
        }
        self.update_texts(gl);
        self.time = frame.time;
        if let Some(p) = &mut self.parallax {
            p.follow(frame.pointer, frame.time);
        }
        for l in &mut self.layers {
            if let Some(a) = &l.anim {
                a.apply(
                    frame.time,
                    &mut l.world,
                    &mut l.color,
                    &mut l.alpha,
                    &mut l.brightness,
                );
            }
            if let Some((rt, mesh)) = &mut l.puppet {
                rt.update(frame.time);
                mesh.upload(gl, &rt.vertices);
            }
        }
        for o in &mut self.particle_objs {
            if let Some(a) = &o.anim {
                let (mut c, mut al, mut b) = ([1.0; 3], 1.0, 1.0);
                a.apply(frame.time, &mut o.world, &mut c, &mut al, &mut b);
            }
        }
        // layers hanging from a puppet's attachment follow its current pose
        for k in 0..self.attach_order.len() {
            let i = self.attach_order[k];
            let Some((parent, name, local)) = self.layers[i].attach.clone() else {
                continue;
            };
            let Some(pi) = self.layers.iter().position(|l| l.id == parent) else {
                continue;
            };
            let p = &self.layers[pi];
            if let Some(att) = p.puppet.as_ref().and_then(|(rt, _)| rt.attachment(&name)) {
                let w = math::mul(&math::mul(&p.world, &att), &local);
                self.layers[i].world = w;
            }
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

    /// Scene projection. Note: `camera.eye`/`center` are NOT applied — WE
    /// ignores them when playing a wallpaper (measured: Yor, camera at
    /// (67, -37), aligns with WE at offset 0 and scale 1).
    fn view_proj(&self) -> Mat4 {
        let (w, h) = (self.width as f32, self.height as f32);
        let mut ortho = math::scene_ortho(w, h);
        if let Some((amp, speed, rough)) = self.shake {
            // Smooth wandering offset. Measured on Ice Princess 2800248288
            // (amplitude 0.35, speed 1) by tracking WE's whole-image shift
            // over 8 s: near-sinusoids of +-4.1 px (x, period ~6 s) and
            // +-4.4 px (y, ~4.8 s) on a 1920 screen = 0.0064 * amplitude of
            // the width. Roughness adds a faster component (not measured).
            let t = self.time * speed;
            let n = |f: f32, a: f32| {
                ((t * f + a).sin() + (t * f * 2.7 + a * 1.9).sin() * 0.4 * rough)
                    / (1.0 + 0.4 * rough)
            };
            let k = amp * 0.0064;
            let shake = math::model(
                [n(1.05, 0.3) * k * w, n(1.31, 2.1) * k * w, 0.0],
                0.0,
                [1.0; 3],
            );
            ortho = math::mul(&ortho, &shake);
        }
        // (camera shake does NOT zoom: Kirito x Asuna 3463520581, amplitude
        // 0.5, fits best unzoomed)
        let zoom_total = self.zoom;
        if (zoom_total - 1.0).abs() < 1e-4 {
            return ortho;
        }
        // zoom around the center: translate(c) * scale(z) * translate(-c)
        let z = zoom_total;
        let zoom = math::model(
            [w / 2.0 * (1.0 - z), h / 2.0 * (1.0 - z), 0.0],
            0.0,
            [z, z, 1.0],
        );
        math::mul(&ortho, &zoom)
    }

    /// `view_proj` shifted by the camera parallax for an object at `depth`.
    fn view_proj_at(&self, depth: [f32; 2]) -> Mat4 {
        let proj = self.view_proj();
        match &self.parallax {
            Some(p) => {
                let [dx, dy] = p.shift(depth, self.width as f32, self.height as f32);
                math::mul(&proj, &math::model([dx, dy, 0.0], 0.0, [1.0; 3]))
            }
            None => proj,
        }
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
        set_params(gl, prog, constants, self.time);
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
        let proj = self.view_proj_at(l.parallax);
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
                let proj = self.view_proj();
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
            for (name, scale, unique) in self.layers[i].effects[e].fbos.clone() {
                let (w, h) = ((fw as f32 / scale) as u32, (fh as f32 / scale) as u32);
                let t = if unique {
                    let key = (i, e, name.clone());
                    match self.unique_fbos.get(&key) {
                        Some(t) => *t,
                        None => {
                            let t = Target::new(gl, w.max(1), h.max(1));
                            t.clear(gl, [0.0; 4]);
                            self.unique_fbos.insert(key, t);
                            t
                        }
                    }
                } else {
                    self.take(gl, w.max(1), h.max(1))
                };
                named.insert(name, t);
            }
            let n_passes = self.layers[i].effects[e].passes.len();
            for p in 0..=n_passes {
                let copies: Vec<(String, String)> = self.layers[i].effects[e]
                    .copies
                    .iter()
                    .filter(|c| c.0 == p)
                    .map(|c| (c.1.clone(), c.2.clone()))
                    .collect();
                for (src, dst) in copies {
                    match (named.get(&src), named.get(&dst)) {
                        (Some(a), Some(b)) => a.blit_to(gl, b),
                        _ => self.warn(format!("effect copy {src} -> {dst}: unknown FBO")),
                    }
                }
                if p == n_passes {
                    break;
                }
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
            let unique: Vec<String> = self.layers[i].effects[e]
                .fbos
                .iter()
                .filter(|f| f.2)
                .map(|f| f.0.clone())
                .collect();
            for (name, t) in named {
                if !unique.contains(&name) {
                    self.give(t);
                }
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
        let proj = self.view_proj_at(l.parallax);
        let mvp = math::mul(&proj, &model);
        if let Some(blend) = l.composite.as_ref().map(|c| c.blend) {
            let scene = self.scene;
            self.draw_pass(
                gl,
                DrawCall {
                    layer: i,
                    which: PassRef::Composite,
                    target: scene,
                    previous: Some(t.tex),
                    effect_targets: &HashMap::new(),
                    mvp,
                    model,
                    proj,
                    verts,
                    blend,
                    frame: FrameInput::default(),
                },
            );
            return;
        }
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
            PassRef::Composite => self.layers[call.layer]
                .composite
                .as_ref()
                .expect("composite pass"),
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
                TexSource::Named(n) if n == "_composite_" => call.previous.unwrap_or(self.white),
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

        // WE keeps the power-of-two padding of .tex files and gives an image
        // layer's quad UVs 0..content/size of texture 0, so every other
        // texture of that material pass is sampled over the same share of ITS
        // padded size (flow maps, masks). Water effect 13 820654165 (flowmask
        // 800x600 in 1024x1024) and nebula 833227004 depend on it: mean SSIM
        // 0.856 -> 0.993 and 0.924 -> 0.985 when emulated. Effects and
        // particles keep cropped textures (their shaders rescale UVs with
        // g_TextureNResolution, which is equivalent).
        let padded_mode = is_material && {
            let l = &self.layers[call.layer];
            l.kind == LayerKind::Image && l.puppet.is_none() && l.flipbook.is_none()
        };
        let mut uv_scale = [1.0f32, 1.0];
        if padded_mode {
            for ((_, src), b) in slots.iter().zip(bound.iter_mut()) {
                if let TexSource::Named(n) = src
                    && let Some(t) = self.padded_texture(gl, n, call.effect_targets)
                {
                    b.1 = t;
                }
            }
            if let Some((_, t0)) = bound.iter().find(|(u, _)| *u == 0) {
                uv_scale = t0.content_uv();
            }
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
                    PassRef::Composite => "composite".to_string(),
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
        set_audio(gl, prog, &f.audio);
        prog.set(gl, "g_Frametime", &[1.0 / 30.0]);
        prog.set(gl, "g_PointerPosition", &f.pointer);
        prog.set(gl, "g_PointerPositionLast", &f.pointer);
        // the (smoothed) cursor, like g_PointerPosition: depthparallax and
        // similar effects shift layers with it
        let par = self
            .parallax
            .as_ref()
            .map_or(f.pointer, |p| [p.offset[0] + 0.5, p.offset[1] + 0.5]);
        prog.set(gl, "g_ParallaxPosition", &par);
        prog.set(gl, "g_TexelSize", &[1.0 / tw, 1.0 / th]);
        prog.set(gl, "g_TexelSizeHalf", &[0.5 / tw, 0.5 / th]);
        prog.set(gl, "g_Screen", &[sw, sh, sw / sh]);
        prog.set(gl, "g_EyePosition", &[sw / 2.0, sh / 2.0, 1.0]);
        prog.set(gl, "g_LightAmbientColor", &self.ambient);
        prog.set(gl, "g_LightSkylightColor", &self.skylight);
        let flip = is_material
            .then(|| self.layers[call.layer].flipbook.as_ref())
            .flatten()
            .and_then(|fb| fb.at(f.time));
        match flip {
            Some((tex, rot, tr)) => {
                prog.set(gl, "g_Texture0Rotation", &rot);
                prog.set(gl, "g_Texture0Translation", &tr);
                unsafe {
                    gl.active_texture(glow::TEXTURE0);
                    gl.bind_texture(glow::TEXTURE_2D, Some(tex.id));
                }
            }
            None => {
                prog.set(gl, "g_Texture0Rotation", &[1.0, 0.0, 0.0, 1.0]);
                prog.set(gl, "g_Texture0Translation", &[0.0, 0.0]);
            }
        }
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
        set_params(gl, prog, &constants, self.time);

        // 3) the object's own properties (image pass only)
        if is_material {
            if l.own_brightness {
                prog.set(gl, "g_Brightness", &[l.brightness]);
            }
            prog.set(gl, "g_UserAlpha", &[l.alpha]);
            prog.set(gl, "g_Alpha", &[l.alpha]);
            prog.set(gl, "g_Color", &l.color);
            prog.set(
                gl,
                "g_Color4",
                &[l.color[0], l.color[1], l.color[2], l.alpha],
            );
        }

        match (&call.which, &self.layers[call.layer].puppet) {
            (PassRef::Material, Some((_, mesh))) => mesh.draw(gl),
            _ if uv_scale != [1.0, 1.0] => {
                let mut v = call.verts;
                for vert in v.chunks_mut(5) {
                    vert[3] *= uv_scale[0];
                    vert[4] *= uv_scale[1];
                }
                self.quad.draw(gl, &v);
            }
            _ => self.quad.draw(gl, &call.verts),
        }
    }

    /// The padded (WE layout) upload of a file texture, when it has padding.
    fn padded_texture(
        &mut self,
        gl: &glow::Context,
        name: &str,
        effect_targets: &HashMap<String, Target>,
    ) -> Option<Tex> {
        if name.starts_with("_rt_")
            || composite_ref(name).is_some()
            || effect_targets.contains_key(name)
        {
            return None;
        }
        if let Some(t) = self.padded_textures.get(name) {
            return *t;
        }
        let t = self
            .assets
            .read(&format!("materials/{name}.tex"))
            .and_then(|b| rustpaper_core::tex::parse(&b).ok())
            .filter(|d| d.width > d.real_width || d.height > d.real_height)
            .map(|d| crate::gpu::upload_texture_padded(gl, &d));
        self.padded_textures.insert(name.to_string(), t);
        t
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
            n if n.starts_with("_text_") => {
                let id: i64 = n[6..].parse().unwrap_or(-1);
                match self.texts.iter().find(|t| t.id == id).and_then(|t| t.tex) {
                    Some(t) => t,
                    None => self.white,
                }
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

    // ------------------------------------------------------------------ text

    fn load_text(&mut self, obj: &Value) -> Result<TextRt, String> {
        let id = obj.get("id").and_then(|i| i.as_i64()).unwrap_or(0);
        let font_name = obj
            .get("font")
            .and_then(|f| f.as_str())
            .unwrap_or("systemfont_arial");
        let bytes = match self.assets.read(font_name) {
            Some(b) => b.into_owned(),
            None => {
                let path = text::system_font_path(font_name)
                    .or_else(|| text::system_font_path("sans-serif"))
                    .ok_or_else(|| format!("font {font_name} not found"))?;
                std::fs::read(path).map_err(|e| e.to_string())?
            }
        };
        let font =
            ab_glyph::FontVec::try_from_vec(bytes).map_err(|e| format!("font {font_name}: {e}"))?;
        let size = value::get_vec2(obj, "size", [512.0, 128.0]);
        let bg = value::get_bool(obj, "opaquebackground", false).then(|| {
            let c = value::get_vec3(obj, "backgroundcolor", [0.0; 3]);
            let b = value::get_f32(obj, "backgroundbrightness", 1.0);
            [c[0] * b, c[1] * b, c[2] * b]
        });
        Ok(TextRt {
            id,
            source: text::TextSource::from_json(&obj["text"]),
            font,
            style: text::TextStyle {
                em: value::get_f32(obj, "pointsize", 32.0) * text::UNITS_PER_POINT,
                halign: obj
                    .get("horizontalalign")
                    .and_then(|v| v.as_str())
                    .unwrap_or("center")
                    .to_string(),
                valign: obj
                    .get("verticalalign")
                    .and_then(|v| v.as_str())
                    .unwrap_or("center")
                    .to_string(),
                padding: value::get_f32(obj, "padding", 0.0),
                size: (
                    (size[0].round() as u32).clamp(1, 4096),
                    (size[1].round() as u32).clamp(1, 4096),
                ),
                background: bg,
            },
            shown: None,
            tex: None,
            world0: None,
        })
    }

    /// Re-rasterizes text whose content changed (clocks tick every second).
    /// The canvas grows past the object's box when the text doesn't fit, and
    /// the text layer's quad follows it.
    fn update_texts(&mut self, gl: &glow::Context) {
        for t in &mut self.texts {
            let now = t.source.current();
            if t.shown.as_deref() == Some(now.as_str()) {
                continue;
            }
            let mut style = t.style.clone();
            style.size = text::canvas_size(&t.font, &now, &t.style);
            let px = text::rasterize(&t.font, &now, &style);
            let (w, h) = style.size;
            let tex = match t.tex {
                Some(tex) if tex.width == w && tex.height == h => tex,
                _ => {
                    let tex = crate::gpu::empty_texture(gl, w, h);
                    t.tex = Some(tex);
                    tex
                }
            };
            crate::gpu::write_texture(gl, &tex, &px);
            if let Some(l) = self.layers.iter_mut().find(|l| l.id == t.id) {
                l.size = [w as f32, h as f32];
                let world0 = *t.world0.get_or_insert(l.world);
                // Left/right aligned text starts/ends AT the object's origin
                // (not at the edge of a box centered on it): Toyota 3560329032's
                // right-aligned clock at x = 3785 of 3840 ends there in WE
                // (we drew it ~400 units off-screen); Asuna 3239767814 (left)
                // 0.973 -> 0.976.
                let edge = w as f32 / 2.0 - t.style.padding;
                let dx = match t.style.halign.as_str() {
                    "right" => -edge,
                    "left" => edge,
                    _ => 0.0,
                };
                l.world = math::mul(&world0, &math::model([dx, 0.0, 0.0], 0.0, [1.0; 3]));
            }
            t.shown = Some(now);
        }
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
            parallax: value::get_vec2(obj, "parallaxDepth", [1.0, 1.0]),
            id,
            name: obj
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or("")
                .to_string(),
            visible: object_visible(obj),
            world: self.world_of(obj),
            anim: LayerAnim::parse(obj, self.world_of(obj)),
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
                    // sizes can be fractional ("width": 102.5 = 512 / 5):
                    // reading them as integers dropped the sheet and drew all
                    // frames as one stretched sprite (rosepetals, Luffy)
                    Some((
                        seq.get("frames")?.as_f64()? as u32,
                        seq.get("width")?.as_f64()? as f32,
                        seq.get("height")?.as_f64()? as f32,
                    ))
                });
            if let Some((frames, fw, fh)) = frames
                && frames > 1
                && fw > 0.0
                && fh > 0.0
            {
                sheet = true;
                render_var1 = [
                    fw / tex.width as f32,
                    fh / tex.height as f32,
                    frames as f32,
                    fh / fw,
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
            // Runtime combo WE always sets for flipbooks: crossfade between
            // frames. Without it big sheets (fog) jump every frame change
            // (Demon Angel 1851646226: motion energy ratio 4.3 -> 2.7).
            combos.insert("SPRITESHEETBLEND".to_string(), 1);
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
                    // children inherit the object's instance override (Elden
                    // Ring 3276312017: its ember glow child drawn at full size
                    // and rate washed a band over the screen; mean SSIM 0.979
                    // -> 0.992, energy ratio 1.66 -> 1.00), count included
                    // (Early Morning Forest 1959836574: leaves at count 0.5
                    // with full-count children moved 1.48x WE, 1.34x with it).
                    ov,
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
        for s in &mut o.systems {
            s.sim.audio = frame.audio;
        }
        // WE-RE-002 1.5 / WE-RE-003 1.5: prewarm (`starttime`) runs at
        // creation in fixed steps of 0.05 s (0.2 s when maxcount >= 500),
        // ceil(starttime / dt) of them; the scene clock then starts at 0.
        if o.prewarm > 0.0 && o.systems[0].sim.time == 0.0 {
            let pdt = if o.systems[0].sim.def.max_count >= 500 {
                0.2
            } else {
                0.05
            };
            let n = (o.prewarm / pdt).ceil() as usize;
            for _ in 0..n {
                step_systems(&mut o.systems, pdt, pointer);
            }
            for s in &mut o.systems {
                s.sim.time = o.prewarm;
            }
        }
        let max_steps = ((o.prewarm + 1.0) / particles::STEP) as usize + 120;
        let mut steps = 0;
        while o.systems[0].sim.time + particles::STEP <= target + 1e-4 && steps < max_steps {
            steps += 1;
            step_systems(&mut o.systems, particles::STEP, pointer);
        }
    }

    fn draw_particles(&mut self, gl: &glow::Context, i: usize) {
        let (w, h) = (self.width as f32, self.height as f32);
        let o = &self.particle_objs[i];
        let model = o.world;
        let proj = self.view_proj_at(o.parallax);
        let mvp_flat = math::mul(&proj, &model);
        let (persp, eye) = math::scene_perspective(w, h, self.fov);
        let mvp_persp = math::mul(&persp, &model);
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
            let sys_perspective = sys.sim.def.perspective;
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
            // World-space systems: move the particles by the object's
            // transform here and draw with an identity model, so their quads
            // keep their own size and orientation (see ParticleDef::world_space).
            let world_space = sys.sim.def.world_space && !rope;
            if world_space {
                for v in verts.chunks_mut(particles::VERTEX_FLOATS) {
                    let p = [v[0], v[1], v[2]];
                    let d = [v[13], v[14], v[15]];
                    for r in 0..3 {
                        v[r] = model[r] * p[0]
                            + model[4 + r] * p[1]
                            + model[8 + r] * p[2]
                            + model[12 + r];
                        v[13 + r] = model[r] * d[0] + model[4 + r] * d[1] + model[8 + r] * d[2];
                    }
                }
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
            // `perspective` systems (flags & 4) get a real camera: depth makes
            // near particles bigger and faster (Kakashi 2811157144 motion
            // energy ratio 0.77 -> 0.88, Luffy 0.50 -> 0.56).
            let perspective = sys_perspective;
            let (mvp, m, m_inv) = if world_space {
                let vp = if perspective { persp } else { proj };
                (vp, math::IDENTITY, math::IDENTITY)
            } else if perspective {
                (mvp_persp, model, inverse)
            } else {
                (mvp_flat, model, inverse)
            };
            prog.set(gl, "g_ModelViewProjectionMatrix", &mvp);
            prog.set(gl, "g_ModelMatrix", &m);
            prog.set(gl, "g_ModelMatrixInverse", &m_inv);
            prog.set(
                gl,
                "g_ViewProjectionMatrix",
                if perspective { &persp } else { &proj },
            );
            // Camera axes in WORLD space (identity for the front-facing ortho
            // camera), so sprites rotate with their object. The sprite
            // `orientation` modes (screen / upright / fixed) coincide here;
            // compensating the object rotation made rotated emitters wrong
            // (White Angel 2382081769: mean SSIM 0.952 -> 0.947).
            prog.set(gl, "g_OrientationRight", &[1.0, 0.0, 0.0]);
            prog.set(gl, "g_OrientationUp", &[0.0, 1.0, 0.0]);
            prog.set(gl, "g_OrientationForward", &[0.0, 0.0, 1.0]);
            prog.set(gl, "g_ViewRight", &[1.0, 0.0, 0.0]);
            prog.set(gl, "g_ViewUp", &[0.0, 1.0, 0.0]);
            // far "in front" of the scene plane: trails get right = cross(view, velocity)
            let eye_pos = if perspective {
                eye
            } else {
                [w / 2.0, h / 2.0, 100_000.0]
            };
            prog.set(gl, "g_EyePosition", &eye_pos);
            prog.set(gl, "g_RenderVar0", &rv0);
            prog.set(gl, "g_RenderVar1", &rv1);
            prog.set(gl, "g_LightAmbientColor", &self.ambient);
            prog.set(gl, "g_LightSkylightColor", &self.skylight);
            for (unit, tex) in &bound {
                prog.set(gl, &format!("g_Texture{unit}"), &[*unit as f32]);
                prog.set(gl, &format!("g_Texture{unit}Resolution"), &tex.resolution());
            }
            set_params(gl, prog, &constants, self.time);
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
    /// the colorBlendMode composite of a layer drawn through its own target
    Composite,
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

/// `g_AudioSpectrum{16,32,64}{Left,Right}` from the current spectrum.
fn set_audio(gl: &glow::Context, prog: &Program, audio: &crate::audio::AudioFrame) {
    for n in [16, 32, 64] {
        for (right, side) in [(false, "Left"), (true, "Right")] {
            let name = format!("g_AudioSpectrum{n}{side}");
            if prog.uniforms.contains_key(&name) {
                prog.set(gl, &name, &audio.bands(right, n));
            }
        }
    }
}

/// Sets the shader's annotated material parameters (`uniform float g_Speed;
/// // {"material":"speed","default":1}`): the annotated default, overridden by
/// the pass constants (exact name first, then case-insensitive).
/// Constants can carry a timeline (`{"value": v, "animation": {...}}`):
/// evaluated at the scene time `time`.
fn set_params(gl: &glow::Context, prog: &Program, constants: &HashMap<String, Value>, time: f32) {
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
            .map(|c| match crate::keyframes::Track::parse(c) {
                Some(track) => track.eval(time),
                None => value::floats(c),
            })
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| p.default.clone());
        prog.set(gl, &p.uniform, &v);
    }
}

/// `visible` of an object, honoring user properties; objects whose visibility
/// is driven by a script that spawns layer clones (`createLayer`, e.g. audio
/// visualizer bars) are templates WE never shows themselves. The script text is
/// only inspected, never run.
fn object_visible(obj: &Value) -> bool {
    let spawns_clones = obj
        .get("visible")
        .and_then(|v| v.get("script"))
        .and_then(|s| s.as_str())
        .is_some_and(|s| s.contains("createLayer"));
    !spawns_clones && value::get_bool(obj, "visible", true)
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

/// Camera parallax (`general.cameraparallax`): the camera leans toward the
/// cursor, so objects shift the opposite way, each scaled by its
/// `parallaxDepth`. WE reads the cursor through XWayland (harness/xpointer.py
/// records what it saw); shift at the screen edge =
/// `amount * mouseinfluence * depth * scene size / 2`.
struct Parallax {
    amount: f32,
    influence: f32,
    /// Seconds to catch up with the cursor (`cameraparallaxdelay`).
    delay: f32,
    /// Smoothed cursor offset from the center, -0.5..0.5, y down.
    offset: [f32; 2],
    last_time: f32,
}

impl Parallax {
    fn from_general(general: &Value) -> Option<Parallax> {
        value::get_bool(general, "cameraparallax", false).then(|| Parallax {
            amount: value::get_f32(general, "cameraparallaxamount", 0.5),
            influence: value::get_f32(general, "cameraparallaxmouseinfluence", 0.5),
            delay: value::get_f32(general, "cameraparallaxdelay", 0.1).max(0.0),
            offset: [0.0; 2],
            last_time: 0.0,
        })
    }

    fn follow(&mut self, pointer: [f32; 2], time: f32) {
        let dt = (time - self.last_time).clamp(0.0, 0.5);
        self.last_time = time;
        // Exponential follow with `delay` as the time constant. Not calibrated:
        // a per-frame "1 - delay" model fit 3366882745 (delay 1.0) but broke
        // Ice Princess 2800248288 (delay 1.1, 0.81 -> 0.66), see GL_RENDERER.md.
        let k = if self.delay <= 0.0 {
            1.0
        } else {
            1.0 - (-dt / self.delay).exp()
        };
        for (o, p) in self.offset.iter_mut().zip(pointer) {
            *o += ((p - 0.5) - *o) * k;
        }
    }

    /// Scene-space translation (Y up) for an object at `depth`.
    fn shift(&self, depth: [f32; 2], width: f32, height: f32) -> [f32; 2] {
        let k = self.amount * self.influence;
        [
            -self.offset[0] * k * depth[0] * width,
            self.offset[1] * k * depth[1] * height,
        ]
    }
}

/// Timeline animations of a layer's object properties (see `keyframes`).
struct LayerAnim {
    alpha: Option<crate::keyframes::Track>,
    color: Option<crate::keyframes::Track>,
    brightness: Option<crate::keyframes::Track>,
    origin: Option<crate::keyframes::Track>,
    angles: Option<crate::keyframes::Track>,
    scale: Option<crate::keyframes::Track>,
    /// Static origin / angle z / scale, for components without a curve.
    base: ([f32; 3], f32, [f32; 3]),
    /// parent world matrix (world = parent * local)
    parent: Mat4,
}

impl LayerAnim {
    fn parse(obj: &Value, world: Mat4) -> Option<LayerAnim> {
        use crate::keyframes::Track;
        let track = |k: &str| obj.get(k).and_then(Track::parse);
        let a = LayerAnim {
            alpha: track("alpha"),
            color: track("color"),
            brightness: track("brightness"),
            origin: track("origin"),
            angles: track("angles"),
            scale: track("scale"),
            base: (
                value::get_vec3(obj, "origin", [0.0; 3]),
                value::get_vec3(obj, "angles", [0.0; 3])[2],
                value::get_vec3(obj, "scale", [1.0; 3]),
            ),
            parent: math::mul(&world, &math::invert(&local_matrix(obj))),
        };
        let any = a.alpha.is_some()
            || a.color.is_some()
            || a.brightness.is_some()
            || a.origin.is_some()
            || a.angles.is_some()
            || a.scale.is_some();
        any.then_some(a)
    }

    fn apply(
        &self,
        t: f32,
        world: &mut Mat4,
        color: &mut [f32; 3],
        alpha: &mut f32,
        brightness: &mut f32,
    ) {
        let v3 = |tr: &Option<crate::keyframes::Track>, base: [f32; 3]| match tr {
            Some(tr) => {
                let v = tr.eval(t);
                [0, 1, 2].map(|i| v.get(i).copied().unwrap_or(base[i]))
            }
            None => base,
        };
        if let Some(tr) = &self.alpha {
            *alpha = tr.eval(t).first().copied().unwrap_or(*alpha);
        }
        if let Some(tr) = &self.brightness {
            *brightness = tr.eval(t).first().copied().unwrap_or(*brightness);
        }
        if self.color.is_some() {
            *color = v3(&self.color, *color);
        }
        if self.origin.is_some() || self.angles.is_some() || self.scale.is_some() {
            let (o, a, s) = self.base;
            let origin = v3(&self.origin, o);
            let angle = v3(&self.angles, [0.0, 0.0, a])[2];
            let scale = v3(&self.scale, s);
            *world = math::mul(&self.parent, &math::model(origin, angle, scale));
        }
    }
}

/// Animated image layer (GIF scenes): frames spread over several images.
struct Flipbook {
    images: Vec<Tex>,
    frames: Vec<rustpaper_core::tex::SheetFrame>,
}

impl Flipbook {
    /// Texture, `g_Texture0Rotation` (u and v axes of the frame in uv) and
    /// `g_Texture0Translation` (its corner) at `time`, looping.
    fn at(&self, time: f32) -> Option<(Tex, [f32; 4], [f32; 2])> {
        let total: f32 = self.frames.iter().map(|f| f.duration.max(0.0)).sum();
        let mut t = if total > 0.0 {
            time.rem_euclid(total)
        } else {
            0.0
        };
        let frame = self
            .frames
            .iter()
            .find(|f| {
                if t < f.duration {
                    true
                } else {
                    t -= f.duration;
                    false
                }
            })
            .or(self.frames.last())?;
        let tex = *self
            .images
            .get(frame.image as usize)
            .or(self.images.first())?;
        let (w, h) = (tex.width.max(1) as f32, tex.height.max(1) as f32);
        Some((
            tex,
            [frame.w / w, 0.0, 0.0, frame.h / h],
            [frame.x / w, frame.y / h],
        ))
    }
}

/// Attached layers ordered so that a parent is placed before its children.
fn attach_order(layers: &[Layer]) -> Vec<usize> {
    let depth = |mut i: usize| {
        let mut d = 0;
        while let Some((parent, _, _)) = &layers[i].attach {
            match layers.iter().position(|l| l.id == *parent) {
                Some(p) if d < 64 => {
                    i = p;
                    d += 1;
                }
                _ => break,
            }
        }
        d
    };
    let mut v: Vec<usize> = (0..layers.len())
        .filter(|&i| layers[i].attach.is_some())
        .collect();
    v.sort_by_key(|&i| depth(i));
    v
}

/// Ids of objects hidden because an ancestor is hidden.
fn hidden_by_ancestor(objects: &[Value]) -> HashSet<i64> {
    let by_id: HashMap<i64, &Value> = objects
        .iter()
        .filter_map(|o| Some((o.get("id")?.as_i64()?, o)))
        .collect();
    let mut out = HashSet::new();
    for (&id, o) in &by_id {
        if value::get_bool(o, "disablepropagation", false) {
            continue;
        }
        let mut cur = o.get("parent").and_then(|p| p.as_i64());
        let mut depth = 0;
        while let (Some(pid), true) = (cur, depth < 64) {
            let Some(p) = by_id.get(&pid) else { break };
            if !object_visible(p) {
                out.insert(id);
                break;
            }
            cur = p.get("parent").and_then(|p| p.as_i64());
            depth += 1;
        }
    }
    out
}

/// One simulation step of `dt` for an object's systems; children run after
/// their parent and spawn from its live particles / events.
fn step_systems(systems: &mut [SystemRt], dt: f32, pointer: [f32; 3]) {
    for k in 0..systems.len() {
        let (before, rest) = systems.split_at_mut(k);
        let sys = &mut rest[0];
        let cps = particles::control_points(&sys.sim.def, pointer);
        match sys.parent {
            None => sys.sim.step(dt, &cps, true),
            Some((_, ChildEvent::Static, _)) => sys.sim.step(dt, &cps, true),
            Some((pi, event, origin)) => {
                let parent = &mut before[pi].sim;
                match event {
                    ChildEvent::Static => {}
                    ChildEvent::Follow => {
                        let rate = sys.sim.rate();
                        let mut spawns = Vec::new();
                        for (pos, acc) in parent.positions_with_acc() {
                            *acc += rate * dt;
                            while *acc >= 1.0 {
                                *acc -= 1.0;
                                spawns.push(pos);
                            }
                        }
                        for pos in spawns {
                            sys.sim.burst_at(
                                [pos[0] + origin[0], pos[1] + origin[1], pos[2] + origin[2]],
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
                                [pos[0] + origin[0], pos[1] + origin[1], pos[2] + origin[2]],
                                n,
                                &cps,
                            );
                        }
                    }
                }
                sys.sim.step(dt, &cps, false);
            }
        }
    }
}

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
