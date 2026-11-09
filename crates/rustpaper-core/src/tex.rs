// Módulo `tex`: decodifica o formato de textura do Wallpaper Engine (.tex) pra
// RGBA8. Formato (baseado no TextureParser do linux-wallpaperengine):
//   "TEXV0005\0" "TEXI0001\0"  (magics de 9 bytes, null-terminated)
//   u32 format, u32 flags, u32 texWidth, u32 texHeight, u32 width, u32 height, u32 (ignore)
//   "TEXB000x\0"  u32 imageCount   (+ FIF/mp4 nos TEXB0003/0004)
//   por imagem: u32 mipmapCount; por mipmap: [extras TEXB0004] w, h,
//     [TEXB0002+: compression, uncompressedSize] compressedSize, dados
//   dados podem ser LZ4 (compression==1). Formato final vem de `format` ou, nos
//   TEXB0003/0004, de uma imagem embutida (JPEG/PNG) decodificada pela crate image.

// Formatos que sabemos converter; o resto dá erro claro.
const FORMAT_ARGB8888: u32 = 0;
const FORMAT_DXT5: u32 = 4; // BC3
const FORMAT_DXT3: u32 = 6; // BC2
const FORMAT_DXT1: u32 = 7; // BC1
const FORMAT_RG88: u32 = 8; // 2 canais (R,G) — sprites com alpha no canal G
const FORMAT_R8: u32 = 9; // 1 canal — máscara (luminância = alpha)
const FIF_UNKNOWN: u32 = 0xFFFF_FFFF; // FreeImage: sem formato = -1

/// Sprite sheet (flipbook) de uma textura: N frames numa grade cols×rows. Muitas
/// partículas do WE (bolhas, faíscas) animam por esses frames ao longo da vida.
/// Sprite sheet stored INSIDE a .tex (block `TEXS000x` after the images), as
/// used by Workshop particle sprites: `frames` cells of `frame_w`x`frame_h` px,
/// laid out left-to-right, top-to-bottom over the image content.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EmbeddedSheet {
    pub frames: u32,
    /// Frame size in texels; fractional sizes exist (5 frames in 512 px).
    pub frame_w: f32,
    pub frame_h: f32,
}

