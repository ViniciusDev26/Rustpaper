// Particle systems: definition (particles/*.json), CPU simulation and the
// vertex stream the WE `genericparticle` shader expects.
//
// Like effects, rendering goes through the REAL WE shader: we only produce, per
// particle, 4 vertices with the attributes its vertex stage reads
//   a_Position        particle center (object space)
//   a_TexCoordVec4    (corner u, corner v, rotation z, size)
//   a_TexCoordC2      (rotation x, rotation y)
//   a_Color           (r, g, b, alpha), 0..1
//   a_TexCoordVec4C1  (velocity xyz, "lifetime" = sprite-sheet phase)  [THICKFORMAT]
// and the shader expands the quad (or the trail along the velocity).
//
// The simulation is deterministic: seeded RNG + fixed time steps, so the same
// time always produces the same frame (the harness relies on this).
//
// Semantics notes. Field names/meanings come from the scene files; where WE's
// exact formula isn't observable from the data (turbulence noise, a few
// defaults) the choice is documented next to it and validated with the harness.

use serde_json::Value;

use crate::value;

// --------------------------------------------------------------- small math

type V3 = [f32; 3];

fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn scale(a: V3, s: f32) -> V3 {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn mul3(a: V3, b: V3) -> V3 {
    [a[0] * b[0], a[1] * b[1], a[2] * b[2]]
}
fn len(a: V3) -> f32 {
    (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt()
}
fn norm(a: V3) -> V3 {
    let l = len(a);
    if l > 1e-6 {
        scale(a, 1.0 / l)
    } else {
        [0.0; 3]
    }
}
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}
fn lerp3(a: V3, b: V3, t: f32) -> V3 {
    [
        lerp(a[0], b[0], t),
        lerp(a[1], b[1], t),
        lerp(a[2], b[2], t),
    ]
}

/// xorshift64*: tiny, fast, good enough for particles, and seedable.
#[derive(Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    /// Uniform in [0, 1).
    fn f(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }
    fn range(&mut self, a: f32, b: f32) -> f32 {
        lerp(a, b, self.f())
    }
}

/// Deterministic per-particle random in [0,1) for (particle seed, slot).
fn hash01(seed: u32, slot: u32) -> f32 {
    let mut h = seed.wrapping_mul(0x9E37_79B1) ^ slot.wrapping_mul(0x85EB_CA77);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297A_2D39);
    h ^= h >> 15;
    (h >> 8) as f32 / (1u32 << 24) as f32
}

/// 3D gradient noise (Perlin-style) in roughly [-1, 1].
fn noise3(p: V3) -> f32 {
    fn fade(t: f32) -> f32 {
        t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
    }
    fn grad(ix: i32, iy: i32, iz: i32, d: V3) -> f32 {
        let h = (ix.wrapping_mul(73_856_093)
            ^ iy.wrapping_mul(19_349_663)
            ^ iz.wrapping_mul(83_492_791)) as u32;
        let h = hash01(h, 7);
        let a = h * std::f32::consts::TAU;
        let b = hash01(h.to_bits(), 3) * 2.0 - 1.0;
        let r = (1.0 - b * b).max(0.0).sqrt();
        d[0] * r * a.cos() + d[1] * r * a.sin() + d[2] * b
    }
    let (fx, fy, fz) = (p[0].floor(), p[1].floor(), p[2].floor());
    let (ix, iy, iz) = (fx as i32, fy as i32, fz as i32);
    let (dx, dy, dz) = (p[0] - fx, p[1] - fy, p[2] - fz);
    let mut c = [0.0f32; 8];
    for (k, v) in c.iter_mut().enumerate() {
        let (ox, oy, oz) = ((k & 1) as i32, ((k >> 1) & 1) as i32, ((k >> 2) & 1) as i32);
        *v = grad(
            ix + ox,
            iy + oy,
            iz + oz,
            [dx - ox as f32, dy - oy as f32, dz - oz as f32],
        );
    }
    let (u, v, w) = (fade(dx), fade(dy), fade(dz));
    let x0 = lerp(lerp(c[0], c[1], u), lerp(c[2], c[3], u), v);
    let x1 = lerp(lerp(c[4], c[5], u), lerp(c[6], c[7], u), v);
    lerp(x0, x1, w) * 1.6
}

/// Noise vector: three decorrelated noise lookups.
fn noise_vec(p: V3) -> V3 {
    [
        noise3(p),
        noise3(add(p, [31.4, 47.2, 12.9])),
        noise3(add(p, [-19.1, 5.3, 63.7])),
    ]
}

// ------------------------------------------------------------ definitions

fn f(o: &Value, k: &str, d: f32) -> f32 {
    value::get_f32(o, k, d)
}
/// vec3 that may be written as a scalar ("5" -> [5,5,5]) or "x y z".
fn v3(o: &Value, k: &str, d: V3) -> V3 {
    match value::get_floats(o, k) {
        Some(v) if v.len() >= 3 => [v[0], v[1], v[2]],
        Some(v) if v.len() == 1 => [v[0]; 3],
        _ => d,
    }
}
/// Rotations: a scalar means "around z".
fn rot3(o: &Value, k: &str) -> V3 {
    match value::get_floats(o, k) {
        Some(v) if v.len() >= 3 => [v[0], v[1], v[2]],
        Some(v) if v.len() == 1 => [0.0, 0.0, v[0]],
        _ => [0.0; 3],
    }
}

#[derive(Clone, Debug)]
enum EmitterKind {
    Sphere,
    Box,
}

