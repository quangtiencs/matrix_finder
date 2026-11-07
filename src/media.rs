//! Photos and videos that need help from outside: HEIC and AVIF decoding and video
//! thumbnails use tools the system already has (ImageIO's `sips` and Quick Look's
//! `qlmanage` on macOS) or common ones (libheif, ImageMagick, FFmpeg). Video facts
//! come from reading the file's own headers, so they work without any tool.
use crate::util::Res;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Set when the preview that asked for this work is gone.
pub type Cancel = Arc<AtomicBool>;

/// One external tool at a time: moving the cursor through a folder of videos must not
/// start a process per file.
static TOOL_LOCK: Mutex<()> = Mutex::new(());

/// A program on PATH, or where package managers put it (apps started from the desktop
/// on macOS do not get the shell's PATH).
pub fn find_tool(name: &str) -> Option<PathBuf> {
    let on_path = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect::<Vec<_>>()).unwrap_or_default();
    let extra = ["/opt/homebrew/bin", "/usr/local/bin", "/opt/local/bin", "/usr/bin"].map(PathBuf::from);
    let names = if cfg!(windows) { vec![format!("{name}.exe"), name.to_string()] } else { vec![name.to_string()] };
    on_path.iter().chain(extra.iter()).find_map(|d| names.iter().map(|n| d.join(n)).find(|p| p.is_file()))
}

/// How long a tool may take before it is given up on.
const TOOL_TIMEOUT: Duration = Duration::from_secs(20);
/// Quick Look can hang on a file it should have refused.
const QUICK_LOOK_TIMEOUT: Duration = Duration::from_secs(6);

/// Run a tool to completion; false if it failed, took too long or is no longer wanted.
fn run_tool(mut cmd: Command, timeout: Duration, cancel: &Cancel) -> bool {
    let _one_at_a_time = TOOL_LOCK.lock();
    if cancel.load(Ordering::Relaxed) {
        return false;
    }
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let Ok(mut child) = cmd.spawn() else { return false };
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if started.elapsed() < timeout && !cancel.load(Ordering::Relaxed) => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
}

/// A private temporary folder, removed with everything in it when dropped.
pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new() -> Res<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("matrix_finder-preview-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        Ok(Self(dir))
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// ---------------------------------------------------------------- HEIC, HEIF, AVIF

/// Adds a tool's arguments for an input and an output file.
type AddArgs = fn(&mut Command, &Path, &Path);

/// Decode a HEIC, HEIF or AVIF file to PNG bytes with the first tool that manages it.
pub fn decode_heif(path: &Path, cancel: &Cancel) -> Res<Vec<u8>> {
    let tmp = TempDir::new()?;
    let out = tmp.path().join("out.png");
    let tools: [(&str, AddArgs); 5] = [
        ("sips", |c, i, o| {
            c.args(["-s", "format", "png"]).arg(i).arg("--out").arg(o);
        }),
        ("heif-dec", |c, i, o| {
            c.arg(i).arg(o);
        }),
        ("heif-convert", |c, i, o| {
            c.arg(i).arg(o);
        }),
        ("magick", |c, i, o| {
            c.arg(i).arg(o);
        }),
        ("ffmpeg", |c, i, o| {
            c.args(["-nostdin", "-v", "error", "-i"]).arg(i).args(["-frames:v", "1", "-y"]).arg(o);
        }),
    ];
    let mut found_a_tool = false;
    for (name, add_args) in tools {
        let Some(program) = find_tool(name) else { continue };
        found_a_tool = true;
        let mut cmd = Command::new(program);
        add_args(&mut cmd, path, &out);
        if run_tool(cmd, TOOL_TIMEOUT, cancel) {
            if let Ok(png) = std::fs::read(&out) {
                return Ok(png);
            }
        }
        if cancel.load(Ordering::Relaxed) {
            break;
        }
    }
    Err(if found_a_tool {
        crate::i18n::t("This image could not be decoded by the tools on this computer.")
    } else {
        crate::i18n::t("Previewing this image needs libheif, ImageMagick or FFmpeg.")
    })
}

// ---------------------------------------------------------------- video facts

#[derive(Default, Debug, Clone, PartialEq)]
pub struct VideoInfo {
    pub duration: Option<f64>,
    pub size: Option<(u32, u32)>,
    pub fps: Option<f64>,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
}

/// Read what a video file says about itself (MP4 and MOV, or Matroska and WebM).
pub fn video_info<R: Read + Seek>(r: &mut R) -> VideoInfo {
    let mut magic = [0u8; 4];
    if r.seek(SeekFrom::Start(0)).is_err() || r.read_exact(&mut magic).is_err() || r.seek(SeekFrom::Start(0)).is_err() {
        return VideoInfo::default();
    }
    if magic == [0x1A, 0x45, 0xDF, 0xA3] {
        matroska_info(r)
    } else {
        mp4_info(r)
    }
}

const MAX_HEADER_BOX: u64 = 64 * 1024 * 1024;

fn be_u32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn be_u64(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_be_bytes(b.get(at..at + 8)?.try_into().ok()?))
}

