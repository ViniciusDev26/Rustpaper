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

/// Scene projection: x in [0,W] -> [-1,1]; y in [0,H] -> [+1,-1] (top = row 0).
#[rustfmt::skip]
pub fn scene_ortho(w: f32, h: f32) -> Mat4 {
    [
        2.0 / w, 0.0,      0.0, 0.0,
        0.0,     -2.0 / h, 0.0, 0.0,
        0.0,     0.0,      1.0, 0.0,
        -1.0,    1.0,      0.0, 1.0,
    ]
}

/// Projection for a quad centered at the origin that must fill a render target
/// of `w`x`h` (effect passes, layer FBOs). Same top = row 0 rule.
#[rustfmt::skip]
pub fn centered_ortho(w: f32, h: f32) -> Mat4 {
    [
        2.0 / w, 0.0,      0.0, 0.0,
        0.0,     -2.0 / h, 0.0, 0.0,
        0.0,     0.0,      1.0, 0.0,
        0.0,     0.0,      0.0, 1.0,
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
        assert_eq!(apply(&p, [1920.0, 0.0, 0.0, 1.0]), [1.0, 1.0, 0.0, 1.0]);
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
