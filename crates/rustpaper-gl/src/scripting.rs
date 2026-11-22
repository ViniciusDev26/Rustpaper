// SceneScript: the renderer's side.
//
// Scripts never run in this process. `Scripts` collects the scripts of a
// scene's objects, hands them to `rustpaper_script::ScriptHost` (a sandboxed
// worker process, see that crate) and turns what the worker answers into
// renderer state: world matrices (the parent hierarchy re-resolved),
// effective visibility (a hidden parent hides its children), color, alpha,
// brightness and text.
//
// Covered: scripts on an object's own properties (origin, scale, angles,
// color, alpha, brightness, visible, text) and everything they write through
// `thisLayer` / `thisScene.getLayer()`. Scripts on effect constants and
// particle overrides run too, but their results are not applied yet.

use std::collections::{HashMap, HashSet};

use rustpaper_script::ScriptHost;
use rustpaper_script::protocol::{
    Binding, Created, Frame, Init, LayerState, LayerUpdate, Reply, ScriptDef, ValueKind,
};
use serde_json::{Map, Value};

use crate::math::{self, Mat4};
use crate::scene::FrameInput;
use crate::value;

/// Object properties whose script result the worker writes into the layer.
const LAYER_FIELDS: [&str; 8] = [
    "origin",
    "scale",
    "angles",
    "color",
    "alpha",
    "brightness",
    "visible",
    "text",
];

/// Frame step when fast-forwarding scripts from t = 0 to the first frame.
const CATCH_UP_DT: f64 = 1.0 / 30.0;
/// Longest fast-forward (seconds).
const CATCH_UP_MAX: f64 = 120.0;
/// Script log lines printed per scene (they can log every frame).
const MAX_LOG_LINES: usize = 200;

struct Obj {
    parent: Option<i64>,
    origin: [f32; 3],
    angles: [f32; 3],
    scale: [f32; 3],
    visible: bool,
    /// `disablepropagation`: a hidden parent doesn't hide this object.
    independent: bool,
}

/// What changed this frame, ready for the renderer.
#[derive(Default)]
pub struct Changes {
    pub worlds: HashMap<i64, Mat4>,
    /// Effective visibility (own flag and every ancestor's).
    pub visible: HashMap<i64, bool>,
    pub color: HashMap<i64, [f32; 3]>,
    pub alpha: HashMap<i64, f32>,
    pub brightness: HashMap<i64, f32>,
    pub text: HashMap<i64, String>,
    /// Texture animation clocks of the layers whose flipbook a script
    /// controls (`getTextureAnimation()`), seconds into the sheet.
    pub anim_time: HashMap<i64, f32>,
    /// Effect constants (`effects[n].passes[p].constantshadervalues.key`).
    pub constants: Vec<ConstantChange>,
    /// (object id, effect index, shown).
    pub effect_visible: Vec<(i64, usize, bool)>,
    /// Puppet animation layer rates: (object id, layer index, rate).
    pub anim_rates: Vec<(i64, usize, f32)>,
    /// Puppet animation layers shown/hidden: (object id, layer index, shown).
    pub anim_visible: Vec<(i64, usize, bool)>,
    /// Sound objects started/stopped: (object id, playing).
    pub sound_playing: Vec<(i64, bool)>,
    /// `general.<key>` (scene settings: bloomstrength...).
    pub general: Vec<(String, Value)>,
    /// Sound object volumes: (object id, 0..1).
    pub sound_volumes: Vec<(i64, f32)>,
    /// Particle `instanceoverride` fields: (object id, key, value).
    pub overrides: Vec<(i64, String, Value)>,
    /// Layers scripts created, with the creating script's `__workshopId`.
    pub created: Vec<(Created, Option<String>)>,
    /// Scene zoom (a `general.zoom` script).
    pub zoom: Option<f32>,
    /// Camera transform zoom (`thisScene.setCameraTransforms`): multiplies
    /// the scene zoom.
    pub camera_zoom: Option<f32>,
}

pub struct ConstantChange {
    pub layer: i64,
    pub effect: usize,
    pub pass: usize,
    pub key: String,
    /// As a material constant: a number or a "x y z" string.
    pub value: Value,
}

