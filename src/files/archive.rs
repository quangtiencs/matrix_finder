//! Reading and writing archives: zip, 7z, rar (read; write via `rar` CLI),
//! tar (+gz/xz/zst/bz2) and single-file gz/xz/zst/bz2 streams.
use crate::files::ops::Progress;
use crate::files::safefs::Conflict;
use crate::files::safefs::{self, TempFile};
use crate::files::vfs::Entry;
use crate::util::{es, Res};
use globset::GlobSet;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------- passwords

/// Passwords typed for archives. Only in memory, only until the app closes: they are
/// never written to the settings or anywhere else.
static PASSWORDS: LazyLock<Mutex<HashMap<PathBuf, String>>> = LazyLock::new(Default::default);

pub fn remember_password(path: &Path, password: &str) {
    if let Ok(mut all) = PASSWORDS.lock() {
        all.insert(path.to_path_buf(), password.to_string());
    }
}

fn password_for(path: &Path) -> Option<String> {
    PASSWORDS.lock().ok()?.get(path).cloned()
}

fn password_needed() -> String {
    crate::i18n::t("This archive is password protected.")
}

fn wrong_password() -> String {
    crate::i18n::t("The password is wrong.")
}

/// Whether an error from this module means the password is missing or wrong.
pub fn is_password_error(e: &str) -> bool {
    e == password_needed() || e == wrong_password()
}

fn zip_err(e: zip::result::ZipError) -> String {
    use zip::result::ZipError;
    match e {
        ZipError::UnsupportedArchive(ZipError::PASSWORD_REQUIRED) => password_needed(),
        ZipError::InvalidPassword => wrong_password(),
        other => es(other),
    }
}

fn sevenz_err(e: sevenz_rust2::Error) -> String {
    match e {
        sevenz_rust2::Error::PasswordRequired => password_needed(),
        sevenz_rust2::Error::MaybeBadPassword(_) => wrong_password(),
        other => es(other),
    }
}

/// A 7z entry whose data stopped short: with a password, almost always a wrong one.
fn sevenz_short(path: &Path) -> String {
    if password_for(path).is_some() {
        wrong_password()
    } else {
        crate::i18n::t("This file is cut short.")
    }
}

/// Reads all `left` bytes of a 7z entry, or fails. sevenz-rust2 ends an entry early
/// without an error when its data stops short, which a wrong password does now and then
/// (about 1 in 200 times): the checksum is only checked once every byte is there.
struct SevenzWhole<'a> {
    inner: &'a mut dyn Read,
    left: u64,
    short: &'a str,
}

impl Read for SevenzWhole<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.left == 0 || buf.is_empty() {
            return Ok(0);
        }
        let n = self.inner.read(buf)?;
        if n == 0 {
            return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, self.short.to_string()));
        }
        self.left = self.left.saturating_sub(n as u64);
        Ok(n)
    }
}

fn rar_err(e: unrar::error::UnrarError) -> String {
    use unrar::error::Code;
    match e.code {
        Code::MissingPassword => password_needed(),
        Code::BadPassword => wrong_password(),
        _ => es(e),
    }
}

fn sevenz_password(path: &Path) -> sevenz_rust2::Password {
    password_for(path).map_or_else(sevenz_rust2::Password::empty, |p| sevenz_rust2::Password::new(&p))
}

fn rar_archive<'a>(path: &'a Path, password: &'a Option<String>) -> unrar::Archive<'a> {
    match password {
        Some(p) => unrar::Archive::with_password(path, p),
        None => unrar::Archive::new(path),
    }
}

/// How much of an archive a password stands in front of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lock {
    Open,
    /// The list of files can be read, but (some of) their contents cannot.
    Contents,
    /// Not even the list of files can be read.
    Everything,
}

/// What the file itself says, whatever passwords are known. Remembered per file version:
/// a large archive takes a moment to look through, and this is asked on the UI thread.
fn encryption_of(path: &Path) -> Lock {
    type Version = (PathBuf, u64, Option<SystemTime>);
    static KNOWN: LazyLock<Mutex<HashMap<Version, Lock>>> = LazyLock::new(Default::default);
    let meta = std::fs::metadata(path).ok();
    let version = (path.to_path_buf(), meta.as_ref().map_or(0, |m| m.len()), meta.and_then(|m| m.modified().ok()));
    if let Some(lock) = KNOWN.lock().ok().and_then(|k| k.get(&version).copied()) {
        return lock;
    }
    let lock = look_for_encryption(path);
    if let Ok(mut known) = KNOWN.lock() {
        known.insert(version, lock);
    }
    lock
}

fn look_for_encryption(path: &Path) -> Lock {
    match ArchiveFormat::detect(path) {
        Some(ArchiveFormat::Zip) => {
            let Ok(file) = File::open(path) else { return Lock::Open };
            let Ok(mut z) = zip::ZipArchive::new(BufReader::new(file)) else { return Lock::Open };
            let any = (0..z.len()).any(|i| z.by_index_raw(i).is_ok_and(|f| f.encrypted()));
            if any {
                Lock::Contents
            } else {
                Lock::Open
            }
        }
        Some(ArchiveFormat::SevenZ) => match sevenz_rust2::Archive::open(path) {
            Err(sevenz_rust2::Error::PasswordRequired) => Lock::Everything,
            Err(_) => Lock::Open,
            Ok(a) => {
                let aes = sevenz_rust2::EncoderMethod::ID_AES256_SHA256;
                let any = a.blocks.iter().any(|b| b.coders.iter().any(|c| c.encoder_method_id() == aes));
                if any {
                    Lock::Contents
                } else {
                    Lock::Open
                }
            }
        },
        Some(ArchiveFormat::Rar) => {
            let Ok(list) = unrar::Archive::new(path).open_for_listing() else { return Lock::Everything };
            let mut lock = Lock::Open;
            for entry in list {
                match entry {
                    Ok(e) if e.is_encrypted() => lock = Lock::Contents,
                    Ok(_) => {}
                    Err(e) if e.code == unrar::error::Code::MissingPassword => return Lock::Everything,
                    Err(_) => break,
                }
            }
            lock
        }
        _ => Lock::Open,
    }
}

/// What still needs a password, given the ones typed so far.
pub fn lock_state(path: &Path) -> Lock {
    if password_for(path).is_some() {
        return Lock::Open;
    }
    encryption_of(path)
}

/// Whether this one entry needs a password. A ZIP can have plain and encrypted entries side by side.
pub fn entry_is_locked(path: &Path, inner: &str) -> bool {
    match lock_state(path) {
        Lock::Open => false,
        Lock::Everything => true,
        Lock::Contents if ArchiveFormat::detect(path) == Some(ArchiveFormat::Zip) => {
            let Some(mut z) = File::open(path).ok().and_then(|f| zip::ZipArchive::new(BufReader::new(f)).ok()) else { return false };
            let Some(i) = z.index_for_name(inner) else { return false };
            z.by_index_raw(i).is_ok_and(|f| f.encrypted())
        }
        Lock::Contents => true,
    }
}