/// Reads the embedded sprite sheet of a .tex, if it has one. Walks every
/// image/mipmap to reach the `TEXS` block:
///   TEXS0002: u32 count
///   TEXS0003: u32 count, u32 frame_w, u32 frame_h (sizes truncated)
///   per frame: u32 image, f32 frametime, f32 x, f32 y, f32 w, f32 _, f32 _, f32 h
/// (Verified against the `.tex-json` of the base particle sheets: fog1 =
/// 64 x 128x128, rosepetals = 5 x 102.5x128. The frame records carry the
/// exact, possibly fractional, size.)
pub fn embedded_sheet(data: &[u8]) -> Option<EmbeddedSheet> {
    let mut c = Cursor { data, pos: 0 };
    c.magic8().ok()?;
    c.magic8().ok()?;
    for _ in 0..7 {
        c.u32().ok()?;
    }
    let container = c.magic8().ok()?;
    let images = c.u32().ok()?;
    let mut is_mp4 = false;
    match container.as_str() {
        "TEXB0004" => {
            c.u32().ok()?;
            is_mp4 = c.u32().ok()? == 1;
        }
        "TEXB0003" => {
            c.u32().ok()?;
        }
        _ => {}
    }
    let has_comp = container != "TEXB0001";
    for _ in 0..images {
        for _ in 0..c.u32().ok()? {
            if container == "TEXB0004" && is_mp4 {
                c.u32().ok()?;
                c.u32().ok()?;
                c.null_string().ok()?;
                c.u32().ok()?;
            }
            c.u32().ok()?; // w
            c.u32().ok()?; // h
            if has_comp {
                c.u32().ok()?;
                c.u32().ok()?;
            }
            let size = c.u32().ok()? as usize;
            c.take(size).ok()?;
        }
    }
    let magic = c.magic8().ok()?;
    if magic != "TEXS0002" && magic != "TEXS0003" {
        return None;
    }
    let frames = c.u32().ok()?;
    let (mut frame_w, mut frame_h) = (0.0, 0.0);
    if magic == "TEXS0003" {
        frame_w = c.u32().ok()? as f32;
        frame_h = c.u32().ok()? as f32;
    }
    if frames == 0 {
        return None;
    }
    // first frame: image, frametime, x, y, w, _, _, h (exact sizes)
    if let Ok(raw) = c.take(32) {
        let f = |i: usize| f32::from_le_bytes(raw[i * 4..i * 4 + 4].try_into().unwrap());
        if f(4) > 0.0 && f(7) > 0.0 {
            frame_w = f(4);
            frame_h = f(7);
        }
    }
    (frame_w > 0.0 && frame_h > 0.0).then_some(EmbeddedSheet {
        frames,
        frame_w,
        frame_h,
    })
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpriteSheet {
    pub frames: u32,
    pub cols: u32,
    pub rows: u32,
}

/// Lê o sprite sheet do JSON companheiro do .tex (`spritesheetsequences`), dado o
/// tamanho da textura. Ex.: frames=64, cell 128×128 em 1024×1024 → 8×8.
pub fn parse_spritesheet(tex_json: &str, tex_w: u32, tex_h: u32) -> Option<SpriteSheet> {
    let v: serde_json::Value = serde_json::from_str(tex_json).ok()?;
    let seq = v.get("spritesheetsequences")?.as_array()?.first()?;
    let frames = seq.get("frames")?.as_u64()? as u32;
    let cw = seq.get("width")?.as_u64()? as u32;
    let ch = seq.get("height")?.as_u64()? as u32;
    if frames == 0 || cw == 0 || ch == 0 {
        return None;
    }
    let cols = (tex_w / cw).max(1);
    let rows = (tex_h / ch).max(1);
    Some(SpriteSheet { frames, cols, rows })
}

pub struct DecodedTexture {
    pub width: u32, // dims do buffer decodificado (pode vir com padding)
    pub height: u32,
    pub real_width: u32, // região "de conteúdo" (dims da imagem no header)
    pub real_height: u32,
    pub rgba: Vec<u8>, // width*height*4
    /// Header flags: bit 0 = no interpolation (nearest filter), bit 1 = clamp UVs.
    pub flags: u32,
    /// WE texture format id (0 = RGBA8888, 4/6/7 = DXT5/3/1, 8 = RG88, 9 = R8;
    /// embedded JPEG/PNG count as 0). Shaders get it as the TEXnFORMAT combo.
    pub format: u32,
    /// Mipmap levels stored in the file. 1 = the texture is meant to be
    /// sampled without mipmaps (e.g. util/noise, sampled at high frequency).
    pub mip_count: u32,
}

struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn u32(&mut self) -> Result<u32, String> {
        let b = self.data.get(self.pos..self.pos + 4).ok_or("EOF (u32)")?;
        self.pos += 4;
        Ok(u32::from_le_bytes(b.try_into().unwrap()))
    }
    // Magic de 9 bytes (8 chars + null). Confere os 8 primeiros.
    fn magic8(&mut self) -> Result<String, String> {
        let b = self.data.get(self.pos..self.pos + 9).ok_or("EOF (magic)")?;
        self.pos += 9;
        Ok(String::from_utf8_lossy(&b[..8]).into_owned())
    }
    fn null_string(&mut self) -> Result<(), String> {
        while *self.data.get(self.pos).ok_or("EOF (str)")? != 0 {
            self.pos += 1;
        }
        self.pos += 1; // pula o \0
        Ok(())
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let b = self.data.get(self.pos..self.pos + n).ok_or("EOF (take)")?;
        self.pos += n;
        Ok(b)
    }
}