/// Where a `Binding::Value` script's result goes.
#[derive(Clone, Debug, PartialEq)]
enum Target {
    Constant {
        layer: i64,
        effect: usize,
        pass: usize,
        key: String,
    },
    EffectVisible {
        layer: i64,
        effect: usize,
    },
    /// A puppet's `animationlayers[index].rate`.
    AnimRate {
        layer: i64,
        index: usize,
    },
    /// A puppet's `animationlayers[index].visible`.
    AnimVisible {
        layer: i64,
        index: usize,
    },
    /// A scene setting, `general.<key>`.
    General {
        key: String,
    },
    /// A sound object's `volume`.
    SoundVolume {
        layer: i64,
    },
    /// A particle object's `instanceoverride.<key>`.
    Override {
        layer: i64,
        key: String,
    },
    /// Runs, but nothing applies its result yet.
    Unapplied,
}

pub struct Scripts {
    host: ScriptHost,
    /// By script id: where a `Binding::Value` result goes.
    targets: Vec<Target>,
    /// By script id: `export let __workshopId = '...'` (WE resolves the
    /// script's asset paths under `<dir>/workshop/<id>/`).
    workshop_ids: Vec<Option<String>>,
    objs: HashMap<i64, Obj>,
    worlds: HashMap<i64, Mat4>,
    effective: HashMap<i64, bool>,
    /// Wall clock (ms since the epoch) at scene time 0.
    clock0_ms: f64,
    last_time: Option<f64>,
    logged: usize,
    count: usize,
    canvas: [f32; 2],
    screen: [f32; 2],
}

impl Scripts {
    /// Starts the sandboxed worker for the scene's scripts. `Ok(None)` when
    /// the scene has none.
    pub fn load(
        objects: &[Value],
        general: &Value,
        user_properties: &Map<String, Value>,
        canvas: [f32; 2],
        screen: [f32; 2],
        flipbooks: &HashMap<i64, Vec<f32>>,
    ) -> Result<Option<Scripts>, String> {
        let mut scripts = Vec::new();
        let mut targets = Vec::new();
        for obj in objects {
            let Some(id) = obj.get("id").and_then(|i| i.as_i64()) else {
                continue;
            };
            collect(obj, id, &mut scripts, &mut targets);
        }
        // scene settings (`general.bloomstrength`...): their thisLayer is none
        for (key, v) in general.as_object().into_iter().flatten() {
            if is_script(v) {
                let t = Target::General { key: key.clone() };
                push(v, key, -1, Binding::Value, t, &mut scripts, &mut targets);
            }
        }
        if scripts.is_empty() {
            return Ok(None);
        }
        let mut layers: Vec<LayerState> = objects.iter().filter_map(layer_state).collect();
        // getTextureAnimation() needs the sheet's frames (only the renderer
        // has parsed the .tex)
        for l in &mut layers {
            if let Some(f) = flipbooks.get(&l.id) {
                l.frames = f.clone();
            }
        }
        let objs: HashMap<i64, Obj> = layers
            .iter()
            .map(|l| {
                let obj = objects
                    .iter()
                    .find(|o| o.get("id").and_then(|i| i.as_i64()) == Some(l.id));
                (
                    l.id,
                    Obj {
                        parent: l.parent,
                        origin: l.origin,
                        angles: l.angles,
                        scale: l.scale,
                        visible: l.visible,
                        independent: obj
                            .is_some_and(|o| value::get_bool(o, "disablepropagation", false)),
                    },
                )
            })
            .collect();
        let props: Map<String, Value> = user_properties
            .iter()
            .map(|(k, v)| (k.clone(), v.get("value").cloned().unwrap_or(Value::Null)))
            .collect();
        let count = scripts.len();
        let workshop_ids = scripts.iter().map(|s| workshop_id(&s.source)).collect();
        let init = Init {
            scripts,
            layers,
            user_properties: props,
            canvas,
            screen,
            // the camera transform starts unzoomed: `general.zoom` is a
            // separate factor (see SceneRenderer::camera_zoom)
            zoom: 1.0,
        };
        let offset_min = chrono::Local::now().offset().local_minus_utc() / 60;
        let (host, reply) = ScriptHost::spawn(&init, offset_min)?;
        // the same clock as clock texts (`RUSTPAPER_CLOCK` pins it in the
        // harness to the capture's start)
        let now_ms = crate::text::clock_now().timestamp_millis() as f64;
        let mut s = Scripts {
            host,
            targets,
            workshop_ids,
            worlds: HashMap::new(),
            effective: HashMap::new(),
            objs,
            clock0_ms: now_ms,
            last_time: None,
            logged: 0,
            count,
            canvas,
            screen,
        };
        s.worlds = s.resolve_worlds();
        s.effective = s.resolve_visible();
        let mut ch = Changes::default();
        s.absorb(reply, &mut ch);
        Ok(Some(s))
    }

