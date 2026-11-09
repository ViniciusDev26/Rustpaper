// GL resources: textures, render targets and the shared quad.

use glow::HasContext;

/// A texture plus what shaders need to know about it.
#[derive(Clone, Copy, Debug)]
pub struct Tex {
    pub id: glow::Texture,
    pub width: u32,
    pub height: u32,
    /// WE format id of the source .tex (see DecodedTexture::format).
    pub format: u32,
}

impl Tex {
    /// Value for `g_TextureNResolution`: (texture w, h, content w, h). We crop
    /// the power-of-two padding on upload, so both pairs are equal.
    pub fn resolution(&self) -> [f32; 4] {
        let (w, h) = (self.width as f32, self.height as f32);
        [w, h, w, h]
    }
}

/// Uploads decoded RGBA8 pixels (cropped to the real content) with mipmaps.
///
/// Textures are LINEAR (not sRGB): WE does all its math and blending on the raw
/// 8-bit values, so treating them as sRGB changes how every blend looks.
pub fn upload_texture(gl: &glow::Context, tex: &rustpaper_core::tex::DecodedTexture) -> Tex {
    let (w, h) = (tex.real_width.max(1), tex.real_height.max(1));
    let mut content = vec![0u8; (w * h * 4) as usize];
    for y in 0..h.min(tex.height) {
        let src = (y * tex.width * 4) as usize;
        let row = (w.min(tex.width) * 4) as usize;
        let dst = (y * w * 4) as usize;
        if src + row <= tex.rgba.len() {
            content[dst..dst + row].copy_from_slice(&tex.rgba[src..src + row]);
        }
    }
    // The core decoder expands 1/2-channel formats for convenience (R8 ->
    // rrrr, RG88 -> rrrg). WE uploads them as R8 / RG8 GL textures, which
    // sample as (r, 0, 0, 1) and (r, g, 0, 1), and shaders that care undo that
    // with TEXnFORMAT (e.g. ConvertTexture0Format: RG88 -> .rrrg). Restore the
    // GL layout, or that conversion would read luminance as alpha.
    match tex.format {
        8 => content.as_chunks_mut::<4>().0.iter_mut().for_each(|px| {
            px[1] = px[3];
            px[2] = 0;
            px[3] = 255;
        }),
        9 => content.as_chunks_mut::<4>().0.iter_mut().for_each(|px| {
            px[1] = 0;
            px[2] = 0;
            px[3] = 255;
        }),
        _ => {}
    }
    let nearest = tex.flags & 1 != 0;
    let clamp = tex.flags & 2 != 0;
    unsafe {
        let id = gl.create_texture().expect("glGenTextures");
        gl.bind_texture(glow::TEXTURE_2D, Some(id));
        gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
        gl.tex_image_2d(
            glow::TEXTURE_2D,
            0,
            glow::RGBA8 as i32,
            w as i32,
            h as i32,
            0,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelUnpackData::Slice(Some(&content)),
        );
        // Mipmap only textures that ship mipmaps: WE samples single-level ones
        // (noise, flow maps...) at full detail even when minified; mipmapping
        // them averages e.g. film grain into flat gray.
        let mipmapped = tex.mip_count > 1;
        if mipmapped {
            gl.generate_mipmap(glow::TEXTURE_2D);
        }
        let (min, mag) = if nearest {
            (glow::NEAREST, glow::NEAREST)
        } else if mipmapped {
            (glow::LINEAR_MIPMAP_LINEAR, glow::LINEAR)
        } else {
            (glow::LINEAR, glow::LINEAR)
        };
        let wrap = if clamp {
            glow::CLAMP_TO_EDGE
        } else {
            glow::REPEAT
        };
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, min as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, mag as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, wrap as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, wrap as i32);
        Tex {
            id,
            width: w,
            height: h,
            format: tex.format,
        }
    }
}