fn be_u16(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn mp4_info<R: Read + Seek>(r: &mut R) -> VideoInfo {
    // Walk the top-level boxes to `moov`, which may sit after the media data.
    let mut info = VideoInfo::default();
    let mut pos = 0u64;
    let end = r.seek(SeekFrom::End(0)).unwrap_or(0);
    while pos + 8 <= end {
        let mut head = [0u8; 16];
        if r.seek(SeekFrom::Start(pos)).is_err() || r.read_exact(&mut head[..8]).is_err() {
            break;
        }
        let mut size = u64::from(u32::from_be_bytes([head[0], head[1], head[2], head[3]]));
        let mut header = 8u64;
        if size == 1 {
            if r.read_exact(&mut head[8..16]).is_err() {
                break;
            }
            size = u64::from_be_bytes(head[8..16].try_into().unwrap_or_default());
            header = 16;
        } else if size == 0 {
            size = end - pos;
        }
        if size < header {
            break;
        }
        if &head[4..8] == b"moov" {
            let body = size - header;
            if body <= MAX_HEADER_BOX {
                let mut moov = vec![0u8; body as usize];
                if r.read_exact(&mut moov).is_ok() {
                    info = parse_moov(&moov);
                }
            }
            break;
        }
        pos = pos.saturating_add(size);
    }
    info
}

/// Child boxes of a box's body, as (type, body).
fn boxes(mut b: &[u8]) -> impl Iterator<Item = (&[u8], &[u8])> {
    std::iter::from_fn(move || {
        let size = be_u32(b, 0)? as usize;
        let kind = b.get(4..8)?;
        let (header, size) = match size {
            0 => (8, b.len()),
            1 => (16, usize::try_from(be_u64(b, 8)?).ok()?),
            n => (8, n),
        };
        if size < header || size > b.len() {
            return None;
        }
        let body = &b[header..size];
        b = &b[size..];
        Some((kind, body))
    })
}

#[derive(Default)]
struct Mp4Track {
    handler: [u8; 4],
    tkhd_size: Option<(u32, u32)>,
    rotated: bool,
    format: Option<[u8; 4]>,
    coded_size: Option<(u32, u32)>,
    seconds: Option<f64>,
    frames: u64,
}

fn parse_moov(moov: &[u8]) -> VideoInfo {
    let mut info = VideoInfo::default();
    let mut movie_seconds = None;
    let mut tracks = vec![];
    for (kind, body) in boxes(moov) {
        match kind {
            b"mvhd" => movie_seconds = header_seconds(body, 8, 16),
            b"trak" => tracks.push(parse_trak(body)),
            _ => {}
        }
    }
    let longest_track = tracks.iter().filter_map(|t| t.seconds).fold(None, |a: Option<f64>, s| Some(a.map_or(s, |a| a.max(s))));
    info.duration = movie_seconds.filter(|s| *s > 0.0).or(longest_track);
    if let Some(v) = tracks.iter().find(|t| &t.handler == b"vide") {
        let size = v.tkhd_size.filter(|(w, h)| *w > 0 && *h > 0).or(v.coded_size);
        info.size = size.map(|(w, h)| if v.rotated { (h, w) } else { (w, h) });
        info.video_codec = v.format.map(|f| codec_name(&f));
        info.fps = v.seconds.filter(|s| *s > 0.0 && v.frames > 0).map(|s| v.frames as f64 / s);
    }
    info.audio_codec = tracks.iter().find(|t| &t.handler == b"soun").and_then(|t| t.format).map(|f| codec_name(&f));
    info
}

/// Seconds from a box starting with version and flags, whose version 0 layout has the
/// timescale at `v0_scale`, and version 1 at `v1_scale`, followed by the duration.
fn header_seconds(b: &[u8], v0_scale: usize, v1_scale: usize) -> Option<f64> {
    let scale_at = 4 + if *b.first()? == 1 { v1_scale } else { v0_scale };
    let scale = be_u32(b, scale_at)?;
    let duration = if b[0] == 1 { be_u64(b, scale_at + 4)? } else { u64::from(be_u32(b, scale_at + 4)?) };
    (scale > 0 && duration != u64::MAX && duration != u64::from(u32::MAX)).then(|| duration as f64 / f64::from(scale))
}

fn parse_trak(trak: &[u8]) -> Mp4Track {
    let mut t = Mp4Track::default();
    for (kind, body) in boxes(trak) {
        match kind {
            b"tkhd" => {
                // Version 0 has 32-bit times, version 1 has 64-bit ones.
                let matrix = 4 + if body.first() == Some(&1) { 32 } else { 20 } + 8 + 8;
                t.rotated = be_u32(body, matrix) == Some(0) && be_u32(body, matrix + 4).is_some_and(|b| b != 0);
                if let (Some(w), Some(h)) = (be_u32(body, matrix + 36), be_u32(body, matrix + 40)) {
                    t.tkhd_size = Some((w >> 16, h >> 16));
                }
            }
            b"mdia" => parse_mdia(body, &mut t),
            _ => {}
        }
    }
    t
}

fn parse_mdia(mdia: &[u8], t: &mut Mp4Track) {
    for (kind, body) in boxes(mdia) {
        match kind {
            b"mdhd" => t.seconds = header_seconds(body, 8, 16),
            b"hdlr" => {
                if let Some(h) = body.get(8..12) {
                    t.handler.copy_from_slice(h);
                }
            }
            b"minf" => {
                for (kind, body) in boxes(body) {
                    if kind == b"stbl" {
                        parse_stbl(body, t);
                    }
                }
            }
            _ => {}
        }
    }
}

fn parse_stbl(stbl: &[u8], t: &mut Mp4Track) {
    for (kind, body) in boxes(stbl) {
        match kind {
            b"stsd" => {
                // Version and flags, entry count, then the first sample entry.
                if let Some(format) = body.get(12..16) {
                    t.format = format.try_into().ok();
                }
                // A visual sample entry: 8 box bytes, 8 reserved, 16 pre-defined, then the size.
                if let (Some(w), Some(h)) = (be_u16(body, 8 + 8 + 8 + 16), be_u16(body, 8 + 8 + 8 + 18)) {
                    t.coded_size = Some((u32::from(w), u32::from(h)));
                }
            }
            b"stts" => {
                let count = be_u32(body, 4).unwrap_or(0) as usize;
                t.frames = (0..count).filter_map(|i| be_u32(body, 8 + i * 8)).map(u64::from).sum();
            }
            _ => {}
        }
    }
}

fn codec_name(fourcc: &[u8; 4]) -> String {
    match fourcc {
        b"avc1" | b"avc3" => "H.264".into(),
        b"hvc1" | b"hev1" => "HEVC (H.265)".into(),
        b"av01" => "AV1".into(),
        b"vp09" => "VP9".into(),
        b"vp08" => "VP8".into(),
        b"mp4v" => "MPEG-4".into(),
        b"apch" | b"apcn" | b"apcs" | b"apco" | b"ap4h" | b"ap4x" => "Apple ProRes".into(),
        b"mp4a" => "AAC".into(),
        b"ac-3" => "AC-3".into(),
        b"ec-3" => "E-AC-3".into(),
        b"Opus" => "Opus".into(),
        b"fLaC" => "FLAC".into(),
        b"alac" => "Apple Lossless".into(),
        b"sowt" | b"twos" | b"lpcm" => "PCM".into(),
        other => String::from_utf8_lossy(other).trim().to_string(),
    }
}

// Matroska and WebM element IDs.
const SEGMENT: u32 = 0x1853_8067;
const INFO: u32 = 0x1549_A966;
const TRACKS: u32 = 0x1654_AE6B;
const CLUSTER: u32 = 0x1F43_B675;
const TIMECODE_SCALE: u32 = 0x2A_D7B1;
const DURATION: u32 = 0x4489;
const TRACK_ENTRY: u32 = 0xAE;
const TRACK_TYPE: u32 = 0x83;
const CODEC_ID: u32 = 0x86;
const DEFAULT_DURATION: u32 = 0x23_E383;
const VIDEO: u32 = 0xE0;
const PIXEL_WIDTH: u32 = 0xB0;
const PIXEL_HEIGHT: u32 = 0xBA;

/// An element's ID (with its marker bits) from the bytes at the start of `b`, and its length.
fn ebml_id(b: &[u8]) -> Option<(u32, usize)> {
    let first = *b.first()?;
    let len = first.leading_zeros() as usize + 1;
    if len > 4 {
        return None;
    }
    let bytes = b.get(..len)?;
    Some((bytes.iter().fold(0u32, |v, x| (v << 8) | u32::from(*x)), len))
}

/// An element's size and its length; `None` as the size means "unknown" (streamed files).
fn ebml_size(b: &[u8]) -> Option<(Option<u64>, usize)> {
    let first = *b.first()?;
    let len = first.leading_zeros() as usize + 1;
    if len > 8 {
        return None;
    }
    let bytes = b.get(..len)?;
    let mask = if len == 8 { 0 } else { 0xFFu8 >> len };
    let value = bytes[1..].iter().fold(u64::from(first & mask), |v, x| (v << 8) | u64::from(*x));
    let unknown = value == (1u64 << (7 * len)) - 1;
    Some(((!unknown).then_some(value), len))
}

/// The child elements of an element's body, as (ID, body).
fn ebml_children(mut b: &[u8]) -> impl Iterator<Item = (u32, &[u8])> {
    std::iter::from_fn(move || {
        let (id, id_len) = ebml_id(b)?;
        let (size, size_len) = ebml_size(b.get(id_len..)?)?;
        let start = id_len + size_len;
        let end = start + usize::try_from(size?).ok()?;
        let body = b.get(start..end)?;
        b = &b[end..];
        Some((id, body))
    })
}

fn ebml_uint(b: &[u8]) -> u64 {
    b.iter().take(8).fold(0u64, |v, x| (v << 8) | u64::from(*x))
}

fn matroska_info<R: Read + Seek>(r: &mut R) -> VideoInfo {
    let mut info = VideoInfo::default();
    let (mut raw_duration, mut scale) = (None, 1_000_000u64);
    let end = r.seek(SeekFrom::End(0)).unwrap_or(0);
    let mut pos = 0u64;
    let mut have_tracks = false;
    while pos < end {
        let mut head = [0u8; 12];
        if r.seek(SeekFrom::Start(pos)).is_err() {
            break;
        }
        let got = read_up_to(r, &mut head);
        let Some((id, id_len)) = ebml_id(&head[..got]) else { break };
        let Some((size, size_len)) = ebml_size(&head[id_len..got]) else { break };
        let body_at = pos + (id_len + size_len) as u64;
        match (id, size) {
            // Its children are what the walk continues with.
            (SEGMENT, _) => {
                pos = body_at;
                continue;
            }
            (CLUSTER, _) if have_tracks => break,
            (INFO | TRACKS, Some(size)) if size <= MAX_HEADER_BOX => {
                let mut body = vec![0u8; size as usize];
                if r.seek(SeekFrom::Start(body_at)).is_err() || r.read_exact(&mut body).is_err() {
                    break;
                }
                if id == INFO {
                    for (child, b) in ebml_children(&body) {
                        match child {
                            TIMECODE_SCALE => scale = ebml_uint(b).max(1),
                            DURATION => {
                                raw_duration = match b.len() {
                                    4 => Some(f64::from(f32::from_be_bytes(b.try_into().unwrap_or_default()))),
                                    8 => Some(f64::from_be_bytes(b.try_into().unwrap_or_default())),
                                    _ => None,
                                }
                            }
                            _ => {}
                        }
                    }
                } else {
                    have_tracks = true;
                    parse_matroska_tracks(&body, &mut info);
                }
            }
            _ => {}
        }
        let Some(size) = size else { break };
        pos = body_at.saturating_add(size);
    }
    info.duration = raw_duration.filter(|d| *d > 0.0).map(|d| d * scale as f64 / 1e9);
    info
}

fn read_up_to<R: Read>(r: &mut R, buf: &mut [u8]) -> usize {
    let mut got = 0;
    while got < buf.len() {
        match r.read(&mut buf[got..]) {
            Ok(0) | Err(_) => break,
            Ok(n) => got += n,
        }
    }
    got
}

fn parse_matroska_tracks(tracks: &[u8], info: &mut VideoInfo) {
    for (id, entry) in ebml_children(tracks) {
        if id != TRACK_ENTRY {
            continue;
        }
        let (mut kind, mut codec, mut size, mut frame_ns) = (0, None, (None, None), None);
        for (child, b) in ebml_children(entry) {
            match child {
                TRACK_TYPE => kind = ebml_uint(b),
                CODEC_ID => codec = Some(matroska_codec(&String::from_utf8_lossy(b))),
                DEFAULT_DURATION => frame_ns = Some(ebml_uint(b)),
                VIDEO => {
                    for (v, b) in ebml_children(b) {
                        match v {
                            PIXEL_WIDTH => size.0 = Some(ebml_uint(b) as u32),
                            PIXEL_HEIGHT => size.1 = Some(ebml_uint(b) as u32),
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
        if kind == 1 && info.video_codec.is_none() {
            info.video_codec = codec;
            info.size = size.0.zip(size.1);
            info.fps = frame_ns.filter(|n| *n > 0).map(|n| 1e9 / n as f64);
        } else if kind == 2 && info.audio_codec.is_none() {
            info.audio_codec = codec;
        }
    }
}

fn matroska_codec(id: &str) -> String {
    match id.trim_end_matches('\0') {
        "V_MPEG4/ISO/AVC" => "H.264".into(),
        "V_MPEGH/ISO/HEVC" => "HEVC (H.265)".into(),
        "V_VP8" => "VP8".into(),
        "V_VP9" => "VP9".into(),
        "V_AV1" => "AV1".into(),
        "A_AAC" => "AAC".into(),
        "A_OPUS" => "Opus".into(),
        "A_VORBIS" => "Vorbis".into(),
        "A_FLAC" => "FLAC".into(),
        "A_AC3" => "AC-3".into(),
        "A_EAC3" => "E-AC-3".into(),
        "A_MPEG/L3" => "MP3".into(),
        other => other.to_string(),
    }
}

/// `83` gives `1:23`, `3723.5` gives `1:02:03`.
pub fn format_duration(seconds: f64) -> String {
    let total = seconds.round() as u64;
    let (h, m, s) = (total / 3600, total / 60 % 60, total % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

// ---------------------------------------------------------------- video thumbnails

/// A frame of the video as PNG bytes: from FFmpeg, or else macOS's Quick Look.
pub fn video_thumbnail(path: &Path, duration: Option<f64>, cancel: &Cancel) -> Option<Vec<u8>> {
    const WIDTH: u32 = 1280;
    let tmp = TempDir::new().ok()?;
    if let Some(ffmpeg) = find_tool("ffmpeg") {
        let out = tmp.path().join("frame.png");
        // A little way in: first frames are often black.
        let at = duration.map_or(0.0, |d| (d * 0.1).min(30.0));
        let mut cmd = Command::new(ffmpeg);
        cmd.args(["-nostdin", "-v", "error", "-ss", &format!("{at:.2}"), "-i"]).arg(path);
        cmd.args(["-frames:v", "1", "-vf", &format!("scale='min({WIDTH},iw)':-2"), "-y"]).arg(&out);
        if run_tool(cmd, TOOL_TIMEOUT, cancel) {
            if let Ok(png) = std::fs::read(&out) {
                return Some(png);
            }
        }
    }
    // macOS has no reader for Matroska and WebM, and Quick Look hangs on them instead of failing.
    let quick_look_reads_it = !matches!(crate::util::ext_of(&path.to_string_lossy()).as_str(), "mkv" | "webm");
    if cfg!(target_os = "macos") && quick_look_reads_it && !cancel.load(Ordering::Relaxed) {
        let mut cmd = Command::new("/usr/bin/qlmanage");
        cmd.args(["-t", "-s", &WIDTH.to_string(), "-o"]).arg(tmp.path()).arg(path);
        if run_tool(cmd, QUICK_LOOK_TIMEOUT, cancel) {
            let name = format!("{}.png", path.file_name()?.to_string_lossy());
            if let Ok(png) = std::fs::read(tmp.path().join(name)) {
                return Some(png);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn mp4_box(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut b = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        b.extend_from_slice(kind);
        b.extend_from_slice(body);
        b
    }

    /// A video track of 50 frames over 2 s, 640×360, and optionally a 90° rotation.
    fn mp4_video_track(rotated: bool) -> Vec<u8> {
        let mut tkhd = vec![0u8; 84];
        let (a, b) = if rotated { (0u32, 0x1_0000u32) } else { (0x1_0000, 0) };
        tkhd[40..44].copy_from_slice(&a.to_be_bytes());
        tkhd[44..48].copy_from_slice(&b.to_be_bytes());
        tkhd[76..80].copy_from_slice(&(640u32 << 16).to_be_bytes());
        tkhd[80..84].copy_from_slice(&(360u32 << 16).to_be_bytes());
        let mut mdhd = vec![0u8; 24];
        mdhd[12..16].copy_from_slice(&1000u32.to_be_bytes());
        mdhd[16..20].copy_from_slice(&2000u32.to_be_bytes());
        let mut hdlr = vec![0u8; 24];
        hdlr[8..12].copy_from_slice(b"vide");
        let mut stsd = vec![0u8; 8];
        stsd.extend(mp4_box(b"avc1", &[0u8; 78]));
        let mut stts = vec![0u8; 8];
        stts[7] = 1;
        stts.extend(50u32.to_be_bytes());
        stts.extend(40u32.to_be_bytes());
        let stbl = [mp4_box(b"stsd", &stsd), mp4_box(b"stts", &stts)].concat();
        let minf = mp4_box(b"stbl", &stbl);
        let mdia = [mp4_box(b"mdhd", &mdhd), mp4_box(b"hdlr", &hdlr), mp4_box(b"minf", &minf)].concat();
        mp4_box(b"trak", &[mp4_box(b"tkhd", &tkhd), mp4_box(b"mdia", &mdia)].concat())
    }

    fn mp4_file(rotated: bool, moov_last: bool) -> Vec<u8> {
        let mut mvhd = vec![0u8; 100];
        mvhd[12..16].copy_from_slice(&600u32.to_be_bytes());
        mvhd[16..20].copy_from_slice(&1200u32.to_be_bytes());
        let moov = mp4_box(b"moov", &[mp4_box(b"mvhd", &mvhd), mp4_video_track(rotated)].concat());
        let ftyp = mp4_box(b"ftyp", b"isom");
        let mdat = mp4_box(b"mdat", &[7u8; 4096]);
        if moov_last {
            [ftyp, mdat, moov].concat()
        } else {
            [ftyp, moov, mdat].concat()
        }
    }

    #[test]
    fn mp4_facts_wherever_the_header_is() {
        for moov_last in [false, true] {
            let info = video_info(&mut Cursor::new(mp4_file(false, moov_last)));
            assert_eq!(info.duration, Some(2.0));
            assert_eq!(info.size, Some((640, 360)));
            assert_eq!(info.video_codec.as_deref(), Some("H.264"));
            assert_eq!(info.fps, Some(25.0));
            assert_eq!(info.audio_codec, None);
        }
    }

    #[test]
    fn mp4_rotation_swaps_the_size() {
        assert_eq!(video_info(&mut Cursor::new(mp4_file(true, false))).size, Some((360, 640)));
    }

    fn ebml(id: &[u8], body: &[u8]) -> Vec<u8> {
        let mut b = id.to_vec();
        b.extend_from_slice(&(0x0100_0000_0000_0000u64 | body.len() as u64).to_be_bytes());
        b.extend_from_slice(body);
        b
    }

    fn webm_file(unknown_segment_size: bool) -> Vec<u8> {
        let video = [ebml(&[0xB0], &[0x07, 0x80]), ebml(&[0xBA], &[0x04, 0x38])].concat();
        let v = [ebml(&[0x83], &[1]), ebml(&[0x86], b"V_VP9"), ebml(&[0x23, 0xE3, 0x83], &40_000_000u32.to_be_bytes()), ebml(&[0xE0], &video)].concat();
        let a = [ebml(&[0x83], &[2]), ebml(&[0x86], b"A_OPUS")].concat();
        let tracks = ebml(&[0x16, 0x54, 0xAE, 0x6B], &[ebml(&[0xAE], &v), ebml(&[0xAE], &a)].concat());
        let info = ebml(&[0x15, 0x49, 0xA9, 0x66], &[ebml(&[0x2A, 0xD7, 0xB1], &[0x0F, 0x42, 0x40]), ebml(&[0x44, 0x89], &83_500f64.to_be_bytes())].concat());
        let cluster = ebml(&[0x1F, 0x43, 0xB6, 0x75], &[0u8; 64]);
        let content = [info, tracks, cluster].concat();
        let header = ebml(&[0x1A, 0x45, 0xDF, 0xA3], &[]);
        let segment = if unknown_segment_size {
            [vec![0x18, 0x53, 0x80, 0x67, 0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF], content].concat()
        } else {
            ebml(&[0x18, 0x53, 0x80, 0x67], &content)
        };
        [header, segment].concat()
    }

    #[test]
    fn matroska_facts() {
        for unknown in [false, true] {
            let info = video_info(&mut Cursor::new(webm_file(unknown)));
            assert_eq!(info.duration, Some(83.5));
            assert_eq!(info.size, Some((1920, 1080)));
            assert_eq!(info.video_codec.as_deref(), Some("VP9"));
            assert_eq!(info.audio_codec.as_deref(), Some("Opus"));
            assert_eq!(info.fps, Some(25.0));
        }
    }

    #[test]
    fn broken_videos_give_no_facts_and_no_panic() {
        for bytes in [vec![], b"not a video at all".to_vec(), vec![0xFF; 64], vec![0, 0, 0, 1, b'm', b'o', b'o', b'v'], webm_file(false)[..30].to_vec()] {
            let _ = video_info(&mut Cursor::new(bytes));
        }
        assert_eq!(video_info(&mut Cursor::new(Vec::new())), VideoInfo::default());
    }

    #[test]
    fn durations() {
        assert_eq!(format_duration(83.0), "1:23");
        assert_eq!(format_duration(3723.5), "1:02:04");
        assert_eq!(format_duration(0.2), "0:00");
    }
}
