// Puppet warp: skeletal animation of an image layer (rustpaper_core::mdl).
//
// The layer's quad is replaced by the puppet mesh. Each frame, every bone gets
// its interpolated key from the active animation layer(s), poses are chained
// through the parents, and vertices are skinned on the CPU:
//   v' = sum_i weight_i * (pose_world_i * bind_world_i^-1) * v
// (a few thousand vertices at most, so the CPU is plenty and the WE material
// shader is used unchanged, without SKINNING).

use rustpaper_core::mdl::{Animation, Key, PlayMode, Puppet};
use serde_json::Value;

use crate::math::{self, Mat4};
use crate::value;

/// One entry of the scene object's `animationlayers`.
#[derive(Clone, Debug)]
struct AnimLayer {
    anim: usize,
    /// Index in the object's `animationlayers` (SceneScripts address it).
    json_index: usize,
    rate: f32,
    /// Animation time: accumulates `dt * rate`, so a rate a script changes
    /// while playing doesn't make the animation jump.
    phase: f32,
    /// `visible` (hidden layers are kept: a SceneScript may show them).
    visible: bool,
    blend: f32,
    additive: bool,
}

pub struct PuppetRt {
    pub puppet: Puppet,
    bind_inv: Vec<Mat4>,
    /// Every bone's bind (rest) local transform as a key: what additive
    /// layers are relative to.
    bind_keys: Vec<Key>,
    layers: Vec<AnimLayer>,
    /// Interleaved (x, y, z, u, v) of the current pose, for the mesh buffer.
    pub vertices: Vec<f32>,
    /// World (model-space) matrix of every bone in the current pose; the
    /// bind pose when nothing animates. Attachments hang from these.
    pub bone_world: Vec<Mat4>,
    bind_world: Vec<Mat4>,
    /// Scene time of the last `update`.
    last_time: Option<f32>,
}

/// Multiplier from an animation's stored rate to frames per second. The
/// field reads as fps in MDLA0003 (30 for the editor default); reading it as
/// a duration instead (fps = frames / value) matched WE captures worse
/// (docs/GL_RENDERER.md, "Puppet warp").
pub const FPS_SCALE: f32 = 1.0;

impl PuppetRt {
    /// Model-space matrix of the attachment named `name` in the current pose.
    pub fn attachment(&self, name: &str) -> Option<Mat4> {
        let a = self.puppet.attachments.iter().find(|a| a.name == name)?;
        let bone = self
            .bone_world
            .get(a.bone)
            .copied()
            .unwrap_or(math::IDENTITY);
        Some(math::mul(&bone, &a.local))
    }

    pub fn new(puppet: Puppet, animation_layers: Option<&Value>) -> PuppetRt {
        let bind_world = world_poses(&puppet, |b| puppet.bones[b].local);
        let bind_inv = bind_world.iter().map(math::invert).collect();
        let bind_keys = puppet.bones.iter().map(|b| matrix_key(&b.local)).collect();
        let bone_world = bind_world.clone();
        let layers = animation_layers
            .and_then(|l| l.as_array())
            .into_iter()
            .flatten()
            .enumerate()
            .filter_map(|(json_index, l)| {
                let id = l.get("animation")?.as_u64()? as u32;
                Some(AnimLayer {
                    anim: puppet.animations.iter().position(|a| a.id == id)?,
                    json_index,
                    rate: value::get_f32(l, "rate", 1.0),
                    phase: 0.0,
                    visible: value::get_bool(l, "visible", true),
                    blend: value::get_f32(l, "blend", 1.0),
                    additive: value::get_bool(l, "additive", false),
                })
            })
            .collect();
        let mut rt = PuppetRt {
            puppet,
            bind_inv,
            bind_keys,
            layers,
            vertices: Vec::new(),
            bone_world,
            bind_world,
            last_time: None,
        };
        rt.update(0.0);
        rt
    }

    /// `animationlayers[json_index].visible`, changed by a SceneScript.
    pub fn set_visible(&mut self, json_index: usize, shown: bool) {
        for l in self
            .layers
            .iter_mut()
            .filter(|l| l.json_index == json_index)
        {
            l.visible = shown;
        }
    }