/// Whether a password opens this archive: tried on its smallest protected file, as
/// one only proves itself by decrypting something. This takes a moment: not for the UI thread.
pub fn check_password(path: &Path, password: &str) -> bool {
    match ArchiveFormat::detect(path) {
        Some(ArchiveFormat::Zip) => {
            let Some(mut z) = File::open(path).ok().and_then(|f| zip::ZipArchive::new(BufReader::new(f)).ok()) else { return false };
            let smallest = (0..z.len()).filter_map(|i| z.by_index_raw(i).ok().filter(|f| f.encrypted() && !f.is_dir()).map(|f| (f.size(), i))).min();
            let Some((_, i)) = smallest else { return true };
            let Ok(mut f) = z.by_index_decrypt(i, password.as_bytes()) else { return false };
            // The whole file: the check at its start lets a wrong password through now and then,
            // and only its checksum at the end tells.
            std::io::copy(&mut f, &mut std::io::sink()).is_ok()
        }
        Some(ArchiveFormat::SevenZ) => {
            let Ok(mut r) = sevenz_rust2::ArchiveReader::open(path, sevenz_rust2::Password::new(password)) else { return false };
            let smallest = r.archive().files.iter().filter(|f| f.has_stream()).min_by_key(|f| f.size()).map(|f| (f.name().to_string(), f.size()));
            // All of it: a wrong password can end the data early without an error (see SevenzWhole).
            smallest.is_none_or(|(name, size)| r.read_file(&name).is_ok_and(|data| data.len() as u64 == size))
        }
        Some(ArchiveFormat::Rar) => {
            let Ok(mut arch) = unrar::Archive::with_password(path, password).open_for_processing() else { return false };
            loop {
                match arch.read_header() {
                    Ok(Some(header)) => {
                        let result = if header.entry().is_encrypted() { return header.test().is_ok() } else { header.skip() };
                        match result {
                            Ok(next) => arch = next,
                            Err(_) => return false,
                        }
                    }
                    Ok(None) => return true,
                    Err(_) => return false,
                }
            }
        }
        _ => true,
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum ArchiveFormat {
    Zip,
    SevenZ,
    Tar,
    TarGz,
    TarXz,
    TarZst,
    TarBz2,
    Gz,
    Xz,
    Zst,
    Bz2,
    Rar,
}

impl ArchiveFormat {
    pub const ALL: [ArchiveFormat; 12] = [
        ArchiveFormat::Zip,
        ArchiveFormat::SevenZ,
        ArchiveFormat::TarZst,
        ArchiveFormat::TarGz,
        ArchiveFormat::TarXz,
        ArchiveFormat::TarBz2,
        ArchiveFormat::Tar,
        ArchiveFormat::Zst,
        ArchiveFormat::Gz,
        ArchiveFormat::Xz,
        ArchiveFormat::Bz2,
        ArchiveFormat::Rar,
    ];

    pub fn extension(self) -> &'static str {
        match self {
            ArchiveFormat::Zip => "zip",
            ArchiveFormat::SevenZ => "7z",
            ArchiveFormat::Tar => "tar",
            ArchiveFormat::TarGz => "tar.gz",
            ArchiveFormat::TarXz => "tar.xz",
            ArchiveFormat::TarZst => "tar.zst",
            ArchiveFormat::TarBz2 => "tar.bz2",
            ArchiveFormat::Gz => "gz",
            ArchiveFormat::Xz => "xz",
            ArchiveFormat::Zst => "zst",
            ArchiveFormat::Bz2 => "bz2",
            ArchiveFormat::Rar => "rar",
        }
    }

    pub fn label(self) -> String {
        let note = match self {
            ArchiveFormat::Gz | ArchiveFormat::Xz | ArchiveFormat::Zst | ArchiveFormat::Bz2 => " (single file)",
            ArchiveFormat::Rar => " (needs rar CLI)",
            _ => "",
        };
        format!(".{}{}", self.extension(), note)
    }

    /// Formats that compress exactly one file (no container).
    pub fn single_file(self) -> bool {
        matches!(self, ArchiveFormat::Gz | ArchiveFormat::Xz | ArchiveFormat::Zst | ArchiveFormat::Bz2)
    }

    /// The formats to offer when compressing: the single-file ones only for one file.
    pub fn offered(one_file: bool) -> Vec<ArchiveFormat> {
        Self::ALL.into_iter().filter(|f| one_file || !f.single_file()).collect()
    }

    /// Whether it has a compression level (plain .tar only packs).
    pub fn compresses(self) -> bool {
        self != ArchiveFormat::Tar
    }

    /// What the format is good for, in a line.
    pub fn blurb(self) -> String {
        use crate::i18n::t;
        match self {
            ArchiveFormat::Zip => t("Opens everywhere, with nothing to install."),
            ArchiveFormat::SevenZ => t("Usually the smallest; 7-Zip, Keka or The Unarchiver open it."),
            ArchiveFormat::TarZst => t("Fast to make and to open, using every core."),
            ArchiveFormat::TarGz => t("The usual choice on Linux and macOS."),
            ArchiveFormat::TarXz => t("Very small, but slow to make."),
            ArchiveFormat::TarBz2 => t("An older format, for tools that ask for it."),
            ArchiveFormat::Tar => t("No compression: everything in one file, quickly."),
            ArchiveFormat::Gz | ArchiveFormat::Xz | ArchiveFormat::Zst | ArchiveFormat::Bz2 => t("Compresses this one file on its own."),
            ArchiveFormat::Rar => t("Needs the rar command-line tool."),
        }
    }

    /// The format a typed name ends in (`photos.tar.gz`), if any: the longest match.
    pub fn typed_in(name: &str) -> Option<ArchiveFormat> {
        let name = name.trim().to_lowercase();
        Self::ALL.into_iter().filter(|f| name.ends_with(&format!(".{}", f.extension()))).max_by_key(|f| f.extension().len())
    }

    pub fn detect(path: &Path) -> Option<ArchiveFormat> {
        let name = path.file_name()?.to_string_lossy().to_lowercase();
        let ends = |s: &str| name.ends_with(s);
        Some(if ends(".tar.gz") || ends(".tgz") {
            ArchiveFormat::TarGz
        } else if ends(".tar.xz") || ends(".txz") {
            ArchiveFormat::TarXz
        } else if ends(".tar.zst") || ends(".tzst") || ends(".tar.zstd") {
            ArchiveFormat::TarZst
        } else if ends(".tar.bz2") || ends(".tbz2") || ends(".tbz") {
            ArchiveFormat::TarBz2
        } else if ends(".tar") {
            ArchiveFormat::Tar
        } else if ends(".zip") || ends(".jar") || ends(".apk") || ends(".xpi") || ends(".cbz") || ends(".whl") {
            ArchiveFormat::Zip
        } else if ends(".7z") || ends(".cb7") {
            ArchiveFormat::SevenZ
        } else if ends(".rar") || ends(".cbr") {
            ArchiveFormat::Rar
        } else if ends(".gz") {
            ArchiveFormat::Gz
        } else if ends(".xz") {
            ArchiveFormat::Xz
        } else if ends(".zst") || ends(".zstd") {
            ArchiveFormat::Zst
        } else if ends(".bz2") {
            ArchiveFormat::Bz2
        } else {
            return None;
        })
    }
}

pub fn is_archive(path: &Path) -> bool {
    ArchiveFormat::detect(path).is_some()
}

/// Locate the external `rar` executable (RAR creation is proprietary).
pub fn rar_cli() -> Option<PathBuf> {
    let names: &[&str] = if cfg!(windows) { &["rar.exe", "Rar.exe"] } else { &["rar"] };
    if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths) {
            for n in names {
                let p = dir.join(n);
                if p.is_file() {
                    return Some(p);
                }
            }
        }
    }
    let extra = ["C:\\Program Files\\WinRAR\\Rar.exe", "C:\\Program Files (x86)\\WinRAR\\Rar.exe", "/opt/homebrew/bin/rar", "/usr/local/bin/rar"];
    extra.iter().map(PathBuf::from).find(|p| p.is_file())
}

#[derive(Clone, Debug)]
pub struct ArchEntry {
    /// `/`-separated, no leading or trailing slash.
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
}

fn norm(p: &str) -> String {
    p.replace('\\', "/").trim_matches('/').to_string()
}

fn stem_name(path: &Path) -> String {
    path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "data".into())
}

fn dos_time(t: u32) -> Option<SystemTime> {
    let date = (t >> 16) as u16;
    let time = (t & 0xffff) as u16;
    let y = 1980 + (date >> 9) as i32;
    let m = ((date >> 5) & 0xf) as u32;
    let d = (date & 0x1f) as u32;
    let h = (time >> 11) as u32;
    let mi = ((time >> 5) & 0x3f) as u32;
    let s = ((time & 0x1f) * 2) as u32;
    let nd = chrono::NaiveDate::from_ymd_opt(y, m, d)?.and_hms_opt(h, mi, s)?;
    let local = nd.and_local_timezone(chrono::Local).single()?;
    Some(local.into())
}

fn zip_time(t: zip::DateTime) -> Option<SystemTime> {
    let nd = chrono::NaiveDate::from_ymd_opt(t.year() as i32, t.month() as u32, t.day() as u32)?.and_hms_opt(
        t.hour() as u32,
        t.minute() as u32,
        t.second() as u32,
    )?;
    let local = nd.and_local_timezone(chrono::Local).single()?;
    Some(local.into())
}

fn to_zip_time(t: SystemTime) -> Option<zip::DateTime> {
    use chrono::{Datelike, Timelike};
    let dt: chrono::DateTime<chrono::Local> = t.into();
    zip::DateTime::from_date_and_time(
        dt.year().clamp(1980, 2107) as u16,
        dt.month() as u8,
        dt.day() as u8,
        dt.hour() as u8,
        dt.minute() as u8,
        dt.second() as u8,
    )
    .ok()
}

fn tar_reader(path: &Path, fmt: ArchiveFormat) -> Res<tar::Archive<Box<dyn Read>>> {
    tar_reader_of(BufReader::new(File::open(path).map_err(es)?), fmt)
}

fn tar_reader_of<'a>(f: impl std::io::BufRead + 'a, fmt: ArchiveFormat) -> Res<tar::Archive<Box<dyn Read + 'a>>> {
    let r: Box<dyn Read + 'a> = match fmt {
        ArchiveFormat::Tar => Box::new(f),
        ArchiveFormat::TarGz => Box::new(flate2::read::MultiGzDecoder::new(f)),
        ArchiveFormat::TarXz => Box::new(xz2::read::XzDecoder::new_multi_decoder(f)),
        ArchiveFormat::TarZst => Box::new(zstd::stream::read::Decoder::with_buffer(f).map_err(es)?),
        ArchiveFormat::TarBz2 => Box::new(bzip2::read::MultiBzDecoder::new(f)),
        _ => return Err("not a tar archive".into()),
    };
    Ok(tar::Archive::new(r))
}

fn single_reader(path: &Path, fmt: ArchiveFormat) -> Res<Box<dyn Read>> {
    let f = BufReader::new(File::open(path).map_err(es)?);
    Ok(match fmt {
        ArchiveFormat::Gz => Box::new(flate2::read::MultiGzDecoder::new(f)),
        ArchiveFormat::Xz => Box::new(xz2::read::XzDecoder::new_multi_decoder(f)),
        ArchiveFormat::Zst => Box::new(zstd::stream::read::Decoder::new(f).map_err(es)?),
        ArchiveFormat::Bz2 => Box::new(bzip2::read::MultiBzDecoder::new(f)),
        _ => return Err("not a single-file stream".into()),
    })
}

/// List every entry of an archive.
pub fn list(path: &Path) -> Res<Vec<ArchEntry>> {
    list_until(path, usize::MAX, &|| false).map(|(all, _)| all)
}

/// A reader that fails once `stop` says so, to end a long read part way.
struct Stoppable<'a, R> {
    r: R,
    stop: &'a dyn Fn() -> bool,
}

impl<R: Read> Read for Stoppable<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if (self.stop)() {
            return Err(std::io::Error::other("stopped"));
        }
        self.r.read(buf)
    }
}

/// Like `list`, but ends after `max` entries or once `stop` says so (checked between
/// entries, and while a compressed tar is read through). True when it was cut short.
pub fn list_until(path: &Path, max: usize, stop: &dyn Fn() -> bool) -> Res<(Vec<ArchEntry>, bool)> {
    let fmt = ArchiveFormat::detect(path).ok_or("Unknown archive format")?;
    let mut out = Vec::new();
    let mut cut = false;
    let full = |out: &Vec<ArchEntry>| out.len() >= max || stop();
    match fmt {
        ArchiveFormat::Zip => {
            let mut z = zip::ZipArchive::new(BufReader::new(File::open(path).map_err(es)?)).map_err(es)?;
            for i in 0..z.len() {
                if full(&out) {
                    cut = true;
                    break;
                }
                let f = z.by_index_raw(i).map_err(es)?;
                let Some(name) = f.enclosed_name() else { continue };
                out.push(ArchEntry { path: norm(&name.to_string_lossy()), is_dir: f.is_dir(), size: f.size(), modified: f.last_modified().and_then(zip_time) });
            }
        }
        ArchiveFormat::SevenZ => {
            let a = match password_for(path) {
                Some(p) => sevenz_rust2::Archive::open_with_password(path, &sevenz_rust2::Password::new(&p)),
                None => sevenz_rust2::Archive::open(path),
            }
            .map_err(sevenz_err)?;
            for f in &a.files {
                if full(&out) {
                    cut = true;
                    break;
                }
                out.push(ArchEntry {
                    path: norm(f.name()),
                    is_dir: f.is_directory(),
                    size: f.size(),
                    modified: if f.has_last_modified_date { Some(SystemTime::from(f.last_modified_date())) } else { None },
                });
            }
        }
        ArchiveFormat::Rar => {
            let password = password_for(path);
            let arch = rar_archive(path, &password).open_for_listing().map_err(rar_err)?;
            for e in arch {
                if full(&out) {
                    cut = true;
                    break;
                }
                let e = e.map_err(rar_err)?;
                out.push(ArchEntry {
                    path: norm(&e.filename.to_string_lossy()),
                    is_dir: e.is_directory(),
                    size: e.unpacked_size,
                    modified: dos_time(e.file_time),
                });
            }
        }
        ArchiveFormat::Tar | ArchiveFormat::TarGz | ArchiveFormat::TarXz | ArchiveFormat::TarZst | ArchiveFormat::TarBz2 => {
            // The whole stream is read through: stop part way when asked.
            let f = BufReader::new(Stoppable { r: File::open(path).map_err(es)?, stop });
            let mut a = tar_reader_of(f, fmt)?;
            for e in a.entries().map_err(es)? {
                if full(&out) {
                    cut = true;
                    break;
                }
                let e = match e {
                    Ok(e) => e,
                    Err(_) if stop() => {
                        cut = true;
                        break;
                    }
                    Err(e) => return Err(es(e)),
                };
                let p = e.path().map_err(es)?.to_string_lossy().to_string();
                let h = e.header();
                out.push(ArchEntry {
                    path: norm(&p),
                    is_dir: h.entry_type().is_dir(),
                    size: h.size().unwrap_or(0),
                    modified: h.mtime().ok().map(|t| UNIX_EPOCH + Duration::from_secs(t)),
                });
            }
        }
        ArchiveFormat::Gz | ArchiveFormat::Xz | ArchiveFormat::Zst | ArchiveFormat::Bz2 => {
            let meta = std::fs::metadata(path).map_err(es)?;
            out.push(ArchEntry { path: stem_name(path), is_dir: false, size: meta.len(), modified: meta.modified().ok() });
        }
    }
    out.retain(|e| !e.path.is_empty());
    Ok((out, cut))
}