#[derive(Clone, Debug)]
struct Emitter {
    kind: EmitterKind,
    origin: V3,
    dist_min: V3,
    dist_max: V3,
    directions: V3,
    sign: V3,
    speed: (f32, f32),
    rate: f32,
    instantaneous: u32,
    /// `audioprocessingmode` != 0: the rate follows the audio spectrum
    /// (mode, first band, last band, bounds, exponent).
    audio: Option<AudioResponse>,
}

/// (mode, first band, last band, bounds, exponent) of an audio-reactive
/// emitter, as `AudioFrame::response` takes them.
type AudioResponse = (u32, usize, usize, (f32, f32), f32);

#[derive(Clone, Debug)]
enum Init {
    Lifetime(f32, f32, f32),
    Size(f32, f32, f32),
    Alpha(f32, f32, f32),
    /// colorrandom, 0..255. ONE random t for the three channels (picking each
    /// channel independently turns warm palettes into rainbows).
    Color(V3, V3, f32),
    Velocity(V3, V3, f32),
    Rotation(V3, V3),
    AngularVelocity(V3, V3),
    Turbulent {
        speed: (f32, f32),
        scale: f32,
        timescale: f32,
        offset: f32,
        forward: V3,
    },
}

#[derive(Clone, Debug)]
struct Osc {
    freq: (f32, f32),
    scale: (f32, f32),
    phase: (f32, f32),
}

#[derive(Clone, Debug)]
enum Op {
    Movement {
        gravity: V3,
        drag: f32,
    },
    AlphaFade {
        fade_in: f32,
        fade_out: f32,
    },
    SizeChange(Ramp),
    AlphaChange(Ramp),
    ColorChange {
        start: f32,
        end: f32,
        from: V3,
        to: V3,
    },
    OscAlpha(Osc),
    OscSize(Osc),
    OscPosition(Osc, V3),
    Turbulence {
        speed: (f32, f32),
        scale: f32,
        timescale: f32,
        mask: V3,
        phase: f32,
    },
    Attract {
        cp: usize,
        origin: V3,
        scale: f32,
        threshold: f32,
    },
    Vortex {
        cp: usize,
        inner: f32,
        outer: f32,
        speed_inner: f32,
        speed_outer: f32,
    },
    AngularMovement {
        force: V3,
    },
}

#[derive(Clone, Copy, Debug)]
struct Ramp {
    start: f32,
    end: f32,
    from: f32,
    to: f32,
}