    /// Number of scripts running.
    pub fn count(&self) -> usize {
        self.count
    }

    /// Runs the scripts for this frame. The first call fast-forwards them
    /// from t = 0 (WE has been running them since the wallpaper started).
    pub fn frame(&mut self, f: &FrameInput, hover: &[i64]) -> Result<Changes, String> {
        let t = f.time as f64;
        let mut ch = Changes::default();
        if self.last_time.is_none() {
            // the clock reads "now" at the frame being rendered
            self.clock0_ms -= t * 1000.0;
            let mut k = 0.0;
            while k + CATCH_UP_DT < t.min(CATCH_UP_MAX) {
                let r = self.host.frame(&self.frame_msg(f, k, CATCH_UP_DT, hover))?;
                self.absorb(r, &mut ch);
                k += CATCH_UP_DT;
            }
            self.last_time = Some(k);
        }
        let dt = (t - self.last_time.unwrap_or(t)).max(0.0);
        let r = self.host.frame(&self.frame_msg(f, t, dt, hover))?;
        self.last_time = Some(t);
        self.absorb(r, &mut ch);
        Ok(ch)
    }

    fn frame_msg(&self, f: &FrameInput, t: f64, dt: f64, hover: &[i64]) -> Frame {
        Frame {
            time: t,
            dt,
            daytime: f.daytime as f64,
            clock_ms: self.clock0_ms + t * 1000.0,
            cursor: self.cursor_in_scene(f.pointer),
            hover: hover.to_vec(),
            audio_left: f.audio.left.to_vec(),
            audio_right: f.audio.right.to_vec(),
        }
    }

    /// The pointer ([0,1]^2 of the screen, y down) in scene coordinates (y
    /// up), through the cover crop that fits the scene to the screen.
    pub fn cursor_in_scene(&self, p: [f32; 2]) -> [f32; 2] {
        cursor_in_scene(p, self.canvas, self.screen)
    }