/// Direct children of `inner` (implicit directories are synthesised).
pub fn children(all: &[ArchEntry], inner: &str) -> Vec<Entry> {
    let prefix = if inner.is_empty() { String::new() } else { format!("{inner}/") };
    let mut map: BTreeMap<String, Entry> = BTreeMap::new();
    for e in all {
        let Some(rest) = e.path.strip_prefix(&prefix) else { continue };
        if rest.is_empty() {
            continue;
        }
        match rest.split_once('/') {
            Some((dir, _)) => {
                let ent = map.entry(dir.to_string()).or_insert_with(|| Entry::new(dir.to_string(), true, 0, None));
                ent.is_dir = true;
                if !e.is_dir {
                    ent.size += e.size;
                }
            }
            None => {
                let ent = map.entry(rest.to_string()).or_insert_with(|| Entry::new(rest.to_string(), e.is_dir, 0, e.modified));
                if e.is_dir {
                    ent.is_dir = true;
                    ent.modified = e.modified;
                } else {
                    ent.size = e.size;
                    ent.modified = e.modified;
                    ent.ext = crate::util::ext_of(rest);
                }
            }
        }
    }
    map.into_values().collect()
}

fn is_selected(path: &str, selection: &[String]) -> bool {
    selection.is_empty() || selection.iter().any(|s| path == s || path.strip_prefix(s.as_str()).is_some_and(|r| r.starts_with('/')))
}

/// Safely join an archive-relative path to a destination (rejects `..`, absolute paths).
fn safe_join(dest: &Path, rel: &str) -> Option<PathBuf> {
    let mut out = dest.to_path_buf();
    for c in Path::new(rel).components() {
        match c {
            Component::Normal(n) => out.push(n),
            Component::CurDir => {}
            _ => return None,
        }
    }
    if out == dest {
        None
    } else {
        Some(out)
    }
}

fn out_path(dest: &Path, path: &str, strip: &str) -> Option<PathBuf> {
    let rel = if strip.is_empty() { path } else { path.strip_prefix(strip)?.trim_start_matches('/') };
    safe_join(dest, rel)
}

struct ProgressWriter<'a, W: Write> {
    inner: W,
    progress: &'a Progress,
}

impl<W: Write> Write for ProgressWriter<'_, W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.progress.cancelled() {
            return Err(std::io::Error::other("Cancelled"));
        }
        let n = self.inner.write(buf)?;
        self.progress.add(n as u64);
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

struct ProgressReader<'a, R: Read> {
    inner: R,
    progress: &'a Progress,
}

impl<R: Read> Read for ProgressReader<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.progress.cancelled() {
            return Err(std::io::Error::other("Cancelled"));
        }
        let n = self.inner.read(buf)?;
        self.progress.add(n as u64);
        Ok(n)
    }
}

/// Skip a file that exists already when the policy says so (`None`), before it is unpacked.
/// Where it finally goes is decided once it is complete (`safefs::place`).
fn resolve_out(out: PathBuf, policy: Conflict) -> Option<PathBuf> {
    (policy != Conflict::Skip || !safefs::exists(&out)).then_some(out)
}

/// Fail unless the nearest existing folder above `out` really lies inside `real_dest`.
/// `out` is already lexically inside it, but an archive can bring its own symlink along
/// (`x -> ~/.ssh`, then `x/authorized_keys`), and nothing may be written through that.
/// Checked before any folder is created, as creating them follows links too.
fn ensure_inside(real_dest: &Path, out: &Path) -> Res<()> {
    let mut above = out.parent();
    while let Some(dir) = above {
        if std::fs::symlink_metadata(dir).is_ok() {
            return match dir.canonicalize() {
                Ok(real) if real.starts_with(real_dest) => Ok(()),
                _ => Err(format!("Refusing to extract '{}': a symbolic link leads it out of the destination folder", out.display())),
            };
        }
        above = dir.parent();
    }
    Ok(())
}

/// Unpack `reader` into a hidden temporary file next to `out`. Nothing at `out` changes
/// until the caller commits it, so a cancel, a damaged archive or a crash keeps what is there.
fn write_temp(reader: &mut dyn Read, out: &Path, progress: &Progress) -> Res<TempFile> {
    write_temp_counted(reader, out, progress).map(|(tmp, _)| tmp)
}

/// `write_temp`, also returning how many bytes were written.
fn write_temp_counted(reader: &mut dyn Read, out: &Path, progress: &Progress) -> Res<(TempFile, u64)> {
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent).map_err(es)?;
    }
    let mut tmp = TempFile::beside(out).map_err(es)?;
    let n = {
        let mut w = ProgressWriter { inner: BufWriter::new(tmp.file()), progress };
        std::io::copy(reader, &mut w).and_then(|n| w.flush().map(|_| n)).map_err(es)?
    };
    Ok((tmp, n))
}

/// The macOS quarantine mark of a downloaded archive, given to what is extracted from
/// it, as Archive Utility does: Gatekeeper then still checks an app or script that came
/// in a downloaded archive before it first runs. Nothing elsewhere.
struct Quarantine(Option<Vec<u8>>);

impl Quarantine {
    #[cfg(target_os = "macos")]
    const NAME: &'static [u8] = b"com.apple.quarantine\0";

    #[cfg(target_os = "macos")]
    fn of(path: &Path) -> Self {
        use std::os::unix::ffi::OsStrExt;
        let Ok(p) = std::ffi::CString::new(path.as_os_str().as_bytes()) else { return Self(None) };
        let mut buf = vec![0u8; 1024];
        // SAFETY: both strings are NUL-terminated and `buf` is as long as said.
        let n = unsafe { libc::getxattr(p.as_ptr(), Self::NAME.as_ptr().cast(), buf.as_mut_ptr().cast(), buf.len(), 0, 0) };
        Self((n > 0).then(|| {
            buf.truncate(n as usize);
            buf
        }))
    }

    #[cfg(not(target_os = "macos"))]
    fn of(_: &Path) -> Self {
        Self(None)
    }

    /// Give the mark to an extracted file or folder (not through a symbolic link).
    fn mark(&self, path: &Path) {
        #[cfg(target_os = "macos")]
        if let Some(value) = &self.0 {
            use std::os::unix::ffi::OsStrExt;
            let Ok(p) = std::ffi::CString::new(path.as_os_str().as_bytes()) else { return };
            // SAFETY: as above; `value` is passed with its length.
            unsafe { libc::setxattr(p.as_ptr(), Self::NAME.as_ptr().cast(), value.as_ptr().cast(), value.len(), 0, libc::XATTR_NOFOLLOW) };
        }
        #[cfg(not(target_os = "macos"))]
        let _ = (path, &self.0);
    }
}