    /// `animationlayers[json_index].rate`, changed by a SceneScript.
    pub fn set_rate(&mut self, json_index: usize, rate: f32) {
        for l in self
            .layers
            .iter_mut()
            .filter(|l| l.json_index == json_index)
        {
            l.rate = rate;
        }
    }

    /// Poses the skeleton at `time` seconds and skins the mesh.
    pub fn update(&mut self, time: f32) {
        // a jump (first frame, seeking back) restarts the phase from time
        match self.last_time {
            Some(t0) if time >= t0 && time - t0 < 1.0 => {
                for l in &mut self.layers {
                    l.phase += (time - t0) * l.rate;
                }
            }
            _ => {
                for l in &mut self.layers {
                    l.phase = time * l.rate;
                }
            }
        }
        self.last_time = Some(time);
        let p = &self.puppet;
        let poses = if p.bones.is_empty() || !self.layers.iter().any(|l| l.visible) {
            None
        } else {
            // local transform per bone: bind pose, replaced by the animation
            // key of a non-additive layer, additive layers on top. An
            // additive layer adds its key's difference to the BIND pose, not
            // to its own first key: Lucy 3521337568's intro layer (single,
            // root key 0.8x and offset at frame 0, the bind pose at its last
            // frame) must end on the bind pose. Relative to its first key it
            // ended 1.25x bigger and shifted up-left (mean SSIM 0.61).
            let mut keys: Vec<Option<Key>> = vec![None; p.bones.len()];
            for l in self.layers.iter().filter(|l| l.visible) {
                let a = &p.animations[l.anim];
                for (b, slot) in keys.iter_mut().enumerate() {
                    let Some(k) = sample(a, b, l.phase) else {
                        continue;
                    };
                    *slot = Some(match (*slot, l.additive) {
                        (Some(base), true) => add_key(base, k, self.bind_keys[b], l.blend),
                        (None, true) => add_key(self.bind_keys[b], k, self.bind_keys[b], l.blend),
                        (Some(base), false) => mix_key(base, k, l.blend),
                        (None, false) => k,
                    });
                }
            }
            Some(world_poses(p, |b| match keys[b] {
                Some(k) => key_matrix(&k),
                None => p.bones[b].local,
            }))
        };
        self.bone_world = poses.clone().unwrap_or_else(|| self.bind_world.clone());
        let p = &self.puppet;
        let skins: Vec<Mat4> = match &poses {
            Some(w) => w
                .iter()
                .zip(&self.bind_inv)
                .map(|(w, i)| math::mul(w, i))
                .collect(),
            None => Vec::new(),
        };
        let m = &p.mesh;
        self.vertices.clear();
        for v in 0..m.positions.len() {
            let pos = m.positions[v];
            let mut out = [0.0f32; 3];
            let mut total = 0.0;
            if !skins.is_empty() {
                for i in 0..4 {
                    let (b, w) = (m.bones[v][i] as usize, m.weights[v][i]);
                    if w <= 0.0 || b >= skins.len() {
                        continue;
                    }
                    let s = &skins[b];
                    for (k, o) in out.iter_mut().enumerate() {
                        *o +=
                            w * (s[k] * pos[0] + s[4 + k] * pos[1] + s[8 + k] * pos[2] + s[12 + k]);
                    }
                    total += w;
                }
            }
            let pos = if total > 0.0 {
                out.map(|c| c / total)
            } else {
                pos
            };
            let uv = m.uvs[v];
            self.vertices
                .extend_from_slice(&[pos[0], pos[1], pos[2], uv[0], uv[1]]);
        }
    }
}