    /// Folds one worker reply into `ch` (later frames override earlier ones).
    fn absorb(&mut self, r: Reply, ch: &mut Changes) {
        for line in r.logs {
            if self.logged < MAX_LOG_LINES {
                eprintln!("[script] {line}");
            } else if self.logged == MAX_LOG_LINES {
                eprintln!("[script] (more script output suppressed)");
            }
            self.logged += 1;
        }
        let mut moved = false;
        let mut shown = false;
        if r.zoom.is_some() {
            ch.camera_zoom = r.zoom;
        }
        for c in r.created {
            self.objs.insert(
                c.id,
                Obj {
                    parent: None,
                    origin: [0.0; 3],
                    angles: [0.0; 3],
                    scale: [1.0; 3],
                    visible: true,
                    independent: false,
                },
            );
            let ws = self.workshop_ids.get(c.script as usize).cloned().flatten();
            ch.created.push((c, ws));
            moved = true;
            shown = true;
        }
        for u in r.layers {
            let LayerUpdate {
                id,
                origin,
                scale,
                angles,
                color,
                alpha,
                brightness,
                visible,
                text,
                playing,
                anim_time,
            } = u;
            if let Some(t) = anim_time {
                ch.anim_time.insert(id, t);
            }
            if let Some(p) = playing {
                ch.sound_playing.retain(|s| s.0 != id);
                ch.sound_playing.push((id, p));
            }
            if let Some(o) = self.objs.get_mut(&id) {
                if let Some(v) = origin {
                    o.origin = v;
                    moved = true;
                }
                if let Some(v) = scale {
                    o.scale = v;
                    moved = true;
                }
                if let Some(v) = angles {
                    o.angles = v;
                    moved = true;
                }
                if let Some(v) = visible {
                    o.visible = v;
                    shown = true;
                }
            }
            if let Some(v) = color {
                ch.color.insert(id, v);
            }
            if let Some(v) = alpha {
                ch.alpha.insert(id, v);
            }
            if let Some(v) = brightness {
                ch.brightness.insert(id, v);
            }
            if let Some(v) = text {
                ch.text.insert(id, v);
            }
        }
        for (id, v) in r.values {
            match self.targets.get(id as usize) {
                Some(Target::Constant {
                    layer,
                    effect,
                    pass,
                    key,
                }) => {
                    // vectors become "x y z" strings, like in materials
                    let value = match &v {
                        Value::Array(a) => Value::from(
                            a.iter()
                                .map(|x| x.as_f64().unwrap_or(0.0).to_string())
                                .collect::<Vec<_>>()
                                .join(" "),
                        ),
                        Value::Bool(b) => Value::from(*b as i32),
                        _ => v.clone(),
                    };
                    ch.constants.retain(|c| {
                        !(c.layer == *layer
                            && c.effect == *effect
                            && c.pass == *pass
                            && &c.key == key)
                    });
                    ch.constants.push(ConstantChange {
                        layer: *layer,
                        effect: *effect,
                        pass: *pass,
                        key: key.clone(),
                        value,
                    });
                }
                Some(Target::AnimVisible { layer, index }) => {
                    let shown = v.as_bool().unwrap_or(true);
                    ch.anim_visible
                        .retain(|a| !(a.0 == *layer && a.1 == *index));
                    ch.anim_visible.push((*layer, *index, shown));
                }
                Some(Target::General { key }) if key == "zoom" => {
                    ch.zoom = v.as_f64().map(|z| z as f32);
                }
                Some(Target::General { key }) => {
                    ch.general.retain(|g| &g.0 != key);
                    ch.general.push((key.clone(), v.clone()));
                }
                Some(Target::SoundVolume { layer }) => {
                    let vol = v.as_f64().unwrap_or(1.0) as f32;
                    ch.sound_volumes.retain(|a| a.0 != *layer);
                    ch.sound_volumes.push((*layer, vol));
                }
                Some(Target::AnimRate { layer, index }) => {
                    let rate = v.as_f64().unwrap_or(1.0) as f32;
                    ch.anim_rates.retain(|a| !(a.0 == *layer && a.1 == *index));
                    ch.anim_rates.push((*layer, *index, rate));
                }
                Some(Target::Override { layer, key }) => {
                    ch.overrides.retain(|o| !(o.0 == *layer && &o.1 == key));
                    ch.overrides.push((*layer, key.clone(), v.clone()));
                }
                Some(Target::EffectVisible { layer, effect }) => {
                    let shown = v.as_bool().unwrap_or(true);
                    ch.effect_visible
                        .retain(|e| !(e.0 == *layer && e.1 == *effect));
                    ch.effect_visible.push((*layer, *effect, shown));
                }
                _ => {}
            }
        }
        if moved {
            let worlds = self.resolve_worlds();
            for (id, w) in &worlds {
                if self.worlds.get(id) != Some(w) {
                    ch.worlds.insert(*id, *w);
                }
            }
            self.worlds = worlds;
        }
        if shown {
            let eff = self.resolve_visible();
            for (id, v) in &eff {
                if self.effective.get(id) != Some(v) {
                    ch.visible.insert(*id, *v);
                }
            }
            self.effective = eff;
        }
    }

    fn resolve_worlds(&self) -> HashMap<i64, Mat4> {
        fn go(id: i64, objs: &HashMap<i64, Obj>, out: &mut HashMap<i64, Mat4>, depth: u32) -> Mat4 {
            if let Some(m) = out.get(&id) {
                return *m;
            }
            let Some(o) = objs.get(&id) else {
                return math::IDENTITY;
            };
            let local = math::model(o.origin, o.angles[2], o.scale);
            let w = match o.parent {
                Some(p) if depth < 32 && p != id => math::mul(&go(p, objs, out, depth + 1), &local),
                _ => local,
            };
            out.insert(id, w);
            w
        }
        let mut out = HashMap::new();
        for &id in self.objs.keys() {
            go(id, &self.objs, &mut out, 0);
        }
        out
    }