/// Extract `selection` (full inner paths; empty = everything) into `dest`,
/// stripping the `strip` prefix (the directory currently browsed). `policy` decides what
/// happens to files that already exist.
pub fn extract(path: &Path, dest: &Path, selection: &[String], strip: &str, policy: Conflict, progress: &Progress) -> Res<()> {
    let fmt = ArchiveFormat::detect(path).ok_or("Unknown archive format")?;
    std::fs::create_dir_all(dest).map_err(es)?;
    let real_dest = dest.canonicalize().map_err(es)?;
    let quarantine = Quarantine::of(path);
    match fmt {
        ArchiveFormat::Zip => {
            let mut z = zip::ZipArchive::new(BufReader::new(File::open(path).map_err(es)?)).map_err(es)?;
            let mut total = 0;
            for i in 0..z.len() {
                if let Ok(f) = z.by_index_raw(i) {
                    if let Some(n) = f.enclosed_name() {
                        if is_selected(&norm(&n.to_string_lossy()), selection) {
                            total += f.size();
                        }
                    }
                }
            }
            progress.set_total(total);
            let password = password_for(path);
            for i in 0..z.len() {
                progress.check()?;
                // The name first: entries that were not asked for need no password.
                let Some(name) = z.by_index_raw(i).map_err(zip_err)?.enclosed_name() else { continue };
                let name = norm(&name.to_string_lossy());
                if !is_selected(&name, selection) {
                    continue;
                }
                let Some(out) = out_path(dest, &name, strip) else { continue };
                let opened = match &password {
                    Some(p) => z.by_index_decrypt(i, p.as_bytes()),
                    None => z.by_index(i),
                };
                let mut f = opened.map_err(zip_err)?;
                progress.set_current(&name);
                if f.is_dir() {
                    std::fs::create_dir_all(&out).map_err(es)?;
                    quarantine.mark(&out);
                } else {
                    let Some(out) = resolve_out(out, policy) else {
                        progress.add(f.size());
                        continue;
                    };
                    let tmp = write_temp(&mut f, &out, progress)?;
                    quarantine.mark(tmp.path());
                    #[cfg(unix)]
                    if let Some(mode) = f.unix_mode() {
                        use std::os::unix::fs::PermissionsExt;
                        // Never setuid, setgid or sticky bits from an archive.
                        let _ = std::fs::set_permissions(tmp.path(), std::fs::Permissions::from_mode(mode & 0o777));
                    }
                    tmp.commit(&out, policy, None).map_err(es)?;
                }
            }
        }
        ArchiveFormat::SevenZ => {
            let mut r = sevenz_rust2::ArchiveReader::open(path, sevenz_password(path)).map_err(sevenz_err)?;
            let total: u64 = r.archive().files.iter().filter(|f| is_selected(&norm(f.name()), selection)).map(|f| f.size()).sum();
            progress.set_total(total);
            let short = sevenz_short(path);
            let mut err: Option<String> = None;
            r.for_each_entries(|entry, reader| {
                if progress.cancelled() {
                    err = Some("Cancelled".into());
                    return Ok(false);
                }
                let name = norm(entry.name());
                if !is_selected(&name, selection) {
                    std::io::copy(reader, &mut std::io::sink())?;
                    return Ok(true);
                }
                if let Some(out) = out_path(dest, &name, strip) {
                    progress.set_current(&name);
                    if entry.is_directory() {
                        let _ = std::fs::create_dir_all(&out);
                        quarantine.mark(&out);
                    } else if let Some(out) = resolve_out(out, policy) {
                        let mut whole = SevenzWhole { inner: reader, left: entry.size(), short: &short };
                        let placed = write_temp(&mut whole, &out, progress).and_then(|tmp| {
                            quarantine.mark(tmp.path());
                            tmp.commit(&out, policy, None).map_err(es)
                        });
                        if let Err(e) = placed {
                            err = Some(e);
                            return Ok(false);
                        }
                    } else {
                        let skipped = std::io::copy(reader, &mut std::io::sink())?;
                        progress.add(skipped);
                    }
                }
                Ok(true)
            })
            .map_err(sevenz_err)?;
            if let Some(e) = err {
                return Err(e);
            }
        }
        ArchiveFormat::Rar => {
            let password = password_for(path);
            let mut arch = rar_archive(path, &password).open_for_processing().map_err(rar_err)?;
            while let Some(header) = arch.read_header().map_err(rar_err)? {
                progress.check()?;
                let e = header.entry();
                let name = norm(&e.filename.to_string_lossy());
                let selected = is_selected(&name, selection);
                let mut target = if selected { out_path(dest, &name, strip) } else { None };
                if let Some(out) = &target {
                    ensure_inside(&real_dest, out)?;
                }
                if !e.is_directory() {
                    target = target.and_then(|out| resolve_out(out, policy));
                }
                arch = match target {
                    Some(out) if e.is_directory() => {
                        std::fs::create_dir_all(&out).map_err(es)?;
                        quarantine.mark(&out);
                        header.skip().map_err(rar_err)?
                    }
                    Some(out) => {
                        progress.set_current(&name);
                        let size = e.unpacked_size;
                        if let Some(p) = out.parent() {
                            std::fs::create_dir_all(p).map_err(es)?;
                        }
                        // Under a temporary name first; a failed entry is removed again with it.
                        let tmp = TempFile::name_beside(&out);
                        let next = header.extract_to(tmp.path()).map_err(rar_err)?;
                        #[cfg(unix)]
                        if let Some(m) = std::fs::symlink_metadata(tmp.path()).ok().filter(|m| m.is_file()) {
                            use std::os::unix::fs::PermissionsExt;
                            // Never setuid, setgid or sticky bits from an archive.
                            if m.permissions().mode() & 0o7000 != 0 {
                                let _ = std::fs::set_permissions(tmp.path(), std::fs::Permissions::from_mode(m.permissions().mode() & 0o777));
                            }
                        }
                        quarantine.mark(tmp.path());
                        tmp.commit(&out, policy, None).map_err(es)?;
                        progress.add(size);
                        next
                    }
                    None => header.skip().map_err(rar_err)?,
                };
            }
        }
        ArchiveFormat::Tar | ArchiveFormat::TarGz | ArchiveFormat::TarXz | ArchiveFormat::TarZst | ArchiveFormat::TarBz2 => {
            let mut a = tar_reader(path, fmt)?;
            for e in a.entries().map_err(es)? {
                progress.check()?;
                let mut e = e.map_err(es)?;
                let name = norm(&e.path().map_err(es)?.to_string_lossy());
                if !is_selected(&name, selection) {
                    continue;
                }
                let Some(out) = out_path(dest, &name, strip) else { continue };
                ensure_inside(&real_dest, &out)?;
                let kind = e.header().entry_type();
                let size = e.header().size().unwrap_or(0);
                if kind.is_pax_global_extensions() || kind.is_pax_local_extensions() || kind.is_gnu_longname() || kind.is_gnu_longlink() {
                    continue;
                }
                progress.set_current(&name);
                // Old archives mark a folder only by its trailing slash.
                if kind.is_dir() || e.path_bytes().ends_with(b"/") {
                    std::fs::create_dir_all(&out).map_err(es)?;
                    e.unpack(&out).map_err(es)?;
                    quarantine.mark(&out);
                    continue;
                }
                let Some(out) = resolve_out(out, policy) else {
                    progress.add(size);
                    continue;
                };
                if let Some(p) = out.parent() {
                    std::fs::create_dir_all(p).map_err(es)?;
                }
                // Every entry is made under a temporary name and then moved into place by the
                // policy, so an existing file is only replaced by a complete one.
                let tmp = if kind.is_hard_link() {
                    // `unpack` would link to the raw name, which can be any file on disk.
                    let link = e.link_name().map_err(es)?.ok_or("A hard link in the archive has no target")?;
                    let src = out_path(dest, &norm(&link.to_string_lossy()), strip)
                        .filter(|src| src.canonicalize().is_ok_and(|real| real.starts_with(&real_dest)))
                        .ok_or_else(|| format!("Refusing to extract '{name}': its hard link leads out of the destination folder"))?;
                    let tmp = TempFile::name_beside(&out);
                    std::fs::hard_link(&src, tmp.path()).map_err(es)?;
                    progress.add(size);
                    tmp
                } else if kind.is_symlink() || kind.is_gnu_sparse() {
                    // tar makes these itself (and checks a sparse file's data).
                    let tmp = TempFile::name_beside(&out);
                    e.unpack(tmp.path()).map_err(es)?;
                    progress.add(size);
                    tmp
                } else {
                    // A file (other kinds are written as files too, as tar does).
                    let (mode, mtime) = (e.header().mode().unwrap_or(0o644), e.header().mtime().ok());
                    let (mut tmp, n) = write_temp_counted(&mut e, &out, progress)?;
                    // A cut-off archive just ends: the file must have all its bytes.
                    if n != size {
                        return Err(format!("{}: {name}", crate::i18n::t("The archive is damaged or incomplete")));
                    }
                    if let Some(secs) = mtime {
                        let _ = tmp.file().set_modified(UNIX_EPOCH + std::time::Duration::from_secs(secs));
                    }
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        // Never setuid, setgid or sticky bits from an archive.
                        let _ = std::fs::set_permissions(tmp.path(), std::fs::Permissions::from_mode(mode & 0o777));
                    }
                    #[cfg(not(unix))]
                    let _ = mode;
                    tmp
                };
                quarantine.mark(tmp.path());
                tmp.commit(&out, policy, None).map_err(es)?;
            }
        }
        ArchiveFormat::Gz | ArchiveFormat::Xz | ArchiveFormat::Zst | ArchiveFormat::Bz2 => {
            let mut r = single_reader(path, fmt)?;
            let name = stem_name(path);
            progress.set_current(&name);
            let out = dest.join(&name);
            // Never overwritten: `report.txt.gz` often sits beside its own `report.txt`.
            if policy != Conflict::Skip || !safefs::exists(&out) {
                let tmp = write_temp(&mut r, &out, progress)?;
                quarantine.mark(tmp.path());
                tmp.commit(&out, Conflict::Rename, None).map_err(es)?;
            }
        }
    }
    Ok(())
}

/// Read (at most `max` bytes of) a single entry, for previews.
/// 7z and RAR entries can only be unpacked whole, even when just the start is wanted.
const MAX_WHOLE_ENTRY: u64 = 512 * 1024 * 1024;

fn entry_too_large() -> String {
    crate::i18n::t("This file is too large to preview inside this kind of archive.")
}

pub fn read_entry(path: &Path, inner: &str, max: usize) -> Res<Vec<u8>> {
    let fmt = ArchiveFormat::detect(path).ok_or("Unknown archive format")?;
    let mut buf = Vec::new();
    match fmt {
        ArchiveFormat::Zip => {
            let mut z = zip::ZipArchive::new(BufReader::new(File::open(path).map_err(es)?)).map_err(es)?;
            let mut f = match password_for(path) {
                Some(p) => z.by_name_decrypt(inner, p.as_bytes()),
                None => z.by_name(inner),
            }
            .map_err(zip_err)?;
            if !f.encrypted() {
                f.take(max as u64).read_to_end(&mut buf).map_err(es)?;
                return Ok(buf);
            }
            // Encrypted data is only proven by the authentication code (AES) or checksum
            // (ZipCrypto) at its end, so it is read whole: a wrong password must not pass as data.
            if f.size() > MAX_WHOLE_ENTRY {
                return Err(entry_too_large());
            }
            (&mut f).take(max as u64).read_to_end(&mut buf).map_err(|_| wrong_password())?;
            std::io::copy(&mut (&mut f).take(MAX_WHOLE_ENTRY + 1), &mut std::io::sink()).map_err(|_| wrong_password())?;
        }
        ArchiveFormat::SevenZ => {
            let mut r = sevenz_rust2::ArchiveReader::open(path, sevenz_password(path)).map_err(sevenz_err)?;
            let size = r.archive().files.iter().find(|f| f.name() == inner).map(|f| f.size());
            if size.is_some_and(|s| s > MAX_WHOLE_ENTRY) {
                return Err(entry_too_large());
            }
            buf = r.read_file(inner).map_err(sevenz_err)?;
            if size.is_some_and(|s| buf.len() as u64 != s) {
                return Err(sevenz_short(path));
            }
            buf.truncate(max);
        }
        ArchiveFormat::Rar => {
            let password = password_for(path);
            let mut arch = rar_archive(path, &password).open_for_processing().map_err(rar_err)?;
            while let Some(header) = arch.read_header().map_err(rar_err)? {
                if norm(&header.entry().filename.to_string_lossy()) == inner {
                    if header.entry().unpacked_size > MAX_WHOLE_ENTRY {
                        return Err(entry_too_large());
                    }
                    let (data, _) = header.read().map_err(rar_err)?;
                    buf = data;
                    buf.truncate(max);
                    return Ok(buf);
                }
                arch = header.skip().map_err(es)?;
            }
            return Err("Entry not found".into());
        }
        ArchiveFormat::Tar | ArchiveFormat::TarGz | ArchiveFormat::TarXz | ArchiveFormat::TarZst | ArchiveFormat::TarBz2 => {
            let mut a = tar_reader(path, fmt)?;
            for e in a.entries().map_err(es)? {
                let e = e.map_err(es)?;
                if norm(&e.path().map_err(es)?.to_string_lossy()) == inner {
                    e.take(max as u64).read_to_end(&mut buf).map_err(es)?;
                    return Ok(buf);
                }
            }
            return Err("Entry not found".into());
        }
        ArchiveFormat::Gz | ArchiveFormat::Xz | ArchiveFormat::Zst | ArchiveFormat::Bz2 => {
            single_reader(path, fmt)?.take(max as u64).read_to_end(&mut buf).map_err(es)?;
        }
    }
    Ok(buf)
}