/// World matrix of every bone from per-bone local matrices.
fn world_poses(p: &Puppet, local: impl Fn(usize) -> Mat4) -> Vec<Mat4> {
    let mut world: Vec<Option<Mat4>> = vec![None; p.bones.len()];
    fn resolve(
        b: usize,
        p: &Puppet,
        local: &dyn Fn(usize) -> Mat4,
        world: &mut [Option<Mat4>],
        depth: u32,
    ) -> Mat4 {
        if let Some(m) = world[b] {
            return m;
        }
        let m = match p.bones[b].parent {
            Some(pa) if pa < p.bones.len() && pa != b && depth < 64 => {
                math::mul(&resolve(pa, p, local, world, depth + 1), &local(b))
            }
            _ => local(b),
        };
        world[b] = Some(m);
        m
    }
    (0..p.bones.len())
        .map(|b| resolve(b, p, &local, &mut world, 0))
        .collect()
}

/// Interpolated key of `bone` at `t` seconds, honoring the play mode.
fn sample(a: &Animation, bone: usize, t: f32) -> Option<Key> {
    let track = a.tracks.get(bone)?;
    if track.is_empty() || a.frames == 0 {
        return track.first().copied();
    }
    let n = a.frames as f32;
    let fps = a.fps * FPS_SCALE;
    let f = (t * fps).max(0.0);
    let f = match a.mode {
        PlayMode::Loop => f % n,
        PlayMode::Mirror => {
            let c = f % (2.0 * n);
            if c > n { 2.0 * n - c } else { c }
        }
        PlayMode::Single => f.min(n),
    };
    let i = (f.floor() as usize).min(track.len() - 1);
    let j = (i + 1).min(track.len() - 1);
    Some(mix_key(track[i], track[j], f - f.floor()))
}

fn mix_key(a: Key, b: Key, t: f32) -> Key {
    let l = |x: [f32; 3], y: [f32; 3]| [0, 1, 2].map(|k| x[k] + (y[k] - x[k]) * t);
    Key {
        position: l(a.position, b.position),
        rotation: l(a.rotation, b.rotation),
        scale: l(a.scale, b.scale),
    }
}

/// Additive layer: its change relative to the bind pose `rest`, on top of base.
fn add_key(base: Key, k: Key, rest: Key, w: f32) -> Key {
    let d = |b: [f32; 3], x: [f32; 3], r: [f32; 3]| [0, 1, 2].map(|i| b[i] + (x[i] - r[i]) * w);
    Key {
        position: d(base.position, k.position, rest.position),
        rotation: d(base.rotation, k.rotation, rest.rotation),
        scale: [0, 1, 2]
            .map(|i| base.scale[i] * (1.0 + (k.scale[i] / rest.scale[i].max(1e-6) - 1.0) * w)),
    }
}

/// Inverse of `key_matrix` for a bone's bind matrix (no shear): column
/// lengths are the scale, Euler angles from the normalized rotation.
fn matrix_key(m: &Mat4) -> Key {
    let len = |c: usize| (m[c] * m[c] + m[c + 1] * m[c + 1] + m[c + 2] * m[c + 2]).sqrt();
    let scale = [len(0), len(4), len(8)];
    let r = |row: usize, col: usize| m[col * 4 + row] / scale[col].max(1e-12);
    let ry = (-r(2, 0)).clamp(-1.0, 1.0).asin();
    Key {
        position: [m[12], m[13], m[14]],
        rotation: [r(2, 1).atan2(r(2, 2)), ry, r(1, 0).atan2(r(0, 0))],
        scale,
    }
}

