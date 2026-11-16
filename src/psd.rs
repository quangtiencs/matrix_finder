//! Photoshop documents (PSD, PSB): the merged picture the file keeps after its layers,
//! read in place so that the layers, often most of the file, are stepped over. If that
//! picture cannot be read, the small thumbnail in the file's resources is shown instead.
use crate::i18n::t;
use crate::util::Res;
use std::io::{Read, Seek, SeekFrom};

/// Most a row-length table may take (one entry per row of every channel).
const MAX_ROW_TABLE: u64 = 64 * 1024 * 1024;

fn not_psd() -> String {
    t("Not a Photoshop file this preview can read.")
}

fn too_complex() -> String {
    t("This file is too complex to preview.")
}

struct In<R> {
    r: R,
}

impl<R: Read + Seek> In<R> {
    fn bytes(&mut self, n: usize) -> Res<Vec<u8>> {
        let mut b = vec![0u8; n];
        self.r.read_exact(&mut b).map_err(|_| t("This file is cut short."))?;
        Ok(b)
    }

    fn u16(&mut self) -> Res<u16> {
        let b = self.bytes(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }

    fn u32(&mut self) -> Res<u32> {
        let b = self.bytes(4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn u64(&mut self) -> Res<u64> {
        let b = self.bytes(8)?;
        Ok(u64::from_be_bytes(b.try_into().unwrap_or_default()))
    }

    fn skip(&mut self, n: u64) -> Res<()> {
        let n = i64::try_from(n).map_err(|_| not_psd())?;
        self.r.seek(SeekFrom::Current(n)).map_err(|e| e.to_string())?;
        Ok(())
    }

    fn pos(&mut self) -> Res<u64> {
        self.r.stream_position().map_err(|e| e.to_string())
    }
}

/// Unpack one PackBits row into `out`, `want` bytes long.
fn packbits(src: &[u8], want: usize, out: &mut Vec<u8>) {
    let start = out.len();
    let mut i = 0;
    while out.len() - start < want && i < src.len() {
        let n = src[i] as i8;
        i += 1;
        if n >= 0 {
            let count = n as usize + 1;
            let end = (i + count).min(src.len());
            out.extend_from_slice(&src[i..end]);
            i = end;
        } else if n != -128 {
            let count = (1 - isize::from(n)) as usize;
            if let Some(&v) = src.get(i) {
                out.extend(std::iter::repeat_n(v, count));
            }
            i += 1;
        }
    }
    // A damaged row is padded or cut to its length, so the rows after it stay in place.
    out.resize(start + want, 0);
}

/// The merged picture, or the thumbnail if that cannot be read. `max_bytes` caps the
/// decoded RGBA picture.
pub fn decode<R: Read + Seek>(r: R, max_bytes: u64) -> Res<image::DynamicImage> {
    let mut f = In { r };
    let head = f.bytes(26).map_err(|_| not_psd())?;
    if &head[..4] != b"8BPS" {
        return Err(not_psd());
    }
    let version = u16::from_be_bytes([head[4], head[5]]);
    let big = match version {
        1 => false,
        2 => true,
        _ => return Err(not_psd()),
    };
    let channels = usize::from(u16::from_be_bytes([head[12], head[13]]));
    let height = u32::from_be_bytes([head[14], head[15], head[16], head[17]]);
    let width = u32::from_be_bytes([head[18], head[19], head[20], head[21]]);
    let depth = u16::from_be_bytes([head[22], head[23]]);
    let mode = u16::from_be_bytes([head[24], head[25]]);
    if width == 0 || height == 0 || channels == 0 || channels > 56 {
        return Err(not_psd());
    }
    if u64::from(width) * u64::from(height) * 4 > max_bytes {
        return Err(t("This image is too large to preview."));
    }
    let palette_len = f.u32()?;
    let palette = if mode == 2 && palette_len == 768 { f.bytes(768)? } else {
        f.skip(u64::from(palette_len))?;
        vec![]
    };
    let resources_len = f.u32()?;
    let resources_at = f.pos()?;
    f.skip(u64::from(resources_len))?;
    let layers_len = if big { f.u64()? } else { u64::from(f.u32()?) };
    f.skip(layers_len)?;
    let merged = merged_image(&mut f, big, (width, height), channels, depth, mode, &palette, max_bytes);
    match merged {
        Ok(img) => Ok(img),
        Err(e) => {
            f.r.seek(SeekFrom::Start(resources_at)).map_err(|e| e.to_string())?;
            thumbnail(&mut f, resources_len).ok_or(e)
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn merged_image<R: Read + Seek>(f: &mut In<R>, big: bool, (width, height): (u32, u32), channels: usize, depth: u16, mode: u16, palette: &[u8], max_bytes: u64) -> Res<image::DynamicImage> {
    let base = match mode {
        // Grayscale, duotone (shown as its gray), indexed.
        1 | 8 | 2 => 1,
        3 => 3,
        4 => 4,
        _ => return Err(too_complex()),
    };
    if channels < base || !matches!(depth, 8 | 16) {
        return Err(too_complex());
    }
    // An extra channel after the colours is the picture's transparency.
    let alpha = channels > base && mode != 4 && mode != 2;
    let wanted = base + usize::from(alpha);
    let (w, h) = (width as usize, height as usize);
    let sample = usize::from(depth / 8);
    let row = w * sample;
    // The channels as read and the picture made of them are in memory together.
    if (row * h * wanted + w * h * 4) as u64 > max_bytes {
        return Err(t("This image is too large to preview."));
    }
    let compression = f.u16()?;
    let mut planes: Vec<u8> = Vec::with_capacity(row * h * wanted);
    match compression {
        0 => {
            for _ in 0..wanted {
                planes.extend(f.bytes(row * h)?);
            }
        }
        1 => {
            let rows = channels * h;
            let each = if big { 4 } else { 2 };
            if (rows * each) as u64 > MAX_ROW_TABLE {
                return Err(t("This image is too large to preview."));
            }
            let table = f.bytes(rows * each)?;
            let lengths: Vec<usize> = table
                .chunks_exact(each)
                .map(|c| if big { u32::from_be_bytes([c[0], c[1], c[2], c[3]]) as usize } else { usize::from(u16::from_be_bytes([c[0], c[1]])) })
                .collect();
            let mut packed = vec![];
            for len in &lengths[..wanted * h] {
                // A packed row is never much more than twice its unpacked size.
                if *len > row * 2 + 128 {
                    return Err(not_psd());
                }
                packed.resize(*len, 0);
                f.r.read_exact(&mut packed).map_err(|_| t("This file is cut short."))?;
                packbits(&packed, row, &mut planes);
            }
        }
        // ZIP-packed pictures, with or without prediction.
        _ => return Err(too_complex()),
    }
    let plane = row * h;
    let at = |c: usize, i: usize| planes[c * plane + i * sample];
    let mut rgba = Vec::with_capacity(w * h * 4);
    for i in 0..w * h {
        let a = if alpha { at(base, i) } else { 255 };
        let px = match mode {
            3 => [at(0, i), at(1, i), at(2, i)],
            4 => {
                // Stored as 255 minus the ink.
                let k = u16::from(at(3, i));
                let mix = |c: u8| (u16::from(c) * k / 255) as u8;
                [mix(at(0, i)), mix(at(1, i)), mix(at(2, i))]
            }
            2 => {
                let v = usize::from(at(0, i));
                match palette.len() {
                    768 => [palette[v], palette[256 + v], palette[512 + v]],
                    _ => [v as u8; 3],
                }
            }
            _ => [at(0, i); 3],
        };
        rgba.extend_from_slice(&[px[0], px[1], px[2], a]);
    }
    image::RgbaImage::from_raw(width, height, rgba).map(image::DynamicImage::ImageRgba8).ok_or_else(not_psd)
}

/// The JPEG thumbnail among the image resources (IDs 1036, or 1033 in old files).
fn thumbnail<R: Read + Seek>(f: &mut In<R>, len: u32) -> Option<image::DynamicImage> {
    let end = f.pos().ok()? + u64::from(len);
    while f.pos().ok()? + 12 <= end {
        if f.bytes(4).ok()? != b"8BIM" {
            return None;
        }
        let id = f.u16().ok()?;
        let name_len = u64::from(f.bytes(1).ok()?[0]);
        // The name and its length byte take an even number of bytes.
        f.skip(name_len + (name_len + 1) % 2).ok()?;
        let size = f.u32().ok()?;
        let padded = u64::from(size) + u64::from(size % 2);
        if matches!(id, 1033 | 1036) && size > 28 && size < 16 * 1024 * 1024 {
            let data = f.bytes(size as usize).ok()?;
            let img = image::load_from_memory_with_format(&data[28..], image::ImageFormat::Jpeg).ok()?;
            // Old files keep the colours in BGR order.
            return Some(if id == 1033 {
                let mut rgb = img.to_rgb8();
                rgb.pixels_mut().for_each(|p| p.0.swap(0, 2));
                image::DynamicImage::ImageRgb8(rgb)
            } else {
                img
            });
        }
        f.skip(padded).ok()?;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn psd(width: u32, height: u32, channels: u16, mode: u16, compression: u16, data: &[u8]) -> Vec<u8> {
        let mut v = b"8BPS".to_vec();
        v.extend(1u16.to_be_bytes());
        v.extend([0u8; 6]);
        v.extend(channels.to_be_bytes());
        v.extend(height.to_be_bytes());
        v.extend(width.to_be_bytes());
        v.extend(8u16.to_be_bytes());
        v.extend(mode.to_be_bytes());
        v.extend(0u32.to_be_bytes()); // colour mode data
        v.extend(0u32.to_be_bytes()); // resources
        v.extend(4u32.to_be_bytes()); // layers, skipped
        v.extend([9u8; 4]);
        v.extend(compression.to_be_bytes());
        v.extend_from_slice(data);
        v
    }

    #[test]
    fn reads_the_merged_picture() {
        // 2×1 RGB, raw: red plane, green plane, blue plane.
        let img = decode(Cursor::new(psd(2, 1, 3, 3, 0, &[255, 0, 0, 255, 10, 20])), u64::MAX).unwrap().to_rgba8();
        assert_eq!(img.get_pixel(0, 0).0, [255, 0, 10, 255]);
        assert_eq!(img.get_pixel(1, 0).0, [0, 255, 20, 255]);
        // PackBits: each 3-pixel row of two channels (gray and alpha) packed as one run.
        let mut data = vec![];
        data.extend(2u16.to_be_bytes());
        data.extend(2u16.to_be_bytes());
        data.extend([0xFE, 77, 0xFE, 128]);
        let img = decode(Cursor::new(psd(3, 1, 2, 1, 1, &data)), u64::MAX).unwrap().to_rgba8();
        assert_eq!(img.get_pixel(2, 0).0, [77, 77, 77, 128]);
    }

    #[test]
    fn refuses_what_it_cannot_read() {
        assert_eq!(decode(Cursor::new(b"not a psd at all, no no no".to_vec()), u64::MAX).err(), Some(not_psd()));
        assert!(decode(Cursor::new(psd(100_000, 100_000, 3, 3, 0, &[])), 384 << 20).unwrap_err().contains("too large"));
        // ZIP-packed, with no thumbnail to fall back on.
        assert_eq!(decode(Cursor::new(psd(2, 1, 3, 3, 2, &[0; 16])), u64::MAX).err(), Some(too_complex()));
        assert!(decode(Cursor::new(psd(2, 2, 3, 3, 0, &[1, 2])), u64::MAX).is_err(), "cut short");
    }
}