/// A file or directory to put into an archive.
pub struct Item {
    pub src: PathBuf,
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
}

/// Walk `sources` and produce archive items named relative to each source's parent.
///
/// An archive missing part of its sources must not pass for a complete one (backup rotation
/// would then delete the good ones), so anything unreadable is an error. The whole walk still
/// finishes, in order to name every such path once.
pub fn collect_items(sources: &[PathBuf], exclude: Option<&GlobSet>, progress: Option<&Progress>) -> Res<Vec<Item>> {
    let mut out = Vec::new();
    let mut unreadable = Vec::new();
    for src in sources {
        let base = src.parent().map(|p| p.to_path_buf()).unwrap_or_default();
        let walker = walkdir::WalkDir::new(src).follow_links(false).into_iter();
        let it = walker.filter_entry(|e| {
            let Some(ex) = exclude else { return true };
            let p = e.path();
            let rel = p.strip_prefix(&base).unwrap_or(p);
            let hit = |q: &Path| ex.is_match(q);
            // A directory is pruned when its own path matches or when every child would
            // (e.g. `**/node_modules/**` excludes the `node_modules` folder itself).
            let dir_hit = e.file_type().is_dir() && (hit(&p.join("\u{1}")) || hit(&rel.join("\u{1}")));
            !(hit(p) || hit(rel) || e.file_name().to_str().is_some_and(|n| ex.is_match(n)) || dir_hit)
        });
        for e in it {
            if progress.is_some_and(|p| p.cancelled()) {
                return Ok(out);
            }
            let e = match e {
                Ok(e) => e,
                Err(err) => {
                    unreadable.push(err.path().unwrap_or(src).display().to_string());
                    continue;
                }
            };
            let rel = e.path().strip_prefix(&base).unwrap_or(e.path());
            let name = rel.components().map(|c| c.as_os_str().to_string_lossy().to_string()).collect::<Vec<_>>().join("/");
            if name.is_empty() {
                continue;
            }
            let ft = e.file_type();
            if ft.is_dir() {
                out.push(Item { src: e.path().to_path_buf(), name, is_dir: true, size: 0 });
            } else if let Ok(meta) = std::fs::metadata(e.path()) {
                if meta.is_file() {
                    out.push(Item { src: e.path().to_path_buf(), name, is_dir: false, size: meta.len() });
                }
            }
            if let Some(p) = progress {
                p.set_current(&e.path().to_string_lossy());
            }
        }
    }
    match unreadable.as_slice() {
        [] => Ok(out),
        [first, ..] => Err(format!("Cannot read {} item(s), such as {first}. Nothing was archived. Fix the permissions or exclude them.", unreadable.len())),
    }
}

/// The file name of an archive named `name` in format `fmt`: the extension is added
/// unless it was typed already.
pub fn archive_file_name(name: &str, fmt: ArchiveFormat) -> String {
    let name = name.trim();
    if ArchiveFormat::typed_in(name) == Some(fmt) {
        name.to_string()
    } else {
        format!("{name}.{}", fmt.extension())
    }
}

/// Create an archive at `dest` from `sources`, and return where it is. It is written to a
/// hidden temporary file and only renamed to `dest` once complete and synced; should the
/// name have been taken meanwhile, it gets a free one (`name (1).zip`) instead.
/// `extra` holds additional in-memory files (name, content), e.g. a backup manifest.
pub fn create(
    sources: &[PathBuf],
    exclude: Option<&GlobSet>,
    dest: &Path,
    fmt: ArchiveFormat,
    level: u32,
    extra: &[(String, Vec<u8>)],
    progress: &Progress,
) -> Res<PathBuf> {
    let level = level.clamp(1, 9);
    if fmt == ArchiveFormat::Rar {
        return create_rar(sources, exclude.is_some(), dest, level, progress);
    }
    progress.set_current("Scanning…");
    let items = collect_items(sources, exclude, Some(progress))?;
    progress.check()?;
    progress.set_total(items.iter().map(|i| i.size).sum::<u64>() + extra.iter().map(|e| e.1.len() as u64).sum::<u64>());
    // Dropped on any error, which removes it: `dest` itself is never touched until the end.
    let mut tmp = TempFile::beside(dest).map_err(es)?;
    let f = tmp.file();
    let result = match fmt {
        ArchiveFormat::Zip => create_zip(&items, extra, f, level, progress),
        ArchiveFormat::SevenZ => create_7z(&items, extra, f, level, progress),
        ArchiveFormat::Tar => write_tar(BufWriter::new(f), &items, extra, progress).and_then(flush),
        ArchiveFormat::TarGz => {
            let enc = flate2::write::GzEncoder::new(BufWriter::new(f), flate2::Compression::new(level));
            write_tar(enc, &items, extra, progress)?.finish().map_err(es).and_then(flush)
        }
        ArchiveFormat::TarXz => {
            let enc = xz2::write::XzEncoder::new(BufWriter::new(f), level);
            write_tar(enc, &items, extra, progress)?.finish().map_err(es).and_then(flush)
        }
        ArchiveFormat::TarZst => {
            let mut enc = zstd::stream::write::Encoder::new(BufWriter::new(f), zstd_level(level)).map_err(es)?;
            let _ = enc.multithread(num_threads());
            write_tar(enc, &items, extra, progress)?.finish().map_err(es).and_then(flush)
        }
        ArchiveFormat::TarBz2 => {
            let enc = bzip2::write::BzEncoder::new(BufWriter::new(f), bzip2::Compression::new(level));
            write_tar(enc, &items, extra, progress)?.finish().map_err(es).and_then(flush)
        }
        ArchiveFormat::Gz | ArchiveFormat::Xz | ArchiveFormat::Zst | ArchiveFormat::Bz2 => {
            let files: Vec<&Item> = items.iter().filter(|i| !i.is_dir).collect();
            if files.len() != 1 || items.len() != 1 {
                return Err(format!(".{} compresses a single file. Use .tar.{} for folders or multiple files.", fmt.extension(), fmt.extension()));
            }
            create_single(&files[0].src, f, fmt, level, progress)
        }
        ArchiveFormat::Rar => unreachable!(),
    };
    result?;
    // Never over a file that appeared meanwhile: that one is kept, and this gets a free name.
    Ok(tmp.commit(dest, Conflict::Rename, None).map_err(es)?.unwrap_or_else(|| dest.to_path_buf()))
}

/// Write out what a buffered writer still holds (dropping it would hide a write error).
fn flush<W: Write>(mut w: BufWriter<W>) -> Res<()> {
    w.flush().map_err(es)
}

fn zstd_level(level: u32) -> i32 {
    // Map 1..=9 onto zstd's 1..=19.
    (level as i32 * 2 + 1).min(19)
}

fn num_threads() -> u32 {
    std::thread::available_parallelism().map(|n| n.get() as u32).unwrap_or(1)
}

fn create_zip(items: &[Item], extra: &[(String, Vec<u8>)], f: &mut File, level: u32, progress: &Progress) -> Res<()> {
    use zip::write::SimpleFileOptions;
    let mut z = zip::ZipWriter::new(BufWriter::new(f));
    let base = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated).compression_level(Some(level as i64));
    for it in items {
        progress.check()?;
        progress.set_current(&it.name);
        let meta = std::fs::metadata(&it.src).ok();
        let mut opts = base;
        if let Some(t) = meta.as_ref().and_then(|m| m.modified().ok()).and_then(to_zip_time) {
            opts = opts.last_modified_time(t);
        }
        #[cfg(unix)]
        if let Some(m) = &meta {
            use std::os::unix::fs::PermissionsExt;
            opts = opts.unix_permissions(m.permissions().mode() & 0o7777);
        }
        if it.is_dir {
            z.add_directory(format!("{}/", it.name), opts).map_err(es)?;
        } else {
            opts = opts.large_file(it.size >= u32::MAX as u64);
            z.start_file(it.name.as_str(), opts).map_err(es)?;
            let mut r = ProgressReader { inner: BufReader::new(File::open(&it.src).map_err(es)?), progress };
            std::io::copy(&mut r, &mut z).map_err(es)?;
        }
    }
    let now_opts = match to_zip_time(SystemTime::now()) {
        Some(t) => base.last_modified_time(t),
        None => base,
    };
    for (name, data) in extra {
        z.start_file(name.as_str(), now_opts).map_err(es)?;
        z.write_all(data).map_err(es)?;
        progress.add(data.len() as u64);
    }
    flush(z.finish().map_err(es)?)
}

fn create_7z(items: &[Item], extra: &[(String, Vec<u8>)], f: &mut File, level: u32, progress: &Progress) -> Res<()> {
    use sevenz_rust2::{encoder_options::Lzma2Options, ArchiveEntry, ArchiveWriter};
    let mut w = ArchiveWriter::new(f).map_err(es)?;
    w.set_content_methods(vec![Lzma2Options::from_level(level).into()]);
    for it in items {
        progress.check()?;
        progress.set_current(&it.name);
        let entry = ArchiveEntry::from_path(&it.src, it.name.clone());
        if it.is_dir {
            w.push_archive_entry::<File>(entry, None).map_err(es)?;
        } else {
            let r = ProgressReader { inner: BufReader::new(File::open(&it.src).map_err(es)?), progress };
            w.push_archive_entry(entry, Some(r)).map_err(es)?;
        }
    }
    for (name, data) in extra {
        let entry = ArchiveEntry::new_file(name);
        w.push_archive_entry(entry, Some(std::io::Cursor::new(data.clone()))).map_err(es)?;
        progress.add(data.len() as u64);
    }
    w.finish().map_err(es)?;
    Ok(())
}

