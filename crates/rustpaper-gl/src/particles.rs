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

/// The emitters' draw range [0.00001, 1) (WE-RE-003 0.1).
fn rng_b(rng: &mut Rng) -> f32 {
    0.00001 + (1.0 - 0.00001) * rng.f()
}

/// Emitter launch velocity: radially out along the offset `o`, speed uniform
/// in speedmin..speedmax; a random direction when `o` is ~0 (WE-RE-003 2.3).
fn launch(rng: &mut Rng, o: V3, em: &Emitter) -> V3 {
    let dir = if o[0] * o[0] + o[1] * o[1] + o[2] * o[2] < 0.0001 {
        let mut d = [0.0; 3];
        for (dk, dir_k) in d.iter_mut().zip(em.directions) {
            *dk = (2.0 * rng_b(rng) - 1.0) * dir_k;
        }
        d
    } else {
        o
    };
    let speed = lerp(em.speed.0, em.speed.1, rng_b(rng));
    let len = len(dir);
    if len > 0.0 {
        scale(dir, speed / len)
    } else {
        [0.0; 3]
    }
}

/// Rotates `v` around the unit `axis` by `angle` radians (Rodrigues).
fn rotate_axis(v: V3, axis: V3, angle: f32) -> V3 {
    let (s, c) = angle.sin_cos();
    let d = axis[0] * v[0] + axis[1] * v[1] + axis[2] * v[2];
    let x = [
        axis[1] * v[2] - axis[2] * v[1],
        axis[2] * v[0] - axis[0] * v[2],
        axis[0] * v[1] - axis[1] * v[0],
    ];
    [0, 1, 2].map(|k| v[k] * c + x[k] * s + axis[k] * d * (1.0 - c))
}

fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Ken Perlin's reference permutation.
const PERM: [u8; 256] = [
    151, 160, 137, 91, 90, 15, 131, 13, 201, 95, 96, 53, 194, 233, 7, 225, 140, 36, 103, 30, 69,
    142, 8, 99, 37, 240, 21, 10, 23, 190, 6, 148, 247, 120, 234, 75, 0, 26, 197, 62, 94, 252, 219,
    203, 117, 35, 11, 32, 57, 177, 33, 88, 237, 149, 56, 87, 174, 20, 125, 136, 171, 168, 68, 175,
    74, 165, 71, 134, 139, 48, 27, 166, 77, 146, 158, 231, 83, 111, 229, 122, 60, 211, 133, 230,
    220, 105, 92, 41, 55, 46, 245, 40, 244, 102, 143, 54, 65, 25, 63, 161, 1, 216, 80, 73, 209, 76,
    132, 187, 208, 89, 18, 169, 200, 196, 135, 130, 116, 188, 159, 86, 164, 100, 109, 198, 173,
    186, 3, 64, 52, 217, 226, 250, 124, 123, 5, 202, 38, 147, 118, 126, 255, 82, 85, 212, 207, 206,
    59, 227, 47, 16, 58, 17, 182, 189, 28, 42, 223, 183, 170, 213, 119, 248, 152, 2, 44, 154, 163,
    70, 221, 153, 101, 155, 167, 43, 172, 9, 129, 22, 39, 253, 19, 98, 108, 110, 79, 113, 224, 232,
    178, 185, 112, 104, 218, 246, 97, 228, 251, 34, 242, 193, 238, 210, 144, 12, 191, 179, 162,
    241, 81, 51, 145, 235, 249, 14, 239, 107, 49, 192, 214, 31, 181, 199, 106, 157, 184, 84, 204,
    176, 115, 121, 50, 45, 127, 4, 150, 254, 138, 236, 205, 93, 222, 114, 67, 29, 24, 72, 243, 141,
    128, 195, 78, 66, 215, 61, 156, 180,
];

/// Standard 1-D gradient (simplex) noise, roughly in [-1, 1].
fn noise1(t: f32) -> f32 {
    let i = t.floor();
    let f = t - i;
    let grad = |h: u8, x: f32| {
        let g = 1.0 + (h & 7) as f32;
        if h & 8 != 0 { -g * x } else { g * x }
    };
    let i = i as i64;
    let h0 = PERM[i.rem_euclid(256) as usize];
    let h1 = PERM[(i + 1).rem_euclid(256) as usize];
    let a = 1.0 - f * f;
    let b = 1.0 - (f - 1.0) * (f - 1.0);
    0.395 * (grad(h0, f) * a.powi(4) + grad(h1, f - 1.0) * b.powi(4))
}