    fn resolve_visible(&self) -> HashMap<i64, bool> {
        self.objs
            .iter()
            .map(|(&id, o)| {
                let mut v = o.visible;
                let mut seen = HashSet::new();
                let mut cur = if o.independent { None } else { o.parent };
                while let Some(p) = cur {
                    if !v || !seen.insert(p) {
                        break;
                    }
                    match self.objs.get(&p) {
                        Some(po) => {
                            v &= po.visible;
                            cur = po.parent;
                        }
                        None => break,
                    }
                }
                (id, v)
            })
            .collect()
    }
}

/// The scripts on one object's properties (`{"script": ..., "value": ...}`):
/// its own, its effects' (visibility, pass constants) and any other nested
/// ones (particle overrides...), each with where its result goes.
fn collect(obj: &Value, layer: i64, out: &mut Vec<ScriptDef>, targets: &mut Vec<Target>) {
    let Some(m) = obj.as_object() else { return };
    for (k, v) in m {
        if k == "effects" {
            continue;
        }
        if k == "animationlayers"
            && let Some(a) = v.as_array()
        {
            for (index, l) in a.iter().enumerate() {
                for (key, v) in l.as_object().into_iter().flatten() {
                    if is_script(v) {
                        let t = match key.as_str() {
                            "rate" => Target::AnimRate { layer, index },
                            "visible" => Target::AnimVisible { layer, index },
                            _ => Target::Unapplied,
                        };
                        push(v, key, layer, Binding::Value, t, out, targets);
                    }
                }
            }
            continue;
        }
        if k == "instanceoverride"
            && let Some(o) = v.as_object()
        {
            for (key, v) in o {
                if is_script(v) {
                    let t = Target::Override {
                        layer,
                        key: key.clone(),
                    };
                    push(v, key, layer, Binding::Value, t, out, targets);
                }
            }
            continue;
        }
        if is_script(v) {
            let binding = if LAYER_FIELDS.contains(&k.as_str()) {
                Binding::Layer
            } else {
                Binding::Value
            };
            let t = if k == "volume" && obj.get("sound").is_some_and(|s| s.is_array()) {
                Target::SoundVolume { layer }
            } else {
                Target::Unapplied
            };
            push(v, k, layer, binding, t, out, targets);
        } else {
            walk_unapplied(v, k, layer, out, targets);
        }
    }
    let effects = m.get("effects").and_then(|e| e.as_array());
    for (e, fx) in effects.into_iter().flatten().enumerate() {
        let Some(fm) = fx.as_object() else { continue };
        for (k, v) in fm {
            if k == "visible" && is_script(v) {
                let t = Target::EffectVisible { layer, effect: e };
                push(v, k, layer, Binding::Value, t, out, targets);
            } else if k != "passes" {
                walk_unapplied(v, k, layer, out, targets);
            }
        }
        let passes = fm.get("passes").and_then(|p| p.as_array());
        for (p, pass) in passes.into_iter().flatten().enumerate() {
            let consts = pass.get("constantshadervalues").and_then(|c| c.as_object());
            for (key, v) in consts.into_iter().flatten() {
                if is_script(v) {
                    let t = Target::Constant {
                        layer,
                        effect: e,
                        pass: p,
                        key: key.clone(),
                    };
                    push(v, key, layer, Binding::Value, t, out, targets);
                }
            }
        }
    }
}

fn is_script(v: &Value) -> bool {
    v.get("script").is_some_and(|s| s.is_string())
}

/// Nested scripts nothing applies yet: they still run (they may write other
/// layers through `thisScene`).
fn walk_unapplied(
    v: &Value,
    key: &str,
    layer: i64,
    out: &mut Vec<ScriptDef>,
    targets: &mut Vec<Target>,
) {
    match v {
        _ if is_script(v) => push(
            v,
            key,
            layer,
            Binding::Value,
            Target::Unapplied,
            out,
            targets,
        ),
        Value::Object(m) => m
            .iter()
            .for_each(|(k, c)| walk_unapplied(c, k, layer, out, targets)),
        Value::Array(a) => a
            .iter()
            .for_each(|c| walk_unapplied(c, key, layer, out, targets)),
        _ => {}
    }
}

