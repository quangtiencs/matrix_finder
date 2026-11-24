//! Audio files: their tags (title, artist, album…), the stream's facts (duration,
//! bitrate, sample rate) and the cover picture, read without decoding any sound.
//! The reader keeps to a small memory budget of its own (16 MB for one item), so a
//! damaged tag that claims gigabytes fails instead of taking them.
use crate::i18n::t;
use crate::preview::certview::Row;
use crate::util::{ext_of, Res};
use lofty::config::{ParseOptions, ParsingMode};
use lofty::file::{AudioFile, FileType, TaggedFileExt};
use lofty::tag::{Accessor, ItemKey};
use std::io::{Read, Seek};

/// The longest text kept from a tag (comments and lyrics can be long).
const MAX_TEXT: usize = 600;

pub fn is_audio(name: &str) -> bool {
    matches!(
        ext_of(name).as_str(),
        "mp3" | "flac" | "wav" | "ogg" | "oga" | "opus" | "m4a" | "m4b" | "aac" | "aif" | "aiff" | "aifc" | "ape" | "wv" | "mpc" | "spx"
    )
}

pub struct AudioInfo {
    pub format: String,
    pub rows: Vec<Row>,
    /// The cover picture's encoded bytes (JPEG or PNG, mostly).
    pub cover: Option<Vec<u8>>,
}

fn format_name(f: &FileType) -> String {
    match f {
        FileType::Aac => "AAC",
        FileType::Aiff => "AIFF",
        FileType::Ape => "Monkey's Audio",
        FileType::Flac => "FLAC",
        FileType::Mpeg => "MP3",
        FileType::Mp4 => "MPEG-4 audio",
        FileType::Mpc => "Musepack",
        FileType::Opus => "Opus",
        FileType::Vorbis => "Ogg Vorbis",
        FileType::Speex => "Speex",
        FileType::Wav => "WAV",
        FileType::WavPack => "WavPack",
        FileType::Custom(name) => name,
        _ => "Audio",
    }
    .to_string()
}

fn clip(s: &str) -> String {
    let s = s.trim();
    match s.char_indices().nth(MAX_TEXT) {
        Some((at, _)) => format!("{}…", &s[..at]),
        None => s.to_string(),
    }
}

fn channels_text(n: u8) -> String {
    match n {
        1 => t("mono"),
        2 => t("stereo"),
        n => format!("{n} {}", t("channels")),
    }
}

/// `3 / 12`, or `3` when the total is not known.
fn of_total(n: Option<u32>, total: Option<u32>) -> Option<String> {
    match (n, total) {
        (Some(n), Some(total)) => Some(format!("{n} / {total}")),
        (Some(n), None) => Some(n.to_string()),
        _ => None,
    }
}