fn write_tar<W: Write>(w: W, items: &[Item], extra: &[(String, Vec<u8>)], progress: &Progress) -> Res<W> {
    let mut b = tar::Builder::new(w);
    b.follow_symlinks(true);
    for it in items {
        progress.check()?;
        progress.set_current(&it.name);
        if it.is_dir {
            b.append_dir(&it.name, &it.src).map_err(es)?;
        } else {
            let f = File::open(&it.src).map_err(es)?;
            let meta = f.metadata().map_err(es)?;
            let mut h = tar::Header::new_gnu();
            h.set_metadata(&meta);
            h.set_size(meta.len());
            let r = ProgressReader { inner: BufReader::new(f), progress };
            b.append_data(&mut h, &it.name, r).map_err(es)?;
        }
    }
    for (name, data) in extra {
        let mut h = tar::Header::new_gnu();
        h.set_size(data.len() as u64);
        h.set_mode(0o644);
        h.set_mtime(SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0));
        b.append_data(&mut h, name, data.as_slice()).map_err(es)?;
        progress.add(data.len() as u64);
    }
    b.into_inner().map_err(es)
}

fn create_single(src: &Path, out: &mut File, fmt: ArchiveFormat, level: u32, progress: &Progress) -> Res<()> {
    progress.set_current(&src.to_string_lossy());
    let mut r = ProgressReader { inner: BufReader::new(File::open(src).map_err(es)?), progress };
    let f = BufWriter::new(out);
    let f = match fmt {
        ArchiveFormat::Gz => {
            let mut e = flate2::write::GzEncoder::new(f, flate2::Compression::new(level));
            std::io::copy(&mut r, &mut e).map_err(es)?;
            e.finish().map_err(es)?
        }
        ArchiveFormat::Xz => {
            let mut e = xz2::write::XzEncoder::new(f, level);
            std::io::copy(&mut r, &mut e).map_err(es)?;
            e.finish().map_err(es)?
        }
        ArchiveFormat::Zst => {
            let mut e = zstd::stream::write::Encoder::new(f, zstd_level(level)).map_err(es)?;
            let _ = e.multithread(num_threads());
            std::io::copy(&mut r, &mut e).map_err(es)?;
            e.finish().map_err(es)?
        }
        ArchiveFormat::Bz2 => {
            let mut e = bzip2::write::BzEncoder::new(f, bzip2::Compression::new(level));
            std::io::copy(&mut r, &mut e).map_err(es)?;
            e.finish().map_err(es)?
        }
        _ => unreachable!(),
    };
    flush(f)
}

