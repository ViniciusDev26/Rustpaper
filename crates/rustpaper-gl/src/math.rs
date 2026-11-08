// Tiny column-major 4x4 matrix helpers (the layout GLSL expects).
//
// Coordinate conventions used across the renderer (keep these in mind!):
//   - Scene space is Y-UP: (0,0) bottom-left, (W,H) top-right, like WE.
//   - Every render target stores the TOP of the image in row 0, exactly like a
//     texture loaded from disk. To get that in GL (whose row 0 is clip y = -1)
//     the projections below map "top" to clip y = -1. Consequence: uv (0,0) is
//     the top-left everywhere — textures, effect FBOs, the final scene — which
//     is what the WE (D3D-born) shaders assume.

pub type Mat4 = [f32; 16];

#[rustfmt::skip]
pub const IDENTITY: Mat4 = [
    1.0, 0.0, 0.0, 0.0, //
    0.0, 1.0, 0.0, 0.0, //
    0.0, 0.0, 1.0, 0.0, //
    0.0, 0.0, 0.0, 1.0,
];

pub fn mul(a: &Mat4, b: &Mat4) -> Mat4 {
    let mut out = [0.0; 16];
    for col in 0..4 {
        for row in 0..4 {
            out[col * 4 + row] = (0..4).map(|k| a[k * 4 + row] * b[col * 4 + k]).sum();
        }
    }
    out
}

/// Depth scale of the orthographic projections. Scenes place things at large
/// z (particle emitters spread hundreds of units in depth, e.g. "directions":
/// "1 0.1 1"); passing z through unscaled clipped everything outside [-1, 1].
/// We never depth-test, so z only has to stay inside the clip volume.
const DEPTH_Z: f32 = 1.0 / 100_000.0;

/// Scene projection: x in [0,W] -> [-1,1]; y in [0,H] -> [+1,-1] (top = row 0).
#[rustfmt::skip]
pub fn scene_ortho(w: f32, h: f32) -> Mat4 {
    [
        2.0 / w, 0.0,      0.0,     0.0,
        0.0,     -2.0 / h, 0.0,     0.0,
        0.0,     0.0,      DEPTH_Z, 0.0,
        -1.0,    1.0,      0.0,     1.0,
    ]
}

/// Projection for a quad centered at the origin that must fill a render target
/// of `w`x`h` (effect passes, layer FBOs). Same top = row 0 rule.
#[rustfmt::skip]
pub fn centered_ortho(w: f32, h: f32) -> Mat4 {
    [
        2.0 / w, 0.0,      0.0,     0.0,
        0.0,     -2.0 / h, 0.0,     0.0,
        0.0,     0.0,      DEPTH_Z, 0.0,
        0.0,     0.0,      0.0,     1.0,
    ]
}

/// Model matrix of a scene object: translate(origin) * rotZ(angle) * scale.
/// WE stores `angles` in radians; only Z matters for 2D scenes.
#[rustfmt::skip]
pub fn model(origin: [f32; 3], angle_z: f32, scale: [f32; 3]) -> Mat4 {
    let (s, c) = angle_z.sin_cos();
    [
        c * scale[0],  s * scale[0], 0.0,      0.0,
        -s * scale[1], c * scale[1], 0.0,      0.0,
        0.0,           0.0,          scale[2], 0.0,
        origin[0],     origin[1],    origin[2], 1.0,
    ]
}