/// RGBA8 texture without data (linear filter, clamped), filled later with
/// `write_texture` — rasterized text.
pub fn empty_texture(gl: &glow::Context, w: u32, h: u32) -> Tex {
    unsafe {
        let id = gl.create_texture().expect("glGenTextures");
        gl.bind_texture(glow::TEXTURE_2D, Some(id));
        gl.tex_image_2d(
            glow::TEXTURE_2D,
            0,
            glow::RGBA8 as i32,
            w as i32,
            h as i32,
            0,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelUnpackData::Slice(None),
        );
        for (p, v) in [
            (glow::TEXTURE_MIN_FILTER, glow::LINEAR),
            (glow::TEXTURE_MAG_FILTER, glow::LINEAR),
            (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
            (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
        ] {
            gl.tex_parameter_i32(glow::TEXTURE_2D, p, v as i32);
        }
        Tex {
            id,
            width: w,
            height: h,
            format: 0,
        }
    }
}

/// Replaces the whole content of an RGBA8 texture.
pub fn write_texture(gl: &glow::Context, tex: &Tex, rgba: &[u8]) {
    unsafe {
        gl.bind_texture(glow::TEXTURE_2D, Some(tex.id));
        gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
        gl.tex_sub_image_2d(
            glow::TEXTURE_2D,
            0,
            0,
            0,
            tex.width as i32,
            tex.height as i32,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelUnpackData::Slice(Some(rgba)),
        );
    }
}

/// 1x1 texture of a solid color (fallback for anything we fail to load).
pub fn solid_texture(gl: &glow::Context, rgba: [u8; 4]) -> Tex {
    unsafe {
        let id = gl.create_texture().expect("glGenTextures");
        gl.bind_texture(glow::TEXTURE_2D, Some(id));
        gl.tex_image_2d(
            glow::TEXTURE_2D,
            0,
            glow::RGBA8 as i32,
            1,
            1,
            0,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelUnpackData::Slice(Some(&rgba)),
        );
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_MIN_FILTER,
            glow::NEAREST as i32,
        );
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_MAG_FILTER,
            glow::NEAREST as i32,
        );
        Tex {
            id,
            width: 1,
            height: 1,
            format: 0,
        }
    }
}

/// An offscreen render target: a framebuffer with one RGBA8 color texture.
#[derive(Clone, Copy, Debug)]
pub struct Target {
    pub fbo: glow::Framebuffer,
    pub tex: Tex,
}

impl Target {
    pub fn new(gl: &glow::Context, width: u32, height: u32) -> Target {
        Self::with_format(gl, width, height, false)
    }

    /// `hdr` = RGBA16F: keeps values above 1.0 (HDR scenes, bloom chains).
    pub fn with_format(gl: &glow::Context, width: u32, height: u32, hdr: bool) -> Target {
        let (w, h) = (width.max(1), height.max(1));
        let (internal, ty) = if hdr {
            (glow::RGBA16F, glow::HALF_FLOAT)
        } else {
            (glow::RGBA8, glow::UNSIGNED_BYTE)
        };
        unsafe {
            let id = gl.create_texture().expect("glGenTextures");
            gl.bind_texture(glow::TEXTURE_2D, Some(id));
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                internal as i32,
                w as i32,
                h as i32,
                0,
                glow::RGBA,
                ty,
                glow::PixelUnpackData::Slice(None),
            );
            for (p, v) in [
                (glow::TEXTURE_MIN_FILTER, glow::LINEAR),
                (glow::TEXTURE_MAG_FILTER, glow::LINEAR),
                (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
                (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
            ] {
                gl.tex_parameter_i32(glow::TEXTURE_2D, p, v as i32);
            }
            let fbo = gl.create_framebuffer().expect("glGenFramebuffers");
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
            gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_2D,
                Some(id),
                0,
            );
            Target {
                fbo,
                tex: Tex {
                    id,
                    width: w,
                    height: h,
                    format: 0,
                },
            }
        }
    }

    pub fn bind(&self, gl: &glow::Context) {
        unsafe {
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.fbo));
            gl.viewport(0, 0, self.tex.width as i32, self.tex.height as i32);
        }
    }

    pub fn clear(&self, gl: &glow::Context, rgba: [f32; 4]) {
        self.bind(gl);
        unsafe {
            gl.clear_color(rgba[0], rgba[1], rgba[2], rgba[3]);
            gl.clear(glow::COLOR_BUFFER_BIT);
        }
    }

    /// Copies this target into `dst` (same size or scaled, linear filter).
    pub fn blit_to(&self, gl: &glow::Context, dst: &Target) {
        unsafe {
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(self.fbo));
            gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(dst.fbo));
            gl.blit_framebuffer(
                0,
                0,
                self.tex.width as i32,
                self.tex.height as i32,
                0,
                0,
                dst.tex.width as i32,
                dst.tex.height as i32,
                glow::COLOR_BUFFER_BIT,
                glow::LINEAR,
            );
        }
    }

    /// Reads the pixels back, row 0 = top of the image (see `math`).
    pub fn read_rgba(&self, gl: &glow::Context) -> Vec<u8> {
        let mut px = vec![0u8; (self.tex.width * self.tex.height * 4) as usize];
        unsafe {
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(self.fbo));
            gl.pixel_store_i32(glow::PACK_ALIGNMENT, 1);
            gl.read_pixels(
                0,
                0,
                self.tex.width as i32,
                self.tex.height as i32,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelPackData::Slice(Some(&mut px)),
            );
        }
        px
    }
}

