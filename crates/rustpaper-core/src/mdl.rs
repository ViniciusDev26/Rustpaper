// Module `mdl`: Wallpaper Engine puppet models (`models/*_puppet.mdl`), the
// skeletal animation behind "puppet warp" (breathing, hair sway...).
//
// Reverse-engineered layout (little endian; strings are NUL-terminated):
//
//   MDLV00xx\0  u32 flags, u32 1, u32 1, material path
//               padding: 4 bytes (v13, v16) / 28 bytes (v17+)
//               u32 vertex flags (v16+; v13: fixed 0x1800009), u32 vertex byte size, vertices,
//               u32 index byte size, u16 triangle indices
//     vertex = position f32x3 [+ normal f32x3 if 0x2] [+ tangent f32x4 if 0x4]
//              [+ bone indices u32x4 if 0x800000] [+ weights f32x4 if 0x1000000]
//              [+ uv f32x2 if 0x8]
//   MDLS0002\0  u32 absolute offset of MDLA, u32 bone count, per bone:
//               name, u32 flags, i32 parent, u32 64, f32x16 bind matrix
//               (local, column-major), string; then extra data we skip
//   MDLA000x\0  u32 end offset, u32 animation count, per animation:
//               u32 id (= scene `animationlayers[].animation`), u32, name,
//               mode ("loop" | "mirror" | "single"), f32 rate, u32 frames,
//               u32, u32 bone count, per bone: u32 id, u32 byte size,
//               (frames + 1) x { f32x3 position, f32x3 euler (radians),
//               f32x3 scale } (local to the parent bone)

#[derive(Debug, Clone, Default)]
pub struct Mesh {
    pub positions: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    /// Up to 4 bone influences per vertex (all zero weights = unskinned).
    pub bones: Vec<[u32; 4]>,
    pub weights: Vec<[f32; 4]>,
    pub indices: Vec<u32>,
}

#[derive(Debug, Clone)]
pub struct Bone {
    pub parent: Option<usize>,
    /// Bind pose, relative to the parent (column-major 4x4).
    pub local: [f32; 16],
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Key {
    pub position: [f32; 3],
    pub rotation: [f32; 3],
    pub scale: [f32; 3],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayMode {
    Loop,
    Mirror,
    Single,
}

#[derive(Debug, Clone)]
pub struct Animation {
    pub id: u32,
    pub name: String,
    pub mode: PlayMode,
    /// Frames per second (see `Puppet::parse` for the v4 caveat).
    pub fps: f32,
    pub frames: u32,
    /// One key track per bone, `frames + 1` keys each.
    pub tracks: Vec<Vec<Key>>,
}

/// Named point on a bone (MDAT block): scene objects with
/// `"attachment": name` and this model as `parent` follow it.
#[derive(Debug, Clone)]
pub struct Attachment {
    pub name: String,
    pub bone: usize,
    /// Transform relative to the bone (column-major 4x4).
    pub local: [f32; 16],
}

#[derive(Debug, Clone)]
pub struct Puppet {
    pub material: String,
    pub mesh: Mesh,
    pub bones: Vec<Bone>,
    pub animations: Vec<Animation>,
    pub attachments: Vec<Attachment>,
}

struct Cursor<'a> {
    d: &'a [u8],
    p: usize,
}

impl Cursor<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8], String> {
        let s = self
            .d
            .get(self.p..self.p + n)
            .ok_or("unexpected end of .mdl")?;
        self.p += n;
        Ok(s)
    }
    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn i32(&mut self) -> Result<i32, String> {
        Ok(self.u32()? as i32)
    }
    fn f32(&mut self) -> Result<f32, String> {
        Ok(f32::from_bits(self.u32()?))
    }
    fn f32s<const N: usize>(&mut self) -> Result<[f32; N], String> {
        let mut out = [0.0; N];
        for v in &mut out {
            *v = self.f32()?;
        }
        Ok(out)
    }
    fn cstr(&mut self) -> Result<String, String> {
        let end = self.d[self.p..]
            .iter()
            .position(|&b| b == 0)
            .ok_or("unterminated string")?;
        let s = String::from_utf8_lossy(&self.d[self.p..self.p + end]).into_owned();
        self.p += end + 1;
        Ok(s)
    }
    fn magic(&mut self) -> Result<String, String> {
        let m = String::from_utf8_lossy(self.take(9)?)
            .trim_end_matches('\0')
            .to_string();
        Ok(m)
    }
}