fn push(
    v: &Value,
    key: &str,
    layer: i64,
    binding: Binding,
    target: Target,
    out: &mut Vec<ScriptDef>,
    targets: &mut Vec<Target>,
) {
    let raw = value::plain(v).clone();
    let kind = kind_of(key, &raw);
    let value = match kind {
        ValueKind::Vec2 | ValueKind::Vec3 | ValueKind::Vec4 => Value::from(value::floats(&raw)),
        _ => raw,
    };
    let script_properties = v
        .get("scriptproperties")
        .and_then(|p| p.as_object())
        .map(|p| {
            p.iter()
                .map(|(k, v)| (k.clone(), value::plain(v).clone()))
                .collect()
        })
        .unwrap_or_default();
    out.push(ScriptDef {
        id: out.len() as u32,
        layer,
        property: key.to_string(),
        binding,
        kind,
        value,
        script_properties,
        source: v["script"].as_str().unwrap_or_default().to_string(),
    });
    targets.push(target);
}

/// See `Scripts::cursor_in_scene`.
pub fn cursor_in_scene(p: [f32; 2], canvas: [f32; 2], screen: [f32; 2]) -> [f32; 2] {
    let k = (screen[0] / canvas[0]).max(screen[1] / canvas[1]).max(1e-6);
    let (cw, ch) = (screen[0] / k, screen[1] / k);
    let (x0, y0) = ((canvas[0] - cw) / 2.0, (canvas[1] - ch) / 2.0);
    [x0 + p[0] * cw, canvas[1] - (y0 + p[1] * ch)]
}

/// `export let __workshopId = '2878462916';` -> "2878462916".
fn workshop_id(source: &str) -> Option<String> {
    let at = source.find("__workshopId")?;
    let rest = &source[at..];
    let q = rest.find(['\'', '"'])?;
    let quote = rest[q..].chars().next()?;
    let body = &rest[q + 1..];
    let end = body.find(quote)?;
    let id = &body[..end];
    (!id.is_empty() && id.chars().all(|c| c.is_ascii_digit())).then(|| id.to_string())
}

/// Candidate paths of a script's asset: as written, then under its
/// workshop folder (`models/bar.json` -> `models/workshop/<id>/bar.json`).
pub fn asset_candidates(path: &str, workshop: Option<&str>) -> Vec<String> {
    let mut out = vec![path.to_string()];
    if let (Some(ws), Some((dir, file))) = (workshop, path.split_once('/')) {
        out.push(format!("{dir}/workshop/{ws}/{file}"));
    }
    out
}

/// The type `update(value)` works with, from the property name and value.
fn kind_of(key: &str, v: &Value) -> ValueKind {
    match key {
        "visible" => return ValueKind::Bool,
        "text" => return ValueKind::String,
        "origin" | "scale" | "angles" | "color" => return ValueKind::Vec3,
        "alpha" | "brightness" => return ValueKind::Float,
        _ => {}
    }
    match v {
        Value::Bool(_) => ValueKind::Bool,
        Value::Number(_) => ValueKind::Float,
        Value::String(s) => {
            let n = s.split_whitespace().count();
            let all_num = s.split_whitespace().all(|t| t.parse::<f32>().is_ok());
            match (all_num, n) {
                (true, 1) => ValueKind::Float,
                (true, 2) => ValueKind::Vec2,
                (true, 3) => ValueKind::Vec3,
                (true, 4) => ValueKind::Vec4,
                _ => ValueKind::String,
            }
        }
        _ => ValueKind::String,
    }
}