/// Inverse of a general 4x4 (cofactor expansion). Used for
/// `g_ModelMatrixInverse`; returns identity for a singular matrix.
pub fn invert(m: &Mat4) -> Mat4 {
    let mut inv = [0.0f32; 16];
    inv[0] = m[5] * m[10] * m[15] - m[5] * m[11] * m[14] - m[9] * m[6] * m[15]
        + m[9] * m[7] * m[14]
        + m[13] * m[6] * m[11]
        - m[13] * m[7] * m[10];
    inv[4] = -m[4] * m[10] * m[15] + m[4] * m[11] * m[14] + m[8] * m[6] * m[15]
        - m[8] * m[7] * m[14]
        - m[12] * m[6] * m[11]
        + m[12] * m[7] * m[10];
    inv[8] = m[4] * m[9] * m[15] - m[4] * m[11] * m[13] - m[8] * m[5] * m[15]
        + m[8] * m[7] * m[13]
        + m[12] * m[5] * m[11]
        - m[12] * m[7] * m[9];
    inv[12] = -m[4] * m[9] * m[14] + m[4] * m[10] * m[13] + m[8] * m[5] * m[14]
        - m[8] * m[6] * m[13]
        - m[12] * m[5] * m[10]
        + m[12] * m[6] * m[9];
    inv[1] = -m[1] * m[10] * m[15] + m[1] * m[11] * m[14] + m[9] * m[2] * m[15]
        - m[9] * m[3] * m[14]
        - m[13] * m[2] * m[11]
        + m[13] * m[3] * m[10];
    inv[5] = m[0] * m[10] * m[15] - m[0] * m[11] * m[14] - m[8] * m[2] * m[15]
        + m[8] * m[3] * m[14]
        + m[12] * m[2] * m[11]
        - m[12] * m[3] * m[10];
    inv[9] = -m[0] * m[9] * m[15] + m[0] * m[11] * m[13] + m[8] * m[1] * m[15]
        - m[8] * m[3] * m[13]
        - m[12] * m[1] * m[11]
        + m[12] * m[3] * m[9];
    inv[13] = m[0] * m[9] * m[14] - m[0] * m[10] * m[13] - m[8] * m[1] * m[14]
        + m[8] * m[2] * m[13]
        + m[12] * m[1] * m[10]
        - m[12] * m[2] * m[9];
    inv[2] = m[1] * m[6] * m[15] - m[1] * m[7] * m[14] - m[5] * m[2] * m[15]
        + m[5] * m[3] * m[14]
        + m[13] * m[2] * m[7]
        - m[13] * m[3] * m[6];
    inv[6] = -m[0] * m[6] * m[15] + m[0] * m[7] * m[14] + m[4] * m[2] * m[15]
        - m[4] * m[3] * m[14]
        - m[12] * m[2] * m[7]
        + m[12] * m[3] * m[6];
    inv[10] = m[0] * m[5] * m[15] - m[0] * m[7] * m[13] - m[4] * m[1] * m[15]
        + m[4] * m[3] * m[13]
        + m[12] * m[1] * m[7]
        - m[12] * m[3] * m[5];
    inv[14] = -m[0] * m[5] * m[14] + m[0] * m[6] * m[13] + m[4] * m[1] * m[14]
        - m[4] * m[2] * m[13]
        - m[12] * m[1] * m[6]
        + m[12] * m[2] * m[5];
    inv[3] = -m[1] * m[6] * m[11] + m[1] * m[7] * m[10] + m[5] * m[2] * m[11]
        - m[5] * m[3] * m[10]
        - m[9] * m[2] * m[7]
        + m[9] * m[3] * m[6];
    inv[7] = m[0] * m[6] * m[11] - m[0] * m[7] * m[10] - m[4] * m[2] * m[11]
        + m[4] * m[3] * m[10]
        + m[8] * m[2] * m[7]
        - m[8] * m[3] * m[6];
    inv[11] = -m[0] * m[5] * m[11] + m[0] * m[7] * m[9] + m[4] * m[1] * m[11]
        - m[4] * m[3] * m[9]
        - m[8] * m[1] * m[7]
        + m[8] * m[3] * m[5];
    inv[15] = m[0] * m[5] * m[10] - m[0] * m[6] * m[9] - m[4] * m[1] * m[10]
        + m[4] * m[2] * m[9]
        + m[8] * m[1] * m[6]
        - m[8] * m[2] * m[5];
    let det = m[0] * inv[0] + m[1] * inv[4] + m[2] * inv[8] + m[3] * inv[12];
    if det.abs() < 1e-12 {
        return IDENTITY;
    }
    inv.map(|v| v / det)
}

/// Quad of `w`x`h` centered at the origin, as 4 vertices of (x, y, z, u, v)
/// in triangle-strip order. uv (0,0) is the TOP-left corner (y = +h/2).
#[rustfmt::skip]
pub fn quad(w: f32, h: f32, u_max: f32, v_max: f32) -> [f32; 20] {
    let (x, y) = (w * 0.5, h * 0.5);
    [
        -x, y, 0.0, 0.0, 0.0, // top-left
        x, y, 0.0, u_max, 0.0, // top-right
        -x, -y, 0.0, 0.0, v_max, // bottom-left
        x, -y, 0.0, u_max, v_max, // bottom-right
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(m: &Mat4, p: [f32; 4]) -> [f32; 4] {
        let mut o = [0.0; 4];
        for (row, out) in o.iter_mut().enumerate() {
            *out = (0..4).map(|k| m[k * 4 + row] * p[k]).sum();
        }
        o
    }

    #[test]
    fn scene_top_maps_to_row_zero() {
        let p = scene_ortho(1920.0, 1080.0);
        assert_eq!(apply(&p, [0.0, 1080.0, 0.0, 1.0]), [-1.0, -1.0, 0.0, 1.0]);
        // deep particles stay inside the clip volume
        assert!(apply(&p, [0.0, 0.0, 5000.0, 1.0])[2].abs() < 1.0);
        assert_eq!(apply(&p, [1920.0, 0.0, 0.0, 1.0]), [1.0, 1.0, 0.0, 1.0]);
    }

    #[test]
    fn invert_undoes_model() {
        let m = model([100.0, 50.0, 0.0], 0.7, [2.0, 3.0, 1.0]);
        let p = apply(&invert(&m), apply(&m, [5.0, -7.0, 0.0, 1.0]));
        assert!((p[0] - 5.0).abs() < 1e-4 && (p[1] + 7.0).abs() < 1e-4);
    }

    #[test]
    fn model_then_projection() {
        // a 100x100 object centered at (960, 540) covers the screen center
        let mvp = mul(
            &scene_ortho(1920.0, 1080.0),
            &model([960.0, 540.0, 0.0], 0.0, [1.0; 3]),
        );
        let c = apply(&mvp, [0.0, 0.0, 0.0, 1.0]);
        assert!(c[0].abs() < 1e-6 && c[1].abs() < 1e-6);
    }
}