/// Read an audio file's facts. `ext` helps when the content does not say what it is.
pub fn read<R: Read + Seek>(r: R, ext: &str) -> Res<AudioInfo> {
    let options = ParseOptions::new().read_cover_art(true).parsing_mode(ParsingMode::Relaxed);
    let probe = lofty::probe::Probe::new(std::io::BufReader::new(r)).options(options).guess_file_type().map_err(|e| e.to_string())?;
    let probe = match (probe.file_type(), FileType::from_ext(ext)) {
        (None, Some(by_name)) => probe.set_file_type(by_name),
        _ => probe,
    };
    let file = probe.read().map_err(|e| format!("{} ({e})", t("Not an audio file this preview can read.")))?;
    let mut rows = vec![];
    let push = |rows: &mut Vec<Row>, label: String, value: Option<String>| {
        if let Some(v) = value.map(|v| clip(&v)).filter(|v| !v.is_empty()) {
            rows.push(Row::Field(label, v));
        }
    };
    let mut cover = None;
    if let Some(tag) = file.primary_tag().or_else(|| file.first_tag()) {
        push(&mut rows, t("Title"), tag.title().map(|s| s.to_string()));
        push(&mut rows, t("Artist"), tag.artist().map(|s| s.to_string()));
        push(&mut rows, t("Album"), tag.album().map(|s| s.to_string()));
        push(&mut rows, t("Album artist"), tag.get_string(ItemKey::AlbumArtist).map(str::to_string));
        push(&mut rows, t("Year"), tag.date().map(|d| d.to_string()).or_else(|| tag.get_string(ItemKey::Year).map(str::to_string)));
        push(&mut rows, t("Track"), of_total(tag.track(), tag.track_total()));
        push(&mut rows, t("Disc"), of_total(tag.disk(), tag.disk_total()));
        push(&mut rows, t("Genre"), tag.genre().map(|s| s.to_string()));
        push(&mut rows, t("Composer"), tag.get_string(ItemKey::Composer).map(str::to_string));
        push(&mut rows, t("Comment"), tag.comment().map(|s| s.to_string()));
    }
    // The front cover, or else the first picture of any tag.
    let pictures = file.tags().iter().flat_map(|tag| tag.pictures());
    let front = pictures.clone().find(|p| p.pic_type() == lofty::picture::PictureType::CoverFront).or_else(|| pictures.clone().next());
    if let Some(p) = front {
        cover = Some(p.data().to_vec());
    }
    let props = file.properties();
    let duration = props.duration().as_secs_f64();
    if duration > 0.0 {
        rows.push(Row::Field(t("Duration"), crate::util::format_duration(duration)));
    }
    push(&mut rows, t("Bitrate"), props.audio_bitrate().or(props.overall_bitrate()).filter(|b| *b > 0).map(|b| format!("{b} kbps")));
    push(&mut rows, t("Sample rate"), props.sample_rate().filter(|r| *r > 0).map(|r| format!("{} kHz", (f64::from(r) / 100.0).round() / 10.0)));
    push(&mut rows, t("Channels"), props.channels().filter(|c| *c > 0).map(channels_text));
    push(&mut rows, t("Bit depth"), props.bit_depth().filter(|b| *b > 0).map(|b| format!("{b}-bit")));
    if rows.is_empty() {
        rows.push(Row::Note(t("This file has no details the app can show.")));
    }
    Ok(AudioInfo { format: format_name(&file.file_type()), rows, cover })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// A WAV file of `seconds` of silence, 8 kHz mono 16-bit, with a title in its INFO list.
    fn wav(seconds: u32, title: &str) -> Vec<u8> {
        let data = vec![0u8; (8000 * 2 * seconds) as usize];
        let mut info = b"INFO".to_vec();
        let mut text = title.as_bytes().to_vec();
        text.push(0);
        if text.len() % 2 == 1 {
            text.push(0);
        }
        info.extend(b"INAM");
        info.extend((text.len() as u32).to_le_bytes());
        info.extend(&text);
        let mut fmt = vec![];
        fmt.extend(1u16.to_le_bytes());
        fmt.extend(1u16.to_le_bytes());
        fmt.extend(8000u32.to_le_bytes());
        fmt.extend(16000u32.to_le_bytes());
        fmt.extend(2u16.to_le_bytes());
        fmt.extend(16u16.to_le_bytes());
        let mut body = b"WAVE".to_vec();
        for (id, chunk) in [(b"fmt ", &fmt), (b"data", &data), (b"LIST", &info)] {
            body.extend(id);
            body.extend((chunk.len() as u32).to_le_bytes());
            body.extend(chunk);
        }
        let mut v = b"RIFF".to_vec();
        v.extend((body.len() as u32).to_le_bytes());
        v.extend(body);
        v
    }

    #[test]
    fn reads_tags_and_stream_facts() {
        let info = read(Cursor::new(wav(2, "Quiet")), "wav").unwrap();
        assert_eq!(info.format, "WAV");
        let field = |l: &str| info.rows.iter().find_map(|r| if let Row::Field(k, v) = r { (k == &t(l)).then(|| v.clone()) } else { None });
        assert_eq!(field("Title").as_deref(), Some("Quiet"));
        assert_eq!(field("Duration").as_deref(), Some(crate::util::format_duration(2.0).as_str()));
        assert_eq!(field("Sample rate").as_deref(), Some("8 kHz"));
        assert_eq!(field("Channels"), Some(t("mono")));
        assert_eq!(field("Bit depth").as_deref(), Some("16-bit"));
        assert!(info.cover.is_none());
    }

    #[test]
    fn refuses_what_is_not_audio() {
        assert!(read(Cursor::new(b"definitely not sound".to_vec()), "mp3").is_err());
        assert!(read(Cursor::new(Vec::new()), "flac").is_err());
        // A tag that claims far more than it has.
        let mut id3 = b"ID3\x04\x00\x00\x7f\x7f\x7f\x7f".to_vec();
        id3.extend([0u8; 64]);
        let _ = read(Cursor::new(id3), "mp3");
    }
}
