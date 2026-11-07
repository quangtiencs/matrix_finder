//! Camera RAW files (CR2, NEF, ARW, DNG). They are all TIFF-based and carry a
//! JPEG preview the camera made, which is what is shown: no RAW development.
use std::collections::HashSet;
use std::io::{Read, Seek, SeekFrom};

const MAX_ENTRIES: u16 = 2000;
const MAX_IFDS: usize = 64;
const MAX_JPEG: u32 = 128 * 1024 * 1024;

#[derive(Default, Debug)]
pub struct RawInfo {
    pub make: Option<String>,
    pub model: Option<String>,
    pub lens: Option<String>,
    pub date: Option<String>,
    pub exposure: Option<String>,
    pub aperture: Option<String>,
    pub iso: Option<u32>,
    pub focal: Option<String>,
    /// The largest uncompressed-or-raw image the file describes.
    pub sensor: Option<(u32, u32)>,
    /// EXIF orientation of the main image (1 to 8).
    pub orientation: u16,
    /// Where embedded JPEGs may be: (offset, length), largest first.
    previews: Vec<(u64, u32)>,
}

struct Tiff<'a, R> {
    r: &'a mut R,
    little: bool,
}

#[derive(Clone, Copy)]
struct Entry {
    tag: u16,
    kind: u16,
    count: u32,
    /// The raw 4 value bytes: the value itself if it fits, or else its offset.
    value: [u8; 4],
}

fn type_size(kind: u16) -> u32 {
    match kind {
        1 | 2 | 6 | 7 => 1,
        3 | 8 => 2,
        4 | 9 | 11 | 13 => 4,
        5 | 10 | 12 => 8,
        _ => 0,
    }
}

impl<R: Read + Seek> Tiff<'_, R> {
    fn u16_of(&self, b: [u8; 2]) -> u16 {
        if self.little { u16::from_le_bytes(b) } else { u16::from_be_bytes(b) }
    }

    fn u32_of(&self, b: [u8; 4]) -> u32 {
        if self.little { u32::from_le_bytes(b) } else { u32::from_be_bytes(b) }
    }

    fn bytes(&mut self, at: u64, n: usize) -> Option<Vec<u8>> {
        self.r.seek(SeekFrom::Start(at)).ok()?;
        let mut b = vec![0u8; n];
        self.r.read_exact(&mut b).ok()?;
        Some(b)
    }

    fn entries(&mut self, at: u64) -> Option<(Vec<Entry>, u32)> {
        let n = self.bytes(at, 2)?;
        let n = self.u16_of([n[0], n[1]]);
        if n > MAX_ENTRIES {
            return None;
        }
        let raw = self.bytes(at + 2, usize::from(n) * 12 + 4)?;
        let entries = raw
            .chunks_exact(12)
            .map(|e| Entry {
                tag: self.u16_of([e[0], e[1]]),
                kind: self.u16_of([e[2], e[3]]),
                count: self.u32_of([e[4], e[5], e[6], e[7]]),
                value: [e[8], e[9], e[10], e[11]],
            })
            .collect();
        let tail = &raw[raw.len() - 4..];
        Some((entries, self.u32_of([tail[0], tail[1], tail[2], tail[3]])))
    }

    /// An entry's values as integers (at most `limit`).
    fn uints(&mut self, e: Entry, limit: u32) -> Vec<u32> {
        let size = type_size(e.kind);
        if !matches!(e.kind, 1 | 3 | 4 | 13) || e.count == 0 {
            return vec![];
        }
        let count = e.count.min(limit);
        let data = if size * e.count <= 4 {
            e.value.to_vec()
        } else {
            let at = u64::from(self.u32_of(e.value));
            match self.bytes(at, (size * count) as usize) {
                Some(b) => b,
                None => return vec![],
            }
        };
        data.chunks_exact(size as usize)
            .take(count as usize)
            .map(|c| match size {
                1 => u32::from(c[0]),
                2 => u32::from(self.u16_of([c[0], c[1]])),
                _ => self.u32_of([c[0], c[1], c[2], c[3]]),
            })
            .collect()
    }

    fn uint(&mut self, e: Entry) -> Option<u32> {
        self.uints(e, 1).first().copied()
    }

    fn text(&mut self, e: Entry) -> Option<String> {
        if e.kind != 2 || e.count == 0 || e.count > 4096 {
            return None;
        }
        let data = if e.count <= 4 { e.value[..e.count as usize].to_vec() } else { self.bytes(u64::from(self.u32_of(e.value)), e.count as usize)? };
        let end = data.iter().position(|b| *b == 0).unwrap_or(data.len());
        let s = String::from_utf8_lossy(&data[..end]).trim().to_string();
        (!s.is_empty()).then_some(s)
    }

    fn rational(&mut self, e: Entry) -> Option<(u32, u32)> {
        if !matches!(e.kind, 5 | 10) || e.count == 0 {
            return None;
        }
        let b = self.bytes(u64::from(self.u32_of(e.value)), 8)?;
        Some((self.u32_of([b[0], b[1], b[2], b[3]]), self.u32_of([b[4], b[5], b[6], b[7]])))
    }
}