/// How a draw is blended onto its target (WE `blending` names).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Blend {
    /// Overwrite (WE "normal" / "disabled").
    Opaque,
    /// Standard alpha blending (WE "translucent").
    Translucent,
    /// Add on top, weighted by alpha (WE "additive").
    Additive,
    /// Plain sum (bloom upsampling).
    Add,
}

impl Blend {
    pub fn from_we(name: &str) -> Blend {
        match name.to_ascii_lowercase().as_str() {
            "translucent" => Blend::Translucent,
            "additive" => Blend::Additive,
            _ => Blend::Opaque,
        }
    }

    pub fn apply(self, gl: &glow::Context) {
        unsafe {
            match self {
                Blend::Opaque => gl.disable(glow::BLEND),
                Blend::Translucent => {
                    gl.enable(glow::BLEND);
                    // alpha accumulates "over" so a translucent target can be
                    // composited again later (layer FBOs).
                    gl.blend_func_separate(
                        glow::SRC_ALPHA,
                        glow::ONE_MINUS_SRC_ALPHA,
                        glow::ONE,
                        glow::ONE_MINUS_SRC_ALPHA,
                    );
                }
                Blend::Additive => {
                    gl.enable(glow::BLEND);
                    gl.blend_func_separate(glow::SRC_ALPHA, glow::ONE, glow::ZERO, glow::ONE);
                }
                Blend::Add => {
                    gl.enable(glow::BLEND);
                    gl.blend_func(glow::ONE, glow::ONE);
                }
            }
        }
    }
}

/// One dynamic quad (VAO + VBO) reused by every draw: attribute 0 = a_Position
/// (vec3), attribute 1 = a_TexCoord (vec2).
pub struct QuadMesh {
    vao: glow::VertexArray,
    vbo: glow::Buffer,
}

