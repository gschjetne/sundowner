//! JPEG and PNG loading for embedding as PDF image XObjects.
//!
//! JPEGs are embedded as-is (DCTDecode). Opaque PNGs are embedded without
//! decoding by handing the IDAT stream to FlateDecode with the PNG predictor.
//! PNGs with an alpha channel are decoded so the alpha can become a soft mask.

use crate::flate;

/// Largest image file that will be read.
pub const MAX_FILE_BYTES: u64 = 64 << 20;
/// Largest total of embedded image data per document.
pub const MAX_TOTAL_BYTES: usize = 256 << 20;
/// Largest image dimensions accepted, for every format and code path.
pub const MAX_SIDE: u32 = 20_000;
pub const MAX_PIXELS: u64 = 64_000_000;
/// Refuse images whose decoded size would exceed this many bytes.
const MAX_DECODED: usize = 512 << 20;

/// Read an image file, refusing anything that is not a regular file or is
/// larger than [`MAX_FILE_BYTES`].
pub fn read_file(path: &std::path::Path) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let meta = file.metadata().map_err(|e| e.to_string())?;
    if !meta.is_file() {
        return Err("not a regular file".into());
    }
    if meta.len() > MAX_FILE_BYTES {
        return Err(format!("file is larger than {} MiB", MAX_FILE_BYTES >> 20));
    }
    let mut buf = Vec::with_capacity(meta.len() as usize);
    file.take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut buf)
        .map_err(|e| e.to_string())?;
    if buf.len() as u64 > MAX_FILE_BYTES {
        return Err(format!("file is larger than {} MiB", MAX_FILE_BYTES >> 20));
    }
    Ok(buf)
}

fn check_dimensions(width: u32, height: u32) -> Result<(), String> {
    if width == 0 || height == 0 {
        return Err("image has zero width or height".into());
    }
    if width > MAX_SIDE || height > MAX_SIDE || width as u64 * height as u64 > MAX_PIXELS {
        return Err(format!("image is too large ({width}x{height} pixels)"));
    }
    Ok(())
}

pub struct Image {
    pub width: u32,
    pub height: u32,
    /// Extra entries for the image dictionary (ColorSpace, Filter, ...).
    pub dict: String,
    pub data: Vec<u8>,
    /// Optional 8-bit grayscale soft mask, zlib-compressed.
    pub smask: Option<Vec<u8>>,
}

pub fn load(bytes: &[u8]) -> Result<Image, String> {
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(format!("file is larger than {} MiB", MAX_FILE_BYTES >> 20));
    }
    if bytes.starts_with(&[0xFF, 0xD8]) {
        jpeg(bytes)
    } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        png(bytes)
    } else {
        Err("unsupported image format (only PNG and JPEG are supported)".into())
    }
}

fn be16(b: &[u8], i: usize) -> Option<u32> {
    Some(u16::from_be_bytes([*b.get(i)?, *b.get(i + 1)?]) as u32)
}

fn be32(b: &[u8], i: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(i..i + 4)?.try_into().ok()?))
}

fn jpeg(b: &[u8]) -> Result<Image, String> {
    let bad = || "malformed JPEG".to_string();
    let mut i = 2;
    let mut adobe = false;
    loop {
        while b.get(i) == Some(&0xFF) && b.get(i + 1) == Some(&0xFF) {
            i += 1;
        }
        if b.get(i) != Some(&0xFF) {
            return Err(bad());
        }
        let marker = *b.get(i + 1).ok_or_else(bad)?;
        i += 2;
        if marker == 0xD8 || marker == 0x01 || (0xD0..=0xD7).contains(&marker) {
            continue;
        }
        if marker == 0xD9 || marker == 0xDA {
            return Err(bad());
        }
        let len = be16(b, i).ok_or_else(bad)? as usize;
        if len < 2 {
            return Err(bad());
        }
        let is_sof = (0xC0..=0xCF).contains(&marker) && ![0xC4, 0xC8, 0xCC].contains(&marker);
        if is_sof {
            let bpc = *b.get(i + 2).ok_or_else(bad)?;
            let height = be16(b, i + 3).ok_or_else(bad)?;
            let width = be16(b, i + 5).ok_or_else(bad)?;
            let comps = *b.get(i + 7).ok_or_else(bad)?;
            check_dimensions(width, height)?;
            if bpc != 8 {
                return Err("unsupported JPEG precision".into());
            }
            let cs = match comps {
                1 => "/DeviceGray",
                3 => "/DeviceRGB",
                // Adobe-written CMYK JPEGs store inverted values.
                4 if adobe => "/DeviceCMYK /Decode [1 0 1 0 1 0 1 0]",
                4 => "/DeviceCMYK",
                _ => return Err("unsupported JPEG component count".into()),
            };
            return Ok(Image {
                width,
                height,
                dict: format!("/ColorSpace {cs} /BitsPerComponent 8 /Filter /DCTDecode"),
                data: b.to_vec(),
                smask: None,
            });
        }
        if marker == 0xEE && b.get(i + 2..i + 7) == Some(b"Adobe") {
            adobe = true;
        }
        i += len;
    }
}