/// 3-D simplex noise as in Gustavson's reference ("Simplex noise
/// demystified"): Perlin's permutation, the 12 cube-edge gradients, kernel
/// (0.6 - d^2)^4, output x32 (signed, mean 0). WE-RE-004 1.4.
fn simplex3(x: f32, y: f32, z: f32) -> f32 {
    const G: [[f32; 3]; 12] = [
        [1.0, 1.0, 0.0],
        [-1.0, 1.0, 0.0],
        [1.0, -1.0, 0.0],
        [-1.0, -1.0, 0.0],
        [1.0, 0.0, 1.0],
        [-1.0, 0.0, 1.0],
        [1.0, 0.0, -1.0],
        [-1.0, 0.0, -1.0],
        [0.0, 1.0, 1.0],
        [0.0, -1.0, 1.0],
        [0.0, 1.0, -1.0],
        [0.0, -1.0, -1.0],
    ];
    let perm = |i: i32| PERM[(i & 255) as usize] as i32;
    const F3: f32 = 1.0 / 3.0;
    const G3: f32 = 1.0 / 6.0;
    let sk = (x + y + z) * F3;
    let (i, j, k) = ((x + sk).floor(), (y + sk).floor(), (z + sk).floor());
    let t = (i + j + k) * G3;
    let (x0, y0, z0) = (x - (i - t), y - (j - t), z - (k - t));
    let (i1, j1, k1, i2, j2, k2) = if x0 >= y0 {
        if y0 >= z0 {
            (1, 0, 0, 1, 1, 0)
        } else if x0 >= z0 {
            (1, 0, 0, 1, 0, 1)
        } else {
            (0, 0, 1, 1, 0, 1)
        }
    } else if y0 < z0 {
        (0, 0, 1, 0, 1, 1)
    } else if x0 < z0 {
        (0, 1, 0, 0, 1, 1)
    } else {
        (0, 1, 0, 1, 1, 0)
    };
    let (ii, jj, kk) = (i as i32 & 255, j as i32 & 255, k as i32 & 255);
    let corner = |di: i32, dj: i32, dk: i32, dx: f32, dy: f32, dz: f32| {
        let t = 0.6 - dx * dx - dy * dy - dz * dz;
        if t < 0.0 {
            return 0.0;
        }
        let g = G[(perm(ii + di + perm(jj + dj + perm(kk + dk))) % 12) as usize];
        let t2 = t * t;
        t2 * t2 * (g[0] * dx + g[1] * dy + g[2] * dz)
    };
    let n0 = corner(0, 0, 0, x0, y0, z0);
    let n1 = corner(
        i1,
        j1,
        k1,
        x0 - i1 as f32 + G3,
        y0 - j1 as f32 + G3,
        z0 - k1 as f32 + G3,
    );
    let n2 = corner(
        i2,
        j2,
        k2,
        x0 - i2 as f32 + 2.0 * G3,
        y0 - j2 as f32 + 2.0 * G3,
        z0 - k2 as f32 + 2.0 * G3,
    );
    let n3 = corner(
        1,
        1,
        1,
        x0 - 1.0 + 3.0 * G3,
        y0 - 1.0 + 3.0 * G3,
        z0 - 1.0 + 3.0 * G3,
    );
    32.0 * (n0 + n1 + n2 + n3)
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
fn kind_is_sphere(e: &Value) -> bool {
    e.get("name").and_then(|n| n.as_str()) == Some("sphererandom")
}

fn rot3(o: &Value, k: &str, default: V3) -> V3 {
    match value::get_floats(o, k) {
        Some(v) if v.len() >= 3 => [v[0], v[1], v[2]],
        Some(v) if v.len() == 1 => [0.0, 0.0, v[0]],
        _ => default,
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
    /// sphererandom: restricts the sphere to a cap around +x (0 = full).
    cone: f32,
    speed: (f32, f32),
    rate: f32,
    instantaneous: u32,
    /// `delay` / `duration` (seconds; duration <= 0 = unlimited).
    delay: f32,
    duration: f32,
    /// `flags` bit 1 (value 2): at most one rate particle per step.
    one_per_step: bool,
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
    Rotation(V3, V3, f32),
    AngularVelocity(V3, V3, f32),
    Turbulent {
        speed: (f32, f32),
        phase: (f32, f32),
        scale: f32,
        timescale: f32,
        offset: f32,
        forward: V3,
        right: V3,
    },
    /// mapsequencearoundcontrolpoint: the n-th particle starts AT the
    /// control point and gets the velocity rotated by 2*pi * (n % count) /
    /// count around z (N even directions). Measured
    /// (`make_particles.py seqcp_v`, control point 0 at (400, 200), emitter
    /// at the center): WE's still dots sit at (394, 227), ours were at the
    /// center. Frieren 3195212886's petals follow the cursor this way.
    SequenceAround {
        count: u32,
        speed: (V3, V3),
        cp: usize,
    },
    /// mapsequencebetweencontrolpoints: the n-th particle is placed at
    /// (n % count) / (count - 1) of the way from control point 0 to 1.
    SequenceBetween {
        count: u32,
    },
}

/// `oscillatealpha` / `oscillatesize` / `oscillateposition` (spec WE-RE-001).
/// Every range is sampled with the particle's single oscillator random
/// (`osc_random`), so frequency, phase and amplitude are correlated, and two
/// oscillators on one particle move in lockstep.
#[derive(Clone, Debug)]
struct Osc {
    freq: (f32, f32),
    scale: (f32, f32),
    phase: (f32, f32),
    envelope: Option<Envelope>,
}

/// Optional fade of an oscillator's effect over normalized lifetime
/// (`blendinstart/end`, `blendoutstart/end`; defaults 0, 0, 1, 1 = inactive).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Envelope {
    in_start: f32,
    in_end: f32,
    out_start: f32,
    out_end: f32,
}

impl Envelope {
    fn parse(o: &Value) -> Option<Self> {
        let get = |k: &str, d: f32| o.get(k).and_then(|v| v.as_f64()).map_or(d, |v| v as f32);
        let in_end = get("blendinend", 0.0);
        let out_start = get("blendoutstart", 1.0);
        let in_start = get("blendinstart", 0.0).min(in_end - 0.0001);
        let mut out_end = get("blendoutend", 1.0);
        if out_end <= out_start + 0.0001 {
            out_end = out_start + 0.0001;
        }
        let (in_w, out_w) = (in_end - in_start, out_end - out_start);
        let active = (in_end > 0.01 || out_start < 0.99)
            && (out_start - in_end > 0.01 || in_w > 0.01 || out_w > 0.01);
        active.then_some(Envelope {
            in_start,
            in_end,
            out_start,
            out_end,
        })
    }

    /// Weight of the oscillator's effect at normalized lifetime `a`.
    fn weight(&self, a: f32) -> f32 {
        let fade_in = ((a - self.in_start) / (self.in_end - self.in_start)).clamp(0.0, 1.0);
        let fade_out = ((self.out_end - a) / (self.out_end - self.out_start)).clamp(0.0, 1.0);
        fade_in * fade_out
    }
}

/// Slot of the per-particle random shared by every oscillator (WE draws one
/// value per particle; it is not per operator).
const OSC_RANDOM_SLOT: u32 = 0xFFFF_0001;

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
        /// phasemax - phasemin (phasemin alone is unused)
        phase: f32,
        envelope: Option<Envelope>,
    },
    Attract {
        cp: usize,
        scale: f32,
        threshold: f32,
        delete_threshold: f32,
        flags: u32,
        envelope: Option<Envelope>,
    },
    Vortex {
        cp: usize,
        offset: V3,
        axis: V3,
        cylinder: bool,
        inner: f32,
        outer: f32,
        speed_inner: f32,
        speed_outer: f32,
    },
    AngularMovement {
        force: V3,
    },
    /// `remapvalue` with `output: velocity` and `operation: remap`: the
    /// velocity is SET to a noise value over the particle's life, mapped into
    /// outputrangemin..max (rain streaks changing speed/direction).
    RemapVelocity {
        min: V3,
        max: V3,
        input_scale: f32,
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
    /// `flags & 1` ("world space"): measured in WE (make_particles.py ws*,
    /// object scaled "1.5 0.08 1" and rotated 0.35 rad): spawn positions and
    /// initial velocities go through the object's transform, then the
    /// simulation runs in scene units (gravity "0 -300 0" falls straight
    /// down the screen instead of being squashed) and the quads are not
    /// scaled or rotated by the object (round dots, 8.6x the light).
    pub world_space: bool,
    /// `flags & 4`: drawn with a perspective camera (depth makes particles
    /// bigger/faster), see `math::scene_perspective`.
    pub perspective: bool,
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
                    // Defaults per WE-RE-003 2.2 (pixel-unit scenes: the
                    // sphere's distancemax is a scalar 256, the box's
                    // "256 256 0").
                    dist_min: v3(e, "distancemin", [0.0; 3]),
                    dist_max: v3(
                        e,
                        "distancemax",
                        if kind_is_sphere(e) {
                            [256.0; 3]
                        } else {
                            [256.0, 256.0, 0.0]
                        },
                    ),
                    directions: v3(e, "directions", [1.0, 1.0, 0.0]),
                    sign: v3(e, "sign", [0.0; 3]),
                    cone: f(e, "cone", 0.0),
                    speed: (f(e, "speedmin", 0.0), f(e, "speedmax", 0.0)),
                    rate: f(e, "rate", 10.0),
                    instantaneous: f(e, "instantaneous", 0.0).max(0.0) as u32,
                    delay: f(e, "delay", 0.0),
                    duration: f(e, "duration", 0.0),
                    one_per_step: (f(e, "flags", 0.0) as u32) & 2 != 0,
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
                        let (a, b, e) = range(i, 0.0, 1.0);
                        Init::Lifetime(a, b, e)
                    }
                    "sizerandom" => {
                        let (a, b, e) = range(i, 5.0, 50.0);
                        Init::Size(a, b, e)
                    }
                    "alpharandom" => {
                        let (a, b, e) = range(i, 0.05, 1.0);
                        Init::Alpha(a, b, e)
                    }
                    "colorrandom" => Init::Color(
                        v3(i, "min", [0.0; 3]),
                        v3(i, "max", [255.0; 3]),
                        f(i, "exponent", 1.0),
                    ),
                    "velocityrandom" => Init::Velocity(
                        v3(i, "min", [-32.0, -32.0, 0.0]),
                        v3(i, "max", [32.0, 32.0, 0.0]),
                        f(i, "exponent", 1.0),
                    ),
                    "rotationrandom" => Init::Rotation(
                        rot3(i, "min", [0.0; 3]),
                        rot3(i, "max", [0.0, 0.0, std::f32::consts::TAU]),
                        f(i, "exponent", 1.0),
                    ),
                    "angularvelocityrandom" => Init::AngularVelocity(
                        v3(i, "min", [0.0, 0.0, -5.0]),
                        v3(i, "max", [0.0, 0.0, 5.0]),
                        f(i, "exponent", 1.0),
                    ),
                    "mapsequencearoundcontrolpoint" => Init::SequenceAround {
                        count: f(i, "count", 1.0).max(1.0) as u32,
                        speed: (
                            v3(i, "speedmin", [0.0; 3]),
                            v3(i, "speedmax", v3(i, "speedmin", [0.0; 3])),
                        ),
                        cp: (f(i, "controlpoint", 0.0).max(0.0) as usize).min(7),
                    },
                    "mapsequencebetweencontrolpoints" => Init::SequenceBetween {
                        count: f(i, "count", 2.0).max(1.0) as u32,
                    },
                    "turbulentvelocityrandom" => Init::Turbulent {
                        speed: (f(i, "speedmin", 100.0), f(i, "speedmax", 250.0)),
                        phase: (f(i, "phasemin", 0.0), f(i, "phasemax", 0.1)),
                        scale: f(i, "scale", 1.0),
                        timescale: f(i, "timescale", 1.0),
                        offset: f(i, "offset", 0.0),
                        forward: v3(i, "forward", [0.0, 1.0, 0.0]),
                        right: v3(i, "right", [0.0, 0.0, 1.0]),
                    },
                    other => {
                        unsupported.push(format!("initializer {other}"));
                        return None;
                    }
                })
            })
            .collect();

        // Defaults per WE-RE-001: frequency 1..10 (1..5 for position).
        let osc = |o: &Value, fmax: f32, smin: f32, smax: f32| Osc {
            freq: (f(o, "frequencymin", 1.0), f(o, "frequencymax", fmax)),
            scale: (f(o, "scalemin", smin), f(o, "scalemax", smax)),
            phase: (
                f(o, "phasemin", 0.0),
                f(o, "phasemax", std::f32::consts::TAU),
            ),
            envelope: Envelope::parse(o),
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
                        to: v3(o, "endvalue", [0.0; 3]),
                    },
                    "oscillatealpha" => Op::OscAlpha(osc(o, 10.0, 0.0, 1.0)),
                    "oscillatesize" => Op::OscSize(osc(o, 10.0, 0.8, 1.2)),
                    "oscillateposition" => {
                        // WE picks 0.5 or 10 for the default `scalemax` by a scene
                        // flag not identified yet (WE-RE-001 open question 1).
                        Op::OscPosition(osc(o, 5.0, 0.0, 10.0), v3(o, "mask", [1.0, 1.0, 0.0]))
                    }
                    // WE-RE-004: defaults of pixel-unit scenes.
                    "turbulence" => Op::Turbulence {
                        speed: (f(o, "speedmin", 500.0), f(o, "speedmax", 1000.0)),
                        scale: f(o, "scale", 0.01),
                        timescale: f(o, "timescale", 20.0),
                        mask: v3(o, "mask", [1.0, 1.0, 0.0]),
                        phase: f(o, "phasemax", 0.0) - f(o, "phasemin", 0.0),
                        envelope: Envelope::parse(o),
                    },
                    "controlpointattract" => Op::Attract {
                        cp: (f(o, "controlpoint", 0.0) as usize).min(7),
                        scale: f(o, "scale", 512.0),
                        threshold: f(o, "threshold", 512.0),
                        delete_threshold: f(o, "deletethreshold", 15.0),
                        flags: f(o, "flags", 2.0) as u32,
                        envelope: Envelope::parse(o),
                    },
                    "vortex" => Op::Vortex {
                        cp: (f(o, "controlpoint", 0.0) as usize).min(7),
                        offset: v3(o, "offset", [0.0; 3]),
                        axis: {
                            let a = v3(o, "axis", [0.0, 0.0, 1.0]);
                            if a[0] * a[0] + a[1] * a[1] + a[2] * a[2] < 0.001 {
                                [0.0, 0.0, 1.0]
                            } else {
                                norm(a)
                            }
                        },
                        cylinder: (f(o, "flags", 0.0) as u32) & 1 != 0,
                        inner: f(o, "distanceinner", 500.0),
                        outer: f(o, "distanceouter", 650.0),
                        speed_inner: f(o, "speedinner", 2500.0),
                        speed_outer: f(o, "speedouter", 0.0),
                    },
                    "angularmovement" => Op::AngularMovement {
                        force: v3(o, "force", [0.0; 3]),
                    },
                    "remapvalue"
                        if o.get("output").and_then(|x| x.as_str()) == Some("velocity")
                            && o.get("operation").and_then(|x| x.as_str()) == Some("remap") =>
                    {
                        Op::RemapVelocity {
                            min: v3(o, "outputrangemin", [0.0; 3]),
                            max: v3(o, "outputrangemax", [0.0; 3]),
                            input_scale: f(o, "transforminputscale", 1.0),
                        }
                    }
                    "remapvalue" => {
                        let out = o.get("output").and_then(|x| x.as_str()).unwrap_or("?");
                        unsupported.push(format!("operator remapvalue -> {out}"));
                        return None;
                    }
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
                    // No `type` = static (rain_screen_4k's static drops are
                    // untyped children and WE draws them).
                    event: match c.get("type").and_then(|t| t.as_str()).unwrap_or("static") {
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
            perspective: (f(j, "flags", 0.0) as u32) & 4 != 0,
            world_space: (f(j, "flags", 0.0) as u32) & 1 != 0,
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

    /// One `instanceoverride` field changed at run time (a SceneScript).
    pub fn set(&mut self, key: &str, v: &Value) {
        let x = value::floats(v);
        let one = x.first().copied();
        match (key, one) {
            ("alpha", Some(a)) => self.alpha = a,
            ("size", Some(a)) => self.size = a,
            ("lifetime", Some(a)) => self.lifetime = a,
            ("rate", Some(a)) => self.rate = a,
            ("speed", Some(a)) => self.speed = a,
            ("count", Some(a)) => self.count = a,
            ("brightness", Some(a)) => self.brightness = a,
            ("colorn", _) if x.len() >= 3 => self.color = Some([x[0], x[1], x[2]]),
            ("color", _) if x.len() >= 3 => {
                self.color = Some([x[0] / 255.0, x[1] / 255.0, x[2] / 255.0])
            }
            _ => {}
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
    /// Position before the last `movement` step (controlpointattract's
    /// swept deletion test).
    prev: V3,
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
    /// `instanceoverride` (SceneScripts may change it while running).
    pub ov: Overrides,
    particles: Vec<Particle>,
    rng: Rng,
    seed: u64,
    emit: Vec<EmitState>,
    pub time: f32,
    next_seed: u32,
    /// Spawn/death events of the last step (consumed by child systems).
    pub events: Vec<Event>,
    /// Current audio spectrum (audio-reactive emitters).
    pub audio: crate::audio::AudioFrame,
    /// World-space systems (`flags & 1`): the object's transform, set by the
    /// scene every frame. Spawns go through it; the simulation then runs in
    /// scene units (see ParticleDef::world_space).
    pub world_xform: Option<[f32; 16]>,
    /// Set while the prewarm (`starttime`) runs: it ignores
    /// `instanceoverride.rate` (see scene.rs).
    pub prewarming: bool,
}

/// Per-emitter emission state (WE-RE-003 1.2).
#[derive(Clone, Copy, Debug)]
struct EmitState {
    acc: f32,
    burst_left: u32,
    delay_left: f32,
    duration_left: f32,
    enabled: bool,
}

impl EmitState {
    fn new(e: &Emitter) -> EmitState {
        EmitState {
            acc: 0.0,
            burst_left: e.instantaneous,
            delay_left: e.delay,
            duration_left: e.duration,
            enabled: true,
        }
    }
}

/// See `spawn`: WE's rendered particle size relative to `sizerandom`.
const SIZE_SCALE: f32 = 0.5;

/// Fixed simulation step (seconds).
pub const STEP: f32 = 1.0 / 60.0;

impl ParticleSim {
    pub fn new(def: ParticleDef, ov: Overrides, seed: u64) -> ParticleSim {
        let emit = def.emitters.iter().map(EmitState::new).collect();
        ParticleSim {
            def,
            ov,
            particles: Vec::new(),
            rng: Rng::new(seed),
            seed,
            emit,
            time: 0.0,
            next_seed: 1,
            events: Vec::new(),
            audio: crate::audio::AudioFrame::default(),
            world_xform: None,
            prewarming: false,
        }
    }

    pub fn reset(&mut self) {
        self.particles.clear();
        self.rng = Rng::new(self.seed);
        self.emit = self.def.emitters.iter().map(EmitState::new).collect();
        self.time = 0.0;
        self.next_seed = 1;
        self.events.clear();
    }

    /// Capacity: `maxcount` as written. `instanceoverride.count` scales the
    /// emission rate instead (calibration ov_count / ov_count2: count 0.5
    /// halves an uncapped stream in WE but leaves a capped one at maxcount).
    pub fn max_count(&self) -> usize {
        self.def.max_count as usize
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
                // WE-RE-003 2.3: a point uniform in the VOLUME of the unit
                // ball (cone axis = x), squashed by `directions` before its
                // length is taken; with distancemin 0 the offset is
                // distancemax * p, i.e. the ball projected onto the plane
                // (denser in the middle than a disc).
                EmitterKind::Sphere => {
                    let phi = std::f32::consts::TAU * rng_b(rng);
                    let k = -(std::f32::consts::PI * em.cone).cos();
                    let c = k + (1.0 - k) * rng.f();
                    let rho = rng_b(rng).cbrt();
                    let sn = (1.0 - c * c).max(0.0).sqrt();
                    let pt = mul3(
                        [rho * c, rho * sn * phi.sin(), rho * sn * phi.cos()],
                        em.directions,
                    );
                    let len = len(pt);
                    let mut d = if len > 0.0 {
                        scale(pt, 1.0 / len)
                    } else {
                        [0.0; 3]
                    };
                    for (k, dk) in d.iter_mut().enumerate() {
                        if em.sign[k] != 0.0 && dk.signum() != em.sign[k].signum() {
                            *dk = -*dk;
                        }
                    }
                    let o = scale(d, em.dist_min[0] + (em.dist_max[0] - em.dist_min[0]) * len);
                    pos = add(o, em.origin);
                    vel = launch(rng, o, em);
                }
                // WE-RE-003 2.4: each axis independent, uniform on
                // +-[min, min + dir * (max - min)].
                EmitterKind::Box => {
                    let mut v = [0.0f32; 3];
                    for k in [2, 1, 0] {
                        v[k] = (2.0 * rng_b(rng) - 1.0) * em.directions[k];
                    }
                    let o = [0, 1, 2].map(|k| {
                        let a = v[k].abs();
                        let m = em.dist_min[k] + a * (em.dist_max[k] - em.dist_min[k]);
                        if v[k] == 0.0 { 0.0 } else { m.copysign(v[k]) }
                    });
                    pos = add(o, em.origin);
                    vel = launch(rng, o, em);
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
            // WE-RE-003 3.1: base size 0.5 (here 1.0, halved by SIZE_SCALE
            // below), lifetime 1 s; initializers multiply / add to these.
            size: 1.0,
            rot: [0.0; 3],
            ang_vel: [0.0; 3],
            age: 0.0,
            lifetime: 1.0,
            seed: self.next_seed,
            frame_phase: 0.0,
            child_acc: 0.0,
            prev: pos,
            trail: Vec::new(),
            trail_t: 0.0,
            draw_alpha: 1.0,
            draw_size: 20.0,
            draw_color: [1.0; 3],
            osc_offset: [0.0; 3],
        };
        self.next_seed = self.next_seed.wrapping_add(1);
        let now = self.time;
        // WE-RE-003 0.2: the draw is raised to `exponent` unless it is 1.
        let biased = |rng: &mut Rng, e: f32| {
            let u = rng.f();
            if e == 1.0 { u } else { u.powf(e) }
        };
        for init in &self.def.inits {
            match *init {
                Init::Lifetime(a, b, e) => p.lifetime = lerp(a, b, biased(rng, e)).max(0.001),
                Init::Size(a, b, e) => p.size *= lerp(a, b, biased(rng, e)),
                Init::Alpha(a, b, e) => p.alpha *= lerp(a, b, biased(rng, e)),
                // one draw for all channels: a random grey level by default
                Init::Color(a, b, e) => {
                    p.color = mul3(p.color, scale(lerp3(a, b, biased(rng, e)), 1.0 / 255.0))
                }
                Init::Velocity(a, b, e) => {
                    let mut v = [0.0; 3];
                    for k in [2, 1, 0] {
                        v[k] = lerp(a[k], b[k], biased(rng, e));
                    }
                    p.vel = add(p.vel, v);
                }
                Init::Rotation(a, b, e) => {
                    for k in 0..3 {
                        p.rot[k] += lerp(a[k], b[k], biased(rng, e));
                    }
                }
                Init::AngularVelocity(a, b, e) => {
                    for k in 0..3 {
                        p.ang_vel[k] += lerp(a[k], b[k], biased(rng, e));
                    }
                }
                Init::SequenceAround { count, speed, cp } => {
                    p.pos = add(p.pos, cps[cp]);
                    let k = (p.seed.wrapping_sub(1) % count) as f32;
                    let a = std::f32::consts::TAU * k / count as f32;
                    let v = lerp3(speed.0, speed.1, rng.f());
                    let (sa, ca) = a.sin_cos();
                    p.vel = add(p.vel, [v[0] * ca - v[1] * sa, v[0] * sa + v[1] * ca, v[2]]);
                }
                Init::SequenceBetween { count } => {
                    let k = (p.seed.wrapping_sub(1) % count) as f32;
                    let t = if count > 1 {
                        k / (count - 1) as f32
                    } else {
                        0.0
                    };
                    let (a, b) = (cps[0], cps[1]);
                    p.pos = add(p.pos, [0, 1, 2].map(|c| a[c] + (b[c] - a[c]) * t));
                }
                // WE-RE-003 3.5: `forward` rotated around `right` by an angle
                // from 1-D noise of the clock, so particles spawned close in
                // time share a direction.
                Init::Turbulent {
                    speed,
                    phase,
                    scale: s,
                    timescale,
                    offset,
                    forward,
                    right,
                } => {
                    let t = (lerp(phase.0, phase.1, rng.f()) + now) * timescale;
                    let angle = noise1(t) * std::f32::consts::PI * s + offset;
                    let d = rotate_axis(forward, norm(right), angle);
                    let sp = rng.range(speed.0, speed.1);
                    p.vel = add(p.vel, scale(d, sp));
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
        // The instance color REPLACES the initializer's (two scenes agree:
        // wind v7 2884352976 draws its orange `Light shafts` lavender with
        // colorn 0.435 0.424 0.525; rain 1444077782's gray smoke is white with
        // colorn 1 1 1, matching WE's brightness only when replaced) and is
        // applied a second time on top, i.e. colorn^2. Calibrated in
        // isolation with make_particles.py `ov_colorn05`: white dots with
        // colorn 0.5 give 0.25x the light in WE (0.020 vs 0.080), while
        // `ov_alpha05` gives 0.5x, so alpha applies once. Mai Sakurajima
        // 1982790211's Light shafts (colorn 0.10 0.27 0.39) dim by 0.166x in
        // WE (colorn^2 predicts 0.163, replacing alone 0.6).
        if let Some(c) = self.ov.color {
            p.color = [c[0] * c[0], c[1] * c[1], c[2] * c[2]];
        }
        p.color = scale(p.color, self.ov.brightness);
        p.frame_phase = rng.f();
        if let (Some(m), None) = (self.world_xform, at) {
            p.pos = xform_point(&m, p.pos);
            p.vel = xform_dir(&m, p.vel);
        }
        p.prev = p.pos;
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
        self.def.emitters.first().map(|e| e.rate).unwrap_or(0.0) * self.ov.rate * self.ov.count
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
        // WE-RE-002 1.2: `instanceoverride.rate` scales the system's dt
        // (emission, ageing and operators alike), at least 0.01.
        // `self.time` is the scene clock (the caller steps until it reaches
        // the frame time), so it advances by the UNSCALED dt below.
        let frame_dt = dt;
        let dt = if self.prewarming {
            dt
        } else {
            dt * self.ov.rate.max(0.01)
        };
        // WE-RE-002 1.3: age and remove expired particles BEFORE emitting, so
        // new particles meet their first operator pass at age 0.
        let mut deaths = Vec::new();
        self.particles.retain_mut(|p| {
            p.age += dt;
            if p.age > p.lifetime {
                deaths.push(p.pos);
                return false;
            }
            true
        });
        if emit {
            // WE-RE-003 1.3 / 1.4 (periodic mode is not used by the corpus).
            for ei in 0..self.def.emitters.len() {
                let em = &self.def.emitters[ei];
                let room = self.particles.len() < self.max_count();
                let st = &mut self.emit[ei];
                let mut n = 0u32;
                // Gate: while full, delayed or disabled nothing accumulates,
                // so a pending burst waits for room.
                if room && st.enabled && st.delay_left <= 0.0 {
                    let gain = em
                        .audio
                        .map(|(mode, s, e, b, x)| self.audio.response(mode, s, e, b, x))
                        .unwrap_or(1.0);
                    n = std::mem::take(&mut st.burst_left);
                    let a = st.acc + em.rate * self.ov.count * gain * dt;
                    // The burst is SUBTRACTED from the accumulator: after a
                    // burst of N the continuous stream pauses ~N / rate s.
                    if a < 1.0 || st.duration_left < 0.0 {
                        st.acc = a - n as f32;
                    } else {
                        let mut k = a.floor();
                        st.acc = a - (k + n as f32);
                        if em.one_per_step {
                            k = k.min(1.0); // surplus discarded
                        }
                        n += k as u32;
                    }
                }
                if st.delay_left > 0.0 {
                    st.delay_left -= dt;
                } else if st.duration_left > 0.0 {
                    st.duration_left -= dt;
                    if st.duration_left <= 0.0 {
                        st.duration_left = -1.0;
                        st.enabled = false;
                    }
                } else if em.rate <= 0.0 {
                    // a pure burst emitter fires once, then shuts off
                    st.enabled = false;
                }
                // excess beyond maxcount is lost (spawn checks the room)
                for _ in 0..n {
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
        // WE-RE-002 1.6: `instanceoverride.speed` scales gravity and the
        // oscillateposition frequency.
        let speed = self.ov.speed;
        // WE-RE-004: turbulence / vortex / attract speeds scale with
        // instanceoverride.speed, the turbulence timescale with rate.
        let (ov_speed, ov_rate) = (self.ov.speed, self.ov.rate);
        let ov_size = self.ov.size;
        let def_cps = &self.def.control_points;
        for p in self.particles.iter_mut() {
            let life = p.age / p.lifetime;
            let (mut alpha, mut size, mut color) = (p.alpha, p.size, p.color);
            // Accumulated, like WE, which moves the particle by the change
            // of the wave each step instead of setting an absolute offset.
            let mut osc_offset = p.osc_offset;
            let osc_r = hash01(p.seed, OSC_RANDOM_SLOT);
            for (k, op) in ops.iter().enumerate() {
                let slot = (k as u32) * 4;
                let osc_at = |o: &Osc, age: f32| {
                    let f = lerp(o.freq.0, o.freq.1, osc_r);
                    let phase = lerp(o.phase.0, o.phase.1, osc_r);
                    // angular frequency, phase added to time (WE-RE-001 1.5)
                    (f * (age + phase)).sin()
                };
                // Multiplier for alpha / size: the wave's height is scaled by
                // the same random, so only particles with r near 1 reach
                // scalemax. No clamping.
                let osc_mul = |o: &Osc| {
                    let s = osc_at(o, p.age);
                    let m = o.scale.0 + osc_r * (o.scale.1 - o.scale.0) * (s + 1.0) * 0.5;
                    match o.envelope {
                        Some(e) => 1.0 - e.weight(life) * (1.0 - m),
                        None => m,
                    }
                };
                match *op {
                    // WE-RE-002 2: the only place velocity moves a particle
                    // (without `movement`, initial velocity does nothing).
                    // Position advances with the post-gravity, PRE-drag
                    // velocity; drag is linear per step.
                    Op::Movement { gravity, drag } => {
                        p.prev = p.pos;
                        let v = add(p.vel, scale(gravity, dt * speed));
                        p.pos = add(p.pos, scale(v, dt));
                        let k = (drag * dt).min(0.999_999_9);
                        p.vel = scale(v, 1.0 - k);
                    }
                    Op::AlphaFade { fade_in, fade_out } => {
                        if fade_in > 0.0 && life < fade_in {
                            alpha *= life / fade_in;
                        }
                        if life > fade_out && fade_out < 1.0 {
                            alpha *= 1.0 - (life - fade_out) / (1.0 - fade_out);
                        }
                    }
                    // instanceoverride.size also scales sizechange's values
                    // (calibration sc_start07_ov: size 0.5 -> 1/16 of the
                    // light, not 1/4; WE 0.101, linear-only 0.414).
                    Op::SizeChange(r) => size *= r.at(life) * ov_size,
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
                    Op::OscAlpha(ref o) => alpha *= osc_mul(o),
                    Op::OscSize(ref o) => size *= osc_mul(o),
                    Op::OscPosition(ref o, mask) => {
                        // Raw sine (no 0..1 remap). x and z share the wave; y
                        // is shifted by 2*pi*r in time (WE-RE-001 4.3).
                        let amp = lerp(o.scale.0, o.scale.1, osc_r)
                            * o.envelope.map_or(1.0, |e| e.weight(life));
                        let f = lerp(o.freq.0, o.freq.1, osc_r) * speed;
                        let phase = lerp(o.phase.0, o.phase.1, osc_r);
                        let wave = |age: f32| (f * (age + phase)).sin();
                        let step = |age: f32| wave(age) - wave(age - dt);
                        let y_shift = std::f32::consts::TAU * osc_r;
                        let (d_xz, d_y) = (step(p.age), step(p.age + y_shift));
                        osc_offset = add(osc_offset, scale(mul3([d_xz, d_y, d_xz], mask), amp));
                    }
                    // WE-RE-004 1: signed simplex noise of the position
                    // (+ clock along the diagonal) ACCELERATES the particle;
                    // speed and phase come from the shared per-particle r.
                    Op::Turbulence {
                        speed: sp,
                        scale: s,
                        timescale,
                        mask,
                        phase,
                        envelope,
                    } => {
                        let w = envelope.map_or(1.0, |e| e.weight(life));
                        let sp = lerp(sp.0, sp.1, osc_r) * ov_speed * w;
                        let tau = osc_r * phase + time * timescale * ov_rate;
                        let q = [0, 1, 2].map(|k| s * (p.pos[k] + tau));
                        let n = [
                            if mask[0] != 0.0 {
                                simplex3(q[0], q[1], q[2])
                            } else {
                                0.0
                            },
                            if mask[1] != 0.0 {
                                simplex3(q[2], q[0], q[1])
                            } else {
                                0.0
                            },
                            if mask[2] != 0.0 {
                                simplex3(q[1], q[2], q[0])
                            } else {
                                0.0
                            },
                        ];
                        p.vel = add(p.vel, scale(mul3(n, mask), sp * dt));
                    }
                    // WE-RE-004 3: linear pull inside `threshold` (negative
                    // scale repels), flags 2 = no overshoot, flags 1 = kill
                    // particles that pass within deletethreshold.
                    Op::Attract {
                        cp,
                        scale: s,
                        threshold,
                        delete_threshold,
                        flags,
                        envelope,
                    } => {
                        let target = cps[cp];
                        let delta = sub(p.pos, target);
                        let d = len(delta);
                        if d > f32::MIN_POSITIVE && d < threshold {
                            let mut f = (1.0 - d / threshold) * s * ov_speed * dt;
                            if flags & 2 != 0 && d < f {
                                f = d;
                            }
                            f *= envelope.map_or(1.0, |e| e.weight(life));
                            p.vel = sub(p.vel, scale(delta, f / d));
                        }
                        if flags & 1 != 0 {
                            let seg = sub(p.pos, p.prev);
                            let sl = len(seg);
                            let closest = if sl > 0.0 {
                                let u = scale(seg, 1.0 / sl);
                                let t = dot(sub(target, p.prev), u).clamp(0.0, sl);
                                add(p.prev, scale(u, t))
                            } else {
                                p.prev
                            };
                            let c = sub(target, closest);
                            if dot(c, c) <= delete_threshold * delete_threshold {
                                p.age = p.lifetime;
                            }
                        }
                    }
                    // WE-RE-004 2: tangential push s * (n x axis); positive
                    // speed turns clockwise seen from +axis.
                    Op::Vortex {
                        cp,
                        offset,
                        axis,
                        cylinder,
                        inner,
                        outer,
                        speed_inner,
                        speed_outer,
                    } => {
                        let c = add(cps[cp], offset);
                        let mut v = sub(p.pos, c);
                        if cylinder {
                            v = sub(v, scale(axis, dot(v, axis)));
                        }
                        let rho = len(v);
                        if rho > 0.0 {
                            let n = scale(v, 1.0 / rho);
                            let width = outer - inner;
                            let t = if width != 0.0 {
                                ((rho - inner) / width).clamp(0.0, 1.0)
                            } else {
                                (rho - inner).clamp(0.0, 1.0)
                            };
                            let sp = lerp(speed_inner, speed_outer, t) * ov_speed * dt;
                            let tangent = [
                                n[1] * axis[2] - n[2] * axis[1],
                                n[2] * axis[0] - n[0] * axis[2],
                                n[0] * axis[1] - n[1] * axis[0],
                            ];
                            p.vel = add(p.vel, scale(tangent, sp));
                        }
                    }
                    Op::AngularMovement { force } => {
                        p.ang_vel = add(p.ang_vel, scale(force, dt));
                        p.rot = add(p.rot, scale(p.ang_vel, dt));
                    }
                    Op::RemapVelocity {
                        min,
                        max,
                        input_scale,
                    } => {
                        // Calibrated with make_particles.py `rv_*` (one dot,
                        // x range -200..200, simplexnoise):
                        // - the noise input is the particle's AGE IN SECONDS
                        //   x inputscale, not its life fraction: WE's sign
                        //   changes/s stay ~2 when the lifetime goes 3 -> 6 s
                        //   (2.36 -> 1.89) and halve with inputscale 10 -> 5
                        //   (1.18). With this noise, age x scale / 2.4 matches
                        //   the rate (fitted over the three runs);
                        // - WE sweeps almost the whole range (velocity std 130
                        //   of +-200), wider than this noise: x2.3, clamped.
                        // A constant range just sets the velocity (100 vs 104).
                        let x = p.age * input_scale / 2.4 + hash01(p.seed, slot) * 97.0;
                        let n = noise_vec([x, 0.5, 0.5]);
                        for c in 0..3 {
                            let t = (n[c] * 2.3).clamp(-1.0, 1.0) * 0.5 + 0.5;
                            p.vel[c] = lerp(min[c], max[c], t);
                        }
                    }
                }
            }
            let _ = def_cps;
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
        }
        for pos in deaths {
            self.events.push(Event {
                pos,
                kind: ChildEvent::Death,
            });
        }
        self.time += frame_dt;
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
/// Column-major 4x4 transform of a point / a direction.
pub fn xform_point(m: &[f32; 16], p: V3) -> V3 {
    [0, 1, 2].map(|r| m[r] * p[0] + m[4 + r] * p[1] + m[8 + r] * p[2] + m[12 + r])
}

pub fn xform_dir(m: &[f32; 16], d: V3) -> V3 {
    [0, 1, 2].map(|r| m[r] * d[0] + m[4 + r] * d[1] + m[8 + r] * d[2])
}

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

    #[test]
    fn oscillator_envelope_defaults_are_inactive() {
        assert_eq!(Envelope::parse(&json!({})), None);
        assert_eq!(
            Envelope::parse(&json!({"blendinstart": 0, "blendinend": 0})),
            None
        );
    }

    #[test]
    fn oscillator_envelope_fades_in_and_out() {
        let e = Envelope::parse(&json!({"blendinend": 0.2, "blendoutstart": 0.8})).unwrap();
        assert_eq!(e.weight(0.0), 0.0);
        assert!((e.weight(0.1) - 0.5).abs() < 1e-3);
        assert_eq!(e.weight(0.5), 1.0);
        assert!(e.weight(0.9999) < 1e-3);
    }

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
    fn turbulence_noise_is_signed_with_zero_mean() {
        // WE-RE-004 1.4: a drift in the cloud means a biased noise.
        let (mut sum, mut max, mut n) = (0.0f64, 0.0f32, 0);
        for i in 0..40 {
            for j in 0..40 {
                for k in 0..10 {
                    let v = simplex3(i as f32 * 0.37, j as f32 * 0.41 - 7.0, k as f32 * 0.53);
                    sum += v as f64;
                    max = max.max(v.abs());
                    n += 1;
                }
            }
        }
        assert_eq!(simplex3(0.0, 0.0, 0.0), 0.0);
        assert!((sum / n as f64).abs() < 0.02, "mean {}", sum / n as f64);
        assert!(max <= 1.0 && max > 0.5, "max {max}");
    }

    #[test]
    fn burst_pauses_the_stream_and_flag_2_caps_one_per_step() {
        let cps = [[0.0; 3]; 8];
        // WE-RE-003 1.3: a burst of 10 drives the accumulator to about -10,
        // so a 10/s stream pauses ~1 s after it.
        let mut s = sim(json!({
            "maxcount": 100,
            "emitter": [{ "name": "boxrandom", "rate": 10, "instantaneous": 10 }],
            "initializer": [{ "name": "lifetimerandom", "min": 100, "max": 100 }]
        }));
        for _ in 0..30 {
            s.step(STEP, &cps, true); // 0.5 s
        }
        assert_eq!(s.len(), 10, "only the burst during the pause");
        for _ in 0..60 {
            s.step(STEP, &cps, true); // 1.5 s total
        }
        assert!((14..=16).contains(&s.len()), "{}", s.len());
        // flags 2: at most one rate particle per step (60/s here).
        let mut s = sim(json!({
            "maxcount": 1000,
            "emitter": [{ "name": "boxrandom", "rate": 600, "flags": 2 }],
            "initializer": [{ "name": "lifetimerandom", "min": 100, "max": 100 }]
        }));
        for _ in 0..60 {
            s.step(STEP, &cps, true);
        }
        assert_eq!(s.len(), 60);
    }

    #[test]
    fn instance_count_scales_the_rate_not_maxcount() {
        let def = ParticleDef::parse(&json!({
            "maxcount": 20,
            "emitter": [{ "name": "boxrandom", "rate": 100 }],
            "initializer": [{ "name": "lifetimerandom", "min": 100, "max": 100 }]
        }));
        let ov = Overrides::parse(Some(&json!({ "count": 0.5 })));
        let mut s = ParticleSim::new(def, ov, 1);
        let cps = [[0.0; 3]; 8];
        for _ in 0..12 {
            s.step(STEP, &cps, true); // 0.2 s at 50/s -> 10
        }
        assert_eq!(s.len(), 10);
        for _ in 0..120 {
            s.step(STEP, &cps, true);
        }
        assert_eq!(s.len(), 20, "capacity stays maxcount");
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
    fn sequence_around_spreads_in_even_directions() {
        let mut s = sim(json!({
            "maxcount": 10,
            "emitter": [{ "name": "sphererandom", "rate": 0, "instantaneous": 4 }],
            "initializer": [
                { "name": "lifetimerandom", "min": 100, "max": 100 },
                { "name": "mapsequencearoundcontrolpoint", "count": 4, "speedmin": "0 100 0", "speedmax": "0 100 0" }
            ]
        }));
        let cps = [[0.0; 3]; 8];
        s.step(STEP, &cps, true);
        let mut dirs: Vec<(i32, i32)> = s
            .particles
            .iter()
            .map(|p| (p.vel[0].round() as i32, p.vel[1].round() as i32))
            .collect();
        dirs.sort();
        assert_eq!(dirs, vec![(-100, 0), (0, -100), (0, 100), (100, 0)]);
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