impl QuadMesh {
    pub fn new(gl: &glow::Context) -> QuadMesh {
        unsafe {
            let vao = gl.create_vertex_array().expect("glGenVertexArrays");
            let vbo = gl.create_buffer().expect("glGenBuffers");
            gl.bind_vertex_array(Some(vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
            gl.buffer_data_size(glow::ARRAY_BUFFER, 20 * 4, glow::DYNAMIC_DRAW);
            gl.enable_vertex_attrib_array(0);
            gl.vertex_attrib_pointer_f32(0, 3, glow::FLOAT, false, 20, 0);
            gl.enable_vertex_attrib_array(1);
            gl.vertex_attrib_pointer_f32(1, 2, glow::FLOAT, false, 20, 12);
            QuadMesh { vao, vbo }
        }
    }

    pub fn draw(&self, gl: &glow::Context, vertices: &[f32; 20]) {
        unsafe {
            gl.bind_vertex_array(Some(self.vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.vbo));
            gl.buffer_sub_data_u8_slice(glow::ARRAY_BUFFER, 0, as_bytes(vertices));
            gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
        }
    }
}

/// Dynamic vertex/index buffers for particle quads (4 vertices each), with
/// the attribute layout of the WE particle shader it feeds.
pub struct ParticleMesh {
    vao: glow::VertexArray,
    vbo: glow::Buffer,
    ebo: glow::Buffer,
    index_capacity: usize, // in quads
    floats_per_vertex: usize,
}

/// genericparticle: (location, components, float offset); see Program::build.
pub const SPRITE_LAYOUT: &[(u32, i32, i32)] =
    &[(0, 3, 0), (2, 4, 3), (3, 4, 7), (4, 2, 11), (5, 4, 13)];
/// genericropeparticle (no geometry shader, thin format): a_PositionVec4,
/// a_TexCoordVec4, a_TexCoordVec4C1, a_TexCoordVec3C2, a_TexCoordC3, a_Color.
pub const ROPE_LAYOUT: &[(u32, i32, i32)] = &[
    (6, 4, 0),
    (2, 4, 4),
    (5, 4, 8),
    (7, 3, 12),
    (8, 2, 15),
    (3, 4, 17),
];

impl ParticleMesh {
    pub fn new(
        gl: &glow::Context,
        layout: &[(u32, i32, i32)],
        floats_per_vertex: usize,
    ) -> ParticleMesh {
        unsafe {
            let vao = gl.create_vertex_array().expect("glGenVertexArrays");
            let vbo = gl.create_buffer().expect("glGenBuffers");
            let ebo = gl.create_buffer().expect("glGenBuffers");
            gl.bind_vertex_array(Some(vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
            gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(ebo));
            let stride = (floats_per_vertex * 4) as i32;
            for &(loc, n, off) in layout {
                gl.enable_vertex_attrib_array(loc);
                gl.vertex_attrib_pointer_f32(loc, n, glow::FLOAT, false, stride, off * 4);
            }
            gl.bind_vertex_array(None);
            ParticleMesh {
                vao,
                vbo,
                ebo,
                index_capacity: 0,
                floats_per_vertex,
            }
        }
    }

    /// Uploads `vertices` (4 per quad) and draws them as quads.
    pub fn draw(&mut self, gl: &glow::Context, vertices: &[f32]) {
        let quads = vertices.len() / (self.floats_per_vertex * 4);
        if quads == 0 {
            return;
        }
        unsafe {
            gl.bind_vertex_array(Some(self.vao));
            if quads > self.index_capacity {
                let cap = quads.next_power_of_two();
                let mut idx: Vec<u32> = Vec::with_capacity(cap * 6);
                for i in 0..cap as u32 {
                    let b = i * 4;
                    idx.extend_from_slice(&[b, b + 1, b + 2, b + 2, b + 1, b + 3]);
                }
                gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(self.ebo));
                gl.buffer_data_u8_slice(
                    glow::ELEMENT_ARRAY_BUFFER,
                    slice_bytes(&idx),
                    glow::STATIC_DRAW,
                );
                self.index_capacity = cap;
            }
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.vbo));
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, slice_bytes(vertices), glow::STREAM_DRAW);
            gl.draw_elements(glow::TRIANGLES, (quads * 6) as i32, glow::UNSIGNED_INT, 0);
            gl.bind_vertex_array(None);
        }
    }
}

/// Indexed triangle mesh of (x, y, z, u, v) vertices on attributes 0 and 1
/// (a_Position / a_TexCoord): puppet meshes. Vertices are re-uploaded each
/// frame (CPU skinning), indices once.
pub struct MeshGpu {
    vao: glow::VertexArray,
    vbo: glow::Buffer,
    count: i32,
}

impl MeshGpu {
    pub fn new(gl: &glow::Context, indices: &[u32]) -> MeshGpu {
        unsafe {
            let vao = gl.create_vertex_array().expect("glGenVertexArrays");
            let vbo = gl.create_buffer().expect("glGenBuffers");
            let ebo = gl.create_buffer().expect("glGenBuffers");
            gl.bind_vertex_array(Some(vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
            gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(ebo));
            gl.buffer_data_u8_slice(
                glow::ELEMENT_ARRAY_BUFFER,
                slice_bytes(indices),
                glow::STATIC_DRAW,
            );
            gl.enable_vertex_attrib_array(0);
            gl.vertex_attrib_pointer_f32(0, 3, glow::FLOAT, false, 20, 0);
            gl.enable_vertex_attrib_array(1);
            gl.vertex_attrib_pointer_f32(1, 2, glow::FLOAT, false, 20, 12);
            gl.bind_vertex_array(None);
            MeshGpu {
                vao,
                vbo,
                count: indices.len() as i32,
            }
        }
    }

    pub fn upload(&self, gl: &glow::Context, vertices: &[f32]) {
        unsafe {
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.vbo));
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, slice_bytes(vertices), glow::STREAM_DRAW);
        }
    }

    pub fn draw(&self, gl: &glow::Context) {
        unsafe {
            gl.bind_vertex_array(Some(self.vao));
            gl.draw_elements(glow::TRIANGLES, self.count, glow::UNSIGNED_INT, 0);
            gl.bind_vertex_array(None);
        }
    }
}