/// Read a RAW file's facts. `None` if it is not TIFF-based.
pub fn parse<R: Read + Seek>(r: &mut R) -> Option<RawInfo> {
    let mut head = [0u8; 8];
    r.seek(SeekFrom::Start(0)).ok()?;
    r.read_exact(&mut head).ok()?;
    let little = match &head[..2] {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    let mut t = Tiff { r, little };
    if t.u16_of([head[2], head[3]]) != 42 {
        return None;
    }
    let mut info = RawInfo { orientation: 1, ..RawInfo::default() };
    let mut queue = vec![(t.u32_of([head[4], head[5], head[6], head[7]]), true)];
    let mut seen = HashSet::new();
    let mut sensor_pixels = 0u64;
    while let Some((at, first)) = queue.pop() {
        if at == 0 || seen.len() >= MAX_IFDS || !seen.insert(at) {
            continue;
        }
        let Some((entries, next)) = t.entries(u64::from(at)) else { continue };
        queue.push((next, false));
        let (mut width, mut height, mut compression) = (None, None, None);
        let (mut jpeg_at, mut jpeg_len, mut strip_at, mut strip_len) = (None, None, vec![], vec![]);
        for e in entries {
            match e.tag {
                0x0100 => width = t.uint(e),
                0x0101 => height = t.uint(e),
                0x0103 => compression = t.uint(e),
                0x010F => info.make = info.make.take().or_else(|| t.text(e)),
                0x0110 => info.model = info.model.take().or_else(|| t.text(e)),
                0x0112 if first => info.orientation = t.uint(e).unwrap_or(1) as u16,
                0x0111 => strip_at = t.uints(e, 2),
                0x0117 => strip_len = t.uints(e, 2),
                0x0132 => info.date = info.date.take().or_else(|| t.text(e)),
                0x014A => queue.extend(t.uints(e, 16).into_iter().map(|o| (o, false))),
                0x0201 => jpeg_at = t.uint(e),
                0x0202 => jpeg_len = t.uint(e),
                0x8769 => {
                    if let Some(exif) = t.uint(e) {
                        read_exif(&mut t, u64::from(exif), &mut info);
                    }
                }
                // DNG stores the camera's name here.
                0xC614 if info.model.is_none() => info.model = t.text(e),
                _ => {}
            }
        }
        if let (Some(a), Some(l)) = (jpeg_at, jpeg_len) {
            info.previews.push((u64::from(a), l));
        }
        // Strips holding one JPEG (compression 6 or 7; some are lossless raw data, which fail to decode).
        if matches!(compression, Some(6 | 7)) && strip_at.len() == 1 && strip_len.len() == 1 {
            info.previews.push((u64::from(strip_at[0]), strip_len[0]));
        }
        if let (Some(w), Some(h)) = (width, height) {
            let pixels = u64::from(w) * u64::from(h);
            if pixels > sensor_pixels {
                sensor_pixels = pixels;
                info.sensor = Some((w, h));
            }
        }
    }
    info.previews.retain(|(_, len)| *len > 2 && *len <= MAX_JPEG);
    info.previews.sort_by_key(|(_, len)| std::cmp::Reverse(*len));
    info.previews.dedup();
    Some(info)
}

fn read_exif<R: Read + Seek>(t: &mut Tiff<'_, R>, at: u64, info: &mut RawInfo) {
    let Some((entries, _)) = t.entries(at) else { return };
    for e in entries {
        match e.tag {
            0x829A => info.exposure = t.rational(e).and_then(format_exposure),
            0x829D => info.aperture = t.rational(e).filter(|(_, d)| *d > 0).map(|(n, d)| format!("f/{:.1}", f64::from(n) / f64::from(d))),
            0x8827 => info.iso = t.uint(e),
            0x9003 => info.date = t.text(e).or(info.date.take()),
            0x920A => info.focal = t.rational(e).filter(|(_, d)| *d > 0).map(|(n, d)| format!("{:.0} mm", f64::from(n) / f64::from(d))),
            0xA434 => info.lens = t.text(e),
            _ => {}
        }
    }
}

fn format_exposure((n, d): (u32, u32)) -> Option<String> {
    if n == 0 || d == 0 {
        return None;
    }
    let seconds = f64::from(n) / f64::from(d);
    Some(if seconds >= 1.0 { format!("{seconds:.1} s") } else { format!("1/{:.0} s", 1.0 / seconds) })
}

/// The largest embedded JPEG that decodes, turned the way the photo was taken.
pub fn preview_image<R: Read + Seek>(r: &mut R, info: &RawInfo) -> Option<image::DynamicImage> {
    for (at, len) in &info.previews {
        let mut bytes = vec![0u8; *len as usize];
        if r.seek(SeekFrom::Start(*at)).is_err() || r.read_exact(&mut bytes).is_err() || !bytes.starts_with(&[0xFF, 0xD8]) {
            continue;
        }
        if let Ok(mut img) = image::load_from_memory_with_format(&bytes, image::ImageFormat::Jpeg) {
            if let Some(o) = image::metadata::Orientation::from_exif(info.orientation.min(255) as u8) {
                img.apply_orientation(o);
            }
            return Some(img);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn jpeg(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbImage::from_pixel(w, h, image::Rgb([200, 40, 40]));
        let mut b = Cursor::new(Vec::new());
        img.write_to(&mut b, image::ImageFormat::Jpeg).unwrap();
        b.into_inner()
    }

    /// A little-endian TIFF: IFD0 with the names and a small JPEG, a SubIFD with a larger JPEG,
    /// an EXIF IFD, and an entry claiming a large sensor.
    fn raw_file() -> Vec<u8> {
        let small = jpeg(16, 8);
        let large = jpeg(120, 80);
        let mut f = b"II*\0".to_vec();
        f.extend(8u32.to_le_bytes());
        let mut extra: Vec<u8> = vec![];
        let extra_at = 512u32;
        let put = |extra: &mut Vec<u8>, b: &[u8]| {
            let at = extra_at + extra.len() as u32;
            extra.extend_from_slice(b);
            at
        };
        let make = put(&mut extra, b"Acme Cameras\0");
        let model = put(&mut extra, b"Model X100\0");
        let date = put(&mut extra, b"2026:01:02 03:04:05\0");
        let exposure = put(&mut extra, &[1, 0, 0, 0, 250, 0, 0, 0]);
        let aperture = put(&mut extra, &[28, 0, 0, 0, 10, 0, 0, 0]);
        let focal = put(&mut extra, &[50, 0, 0, 0, 1, 0, 0, 0]);
        let small_at = put(&mut extra, &small);
        let large_at = put(&mut extra, &large);
        let entry = |tag: u16, kind: u16, count: u32, value: u32| {
            let mut e = tag.to_le_bytes().to_vec();
            e.extend(kind.to_le_bytes());
            e.extend(count.to_le_bytes());
            e.extend(value.to_le_bytes());
            e
        };
        let ifd = |entries: Vec<Vec<u8>>| {
            let mut b = (entries.len() as u16).to_le_bytes().to_vec();
            b.extend(entries.concat());
            b.extend(0u32.to_le_bytes());
            b
        };
        let (sub_at, exif_at) = (200u32, 300u32);
        f.extend(ifd(vec![
            entry(0x0100, 4, 1, 6000),
            entry(0x0101, 4, 1, 4000),
            entry(0x010F, 2, 13, make),
            entry(0x0110, 2, 11, model),
            entry(0x0112, 3, 1, 6),
            entry(0x014A, 4, 1, sub_at),
            entry(0x0201, 4, 1, small_at),
            entry(0x0202, 4, 1, small.len() as u32),
            entry(0x8769, 4, 1, exif_at),
        ]));
        f.resize(sub_at as usize, 0);
        f.extend(ifd(vec![entry(0x0201, 4, 1, large_at), entry(0x0202, 4, 1, large.len() as u32)]));
        f.resize(exif_at as usize, 0);
        f.extend(ifd(vec![entry(0x829A, 5, 1, exposure), entry(0x829D, 5, 1, aperture), entry(0x8827, 3, 1, 400), entry(0x9003, 2, 20, date), entry(0x920A, 5, 1, focal)]));
        f.resize(extra_at as usize, 0);
        f.extend(extra);
        f
    }

    #[test]
    fn facts_and_the_largest_preview_turned_upright() {
        let bytes = raw_file();
        let mut r = Cursor::new(bytes);
        let info = parse(&mut r).unwrap();
        assert_eq!(info.make.as_deref(), Some("Acme Cameras"));
        assert_eq!(info.model.as_deref(), Some("Model X100"));
        assert_eq!(info.date.as_deref(), Some("2026:01:02 03:04:05"));
        assert_eq!(info.exposure.as_deref(), Some("1/250 s"));
        assert_eq!(info.aperture.as_deref(), Some("f/2.8"));
        assert_eq!(info.focal.as_deref(), Some("50 mm"));
        assert_eq!(info.iso, Some(400));
        assert_eq!(info.sensor, Some((6000, 4000)));
        // Orientation 6: a 120×80 JPEG shows as 80×120.
        let img = preview_image(&mut r, &info).unwrap();
        assert_eq!((img.width(), img.height()), (80, 120));
    }

    #[test]
    fn broken_files_do_not_panic() {
        let whole = raw_file();
        for bytes in [vec![], b"II".to_vec(), b"MM\0*\xff\xff\xff\xff".to_vec(), b"II*\0\x08\0\0\0\xff\xff".to_vec(), whole[..100].to_vec()] {
            let mut r = Cursor::new(bytes);
            if let Some(info) = parse(&mut r) {
                assert!(preview_image(&mut r, &info).is_none());
            }
        }
        // An IFD pointing at itself ends instead of looping.
        let mut looped = b"II*\0\x08\0\0\0\0\0\x08\0\0\0".to_vec();
        looped.resize(32, 0);
        assert!(parse(&mut Cursor::new(looped)).is_some());
    }
}