fn png(b: &[u8]) -> Result<Image, String> {
    let bad = |m: &str| format!("malformed PNG: {m}");
    let mut i = 8;
    let mut ihdr = None;
    let mut palette: &[u8] = &[];
    let mut idat = Vec::new();
    while i + 8 <= b.len() {
        let len = be32(b, i).ok_or_else(|| bad("chunk"))? as usize;
        let kind = &b[i + 4..i + 8];
        let data = b
            .get(i + 8..i.saturating_add(8).saturating_add(len))
            .ok_or_else(|| bad("truncated chunk"))?;
        match kind {
            b"IHDR" => ihdr = Some(data),
            b"PLTE" => palette = data,
            b"IDAT" => idat.extend_from_slice(data),
            b"IEND" => break,
            _ => {}
        }
        i += 12 + len;
    }
    let h = ihdr
        .filter(|h| h.len() >= 13)
        .ok_or_else(|| bad("missing IHDR"))?;
    let width = be32(h, 0).unwrap_or(0);
    let height = be32(h, 4).unwrap_or(0);
    let (depth, color, interlace) = (h[8], h[9], h[12]);
    check_dimensions(width, height)?;
    if idat.is_empty() {
        return Err(bad("no image data"));
    }
    let channels: u32 = match (color, depth) {
        (0, 1 | 2 | 4 | 8 | 16) => 1,
        (2, 8 | 16) => 3,
        (3, 1 | 2 | 4 | 8) => 1,
        (4, 8 | 16) => 2,
        (6, 8 | 16) => 4,
        _ => return Err(bad("unsupported color type / bit depth")),
    };
    if interlace != 0 {
        return Err("interlaced PNGs are not supported".into());
    }

    if color == 0 || color == 2 || color == 3 {
        let cs = match color {
            0 => "/DeviceGray".to_string(),
            2 => "/DeviceRGB".to_string(),
            _ => {
                let n = palette.len() / 3;
                if n == 0 || n > 256 {
                    return Err(bad("bad palette"));
                }
                let hex: String = palette[..n * 3].iter().map(|x| format!("{x:02X}")).collect();
                format!("[/Indexed /DeviceRGB {} <{hex}>]", n - 1)
            }
        };
        let colors = if color == 2 { 3 } else { 1 };
        return Ok(Image {
            width,
            height,
            dict: format!(
                "/ColorSpace {cs} /BitsPerComponent {depth} /Filter /FlateDecode \
                 /DecodeParms << /Predictor 15 /Colors {colors} /BitsPerComponent {depth} /Columns {width} >>"
            ),
            data: idat,
            smask: None,
        });
    }

    // Gray+alpha or RGBA: decode, unfilter and split off the alpha channel.
    let bytes_pp = (channels * depth as u32 / 8) as usize;
    let row = (width as usize)
        .checked_mul(bytes_pp)
        .ok_or_else(|| bad("too large"))?;
    let total = (row + 1)
        .checked_mul(height as usize)
        .filter(|&t| t <= MAX_DECODED)
        .ok_or_else(|| bad("too large"))?;
    let raw = flate::zlib_decompress(&idat, total).map_err(&bad)?;
    if raw.len() < total {
        return Err(bad("image data too short"));
    }
    let pixels = unfilter(&raw, row, height as usize, bytes_pp).ok_or_else(|| bad("bad filter"))?;
    let step = depth as usize / 8; // 1 or 2 bytes per sample; keep the high byte
    let color_ch = channels as usize - 1;
    let npx = width as usize * height as usize;
    let mut rgb = Vec::with_capacity(npx * color_ch);
    let mut alpha = Vec::with_capacity(npx);
    for px in pixels.chunks_exact(bytes_pp) {
        for c in 0..color_ch {
            rgb.push(px[c * step]);
        }
        alpha.push(px[color_ch * step]);
    }
    let cs = if color_ch == 1 {
        "/DeviceGray"
    } else {
        "/DeviceRGB"
    };
    let opaque = alpha.iter().all(|&a| a == 255);
    Ok(Image {
        width,
        height,
        dict: format!("/ColorSpace {cs} /BitsPerComponent 8 /Filter /FlateDecode"),
        data: flate::zlib_compress(&rgb),
        smask: if opaque {
            None
        } else {
            Some(flate::zlib_compress(&alpha))
        },
    })
}