impl Puppet {
    pub fn parse(d: &[u8]) -> Result<Puppet, String> {
        let mut c = Cursor { d, p: 0 };
        let magic = c.magic()?;
        let version: u32 = magic
            .strip_prefix("MDLV")
            .and_then(|v| v.parse().ok())
            .ok_or_else(|| format!("not a puppet model ({magic})"))?;
        c.u32()?;
        c.u32()?;
        c.u32()?;
        let material = c.cstr()?;
        c.take(if version >= 17 { 28 } else { 4 })?;
        // v16+ store the vertex layout flags; older versions (MDLV0013) use a
        // fixed layout: position, 4 bone indices, 4 weights, uv (52 bytes)
        let vflags = if version >= 16 { c.u32()? } else { 0x0180_0009 };
        let vbytes = c.u32()? as usize;
        let mut stride = 12;
        for (bit, size) in [
            (0x2, 12),
            (0x4, 16),
            (0x80_0000, 16),
            (0x100_0000, 16),
            (0x8, 8),
        ] {
            if vflags & bit != 0 {
                stride += size;
            }
        }
        if stride == 0 || !vbytes.is_multiple_of(stride) {
            return Err(format!(
                "unexpected vertex layout (flags {vflags:#x}, {vbytes} bytes)"
            ));
        }
        let mut mesh = Mesh::default();
        for _ in 0..vbytes / stride {
            mesh.positions.push(c.f32s::<3>()?);
            if vflags & 0x2 != 0 {
                c.take(12)?;
            }
            if vflags & 0x4 != 0 {
                c.take(16)?;
            }
            let bones = if vflags & 0x80_0000 != 0 {
                [c.u32()?, c.u32()?, c.u32()?, c.u32()?]
            } else {
                [0; 4]
            };
            let weights = if vflags & 0x100_0000 != 0 {
                c.f32s::<4>()?
            } else {
                [0.0; 4]
            };
            mesh.bones.push(bones);
            mesh.weights.push(weights);
            mesh.uvs.push(if vflags & 0x8 != 0 {
                c.f32s::<2>()?
            } else {
                [0.0; 2]
            });
        }
        let ibytes = c.u32()? as usize;
        for ch in c.take(ibytes)?.as_chunks::<2>().0 {
            mesh.indices.push(u16::from_le_bytes([ch[0], ch[1]]) as u32);
        }

        let mut bones = Vec::new();
        let mut animations = Vec::new();
        let mut attachments = Vec::new();
        // Newer versions (MDLV0023) put extra blocks between the indices and
        // the skeleton: look for the MDLS magic instead of assuming it follows.
        if let Some(k) = d[c.p..].windows(4).position(|w| w == b"MDLS") {
            c.p += k;
        }
        if c.p + 9 <= d.len() && c.magic()?.starts_with("MDLS") {
            let anim_at = c.u32()? as usize;
            for _ in 0..c.u32()? {
                c.cstr()?;
                c.u32()?;
                let parent = c.i32()?;
                let msize = c.u32()? as usize;
                let m = c.take(msize)?;
                let mut local = [0.0f32; 16];
                for (k, v) in local.iter_mut().enumerate() {
                    *v = f32::from_le_bytes(m[k * 4..k * 4 + 4].try_into().unwrap());
                }
                c.cstr()?;
                bones.push(Bone {
                    parent: (parent >= 0).then_some(parent as usize),
                    local,
                });
            }
            c.p = anim_at;
            // MDAT0001: u32, u16 count, per attachment u16 bone, name, f32x16
            if d.get(c.p..c.p + 4).is_some_and(|m| m == b"MDAT") {
                let mut a = Cursor { d, p: c.p + 9 };
                let parsed = (|| -> Result<Vec<Attachment>, String> {
                    a.u32()?;
                    let u16 = |a: &mut Cursor| -> Result<u16, String> {
                        Ok(u16::from_le_bytes(a.take(2)?.try_into().unwrap()))
                    };
                    let n = u16(&mut a)?;
                    let mut out = Vec::new();
                    for _ in 0..n {
                        let bone = u16(&mut a)? as usize;
                        let name = a.cstr()?;
                        let local = a.f32s::<16>()?;
                        out.push(Attachment { name, bone, local });
                    }
                    Ok(out)
                })();
                if let Ok(v) = parsed {
                    attachments = v;
                }
            }
            // MDLS0004 points at an MDAT block (attachments) that precedes MDLA
            if !d.get(c.p..c.p + 4).is_some_and(|m| m == b"MDLA")
                && let Some(k) = d
                    .get(c.p..)
                    .and_then(|r| r.windows(4).position(|w| w == b"MDLA"))
            {
                c.p += k;
            }
            let mdla = if c.p + 9 <= d.len() {
                c.magic()?
            } else {
                String::new()
            };
            if let Some(v) = mdla.strip_prefix("MDLA") {
                let mdla_version: u32 = v.parse().unwrap_or(3);
                c.u32()?;
                let count = c.u32()?;
                for n in 0..count {
                    // Animations can be followed by extra bytes (9 zero
                    // bytes seen between animations: likely an empty event
                    // list); find the next header instead of trusting the
                    // previous one ended where its tracks did.
                    if n > 0 {
                        c.p = next_animation(d, c.p).ok_or("animation header not found")?;
                    }
                    let id = c.u32()?;
                    c.u32()?;
                    let name = c.cstr()?;
                    let mode = match c.cstr()?.as_str() {
                        "mirror" => PlayMode::Mirror,
                        "single" => PlayMode::Single,
                        _ => PlayMode::Loop,
                    };
                    // Frames per second in every version seen (0001, 0003,
                    // 0004). Checked on MDLA0001 data: Summer Feeling's train
                    // (rate 1.0, 360 frames) crosses the screen between two
                    // consecutive keys, i.e. in ~1 s at 1 fps; at 30 fps it
                    // would be an invisible 1/30 s jump.
                    let fps = c.f32()?;
                    let _ = mdla_version;
                    let frames = c.u32()?;
                    c.u32()?;
                    let mut tracks = Vec::new();
                    for _ in 0..c.u32()? {
                        c.u32()?;
                        let size = c.u32()? as usize;
                        let mut keys = Vec::with_capacity(size / 36);
                        let end = c.p + size;
                        while c.p + 36 <= end {
                            keys.push(Key {
                                position: c.f32s::<3>()?,
                                rotation: c.f32s::<3>()?,
                                scale: c.f32s::<3>()?,
                            });
                        }
                        c.p = end;
                        tracks.push(keys);
                    }
                    skip_events(d, &mut c.p);
                    animations.push(Animation {
                        id,
                        name,
                        mode,
                        fps,
                        frames,
                        tracks,
                    });
                }
            }
        }
        Ok(Puppet {
            material,
            mesh,
            bones,
            animations,
            attachments,
        })
    }
}

