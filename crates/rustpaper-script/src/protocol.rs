// Messages between the renderer (`ScriptHost`) and the sandboxed worker.
//
// One JSON document per line on the worker's stdin (host -> worker) and
// stdout (worker -> host). Only plain data crosses the pipe: the worker never
// gets a handle to anything of the renderer's, and the host only accepts
// values it knows how to apply (layer properties, script values by id).

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// How a script's property value is typed (what `update(value)` receives and
/// must return). Vectors travel as arrays.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ValueKind {
    Float,
    Bool,
    String,
    Vec2,
    Vec3,
    Vec4,
}

/// Where a property script's result goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Binding {
    /// A property of the object itself (`origin`, `alpha`, `text`...): the
    /// result is written into the layer state that `thisLayer` also exposes.
    Layer,
    /// Anything else (effect constants, particle overrides...): the result is
    /// reported back under the script's id.
    Value,
}

/// One script attached to a property (`{"script": "...", "value": x}`).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScriptDef {
    pub id: u32,
    /// Object (layer) the script belongs to: its `thisLayer`.
    pub layer: i64,
    /// Property name (`origin`, `text`, a shader constant's name...).
    pub property: String,
    pub binding: Binding,
    pub kind: ValueKind,
    /// The serialized value (the static value WE starts from).
    pub value: Value,
    /// Stored `scriptproperties` values (user property bindings resolved).
    pub script_properties: Map<String, Value>,
    pub source: String,
}

/// The scriptable state of one scene object.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct LayerState {
    pub id: i64,
    pub name: String,
    /// `image`, `particle`, `text`, `sound`, `group`...
    pub kind: String,
    pub parent: Option<i64>,
    pub origin: [f32; 3],
    pub scale: [f32; 3],
    pub angles: [f32; 3],
    pub size: [f32; 2],
    pub color: [f32; 3],
    pub alpha: f32,
    pub brightness: f32,
    pub visible: bool,
    pub text: Option<String>,
}

/// Host -> worker, once, before any frame.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Init {
    pub scripts: Vec<ScriptDef>,
    /// Every scene object, in scene order.
    pub layers: Vec<LayerState>,
    /// project.json user properties (name -> current value).
    pub user_properties: Map<String, Value>,
    /// Scene (orthographic projection) size.
    pub canvas: [f32; 2],
    /// Output resolution.
    pub screen: [f32; 2],
    /// Camera zoom (`general.zoom`): what `getCameraTransforms().zoom` starts at.
    #[serde(default = "one")]
    pub zoom: f32,
}

fn one() -> f32 {
    1.0
}

/// Host -> worker, every frame.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Frame {
    /// Seconds since the wallpaper started (`engine.runtime`).
    pub time: f64,
    /// Seconds since the previous frame (`engine.frametime`).
    pub dt: f64,
    /// Fraction of the day in [0,1) (`engine.timeOfDay`).
    pub daytime: f64,
    /// Wall clock, ms since the Unix epoch (`Date.now()`, `new Date()`).
    pub clock_ms: f64,
    /// Cursor in scene coordinates (`input.cursorWorldPosition`).
    pub cursor: [f32; 2],
    /// Objects under the cursor (cursorEnter / cursorLeave).
    #[serde(default)]
    pub hover: Vec<i64>,
    /// 64 bands per channel, empty when there is no audio.
    pub audio_left: Vec<f32>,
    pub audio_right: Vec<f32>,
}

/// Changed fields of one object (only what scripts wrote).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LayerUpdate {
    pub id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<[f32; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale: Option<[f32; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub angles: Option<[f32; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<[f32; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alpha: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brightness: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visible: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Sound objects: play() (true) / stop() (false).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub playing: Option<bool>,
}

/// A layer a script created (`thisScene.createLayer`).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Created {
    /// New object id (above every scene id).
    pub id: i64,
    /// The script that created it (its `__workshopId` resolves asset paths).
    pub script: u32,
    /// The script's own layer (the template clones usually copy).
    pub template: i64,
    /// `createLayer`'s argument: an asset path, or an object configuration.
    pub config: Value,
    /// Draw it just before this object (`thisScene.sortLayer`); None: on top.
    pub before: Option<i64>,
}

/// Worker -> host: the answer to `Init` (`Ready`) and to every `Frame`.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Reply {
    /// Layers created since the last reply (before `layers`, which may
    /// already update them).
    #[serde(default)]
    pub created: Vec<Created>,
    #[serde(default)]
    pub layers: Vec<LayerUpdate>,
    /// Changed `Binding::Value` results: (script id, value).
    #[serde(default)]
    pub values: Vec<(u32, Value)>,
    /// `thisScene.setCameraTransforms({zoom})`: the new camera zoom.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zoom: Option<f32>,
    /// `console.*` output and script errors, already capped by the worker.
    #[serde(default)]
    pub logs: Vec<String>,
}