pub fn parse(data: &[u8]) -> Result<DecodedTexture, String> {
    let mut c = Cursor { data, pos: 0 };

    // --- header ---
    let v = c.magic8()?;
    if v != "TEXV0005" {
        return Err(format!("container inesperado: {v:?}"));
    }
    let vi = c.magic8()?;
    if vi != "TEXI0001" {
        return Err(format!("sub-container inesperado: {vi:?}"));
    }
    let format = c.u32()?;
    let flags = c.u32()?;
    let _tex_w = c.u32()?;
    let _tex_h = c.u32()?;
    let width = c.u32()?; // dims reais da imagem
    let height = c.u32()?;
    let _ignore = c.u32()?;

    // --- container ---
    let container = c.magic8()?;
    let _image_count = c.u32()?;
    let mut free_image_format = FIF_UNKNOWN;
    let mut is_mp4 = false;
    match container.as_str() {
        "TEXB0004" => {
            free_image_format = c.u32()?;
            is_mp4 = c.u32()? == 1;
        }
        "TEXB0003" => {
            free_image_format = c.u32()?;
        }
        "TEXB0002" | "TEXB0001" => {}
        other => return Err(format!("container de bitmap desconhecido: {other:?}")),
    }
    let has_comp_fields = matches!(container.as_str(), "TEXB0004" | "TEXB0003" | "TEXB0002");
    let is_texb0004 = container == "TEXB0004";

    // --- primeiro mipmap (o maior) da primeira imagem ---
    let mip_count = c.u32()?;
    // The extra per-mipmap header only exists for video (mp4) textures; for
    // regular TEXB0004 images the width/height follow the mipmap count directly.
    if is_texb0004 && is_mp4 {
        let _ = c.u32()?;
        let _ = c.u32()?;
        c.null_string()?; // json do editor
        let _ = c.u32()?;
    }
    let mip_w = c.u32()?;
    let mip_h = c.u32()?;

    let mut compression = 0u32;
    let mut uncompressed_size = 0u32;
    if has_comp_fields {
        compression = c.u32()?;
        uncompressed_size = c.u32()?;
    }
    let compressed_size = c.u32()?;
    if compression == 0 {
        uncompressed_size = compressed_size;
    }

    // bytes do mipmap (descomprimidos se LZ4)
    let raw = if compression == 1 {
        let comp = c.take(compressed_size as usize)?;
        lz4_flex::block::decompress(comp, uncompressed_size as usize)
            .map_err(|e| format!("falha LZ4: {e}"))?
    } else {
        c.take(uncompressed_size as usize)?.to_vec()
    };

    // --- converte pra RGBA ---
    // Caso free-image (JPEG/PNG embutido): a crate image decodifica.
    if free_image_format != FIF_UNKNOWN {
        let img = image::load_from_memory(&raw)
            .map_err(|e| format!("falha ao decodificar imagem embutida: {e}"))?
            .to_rgba8();
        let (w, h) = img.dimensions();
        return Ok(DecodedTexture {
            flags,
            format: 0,
            mip_count,
            width: w,
            height: h,
            real_width: width,
            real_height: height,
            rgba: img.into_raw(),
        });
    }

    let make = |rgba: Vec<u8>| {
        Ok(DecodedTexture {
            flags,
            format,
            mip_count,
            width: mip_w,
            height: mip_h,
            real_width: width,
            real_height: height,
            rgba,
        })
    };

    match format {
        // ARGB8888: apesar do nome, os bytes já estão em RGBA (o WE faz upload como
        // GL_RGBA, não GL_BGRA) — sem troca de canais.
        FORMAT_ARGB8888 => make(raw),
        // RG88: 2 bytes/pixel. WE guarda a luminância em R e o alpha em G. Expande
        // pra rgb = R (cinza, a cor vem do tint da partícula) e a = G.
        FORMAT_RG88 => {
            let mut rgba = Vec::with_capacity(raw.len() * 2);
            for px in raw.as_chunks::<2>().0 {
                rgba.extend_from_slice(&[px[0], px[0], px[0], px[1]]);
            }
            make(rgba)
        }
        // R8: 1 byte/pixel, usado como máscara (luminância = alpha).
        FORMAT_R8 => {
            let mut rgba = Vec::with_capacity(raw.len() * 4);
            for &r in &raw {
                rgba.extend_from_slice(&[r, r, r, r]);
            }
            make(rgba)
        }
        // DXT/BC (S3TC): blocos 4x4 comprimidos -> RGBA via texpresso.
        FORMAT_DXT1 | FORMAT_DXT3 | FORMAT_DXT5 => {
            let bc = match format {
                FORMAT_DXT1 => texpresso::Format::Bc1,
                FORMAT_DXT3 => texpresso::Format::Bc2,
                _ => texpresso::Format::Bc3,
            };
            let mut rgba = vec![0u8; (mip_w * mip_h * 4) as usize];
            bc.decompress(&raw, mip_w as usize, mip_h as usize, &mut rgba);
            make(rgba)
        }
        other => Err(format!(
            "formato de textura {other} ainda não suportado (ARGB8888/RG88/R8/DXT/free-image)"
        )),
    }
}

#[cfg(test)]
mod sheet_tests {
    use super::*;

    /// TEXV/TEXI header, one 1-mip TEXB0001 image, then `sheet`.
    fn tex_with(sheet: &[u8]) -> Vec<u8> {
        let mut d = Vec::new();
        let u = |d: &mut Vec<u8>, v: u32| d.extend_from_slice(&v.to_le_bytes());
        d.extend_from_slice(b"TEXV0005\0TEXI0001\0");
        for _ in 0..7 {
            u(&mut d, 0);
        }
        d.extend_from_slice(b"TEXB0001\0");
        u(&mut d, 1); // images
        u(&mut d, 1); // mips
        u(&mut d, 4); // w
        u(&mut d, 4); // h
        u(&mut d, 2); // byte size
        d.extend_from_slice(&[0, 0]);
        d.extend_from_slice(sheet);
        d
    }

    #[test]
    fn texs0003_is_count_then_size_with_exact_frame_records() {
        let mut s = b"TEXS0003\0".to_vec();
        for v in [5u32, 102, 128] {
            s.extend_from_slice(&v.to_le_bytes());
        }
        s.extend_from_slice(&0u32.to_le_bytes());
        for v in [0.2f32, 0.0, 0.0, 102.5, 0.0, 0.0, 128.0] {
            s.extend_from_slice(&v.to_le_bytes());
        }
        let e = embedded_sheet(&tex_with(&s)).unwrap();
        assert_eq!((e.frames, e.frame_w, e.frame_h), (5, 102.5, 128.0));
    }
}