/// Skips an animation's event list when one is there: u32 count, then per
/// event f32 frame + JSON string (`{"frame":0,"name":"flashStart"}`). Left
/// alone when it doesn't parse (`next_animation` resyncs instead).
fn skip_events(d: &[u8], p: &mut usize) {
    let mut c = Cursor { d, p: *p };
    let Ok(n) = c.u32() else { return };
    if n == 0 || n > 4096 {
        return;
    }
    for _ in 0..n {
        if c.f32().is_err() || c.cstr().is_err() {
            return;
        }
    }
    *p = c.p;
}

/// Offset of the next MDLA animation header at or after `from` (within a
/// few bytes): `u32 id, u32, name\0, mode\0` with a known play mode.
fn next_animation(d: &[u8], from: usize) -> Option<usize> {
    let is_header = |at: usize| {
        let rest = d.get(at + 8..)?;
        let name_end = rest.iter().take(256).position(|&b| b == 0)?;
        let mode = rest.get(name_end + 1..)?;
        let mode_end = mode.iter().take(16).position(|&b| b == 0)?;
        matches!(&mode[..mode_end], b"loop" | b"mirror" | b"single").then_some(())
    };
    (from..(from + 4096).min(d.len())).find(|&at| is_header(at).is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal v16 model: 1 triangle, 1 bone, 1 animation of 1 frame.
    fn sample() -> Vec<u8> {
        let mut d = Vec::new();
        let u = |d: &mut Vec<u8>, v: u32| d.extend_from_slice(&v.to_le_bytes());
        let f = |d: &mut Vec<u8>, v: f32| d.extend_from_slice(&v.to_le_bytes());
        d.extend_from_slice(b"MDLV0016\0");
        u(&mut d, 0x0180_0009);
        u(&mut d, 1);
        u(&mut d, 1);
        d.extend_from_slice(b"materials/x.json\0");
        u(&mut d, 0);
        u(&mut d, 0x0180_0009);
        u(&mut d, 3 * 52);
        for i in 0..3 {
            for v in [i as f32, 0.0, 0.0] {
                f(&mut d, v);
            }
            for b in [0, 0, 0, 0] {
                u(&mut d, b);
            }
            for w in [1.0, 0.0, 0.0, 0.0] {
                f(&mut d, w);
            }
            f(&mut d, 0.5);
            f(&mut d, 0.25);
        }
        u(&mut d, 6);
        d.extend_from_slice(&[0, 0, 1, 0, 2, 0]);
        d.extend_from_slice(b"MDLS0002\0");
        let anim_at_pos = d.len();
        u(&mut d, 0);
        u(&mut d, 1);
        d.push(0);
        u(&mut d, 1);
        u(&mut d, u32::MAX);
        u(&mut d, 64);
        for k in 0..16 {
            f(&mut d, if k % 5 == 0 { 1.0 } else { 0.0 });
        }
        d.push(0);
        let at = d.len() as u32;
        d[anim_at_pos..anim_at_pos + 4].copy_from_slice(&at.to_le_bytes());
        d.extend_from_slice(b"MDLA0003\0");
        u(&mut d, 0);
        u(&mut d, 1);
        u(&mut d, 42);
        u(&mut d, 0);
        d.extend_from_slice(b"Animation 1\0mirror\0");
        f(&mut d, 30.0);
        u(&mut d, 1);
        u(&mut d, 0);
        u(&mut d, 1);
        u(&mut d, 0);
        u(&mut d, 72);
        for _ in 0..2 {
            for v in [5.0, 6.0, 0.0, 0.0, 0.0, 0.1, 1.0, 1.0, 1.0] {
                f(&mut d, v);
            }
        }
        d
    }

    #[test]
    fn parses_mesh_skeleton_and_animation() {
        let p = Puppet::parse(&sample()).unwrap();
        assert_eq!(p.material, "materials/x.json");
        assert_eq!(p.mesh.positions.len(), 3);
        assert_eq!(p.mesh.indices, vec![0, 1, 2]);
        assert_eq!(p.mesh.uvs[2], [0.5, 0.25]);
        assert_eq!(p.bones.len(), 1);
        assert_eq!(p.bones[0].parent, None);
        let a = &p.animations[0];
        assert_eq!((a.id, a.mode, a.frames), (42, PlayMode::Mirror, 1));
        assert_eq!(a.tracks[0].len(), 2);
        assert_eq!(a.tracks[0][1].rotation[2], 0.1);
    }
}