fn unfilter(raw: &[u8], row: usize, height: usize, bpp: usize) -> Option<Vec<u8>> {
    let mut out = vec![0u8; row * height];
    for y in 0..height {
        let src = raw.get(y * (row + 1)..(y + 1) * (row + 1))?;
        let filter = src[0];
        let src = &src[1..];
        let (done, rest) = out.split_at_mut(y * row);
        let prev: &[u8] = if y == 0 { &[] } else { &done[(y - 1) * row..] };
        let cur = &mut rest[..row];
        for x in 0..row {
            let a = if x >= bpp { cur[x - bpp] as i16 } else { 0 };
            let b = prev.get(x).copied().unwrap_or(0) as i16;
            let c = if x >= bpp {
                prev.get(x - bpp).copied().unwrap_or(0) as i16
            } else {
                0
            };
            let pred = match filter {
                0 => 0,
                1 => a,
                2 => b,
                3 => (a + b) / 2,
                4 => {
                    let p = a + b - c;
                    let (pa, pb, pc) = ((p - a).abs(), (p - b).abs(), (p - c).abs());
                    if pa <= pb && pa <= pc {
                        a
                    } else if pb <= pc {
                        b
                    } else {
                        c
                    }
                }
                _ => return None,
            };
            cur[x] = src[x].wrapping_add(pred as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(out: &mut Vec<u8>, kind: &[u8], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        out.extend_from_slice(&[0, 0, 0, 0]); // CRC is not checked
    }

    fn make_png(color: u8, depth: u8, w: u32, h: u32, raw: &[u8]) -> Vec<u8> {
        let mut p = b"\x89PNG\r\n\x1a\n".to_vec();
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&w.to_be_bytes());
        ihdr.extend_from_slice(&h.to_be_bytes());
        ihdr.extend_from_slice(&[depth, color, 0, 0, 0]);
        chunk(&mut p, b"IHDR", &ihdr);
        chunk(&mut p, b"IDAT", &flate::zlib_compress(raw));
        chunk(&mut p, b"IEND", &[]);
        p
    }

    #[test]
    fn rgba_png_splits_alpha() {
        // 2x2 RGBA, rows filtered with None and Sub.
        let raw = [0, 255, 0, 0, 128, 0, 255, 0, 255, 1, 0, 0, 255, 255, 0, 0, 0, 0];
        let img = load(&make_png(6, 8, 2, 2, &raw)).unwrap();
        assert_eq!((img.width, img.height), (2, 2));
        let rgb = flate::zlib_decompress(&img.data, 100).unwrap();
        assert_eq!(rgb, [255, 0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 255]);
        let a = flate::zlib_decompress(img.smask.as_ref().unwrap(), 100).unwrap();
        assert_eq!(a, [128, 255, 255, 255]);
    }

    #[test]
    fn rgb_png_passes_through() {
        let img = load(&make_png(2, 8, 1, 1, &[0, 1, 2, 3])).unwrap();
        assert!(img.dict.contains("/Predictor 15"));
        assert!(img.smask.is_none());
    }

    #[test]
    fn jpeg_header() {
        let mut j = vec![
            0xFF, 0xD8, 0xFF, 0xE0, 0, 4, 0, 0, 0xFF, 0xC0, 0, 11, 8, 0, 20, 0, 30, 3,
        ];
        j.extend_from_slice(&[0; 9]);
        let img = load(&j).unwrap();
        assert_eq!((img.width, img.height), (30, 20));
    }

    #[test]
    fn oversized_images_are_rejected() {
        let big = make_png(2, 8, MAX_SIDE + 1, 1, &[0; 8]);
        assert!(load(&big).err().unwrap().contains("too large"));
        let big = make_png(0, 8, 10_000, 10_000, &[0; 8]);
        assert!(load(&big).err().unwrap().contains("too large"));
        let mut j = vec![0xFF, 0xD8, 0xFF, 0xC0, 0, 11, 8, 0x4E, 0x21, 0x4E, 0x21, 3];
        j.extend_from_slice(&[0; 9]);
        assert!(load(&j).err().unwrap().contains("too large"));
    }

    #[test]
    fn garbage_is_rejected_not_panicking() {
        let mut seed = 7u64;
        for n in 0..3000 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let mut data: Vec<u8> = (0..(seed % 200)).map(|k| (seed >> (k % 56)) as u8).collect();
            let prefix: &[u8] = if n % 2 == 0 {
                b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR"
            } else {
                &[0xFF, 0xD8, 0xFF]
            };
            data.splice(0..0, prefix.iter().copied());
            let _ = load(&data);
        }
    }
}