/// T * Rz * Ry * Rx * S (column-major).
fn key_matrix(k: &Key) -> Mat4 {
    let (sx, cx) = k.rotation[0].sin_cos();
    let (sy, cy) = k.rotation[1].sin_cos();
    let (sz, cz) = k.rotation[2].sin_cos();
    let rz: Mat4 = [
        cz, sz, 0.0, 0.0, -sz, cz, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
    ];
    let ry: Mat4 = [
        cy, 0.0, -sy, 0.0, 0.0, 1.0, 0.0, 0.0, sy, 0.0, cy, 0.0, 0.0, 0.0, 0.0, 1.0,
    ];
    let rx: Mat4 = [
        1.0, 0.0, 0.0, 0.0, 0.0, cx, sx, 0.0, 0.0, -sx, cx, 0.0, 0.0, 0.0, 0.0, 1.0,
    ];
    let s: Mat4 = [
        k.scale[0], 0.0, 0.0, 0.0, 0.0, k.scale[1], 0.0, 0.0, 0.0, 0.0, k.scale[2], 0.0, 0.0, 0.0,
        0.0, 1.0,
    ];
    let mut m = math::mul(&rz, &math::mul(&ry, &math::mul(&rx, &s)));
    m[12] = k.position[0];
    m[13] = k.position[1];
    m[14] = k.position[2];
    m
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustpaper_core::mdl::{Bone, Mesh};

    fn one_bone(keys: Vec<Key>, mode: PlayMode) -> Puppet {
        let rest = key_matrix(&keys[0]);
        Puppet {
            attachments: Vec::new(),
            material: String::new(),
            mesh: Mesh {
                positions: vec![[10.0, 0.0, 0.0]],
                uvs: vec![[0.0, 0.0]],
                bones: vec![[0; 4]],
                weights: vec![[1.0, 0.0, 0.0, 0.0]],
                indices: vec![],
            },
            bones: vec![Bone {
                parent: None,
                local: rest,
            }],
            animations: vec![Animation {
                id: 7,
                name: "a".into(),
                mode,
                fps: 1.0,
                frames: (keys.len() - 1) as u32,
                tracks: vec![keys],
            }],
        }
    }

    fn key(x: f32) -> Key {
        Key {
            position: [x, 0.0, 0.0],
            rotation: [0.0; 3],
            scale: [1.0; 3],
        }
    }

    #[test]
    fn rest_pose_is_identity_and_keys_move_vertices() {
        let p = one_bone(vec![key(0.0), key(4.0)], PlayMode::Loop);
        let layers =
            serde_json::json!([{ "animation": 7, "rate": 1.0, "blend": 1.0, "visible": true }]);
        let mut rt = PuppetRt::new(p, Some(&layers));
        assert!((rt.vertices[0] - 10.0).abs() < 1e-4, "frame 0 = bind pose");
        rt.update(0.5); // halfway between keys: +2
        assert!((rt.vertices[0] - 12.0).abs() < 1e-3, "{}", rt.vertices[0]);
    }

    #[test]
    fn matrix_key_inverts_key_matrix() {
        let k = Key {
            position: [3.0, -2.0, 1.0],
            rotation: [0.1, -0.2, 0.7],
            scale: [0.8, 1.5, 1.0],
        };
        let back = matrix_key(&key_matrix(&k));
        for i in 0..3 {
            assert!((back.position[i] - k.position[i]).abs() < 1e-4);
            assert!((back.rotation[i] - k.rotation[i]).abs() < 1e-4, "{back:?}");
            assert!((back.scale[i] - k.scale[i]).abs() < 1e-4, "{back:?}");
        }
    }

    #[test]
    fn additive_layer_is_relative_to_the_bind_pose() {
        // bind at x = 0; an additive "intro" animation going from x = 5 back
        // to the bind pose must end on the bind pose (Lucy 3521337568), not
        // at -5 (relative to its own first key).
        let mut p = one_bone(vec![key(0.0), key(0.0)], PlayMode::Single);
        p.animations[0].tracks[0] = vec![key(5.0), key(0.0)];
        let layers = serde_json::json!([{ "animation": 7, "additive": true }]);
        let mut rt = PuppetRt::new(p, Some(&layers));
        assert!((rt.vertices[0] - 15.0).abs() < 1e-4, "{}", rt.vertices[0]);
        rt.update(2.0); // past the end: holds the last key (bind)
        assert!((rt.vertices[0] - 10.0).abs() < 1e-4, "{}", rt.vertices[0]);
    }

    #[test]
    fn mirror_plays_back_and_forth() {
        let p = one_bone(vec![key(0.0), key(4.0)], PlayMode::Mirror);
        let layers = serde_json::json!([{ "animation": 7 }]);
        let mut rt = PuppetRt::new(p, Some(&layers));
        rt.update(1.5); // 0 -> 1 -> back to 0.5
        assert!((rt.vertices[0] - 12.0).abs() < 1e-3, "{}", rt.vertices[0]);
    }
}