fn create_rar(sources: &[PathBuf], has_exclude: bool, dest: &Path, level: u32, progress: &Progress) -> Res<PathBuf> {
    let rar = rar_cli().ok_or("RAR is a proprietary format: creating .rar requires the `rar` command-line tool (WinRAR / rarlab) in PATH.")?;
    let _ = has_exclude;
    progress.set_current("rar…");
    // `rar a` adds to an archive that is there, so it writes a new one in a temporary
    // folder (removed again with whatever is left in it), which is then moved into place.
    let dir = TempFile::name_beside(dest);
    std::fs::create_dir(dir.path()).map_err(es)?;
    let inner = dir.path().join(dest.file_name().unwrap_or(std::ffi::OsStr::new("archive.rar")));
    let mut cmd = std::process::Command::new(rar);
    cmd.arg("a").arg("-r").arg("-ep1").arg("-idq").arg("-y").arg(format!("-m{}", (level * 5 / 9).clamp(1, 5))).arg(&inner);
    for s in sources {
        cmd.arg(s);
    }
    let out = cmd.output().map_err(es)?;
    if out.status.success() {
        safefs::sync_file(&File::open(&inner).map_err(es)?).map_err(es)?;
        Ok(safefs::place(&inner, dest, Conflict::Rename, None).map_err(es)?.unwrap_or_else(|| dest.to_path_buf()))
    } else {
        Err(format!("rar failed: {}", String::from_utf8_lossy(&out.stderr)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What comes out of a downloaded (quarantined) archive is quarantined too, so
    /// Gatekeeper still checks it; nothing else is marked.
    #[cfg(target_os = "macos")]
    #[test]
    fn extracted_files_keep_the_archive_quarantine() {
        let d = tmpdir("quarantine");
        let zip_path = d.join("download.zip");
        {
            let mut w = zip::ZipWriter::new(File::create(&zip_path).unwrap());
            w.start_file("app/run.command", zip::write::SimpleFileOptions::default()).unwrap();
            w.write_all(b"#!/bin/sh\necho hi\n").unwrap();
            w.finish().unwrap();
        }
        let value = b"0083;00000000;Safari;";
        let set = |p: &Path| {
            use std::os::unix::ffi::OsStrExt;
            let c = std::ffi::CString::new(p.as_os_str().as_bytes()).unwrap();
            // SAFETY: NUL-terminated strings and a buffer of the given length.
            unsafe { libc::setxattr(c.as_ptr(), Quarantine::NAME.as_ptr().cast(), value.as_ptr().cast(), value.len(), 0, 0) };
        };
        let out = d.join("out");
        extract(&zip_path, &out, &[], "", Conflict::Overwrite, &Progress::default()).unwrap();
        assert!(Quarantine::of(&out.join("app/run.command")).0.is_none(), "not downloaded: not marked");
        set(&zip_path);
        let out = d.join("out2");
        extract(&zip_path, &out, &[], "", Conflict::Overwrite, &Progress::default()).unwrap();
        assert_eq!(Quarantine::of(&out.join("app/run.command")).0.as_deref(), Some(&value[..]));
        assert_eq!(Quarantine::of(&out.join("app")).0, None, "folders made on the way are not entries");
        let _ = std::fs::remove_dir_all(&d);
    }

    fn tmpdir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("matrix_finder-test-{}-{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn sample(dir: &Path) -> PathBuf {
        let src = dir.join("src");
        std::fs::create_dir_all(src.join("sub/empty")).unwrap();
        std::fs::write(src.join("a.txt"), b"hello world").unwrap();
        std::fs::write(src.join("sub/b.rs"), b"fn main() {}").unwrap();
        src
    }

    #[test]
    fn formats_offered_for_compressing() {
        use ArchiveFormat as F;
        // Folders or several files: containers only.
        let many = ArchiveFormat::offered(false);
        assert_eq!(many[..3], [F::Zip, F::SevenZ, F::TarZst]);
        assert!(many.iter().all(|f| !f.single_file()));
        assert!(many.contains(&F::Rar), "shown, and turned off without the rar tool");
        // One file: the single-file formats too.
        let one = ArchiveFormat::offered(true);
        assert!(one.contains(&F::Gz) && one.contains(&F::Zst));
        assert_eq!(one.len(), F::ALL.len());
        // Every format says what it is good for; plain .tar has no level.
        assert!(F::ALL.iter().all(|f| !f.blurb().is_empty()));
        assert!(!F::Tar.compresses() && F::TarGz.compresses() && F::Zip.compresses());
    }

    #[test]
    fn format_typed_in_the_name() {
        use ArchiveFormat as F;
        assert_eq!(ArchiveFormat::typed_in("photos.7z"), Some(F::SevenZ));
        assert_eq!(ArchiveFormat::typed_in("photos.TAR.GZ"), Some(F::TarGz), "the longest ending wins");
        assert_eq!(ArchiveFormat::typed_in("notes.gz"), Some(F::Gz));
        assert_eq!(ArchiveFormat::typed_in("photos"), None);
        assert_eq!(ArchiveFormat::typed_in("v1.2"), None);
        // The file name: the extension is added once.
        assert_eq!(archive_file_name("  photos ", F::Zip), "photos.zip");
        assert_eq!(archive_file_name("photos.ZIP", F::Zip), "photos.ZIP");
        assert_eq!(archive_file_name("photos.tar.gz", F::TarGz), "photos.tar.gz");
        assert_eq!(archive_file_name("photos.gz", F::TarGz), "photos.gz.tar.gz");
        assert_eq!(archive_file_name("v1.2", F::SevenZ), "v1.2.7z");
    }

    #[test]
    fn roundtrip_all_container_formats() {
        for fmt in [
            ArchiveFormat::Zip,
            ArchiveFormat::SevenZ,
            ArchiveFormat::Tar,
            ArchiveFormat::TarGz,
            ArchiveFormat::TarXz,
            ArchiveFormat::TarZst,
            ArchiveFormat::TarBz2,
        ] {
            let dir = tmpdir(fmt.extension());
            let src = sample(&dir);
            let dest = dir.join(format!("out.{}", fmt.extension()));
            let p = Progress::default();
            create(&[src.clone()], None, &dest, fmt, 6, &[("manifest.json".into(), b"{}".to_vec())], &p).unwrap();
            let entries = list(&dest).unwrap();
            let names: Vec<_> = entries.iter().map(|e| e.path.clone()).collect();
            assert!(names.contains(&"src/a.txt".to_string()), "{fmt:?}: {names:?}");
            assert!(names.contains(&"src/sub/b.rs".to_string()), "{fmt:?}: {names:?}");
            assert!(names.contains(&"manifest.json".to_string()), "{fmt:?}: {names:?}");

            let kids = children(&entries, "src");
            assert!(kids.iter().any(|k| k.name == "sub" && k.is_dir), "{fmt:?}");

            assert_eq!(read_entry(&dest, "src/a.txt", 1000).unwrap(), b"hello world", "{fmt:?}");

            // Extract only the `sub` folder, relative to `src`.
            let out = dir.join("x");
            extract(&dest, &out, &["src/sub".into()], "src", Conflict::Overwrite, &Progress::default()).unwrap();
            assert_eq!(std::fs::read(out.join("sub/b.rs")).unwrap(), b"fn main() {}", "{fmt:?}");
            assert!(!out.join("a.txt").exists());

            let all = dir.join("all");
            extract(&dest, &all, &[], "", Conflict::Overwrite, &Progress::default()).unwrap();
            assert!(all.join("src/a.txt").exists(), "{fmt:?}");
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    /// A summary can end a listing part way: after some entries, or when told to stop
    /// (a compressed tar is otherwise read through to its end).
    #[test]
    fn listing_stops_when_asked() {
        for fmt in [ArchiveFormat::Zip, ArchiveFormat::TarGz, ArchiveFormat::TarZst] {
            let dir = tmpdir(&format!("stop-{}", fmt.extension()));
            let src = sample(&dir);
            let dest = dir.join(format!("out.{}", fmt.extension()));
            create(&[src], None, &dest, fmt, 6, &[], &Progress::default()).unwrap();
            let (all, cut) = list_until(&dest, usize::MAX, &|| false).unwrap();
            assert!(!cut && all.len() >= 4, "{fmt:?}: {all:?}");
            let (some, cut) = list_until(&dest, 2, &|| false).unwrap();
            assert!(cut && some.len() == 2, "{fmt:?}: {some:?}");
            let (none, cut) = list_until(&dest, usize::MAX, &|| true).unwrap();
            assert!(cut && none.is_empty(), "{fmt:?}: {none:?}");
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn single_file_streams() {
        for fmt in [ArchiveFormat::Gz, ArchiveFormat::Xz, ArchiveFormat::Zst, ArchiveFormat::Bz2] {
            let dir = tmpdir(&format!("single-{}", fmt.extension()));
            let f = dir.join("data.txt");
            std::fs::write(&f, b"abcabcabc").unwrap();
            let dest = dir.join(format!("data.txt.{}", fmt.extension()));
            create(&[f.clone()], None, &dest, fmt, 5, &[], &Progress::default()).unwrap();
            assert_eq!(read_entry(&dest, "data.txt", 100).unwrap(), b"abcabcabc");
            let out = dir.join("o");
            extract(&dest, &out, &[], "", Conflict::Overwrite, &Progress::default()).unwrap();
            assert_eq!(std::fs::read(out.join("data.txt")).unwrap(), b"abcabcabc");
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn rejects_traversal() {
        assert!(safe_join(Path::new("/tmp/x"), "../etc/passwd").is_none());
        assert!(safe_join(Path::new("/tmp/x"), "/etc/passwd").is_none());
        assert!(safe_join(Path::new("/tmp/x"), "a/b").is_some());
    }

    /// A tar file from (name, link target or contents) entries; `kind` is what they are.
    fn tar_with(path: &Path, entries: &[(tar::EntryType, &str, &str)]) {
        let mut b = tar::Builder::new(File::create(path).unwrap());
        for (kind, name, data) in entries {
            let mut h = tar::Header::new_gnu();
            h.set_entry_type(*kind);
            h.set_mode(0o644);
            if *kind == tar::EntryType::Regular {
                h.set_size(data.len() as u64);
                b.append_data(&mut h, name, data.as_bytes()).unwrap();
            } else {
                h.set_size(0);
                b.append_link(&mut h, name, data).unwrap();
            }
        }
        b.finish().unwrap();
    }

    #[test]
    fn tar_cannot_write_through_its_own_symlink() {
        use tar::EntryType::{Regular, Symlink};
        let d = tmpdir("tar-symlink");
        let outside = d.join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("victim"), "safe").unwrap();
        let target = outside.to_string_lossy().to_string();
        for (i, inner) in ["x/pwn.txt", "x/new/pwn.txt"].iter().enumerate() {
            let t = d.join(format!("evil{i}.tar"));
            tar_with(&t, &[(Symlink, "x", target.as_str()), (Regular, inner, "pwned")]);
            let out = d.join(format!("out{i}"));
            let r = extract(&t, &out, &[], "", Conflict::Overwrite, &Progress::default());
            assert!(r.unwrap_err().contains("symbolic link"));
        }
        assert!(!outside.join("pwn.txt").exists());
        assert!(!outside.join("new").exists(), "no folder may be created outside either");
        // A file over the archive's own link replaces the link, not what it points to.
        let t = d.join("over.tar");
        let victim = outside.join("victim").to_string_lossy().to_string();
        tar_with(&t, &[(Symlink, "f", victim.as_str()), (Regular, "f", "pwned")]);
        extract(&t, &d.join("over"), &[], "", Conflict::Overwrite, &Progress::default()).unwrap();
        assert_eq!(std::fs::read(outside.join("victim")).unwrap(), b"safe");
        assert_eq!(std::fs::read(d.join("over/f")).unwrap(), b"pwned");
    }

    #[test]
    fn tar_links_that_stay_inside_still_work() {
        use tar::EntryType::{Link, Regular, Symlink};
        let d = tmpdir("tar-inside");
        let t = d.join("ok.tar");
        tar_with(&t, &[(Regular, "real/a.txt", "hello"), (Symlink, "alias", "real"), (Regular, "alias/b.txt", "via link"), (Link, "hard.txt", "real/a.txt")]);
        let out = d.join("out");
        extract(&t, &out, &[], "", Conflict::Overwrite, &Progress::default()).unwrap();
        assert_eq!(std::fs::read(out.join("real/b.txt")).unwrap(), b"via link");
        assert_eq!(std::fs::read(out.join("hard.txt")).unwrap(), b"hello");
    }

    #[test]
    fn tar_hard_links_cannot_reach_outside() {
        use tar::EntryType::Link;
        let d = tmpdir("tar-hard");
        let secret = d.join("secret");
        std::fs::write(&secret, "secret").unwrap();
        let abs = secret.to_string_lossy().to_string();
        for (i, target) in [abs.as_str(), "../secret"].iter().enumerate() {
            let t = d.join(format!("hard{i}.tar"));
            tar_with(&t, &[(Link, "stolen", target)]);
            let out = d.join(format!("out{i}"));
            let r = extract(&t, &out, &[], "", Conflict::Overwrite, &Progress::default());
            assert!(r.unwrap_err().contains("hard link"), "{target}");
            assert!(!out.join("stolen").exists());
        }
    }

    #[test]
    fn extract_follows_the_conflict_policy() {
        for fmt in [ArchiveFormat::Zip, ArchiveFormat::SevenZ, ArchiveFormat::Tar] {
            let d = tmpdir(&format!("policy-{}", fmt.extension()));
            let src = sample(&d);
            let dest = d.join(format!("a.{}", fmt.extension()));
            create(std::slice::from_ref(&src), None, &dest, fmt, 3, &[], &Progress::default()).unwrap();
            let sel = ["src/a.txt".to_string()];
            let run = |name: &str, policy| {
                let out = d.join(name);
                std::fs::create_dir_all(&out).unwrap();
                std::fs::write(out.join("a.txt"), "mine").unwrap();
                extract(&dest, &out, &sel, "src", policy, &Progress::default()).unwrap();
                out
            };
            let out = run("skip", Conflict::Skip);
            assert_eq!(std::fs::read(out.join("a.txt")).unwrap(), b"mine", "{fmt:?}");
            let out = run("rename", Conflict::Rename);
            assert_eq!(std::fs::read(out.join("a.txt")).unwrap(), b"mine", "{fmt:?}");
            assert_eq!(std::fs::read(out.join("a (1).txt")).unwrap(), b"hello world", "{fmt:?}");
            let out = run("over", Conflict::Overwrite);
            assert_eq!(std::fs::read(out.join("a.txt")).unwrap(), b"hello world", "{fmt:?}");
        }
    }

    fn names_in(d: &Path) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(d).unwrap().flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect();
        v.sort();
        v
    }

    /// An archive that fails halfway through a file (damaged, cut off, or cancelled: the
    /// same path) leaves the file it was to overwrite as it was, and nothing else behind.
    #[test]
    fn a_failed_overwrite_while_extracting_keeps_the_existing_file() {
        let d = tmpdir("extract-fail");
        let big: Vec<u8> = (0..400_000u32).map(|i| (i * 7 % 251) as u8).collect();
        // A zip whose stored data is changed: the checksum fails at its end.
        let zip_path = d.join("bad.zip");
        {
            let mut w = zip::ZipWriter::new(File::create(&zip_path).unwrap());
            w.start_file("a.txt", zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored)).unwrap();
            w.write_all(&big).unwrap();
            w.finish().unwrap();
        }
        let mut bytes = std::fs::read(&zip_path).unwrap();
        bytes[200_000] ^= 0xff;
        std::fs::write(&zip_path, bytes).unwrap();
        // A tar cut off in the middle of the file.
        let tar_path = d.join("cut.tar");
        {
            let mut b = tar::Builder::new(File::create(&tar_path).unwrap());
            let mut h = tar::Header::new_gnu();
            h.set_size(big.len() as u64);
            h.set_mode(0o644);
            b.append_data(&mut h, "a.txt", big.as_slice()).unwrap();
            b.finish().unwrap();
        }
        let f = std::fs::OpenOptions::new().write(true).open(&tar_path).unwrap();
        f.set_len(512 + 100_000).unwrap();
        for archive in [&zip_path, &tar_path] {
            let out = d.join(format!("out-{}", archive.extension().unwrap().to_string_lossy()));
            std::fs::create_dir_all(&out).unwrap();
            std::fs::write(out.join("a.txt"), "precious").unwrap();
            assert!(extract(archive, &out, &[], "", Conflict::Overwrite, &Progress::default()).is_err(), "{archive:?}");
            assert_eq!(std::fs::read(out.join("a.txt")).unwrap(), b"precious", "{archive:?}");
            assert_eq!(names_in(&out), ["a.txt"], "no temporary file left: {archive:?}");
        }
    }

    /// A name taken after the dialog checked it is kept: the archive gets a free name.
    #[test]
    fn compressing_never_replaces_a_file() {
        let d = tmpdir("compress-taken");
        let src = sample(&d);
        for fmt in [ArchiveFormat::Zip, ArchiveFormat::SevenZ, ArchiveFormat::TarGz] {
            let dest = d.join(format!("out.{}", fmt.extension()));
            std::fs::write(&dest, "someone else's").unwrap();
            let made = create(std::slice::from_ref(&src), None, &dest, fmt, 3, &[], &Progress::default()).unwrap();
            assert_eq!(made, d.join(format!("out (1).{}", fmt.extension())), "{fmt:?}");
            assert_eq!(std::fs::read(&dest).unwrap(), b"someone else's");
            assert!(list(&made).unwrap().iter().any(|e| e.path == "src/a.txt"), "{fmt:?}");
        }
        // A cancelled one leaves nothing, and takes nothing.
        let p = Progress::default();
        p.cancel();
        let before = names_in(&d);
        assert!(create(std::slice::from_ref(&src), None, &d.join("out.zip"), ArchiveFormat::Zip, 3, &[], &p).is_err());
        assert_eq!(names_in(&d), before);
        assert_eq!(std::fs::read(d.join("out.zip")).unwrap(), b"someone else's");
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_folders_fail_the_archive_instead_of_truncating_it() {
        use std::os::unix::fs::PermissionsExt;
        // Root reads a folder whatever its permissions say (as in a container).
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let d = tmpdir("unreadable");
        let src = d.join("src");
        let locked = src.join("locked");
        std::fs::create_dir_all(&locked).unwrap();
        std::fs::write(src.join("a.txt"), "a").unwrap();
        std::fs::write(src.join("z.txt"), "z").unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        let dest = d.join("a.zip");
        let r = create(std::slice::from_ref(&src), None, &dest, ArchiveFormat::Zip, 3, &[], &Progress::default());
        let err = r.unwrap_err();
        assert!(err.contains("locked") && err.contains("1 item"), "{err}");
        assert!(!dest.exists(), "no partial archive");
        assert_eq!(names_in(&d), ["src"], "nor a temporary one");
        // Excluding it is the way out, and everything else then gets in.
        let ex = crate::util::build_globset(&["**/locked".to_string()]).unwrap();
        create(std::slice::from_ref(&src), ex.as_ref(), &dest, ArchiveFormat::Zip, 3, &[], &Progress::default()).unwrap();
        let names: Vec<String> = list(&dest).unwrap().into_iter().map(|e| e.path).collect();
        assert!(names.contains(&"src/a.txt".to_string()) && names.contains(&"src/z.txt".to_string()), "{names:?}");
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

#[cfg(test)]
mod password_tests {
    use super::*;
    use std::io::Cursor;
    use zip::write::SimpleFileOptions;
    use zip::{AesMode, ZipWriter};

    const PASSWORD: &str = "correct horse";

    fn temp_file(name: &str, bytes: &[u8]) -> (crate::util::TempDir, PathBuf) {
        let dir = crate::util::TempDir::new().unwrap();
        let path = dir.path().join(name);
        std::fs::write(&path, bytes).unwrap();
        (dir, path)
    }

    fn zip_bytes(kind: &str) -> Vec<u8> {
        // The old scheme cannot be written from here: made with `zip -P`, one file encrypted.
        if kind == "crypto" {
            return include_bytes!("../../assets/testdata/zipcrypto.zip").to_vec();
        }
        let mut z = ZipWriter::new(Cursor::new(Vec::new()));
        let plain = SimpleFileOptions::default();
        let locked = if kind == "aes" { plain.with_aes_encryption(AesMode::Aes256, PASSWORD) } else { plain };
        z.start_file("secret.txt", locked).unwrap();
        z.write_all(b"top secret").unwrap();
        z.start_file("plain.txt", plain).unwrap();
        z.write_all(b"for everyone").unwrap();
        z.finish().unwrap().into_inner()
    }

    #[test]
    fn plain_archives_are_open() {
        let (_d, p) = temp_file("plain.zip", &zip_bytes("none"));
        assert_eq!(lock_state(&p), Lock::Open);
        assert_eq!(read_entry(&p, "secret.txt", 100).unwrap(), b"top secret");
        assert!(check_password(&p, "anything"), "nothing to prove");
    }

    #[test]
    fn zip_needs_the_password_only_for_protected_entries() {
        for kind in ["aes", "crypto"] {
            let (_d, p) = temp_file("a.zip", &zip_bytes(kind));
            assert_eq!(lock_state(&p), Lock::Contents, "{kind}");
            assert_eq!(list(&p).unwrap().len(), 2, "the list is readable");
            assert!(is_password_error(&read_entry(&p, "secret.txt", 100).unwrap_err()));
            assert_eq!(read_entry(&p, "plain.txt", 100).unwrap(), b"for everyone");
            assert!(entry_is_locked(&p, "secret.txt"));
            assert!(!entry_is_locked(&p, "plain.txt"));
            // Asking for only the plain file works; everything asks for the password, leaving nothing behind.
            let out = crate::util::TempDir::new().unwrap();
            extract(&p, out.path(), &["plain.txt".to_string()], "", Conflict::Overwrite, &Progress::default()).unwrap();
            assert!(out.path().join("plain.txt").exists());
            assert!(!check_password(&p, "wrong"), "{kind}");
            assert!(check_password(&p, PASSWORD), "{kind}");
            remember_password(&p, PASSWORD);
            assert_eq!(lock_state(&p), Lock::Open);
            assert!(!entry_is_locked(&p, "secret.txt"));
            assert_eq!(read_entry(&p, "secret.txt", 100).unwrap(), b"top secret");
            assert_eq!(read_entry(&p, "plain.txt", 100).unwrap(), b"for everyone");
            let out = crate::util::TempDir::new().unwrap();
            extract(&p, out.path(), &[], "", Conflict::Overwrite, &Progress::default()).unwrap();
            assert_eq!(std::fs::read(out.path().join("secret.txt")).unwrap(), b"top secret");
            assert_eq!(std::fs::read(out.path().join("plain.txt")).unwrap(), b"for everyone");
        }
    }

    #[test]
    fn a_wrong_remembered_password_says_so() {
        let (_d, p) = temp_file("a.zip", &zip_bytes("aes"));
        remember_password(&p, "wrong");
        assert_eq!(read_entry(&p, "secret.txt", 100).unwrap_err(), wrong_password());
    }

    #[test]
    fn a_changed_aes_entry_is_an_error_even_when_only_its_start_is_read() {
        let mut bytes = zip_bytes("aes");
        let end = {
            let mut z = zip::ZipArchive::new(Cursor::new(&bytes)).unwrap();
            let f = z.by_name_raw(b"secret.txt").unwrap();
            f.data_start().unwrap() + f.compressed_size()
        };
        // The last byte of the authentication code: the text itself still decrypts fine.
        bytes[end as usize - 1] ^= 0x55;
        let (_d, p) = temp_file("a.zip", &bytes);
        remember_password(&p, PASSWORD);
        assert_eq!(read_entry(&p, "secret.txt", 3).unwrap_err(), wrong_password());
        assert_eq!(read_entry(&p, "secret.txt", 100).unwrap_err(), wrong_password());
        assert_eq!(read_entry(&p, "plain.txt", 100).unwrap(), b"for everyone");
        assert!(!check_password(&p, PASSWORD));
    }

    #[test]
    fn rar_files_and_file_lists() {
        // Real archives, from the test data of the unrar crate. Their passwords: "unrar" and "password".
        for (bytes, password, lock) in [
            (&include_bytes!("../../assets/testdata/rar-crypted.rar")[..], "unrar", Lock::Contents),
            (&include_bytes!("../../assets/testdata/rar-encrypted-headers.rar")[..], "password", Lock::Everything),
        ] {
            let (_d, p) = temp_file("a.rar", bytes);
            assert_eq!(lock_state(&p), lock);
            if lock == Lock::Everything {
                assert!(is_password_error(&list(&p).unwrap_err()));
            }
            assert!(!check_password(&p, "wrong"), "{password}");
            assert!(check_password(&p, password), "{password}");
            remember_password(&p, password);
            let entries = list(&p).unwrap();
            let file = entries.iter().find(|e| !e.is_dir).expect("a file");
            let contents = read_entry(&p, &file.path, 1000).unwrap();
            assert_eq!(contents.len() as u64, file.size.min(1000));
            let out = crate::util::TempDir::new().unwrap();
            extract(&p, out.path(), &[], "", Conflict::Overwrite, &Progress::default()).unwrap();
            assert!(out.path().join(&file.path).is_file());
            assert_eq!(lock_state(&p), Lock::Open);
        }
    }

    fn sevenz_bytes(encrypt_headers: bool) -> Vec<u8> {
        use sevenz_rust2::encoder_options::AesEncoderOptions;
        let mut out = Cursor::new(Vec::new());
        {
            let mut w = sevenz_rust2::ArchiveWriter::new(&mut out).unwrap();
            let password = sevenz_rust2::Password::new(PASSWORD);
            w.set_content_methods(vec![AesEncoderOptions::new(password).into(), sevenz_rust2::EncoderMethod::LZMA2.into()]);
            w.set_encrypt_header(encrypt_headers);
            let entry = sevenz_rust2::ArchiveEntry::new_file("secret.txt");
            w.push_archive_entry(entry, Some(Cursor::new(b"top secret".to_vec()))).unwrap();
            w.finish().unwrap();
        }
        out.into_inner()
    }

    /// A wrong password that ends the data early, which sevenz-rust2 does not see as an
    /// error (about 1 in 200 passwords). Made with `sevenz_bytes(false)`; "wrong-91" is such
    /// a password for it.
    #[test]
    fn sevenz_wrong_password_that_cuts_the_data_short() {
        let bytes = include_bytes!("../../assets/testdata/7z-short-with-wrong-password.7z");
        let (_d, p) = temp_file("a.7z", bytes);
        // The library on its own reads nothing, without an error.
        let mut r = sevenz_rust2::ArchiveReader::open(&p, sevenz_rust2::Password::new("wrong-91")).unwrap();
        assert_eq!(r.read_file("secret.txt").unwrap(), b"");
        assert!(!check_password(&p, "wrong-91"));
        assert!(check_password(&p, PASSWORD));
        remember_password(&p, "wrong-91");
        assert_eq!(read_entry(&p, "secret.txt", 100).unwrap_err(), wrong_password());
        let out = crate::util::TempDir::new().unwrap();
        let e = extract(&p, out.path(), &[], "", Conflict::Overwrite, &Progress::default()).unwrap_err();
        assert!(e.contains(&wrong_password()), "{e}");
        assert!(!out.path().join("secret.txt").exists(), "no empty file left behind");
        remember_password(&p, PASSWORD);
        assert_eq!(read_entry(&p, "secret.txt", 100).unwrap(), b"top secret");
        let out = crate::util::TempDir::new().unwrap();
        extract(&p, out.path(), &[], "", Conflict::Overwrite, &Progress::default()).unwrap();
        assert_eq!(std::fs::read(out.path().join("secret.txt")).unwrap(), b"top secret");
    }

    #[test]
    fn sevenz_with_and_without_encrypted_file_lists() {
        for (headers, lock) in [(false, Lock::Contents), (true, Lock::Everything)] {
            let (_d, p) = temp_file("a.7z", &sevenz_bytes(headers));
            assert_eq!(lock_state(&p), lock);
            assert!(is_password_error(&read_entry(&p, "secret.txt", 100).unwrap_err()));
            assert_eq!(list(&p).is_ok(), !headers);
            assert!(!check_password(&p, "wrong"));
            assert!(check_password(&p, PASSWORD));
            remember_password(&p, PASSWORD);
            assert_eq!(list(&p).unwrap()[0].path, "secret.txt");
            assert_eq!(read_entry(&p, "secret.txt", 100).unwrap(), b"top secret");
        }
    }
}