impl Ramp {
    fn at(&self, life: f32) -> f32 {
        if life <= self.start {
            self.from
        } else if life >= self.end {
            self.to
        } else {
            lerp(
                self.from,
                self.to,
                (life - self.start) / (self.end - self.start).max(1e-6),
            )
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RendererKind {
    Sprite,
    /// Quad stretched along the velocity (genericparticle with TRAILRENDERER).
    Trail {
        length: f32,
        max: f32,
        min: f32,
    },
    /// A rope through every live particle, in spawn order (genericropeparticle).
    Rope,
    /// Each particle drags a rope made of its own recent positions, `length`
    /// seconds long (genericropeparticle).
    RopeTrail {
        length: f32,
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ChildEvent {
    /// No `type`: an independent system emitting on its own from the parent
    /// system's origin (e.g. a few big glows around a fire).
    Static,
    Follow,
    Spawn,
    Death,
}

#[derive(Clone, Debug)]
pub struct ChildRef {
    pub file: String,
    pub event: ChildEvent,
    pub origin: V3,
    pub max_count: Option<u32>,
}

#[derive(Clone, Debug)]
pub struct ParticleDef {
    pub material: String,
    pub max_count: u32,
    pub start_time: f32,
    emitters: Vec<Emitter>,
    inits: Vec<Init>,
    ops: Vec<Op>,
    pub renderer: RendererKind,
    random_frame: bool,
    sequence_multiplier: f32,
    /// Control point offsets (object space) and whether they follow the cursor.
    control_points: [(V3, bool); 8],
    pub children: Vec<ChildRef>,
    /// Component names we don't simulate (for warnings).
    pub unsupported: Vec<String>,
}

impl ParticleDef {
    pub fn parse(j: &Value) -> ParticleDef {
        let arr = |k: &str| {
            j.get(k)
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default()
        };
        let mut unsupported = Vec::new();

        let emitters = arr("emitter")
            .iter()
            .filter_map(|e| {
                let kind = match e.get("name")?.as_str()? {
                    "sphererandom" => EmitterKind::Sphere,
                    "boxrandom" => EmitterKind::Box,
                    other => {
                        unsupported.push(format!("emitter {other}"));
                        return None;
                    }
                };
                Some(Emitter {
                    kind,
                    origin: v3(e, "origin", [0.0; 3]),
                    dist_min: v3(e, "distancemin", [0.0; 3]),
                    dist_max: v3(e, "distancemax", [256.0; 3]),
                    directions: v3(e, "directions", [1.0, 1.0, 0.0]),
                    sign: v3(e, "sign", [0.0; 3]),
                    speed: (f(e, "speedmin", 0.0), f(e, "speedmax", 0.0)),
                    rate: f(e, "rate", 5.0),
                    instantaneous: f(e, "instantaneous", 0.0).max(0.0) as u32,
                    audio: {
                        let mode = f(e, "audioprocessingmode", 0.0) as u32;
                        let b = value::get_vec2(e, "audioprocessingbounds", [0.0, 1.0]);
                        (mode != 0).then(|| {
                            (
                                mode,
                                f(e, "audioprocessingfrequencystart", 0.0) as usize,
                                f(e, "audioprocessingfrequencyend", 15.0) as usize,
                                (b[0], b[1]),
                                f(e, "audioprocessingexponent", 1.0),
                            )
                        })
                    },
                })
            })
            .collect();

        let range = |o: &Value, dmin: f32, dmax: f32| {
            (f(o, "min", dmin), f(o, "max", dmax), f(o, "exponent", 1.0))
        };
        let inits = arr("initializer")
            .iter()
            .filter_map(|i| {
                let name = i.get("name")?.as_str()?;
                Some(match name {
                    "lifetimerandom" => {
                        let (a, b, e) = range(i, 1.0, 1.0);
                        Init::Lifetime(a, b, e)
                    }
                    "sizerandom" => {
                        let (a, b, e) = range(i, 20.0, 20.0);
                        Init::Size(a, b, e)
                    }
                    "alpharandom" => {
                        let (a, b, e) = range(i, 1.0, 1.0);
                        Init::Alpha(a, b, e)
                    }
                    "colorrandom" => Init::Color(
                        v3(i, "min", [255.0; 3]),
                        v3(i, "max", v3(i, "min", [255.0; 3])),
                        f(i, "exponent", 1.0),
                    ),
                    "velocityrandom" => Init::Velocity(
                        v3(i, "min", [-32.0, -32.0, 0.0]),
                        v3(i, "max", [32.0, 32.0, 0.0]),
                        f(i, "exponent", 1.0),
                    ),
                    "rotationrandom" => Init::Rotation(rot3(i, "min"), rot3(i, "max")),
                    "angularvelocityrandom" => {
                        Init::AngularVelocity(v3(i, "min", [0.0; 3]), v3(i, "max", [0.0; 3]))
                    }
                    "turbulentvelocityrandom" => Init::Turbulent {
                        speed: (f(i, "speedmin", 100.0), f(i, "speedmax", 250.0)),
                        scale: f(i, "scale", 1.0),
                        timescale: f(i, "timescale", 1.0),
                        offset: f(i, "offset", 0.0),
                        forward: v3(i, "forward", [0.0, 1.0, 0.0]),
                    },
                    other => {
                        unsupported.push(format!("initializer {other}"));
                        return None;
                    }
                })
            })
            .collect();

        let osc = |o: &Value, smin: f32, smax: f32| Osc {
            freq: (f(o, "frequencymin", 0.0), f(o, "frequencymax", 10.0)),
            scale: (f(o, "scalemin", smin), f(o, "scalemax", smax)),
            phase: (
                f(o, "phasemin", 0.0),
                f(o, "phasemax", std::f32::consts::TAU),
            ),
        };
        let ramp = |o: &Value| Ramp {
            start: f(o, "starttime", 0.0),
            end: f(o, "endtime", 1.0),
            from: f(o, "startvalue", 1.0),
            to: f(o, "endvalue", 0.0),
        };
        let ops = arr("operator")
            .iter()
            .filter_map(|o| {
                let name = o.get("name")?.as_str()?;
                Some(match name {
                    "movement" => Op::Movement {
                        gravity: v3(o, "gravity", [0.0; 3]),
                        drag: f(o, "drag", 0.0),
                    },
                    "alphafade" => Op::AlphaFade {
                        fade_in: f(o, "fadeintime", 0.5),
                        fade_out: f(o, "fadeouttime", 0.5),
                    },
                    "sizechange" => Op::SizeChange(ramp(o)),
                    "alphachange" => Op::AlphaChange(ramp(o)),
                    "colorchange" => Op::ColorChange {
                        start: f(o, "starttime", 0.0),
                        end: f(o, "endtime", 1.0),
                        from: v3(o, "startvalue", [1.0; 3]),
                        to: v3(o, "endvalue", [1.0; 3]),
                    },
                    "oscillatealpha" => Op::OscAlpha(osc(o, 0.0, 1.0)),
                    "oscillatesize" => Op::OscSize(osc(o, 0.8, 1.2)),
                    "oscillateposition" => {
                        Op::OscPosition(osc(o, 0.0, 10.0), v3(o, "mask", [1.0, 1.0, 0.0]))
                    }
                    "turbulence" => Op::Turbulence {
                        speed: (f(o, "speedmin", 500.0), f(o, "speedmax", 1000.0)),
                        // 0.005 is the value the editor serializes by default
                        scale: f(o, "scale", 0.005),
                        timescale: f(o, "timescale", 1.0),
                        mask: v3(o, "mask", [1.0, 1.0, 0.0]),
                        phase: f(o, "phasemax", 0.0),
                    },
                    "controlpointattract" => Op::Attract {
                        cp: f(o, "controlpoint", 0.0) as usize,
                        origin: v3(o, "origin", [0.0; 3]),
                        scale: f(o, "scale", 100.0),
                        threshold: f(o, "threshold", 512.0),
                    },
                    "vortex" => Op::Vortex {
                        cp: f(o, "controlpoint", 0.0) as usize,
                        inner: f(o, "distanceinner", 500.0),
                        outer: f(o, "distanceouter", 650.0),
                        speed_inner: f(o, "speedinner", 2500.0),
                        speed_outer: f(o, "speedouter", 0.0),
                    },
                    "angularmovement" => Op::AngularMovement {
                        force: v3(o, "force", [0.0; 3]),
                    },
                    other => {
                        unsupported.push(format!("operator {other}"));
                        return None;
                    }
                })
            })
            .collect();

        let r = arr("renderer");
        let renderer = match r.first() {
            Some(r) => match r.get("name").and_then(|n| n.as_str()).unwrap_or("sprite") {
                "spritetrail" => RendererKind::Trail {
                    length: f(r, "length", 0.05),
                    max: f(r, "maxlength", 10.0),
                    min: f(r, "minlength", 0.0),
                },
                "rope" => RendererKind::Rope,
                "ropetrail" => RendererKind::RopeTrail {
                    length: f(r, "length", 0.2).max(0.01),
                },
                _ => RendererKind::Sprite,
            },
            None => RendererKind::Sprite,
        };

        let mut control_points = [([0.0f32; 3], false); 8];
        for cp in arr("controlpoint") {
            let id = f(&cp, "id", 0.0) as usize;
            if id < 8 {
                let flags = f(&cp, "flags", 0.0) as u32;
                control_points[id] = (v3(&cp, "offset", [0.0; 3]), flags & 1 != 0);
            }
        }

        let children = arr("children")
            .iter()
            .filter_map(|c| {
                Some(ChildRef {
                    file: c.get("name")?.as_str()?.to_string(),
                    // Legacy presets (e.g. the stock "ember" -> "emberglow")
                    // list children without a `type`; WE does not draw them
                    // (Akame 1215952856: no trace of the 1000 px halos in the
                    // capture), so they are dropped. See GL_RENDERER.md.
                    event: match c.get("type")?.as_str()? {
                        "eventspawn" => ChildEvent::Spawn,
                        "eventdeath" => ChildEvent::Death,
                        "eventfollow" => ChildEvent::Follow,
                        _ => ChildEvent::Static,
                    },
                    origin: v3(c, "origin", [0.0; 3]),
                    max_count: c.get("maxcount").and_then(|m| m.as_u64()).map(|m| m as u32),
                })
            })
            .collect();

        ParticleDef {
            material: j
                .get("material")
                .and_then(|m| m.as_str())
                .unwrap_or("")
                .to_string(),
            max_count: f(j, "maxcount", 100.0).max(0.0) as u32,
            start_time: f(j, "starttime", 0.0).max(0.0),
            emitters,
            inits,
            ops,
            renderer,
            random_frame: j.get("animationmode").and_then(|a| a.as_str()) == Some("randomframe"),
            sequence_multiplier: f(j, "sequencemultiplier", 1.0),
            control_points,
            children,
            unsupported,
        }
    }
}

/// `instanceoverride` of a scene object: multipliers on top of the definition.
#[derive(Clone, Copy, Debug)]
pub struct Overrides {
    pub alpha: f32,
    pub size: f32,
    pub lifetime: f32,
    pub rate: f32,
    pub speed: f32,
    pub count: f32,
    pub color: Option<V3>,
    pub brightness: f32,
}

impl Overrides {
    pub fn parse(o: Option<&Value>) -> Overrides {
        let d = Value::Null;
        let o = o.unwrap_or(&d);
        let color = value::get_floats(o, "colorn")
            .filter(|c| c.len() >= 3)
            .map(|c| [c[0], c[1], c[2]])
            .or_else(|| {
                value::get_floats(o, "color")
                    .filter(|c| c.len() >= 3)
                    .map(|c| [c[0] / 255.0, c[1] / 255.0, c[2] / 255.0])
            });
        Overrides {
            alpha: f(o, "alpha", 1.0),
            size: f(o, "size", 1.0),
            lifetime: f(o, "lifetime", 1.0),
            rate: f(o, "rate", 1.0),
            speed: f(o, "speed", 1.0),
            count: f(o, "count", 1.0),
            color,
            brightness: f(o, "brightness", 1.0),
        }
    }

    pub const NONE: Overrides = Overrides {
        alpha: 1.0,
        size: 1.0,
        lifetime: 1.0,
        rate: 1.0,
        speed: 1.0,
        count: 1.0,
        color: None,
        brightness: 1.0,
    };
}

// ------------------------------------------------------------- simulation

#[derive(Clone, Debug)]
struct Particle {
    pos: V3,
    vel: V3,
    color: V3,
    alpha: f32,
    size: f32,
    rot: V3,
    ang_vel: V3,
    age: f32,
    lifetime: f32,
    seed: u32,
    /// Frame for `randomframe` sprite sheets, as a phase in [0,1).
    frame_phase: f32,
    /// Accumulated child emissions (eventfollow).
    child_acc: f32,
    /// ropetrail: recent positions, newest first, and time since the last sample.
    trail: Vec<V3>,
    trail_t: f32,
    // values computed by operators for the current step (drawn as-is)
    draw_alpha: f32,
    draw_size: f32,
    draw_color: V3,
    osc_offset: V3,
}

/// Where a particle was born or died, for child systems.
#[derive(Clone, Copy, Debug)]
pub struct Event {
    pub pos: V3,
    pub kind: ChildEvent,
}

pub struct ParticleSim {
    pub def: ParticleDef,
    ov: Overrides,
    particles: Vec<Particle>,
    rng: Rng,
    seed: u64,
    emit_acc: Vec<f32>,
    pub time: f32,
    started: bool,
    next_seed: u32,
    /// Spawn/death events of the last step (consumed by child systems).
    pub events: Vec<Event>,
    /// Current audio spectrum (audio-reactive emitters).
    pub audio: crate::audio::AudioFrame,
}

/// See `spawn`: WE's rendered particle size relative to `sizerandom`.
const SIZE_SCALE: f32 = 0.5;

/// How fast the turbulence field evolves relative to `timescale` (tuned with
/// the harness: a fast-changing field makes particles curl in place).
const TURBULENCE_TIME: f32 = 0.05;

/// Fixed simulation step (seconds).
pub const STEP: f32 = 1.0 / 60.0;

impl ParticleSim {
    pub fn new(def: ParticleDef, ov: Overrides, seed: u64) -> ParticleSim {
        let n = def.emitters.len();
        ParticleSim {
            def,
            ov,
            particles: Vec::new(),
            rng: Rng::new(seed),
            seed,
            emit_acc: vec![0.0; n],
            time: 0.0,
            started: false,
            next_seed: 1,
            events: Vec::new(),
            audio: crate::audio::AudioFrame::default(),
        }
    }

    pub fn reset(&mut self) {
        self.particles.clear();
        self.rng = Rng::new(self.seed);
        self.emit_acc.iter_mut().for_each(|a| *a = 0.0);
        self.time = 0.0;
        self.started = false;
        self.next_seed = 1;
        self.events.clear();
    }

    pub fn max_count(&self) -> usize {
        ((self.def.max_count as f32) * self.ov.count)
            .round()
            .max(0.0) as usize
    }

    pub fn len(&self) -> usize {
        self.particles.len()
    }

    pub fn is_empty(&self) -> bool {
        self.particles.is_empty()
    }

    /// Positions of live particles (eventfollow children emit from them).
    pub fn positions_with_acc(&mut self) -> impl Iterator<Item = (V3, &mut f32)> {
        self.particles.iter_mut().map(|p| (p.pos, &mut p.child_acc))
    }

    /// Spawns one particle at `at` (object space) using emitter `e`'s velocity
    /// rules if given, then runs all initializers.
    fn spawn(&mut self, e: Option<usize>, at: Option<V3>, cps: &[V3; 8]) {
        if self.particles.len() >= self.max_count() {
            return;
        }
        let rng = &mut self.rng;
        let (mut pos, mut vel) = ([0.0; 3], [0.0; 3]);
        if let Some(ei) = e {
            let em = &self.def.emitters[ei];
            match em.kind {
                EmitterKind::Sphere => {
                    // random direction, flattened/stretched by `directions`,
                    // optionally forced to one side per axis by `sign`
                    let mut d = [
                        rng.range(-1.0, 1.0),
                        rng.range(-1.0, 1.0),
                        rng.range(-1.0, 1.0),
                    ];
                    for (k, dk) in d.iter_mut().enumerate() {
                        if em.sign[k] != 0.0 {
                            *dk = dk.abs() * em.sign[k].signum();
                        }
                    }
                    let d = norm(mul3(d, em.directions));
                    let t = rng.f();
                    let dist = lerp3(em.dist_min, em.dist_max, t);
                    pos = add(em.origin, mul3(d, dist));
                    let speed = rng.range(em.speed.0, em.speed.1);
                    vel = scale(d, speed);
                }
                EmitterKind::Box => {
                    let r = [
                        rng.range(-1.0, 1.0),
                        rng.range(-1.0, 1.0),
                        rng.range(-1.0, 1.0),
                    ];
                    pos = add(em.origin, mul3(r, em.dist_max));
                }
            }
        }
        if let Some(a) = at {
            pos = add(pos, a);
        }
        let _ = cps;

        let mut p = Particle {
            pos,
            vel,
            color: [1.0; 3],
            alpha: 1.0,
            size: 20.0,
            rot: [0.0; 3],
            ang_vel: [0.0; 3],
            age: 0.0,
            lifetime: 1.0,
            seed: self.next_seed,
            frame_phase: 0.0,
            child_acc: 0.0,
            trail: Vec::new(),
            trail_t: 0.0,
            draw_alpha: 1.0,
            draw_size: 20.0,
            draw_color: [1.0; 3],
            osc_offset: [0.0; 3],
        };
        self.next_seed = self.next_seed.wrapping_add(1);
        let biased = |rng: &mut Rng, e: f32| rng.f().powf(if e > 0.0 { e } else { 1.0 });
        for init in &self.def.inits {
            match *init {
                Init::Lifetime(a, b, e) => p.lifetime = lerp(a, b, biased(rng, e)),
                Init::Size(a, b, e) => p.size = lerp(a, b, biased(rng, e)),
                Init::Alpha(a, b, e) => p.alpha = lerp(a, b, biased(rng, e)),
                Init::Color(a, b, e) => p.color = scale(lerp3(a, b, biased(rng, e)), 1.0 / 255.0),
                Init::Velocity(a, b, e) => {
                    let mut v = [0.0; 3];
                    for (k, vk) in v.iter_mut().enumerate() {
                        *vk = lerp(a[k], b[k], biased(rng, e));
                    }
                    p.vel = add(p.vel, v);
                }
                Init::Rotation(a, b) => {
                    p.rot = [
                        rng.range(a[0], b[0]),
                        rng.range(a[1], b[1]),
                        rng.range(a[2], b[2]),
                    ]
                }
                Init::AngularVelocity(a, b) => {
                    p.ang_vel = [
                        rng.range(a[0], b[0]),
                        rng.range(a[1], b[1]),
                        rng.range(a[2], b[2]),
                    ]
                }
                Init::Turbulent {
                    speed,
                    scale: s,
                    timescale,
                    offset,
                    forward,
                } => {
                    // Direction from a noise field sampled at the spawn point
                    // (neighbours move alike), biased along `forward` by
                    // `offset`. Measured on Luffy/Akame (sakura with offset 3
                    // over a downward velocityrandom): in WE about half of the
                    // petals rise and half fall, i.e. offset pushes along
                    // forward. (An earlier "noise phase" reading was a
                    // misdiagnosis of the z-clipping bug fixed in math.rs.)
                    // The sign of `offset` does not reverse the push: the
                    // stock "ember" preset (offset -0.5) rises in WE (Akame
                    // 1215952856, motion energy ratio 0.21 -> 0.90 with |offset|).
                    // Flattened to the scene plane when forward.z = 0.
                    let n = noise_vec(add(scale(p.pos, s * 0.01), [self.time * timescale; 3]));
                    let mut d = add(n, scale(forward, offset.abs()));
                    if forward[2] == 0.0 {
                        d[2] = 0.0;
                    }
                    let sp = rng.range(speed.0, speed.1);
                    p.vel = add(p.vel, scale(norm(d), sp));
                }
            }
        }
        p.lifetime = (p.lifetime * self.ov.lifetime).max(0.01);
        // WE draws a particle with HALF its `size` (the value behaves like a
        // radius): measured with the harness on a 4K and a 1080p scene
        // (Ayanami 3571376089, Akame 1827211233), x0.5 matches WE's star sizes.
        p.size *= self.ov.size * SIZE_SCALE;
        p.alpha *= self.ov.alpha;
        p.vel = scale(p.vel, self.ov.speed);
        if let Some(c) = self.ov.color {
            p.color = mul3(p.color, c);
        }
        p.color = scale(p.color, self.ov.brightness);
        p.frame_phase = rng.f();
        p.draw_alpha = p.alpha;
        p.draw_size = p.size;
        p.draw_color = p.color;
        self.events.push(Event {
            pos: p.pos,
            kind: ChildEvent::Spawn,
        });
        self.particles.push(p);
    }

    /// Emits `n` particles at `at` (children triggered by parent events).
    pub fn burst_at(&mut self, at: V3, n: u32, cps: &[V3; 8]) {
        let emitter = (!self.def.emitters.is_empty()).then_some(0);
        for _ in 0..n {
            self.spawn(emitter, Some(at), cps);
        }
    }

    /// Particles per second of the first emitter (children emitting from
    /// followed parents use it as their per-parent rate).
    pub fn rate(&self) -> f32 {
        self.def.emitters.first().map(|e| e.rate).unwrap_or(0.0) * self.ov.rate
    }

    /// Burst size for spawn/death events: the emitter's `instantaneous` count,
    /// or 1 when it only has a rate.
    pub fn burst_size(&self) -> u32 {
        self.def
            .emitters
            .first()
            .map(|e| e.instantaneous.max(1))
            .unwrap_or(1)
    }

    /// Advances one fixed step. `emit` = false for child systems driven by
    /// their parent's events (they don't emit on their own).
    pub fn step(&mut self, dt: f32, cps: &[V3; 8], emit: bool) {
        self.events.clear();
        if emit {
            if !self.started {
                self.started = true;
                for ei in 0..self.def.emitters.len() {
                    for _ in 0..self.def.emitters[ei].instantaneous {
                        self.spawn(Some(ei), None, cps);
                    }
                }
            }
            for ei in 0..self.def.emitters.len() {
                let em = &self.def.emitters[ei];
                let gain = em
                    .audio
                    .map(|(mode, s, e, b, x)| self.audio.response(mode, s, e, b, x))
                    .unwrap_or(1.0);
                self.emit_acc[ei] += em.rate * self.ov.rate * gain * dt;
                while self.emit_acc[ei] >= 1.0 {
                    self.emit_acc[ei] -= 1.0;
                    self.spawn(Some(ei), None, cps);
                }
            }
        }

        let time = self.time;
        let trail_len = match self.def.renderer {
            RendererKind::RopeTrail { length } => Some(length),
            _ => None,
        };
        let ops = &self.def.ops;
        let def_cps = &self.def.control_points;
        let mut deaths = Vec::new();
        self.particles.retain_mut(|p| {
            p.age += dt;
            let life = p.age / p.lifetime;
            if life >= 1.0 {
                deaths.push(p.pos);
                return false;
            }
            let (mut alpha, mut size, mut color) = (p.alpha, p.size, p.color);
            let mut osc_offset = [0.0f32; 3];
            for (k, op) in ops.iter().enumerate() {
                let slot = (k as u32) * 4;
                let pick = |i: u32, r: (f32, f32)| lerp(r.0, r.1, hash01(p.seed, slot + i));
                let wave = |o: &Osc| {
                    let fr = pick(0, o.freq);
                    let ph = pick(1, o.phase);
                    // frequencies are angular (rad/s), like the editor's ranges suggest
                    (p.age * fr + ph).sin()
                };
                match *op {
                    Op::Movement { gravity, drag } => {
                        p.vel = add(p.vel, scale(gravity, dt));
                        p.vel = scale(p.vel, (1.0 - drag * dt).max(0.0));
                    }
                    Op::AlphaFade { fade_in, fade_out } => {
                        if fade_in > 0.0 && life < fade_in {
                            alpha *= life / fade_in;
                        }
                        if life > fade_out && fade_out < 1.0 {
                            alpha *= 1.0 - (life - fade_out) / (1.0 - fade_out);
                        }
                    }
                    Op::SizeChange(r) => size *= r.at(life),
                    Op::AlphaChange(r) => alpha *= r.at(life),
                    Op::ColorChange {
                        start,
                        end,
                        from,
                        to,
                    } => {
                        let t = ((life - start) / (end - start).max(1e-6)).clamp(0.0, 1.0);
                        color = mul3(color, lerp3(from, to, t));
                    }
                    // oscillate between scalemin and scalemax (per-particle
                    // frequency and phase)
                    Op::OscAlpha(ref o) => {
                        alpha *= lerp(o.scale.0, o.scale.1, wave(o) * 0.5 + 0.5).max(0.0);
                    }
                    Op::OscSize(ref o) => {
                        size *= lerp(o.scale.0, o.scale.1, wave(o) * 0.5 + 0.5).max(0.0);
                    }
                    Op::OscPosition(ref o, mask) => {
                        let s = pick(2, o.scale);
                        // per-axis phase so x and y don't move in lockstep
                        let fr = pick(0, o.freq);
                        let ph = pick(1, o.phase);
                        let w = [
                            (p.age * fr + ph).sin(),
                            (p.age * fr + ph + 1.7).sin(),
                            (p.age * fr + ph + 3.1).sin(),
                        ];
                        osc_offset = add(osc_offset, scale(mul3(w, mask), s));
                    }
                    Op::Turbulence {
                        speed,
                        scale: s,
                        timescale,
                        mask,
                        phase,
                    } => {
                        // Flow field: particles FOLLOW a slowly evolving noise
                        // vector field at a per-particle constant speed. Matched
                        // against WE with the harness (Ayanami, 3571376089):
                        // - accelerating by the noise left young particles still;
                        // - a curl field draws closed loops/spirals; WE's paths
                        //   are open and gently wavy, like a plain noise field.
                        let sp = pick(0, speed);
                        let t =
                            time * timescale * TURBULENCE_TIME + phase * hash01(p.seed, slot + 1);
                        let n = noise_vec(add(scale(p.pos, s), [t, t * 0.7, t * 1.3]));
                        let flow = norm(n);
                        p.pos = add(p.pos, scale(mul3(flow, mask), sp * dt));
                    }
                    Op::Attract {
                        cp,
                        origin,
                        scale: s,
                        threshold,
                    } => {
                        let target = add(cps[cp.min(7)], origin);
                        let d = sub(target, p.pos);
                        let dist = len(d);
                        if dist < threshold && dist > 1e-3 {
                            p.vel = add(p.vel, scale(norm(d), s * dt));
                        }
                    }
                    Op::Vortex {
                        cp,
                        inner,
                        outer,
                        speed_inner,
                        speed_outer,
                    } => {
                        let c = cps[cp.min(7)];
                        let d = sub(p.pos, c);
                        let dist = len([d[0], d[1], 0.0]);
                        let t = ((dist - inner) / (outer - inner).max(1e-3)).clamp(0.0, 1.0);
                        let sp = lerp(speed_inner, speed_outer, t);
                        let tangent = norm([-d[1], d[0], 0.0]);
                        p.vel = add(p.vel, scale(tangent, sp * dt));
                    }
                    Op::AngularMovement { force } => {
                        p.ang_vel = add(p.ang_vel, scale(force, dt));
                        p.rot = add(p.rot, scale(p.ang_vel, dt));
                    }
                }
            }
            let _ = def_cps;
            p.pos = add(p.pos, scale(p.vel, dt));
            p.osc_offset = osc_offset;
            if let Some(len) = trail_len {
                p.trail_t += dt;
                let interval = len / ROPE_SEGMENTS as f32;
                if p.trail.is_empty() || p.trail_t >= interval {
                    p.trail_t = 0.0;
                    p.trail.insert(0, add(p.pos, osc_offset));
                    p.trail.truncate(ROPE_SEGMENTS + 1);
                }
            }
            p.draw_alpha = alpha.clamp(0.0, 1.0);
            p.draw_size = size.max(0.0);
            p.draw_color = color;
            true
        });
        for pos in deaths {
            self.events.push(Event {
                pos,
                kind: ChildEvent::Death,
            });
        }
        self.time += dt;
    }

    /// Builds the vertex stream (4 vertices x `VERTEX_FLOATS` per particle).
    pub fn vertices(&self, out: &mut Vec<f32>) {
        out.clear();
        let seq = self.def.sequence_multiplier.max(0.0);
        for p in &self.particles {
            let life = (p.age / p.lifetime).clamp(0.0, 0.9999);
            let phase = if self.def.random_frame {
                p.frame_phase
            } else {
                (life * seq).fract()
            };
            let pos = add(p.pos, p.osc_offset);
            for (u, v) in [(0.0f32, 0.0f32), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)] {
                out.extend_from_slice(&[
                    pos[0],
                    pos[1],
                    pos[2],
                    u,
                    v,
                    p.rot[2],
                    p.draw_size,
                    p.draw_color[0],
                    p.draw_color[1],
                    p.draw_color[2],
                    p.draw_alpha,
                    p.rot[0],
                    p.rot[1],
                    p.vel[0],
                    p.vel[1],
                    p.vel[2],
                    phase,
                ]);
            }
        }
    }
}

/// History samples of a `ropetrail` (segments per particle).
const ROPE_SEGMENTS: usize = 16;

/// Floats per rope vertex: a_PositionVec4 (start, size), a_TexCoordVec4 (end,
/// rope length), a_TexCoordVec4C1 (spline cp0, segment index), a_TexCoordVec3C2
/// (spline cp1), a_TexCoordC3 (uv), a_Color.
pub const ROPE_FLOATS: usize = 21;

impl ParticleSim {
    /// Rope vertex stream (4 vertices per segment). Spline control points are
    /// set to the segment ends, i.e. straight segments with the right tangent.
    pub fn rope_vertices(&self, out: &mut Vec<f32>) {
        out.clear();
        let mut push_rope = |pts: &[(V3, f32, [f32; 4])]| {
            let n = pts.len();
            if n < 2 {
                return;
            }
            for i in 0..n - 1 {
                let (a, size, c) = pts[i];
                let (b, _, _) = pts[i + 1];
                for (u, v) in [(0.0f32, 0.0f32), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)] {
                    out.extend_from_slice(&[
                        a[0], a[1], a[2], size, // start, size
                        b[0], b[1], b[2], n as f32, // end, rope length
                        a[0], a[1], a[2], i as f32, // cp0 (= start), segment index
                        b[0], b[1], b[2], // cp1 (= end)
                        u, v, // uv: across, along
                        c[0], c[1], c[2], c[3],
                    ]);
                }
            }
        };
        let point = |p: &Particle, pos: V3| {
            (
                pos,
                p.draw_size,
                [
                    p.draw_color[0],
                    p.draw_color[1],
                    p.draw_color[2],
                    p.draw_alpha,
                ],
            )
        };
        match self.def.renderer {
            RendererKind::Rope => {
                let pts: Vec<_> = self
                    .particles
                    .iter()
                    .map(|p| point(p, add(p.pos, p.osc_offset)))
                    .collect();
                push_rope(&pts);
            }
            RendererKind::RopeTrail { .. } => {
                for p in &self.particles {
                    let pts: Vec<_> = p.trail.iter().map(|&q| point(p, q)).collect();
                    push_rope(&pts);
                }
            }
            _ => {}
        }
    }
}

/// Floats per vertex: pos3, texcoordvec4 (u, v, rot z, size), color4, c2 (rot
/// x, rot y), vec4c1 (velocity3, lifetime phase).
pub const VERTEX_FLOATS: usize = 17;

/// Control point positions in object space: their offsets, or the cursor for
/// points flagged to follow it (`pointer_local`).
pub fn control_points(def: &ParticleDef, pointer_local: V3) -> [V3; 8] {
    let mut out = [[0.0f32; 3]; 8];
    for (i, (off, follow)) in def.control_points.iter().enumerate() {
        out[i] = if *follow { pointer_local } else { *off };
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sim(j: Value) -> ParticleSim {
        ParticleSim::new(ParticleDef::parse(&j), Overrides::NONE, 42)
    }

    #[test]
    fn emits_at_rate_and_respects_maxcount() {
        let mut s = sim(json!({
            "maxcount": 50,
            "emitter": [{ "name": "boxrandom", "rate": 100, "distancemax": "10 10 0" }],
            "initializer": [{ "name": "lifetimerandom", "min": 100, "max": 100 }]
        }));
        let cps = [[0.0; 3]; 8];
        for _ in 0..30 {
            s.step(STEP, &cps, true); // 0.5 s -> 50 wanted
        }
        assert_eq!(s.len(), 50);
        for _ in 0..60 {
            s.step(STEP, &cps, true);
        }
        assert_eq!(s.len(), 50, "capped by maxcount");
    }

    #[test]
    fn particles_die_after_lifetime_and_report_events() {
        let mut s = sim(json!({
            "maxcount": 10,
            "emitter": [{ "name": "sphererandom", "rate": 0, "instantaneous": 3 }],
            "initializer": [{ "name": "lifetimerandom", "min": 0.1, "max": 0.1 }]
        }));
        let cps = [[0.0; 3]; 8];
        s.step(STEP, &cps, true);
        assert_eq!(s.len(), 3);
        assert_eq!(
            s.events
                .iter()
                .filter(|e| e.kind == ChildEvent::Spawn)
                .count(),
            3
        );
        let mut deaths = 0;
        for _ in 0..10 {
            s.step(STEP, &cps, true);
            deaths += s
                .events
                .iter()
                .filter(|e| e.kind == ChildEvent::Death)
                .count();
        }
        assert_eq!(s.len(), 0);
        assert_eq!(deaths, 3);
    }

    #[test]
    fn same_seed_same_result() {
        let j = json!({
            "maxcount": 100,
            "emitter": [{ "name": "sphererandom", "rate": 50, "distancemax": 100, "speedmin": 10, "speedmax": 50 }],
            "initializer": [{ "name": "lifetimerandom", "min": 1, "max": 3 }, { "name": "colorrandom", "min": "0 0 0", "max": "255 255 255" }],
            "operator": [{ "name": "movement", "gravity": "0 -10 0" }, { "name": "turbulence", "speedmin": 10, "speedmax": 20 }]
        });
        let (mut a, mut b) = (sim(j.clone()), sim(j));
        let cps = [[0.0; 3]; 8];
        for _ in 0..90 {
            a.step(STEP, &cps, true);
            b.step(STEP, &cps, true);
        }
        let (mut va, mut vb) = (Vec::new(), Vec::new());
        a.vertices(&mut va);
        b.vertices(&mut vb);
        assert!(!va.is_empty());
        assert_eq!(va, vb);
    }

    #[test]
    fn colorrandom_uses_one_t_for_all_channels() {
        let mut s = sim(json!({
            "maxcount": 200,
            "emitter": [{ "name": "boxrandom", "rate": 0, "instantaneous": 200 }],
            "initializer": [{ "name": "colorrandom", "min": "255 0 0", "max": "0 0 255" }]
        }));
        s.step(STEP, &[[0.0; 3]; 8], true);
        let mut v = Vec::new();
        s.vertices(&mut v);
        for px in v.chunks(VERTEX_FLOATS) {
            // on the red->blue line, r + b == 1 and g == 0
            assert!((px[7] + px[9] - 1.0).abs() < 1e-4 && px[8] == 0.0);
        }
    }

    #[test]
    fn alphafade_ramps_in_and_out() {
        let mut s = sim(json!({
            "maxcount": 1,
            "emitter": [{ "name": "boxrandom", "rate": 0, "instantaneous": 1, "distancemax": "0 0 0" }],
            "initializer": [{ "name": "lifetimerandom", "min": 1, "max": 1 }],
            "operator": [{ "name": "alphafade", "fadeintime": 0.5, "fadeouttime": 0.5 }]
        }));
        let cps = [[0.0; 3]; 8];
        let mut alphas = Vec::new();
        for _ in 0..55 {
            s.step(STEP, &cps, true);
            let mut v = Vec::new();
            s.vertices(&mut v);
            if !v.is_empty() {
                alphas.push(v[10]);
            }
        }
        let peak = alphas.iter().cloned().fold(0.0, f32::max);
        assert!(alphas[2] < 0.2 && peak > 0.9 && *alphas.last().unwrap() < 0.3);
    }
}