fn layer_state(obj: &Value) -> Option<LayerState> {
    let id = obj.get("id")?.as_i64()?;
    let kind = if obj.get("image").and_then(|i| i.as_str()).is_some() {
        "image"
    } else if obj.get("particle").and_then(|i| i.as_str()).is_some() {
        "particle"
    } else if obj.get("text").is_some_and(|t| !t.is_null()) {
        "text"
    } else if obj.get("sound").is_some_and(|t| !t.is_null()) {
        "sound"
    } else {
        "group"
    };
    let text = obj
        .get("text")
        .map(value::plain)
        .and_then(|t| t.as_str())
        .map(String::from);
    Some(LayerState {
        id,
        name: obj
            .get("name")
            .and_then(|n| n.as_str())
            .unwrap_or("")
            .to_string(),
        kind: kind.into(),
        parent: obj.get("parent").and_then(|p| p.as_i64()),
        origin: value::get_vec3(obj, "origin", [0.0; 3]),
        scale: value::get_vec3(obj, "scale", [1.0; 3]),
        angles: value::get_vec3(obj, "angles", [0.0; 3]),
        size: value::get_vec2(obj, "size", [0.0; 2]),
        color: value::get_vec3(obj, "color", [1.0; 3]),
        alpha: value::get_f32(obj, "alpha", 1.0),
        brightness: value::get_f32(obj, "brightness", 1.0),
        visible: value::get_bool(obj, "visible", true),
        text,
        frames: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn collects_object_and_nested_scripts() {
        let obj = json!({
            "id": 7,
            "origin": { "script": "export function update(v) { return v; }", "value": "1 2 3" },
            "visible": { "script": "s", "value": true, "scriptproperties": { "a": { "user": "u", "value": 2 } } },
            "effects": [{ "passes": [{ "constantshadervalues": {
                "multiply": { "script": "s2", "value": 0.5 } } }] }]
        });
        let mut out = Vec::new();
        let mut targets = Vec::new();
        collect(&obj, 7, &mut out, &mut targets);
        assert_eq!(out.len(), 3);
        assert_eq!(targets.len(), 3);
        let origin = out.iter().find(|s| s.property == "origin").unwrap();
        assert_eq!(origin.binding, Binding::Layer);
        assert_eq!(origin.kind, ValueKind::Vec3);
        assert_eq!(origin.value, json!([1.0, 2.0, 3.0]));
        let vis = out.iter().find(|s| s.property == "visible").unwrap();
        assert_eq!(vis.script_properties["a"], json!(2));
        let mul = out.iter().find(|s| s.property == "multiply").unwrap();
        assert_eq!(mul.binding, Binding::Value);
        assert_eq!(mul.kind, ValueKind::Float);
        assert_eq!(
            targets[mul.id as usize],
            Target::Constant {
                layer: 7,
                effect: 0,
                pass: 0,
                key: "multiply".into()
            }
        );
    }

    #[test]
    fn workshop_asset_paths() {
        let src =
            "'use strict';\nexport let __workshopId = '2652493753';\nexport function init() {}";
        let ws = workshop_id(src);
        assert_eq!(ws.as_deref(), Some("2652493753"));
        assert_eq!(
            asset_candidates("models/bar.json", ws.as_deref()),
            ["models/bar.json", "models/workshop/2652493753/bar.json"]
        );
        assert_eq!(workshop_id("let x = 1;"), None);
    }

    #[test]
    fn cursor_maps_through_the_cover_crop() {
        // 16:9 screen on a 16:9 scene: plain scaling, y flipped
        assert_eq!(
            cursor_in_scene([0.0, 0.0], [3840.0, 2160.0], [1920.0, 1080.0]),
            [0.0, 2160.0]
        );
        assert_eq!(
            cursor_in_scene([0.5, 0.5], [3840.0, 2160.0], [1920.0, 1080.0]),
            [1920.0, 1080.0]
        );
        // 4:3 scene on 16:9: top and bottom cropped
        let c = cursor_in_scene([0.5, 0.0], [1600.0, 1200.0], [1920.0, 1080.0]);
        assert_eq!(c, [800.0, 1200.0 - 150.0]);
    }

    #[test]
    fn value_kinds() {
        assert_eq!(kind_of("x", &json!("1 2")), ValueKind::Vec2);
        assert_eq!(kind_of("x", &json!("0.1 0.2 0.3 1")), ValueKind::Vec4);
        assert_eq!(kind_of("x", &json!("hello")), ValueKind::String);
        assert_eq!(kind_of("visible", &json!(1)), ValueKind::Bool);
    }
}