fn slice_bytes<T: Copy>(v: &[T]) -> &[u8] {
    // SAFETY: plain-old-data numeric slices; the byte view covers exactly v.
    unsafe { std::slice::from_raw_parts(v.as_ptr() as *const u8, std::mem::size_of_val(v)) }
}

fn as_bytes(v: &[f32; 20]) -> &[u8] {
    // SAFETY: f32 has no padding/invalid bit patterns; the slice covers exactly v.
    unsafe { std::slice::from_raw_parts(v.as_ptr() as *const u8, std::mem::size_of_val(v)) }
}

/// Source rectangle (x0, y0, x1, y1) of a `src_w`x`src_h` image that "covers" a
/// `dst_w`x`dst_h` screen: scaled to fill it, centered, overflow cropped.
pub fn cover_rect(src_w: u32, src_h: u32, dst_w: u32, dst_h: u32) -> (i32, i32, i32, i32) {
    let (sw, sh) = (src_w as f32, src_h as f32);
    let scale = (dst_w as f32 / sw).max(dst_h as f32 / sh);
    let (cw, ch) = (dst_w as f32 / scale, dst_h as f32 / scale);
    let (x0, y0) = ((sw - cw) * 0.5, (sh - ch) * 0.5);
    (
        x0.round() as i32,
        y0.round() as i32,
        (x0 + cw).round() as i32,
        (y0 + ch).round() as i32,
    )
}

/// Cover-blits a finished frame into another target (same convention: row 0
/// = top), e.g. to emulate a 1920x1080 screen offscreen.
pub fn cover_into(gl: &glow::Context, src: &Target, dst: &Target) {
    let (x0, y0, x1, y1) = cover_rect(src.tex.width, src.tex.height, dst.tex.width, dst.tex.height);
    unsafe {
        gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(src.fbo));
        gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(dst.fbo));
        gl.blit_framebuffer(
            x0,
            y0,
            x1,
            y1,
            0,
            0,
            dst.tex.width as i32,
            dst.tex.height as i32,
            glow::COLOR_BUFFER_BIT,
            glow::LINEAR,
        );
    }
}

/// Copies a finished frame to the current window surface (default framebuffer)
/// of `dst_w`x`dst_h`, cover-scaled. Our targets keep the image top in row 0,
/// the window's row 0 is the bottom, so the destination rectangle is flipped.
pub fn present_cover(gl: &glow::Context, src: &Target, dst_w: u32, dst_h: u32) {
    let (x0, y0, x1, y1) = cover_rect(src.tex.width, src.tex.height, dst_w, dst_h);
    unsafe {
        gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(src.fbo));
        gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, None);
        // A context first made current WITHOUT a surface (EGL_KHR_surfaceless_
        // context, how the live wallpaper starts) keeps DRAW_BUFFER = GL_NONE
        // even after a window surface is bound: every draw to the window would
        // be silently discarded. Select the back buffer explicitly.
        gl.draw_buffer(glow::BACK);
        gl.viewport(0, 0, dst_w as i32, dst_h as i32);
        gl.blit_framebuffer(
            x0,
            y0,
            x1,
            y1,
            0,
            dst_h as i32,
            dst_w as i32,
            0,
            glow::COLOR_BUFFER_BIT,
            glow::LINEAR,
        );
        // The wallpaper is opaque: force alpha to 1 so the compositor never
        // blends it with whatever is below the background layer.
        gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        gl.color_mask(false, false, false, true);
        gl.clear_color(0.0, 0.0, 0.0, 1.0);
        gl.clear(glow::COLOR_BUFFER_BIT);
        gl.color_mask(true, true, true, true);
    }
}

#[cfg(test)]
mod tests {
    use super::cover_rect;

    #[test]
    fn cover_crops_the_overflowing_axis() {
        // same aspect: whole image
        assert_eq!(cover_rect(3840, 2160, 1920, 1080), (0, 0, 3840, 2160));
        // 16:9 scene on a 16:10 screen: crop left/right
        assert_eq!(cover_rect(1920, 1080, 1920, 1200), (96, 0, 1824, 1080));
        // 16:9 scene on an ultrawide: crop top/bottom
        assert_eq!(cover_rect(1920, 1080, 3440, 1440), (0, 138, 1920, 942));
    }
}
