//! Small helpers shared across the application.
use std::cmp::Ordering;
use std::fmt::Display;
use std::time::SystemTime;

pub type Res<T> = Result<T, String>;

/// Convert any displayable error to a `String` (used with `map_err`).
pub fn es<E: Display>(e: E) -> String {
    e.to_string()
}

/// Human readable size, e.g. `12.3 MB`.
pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if v >= 100.0 {
        format!("{v:.0} {}", UNITS[i])
    } else {
        format!("{v:.1} {}", UNITS[i])
    }
}

pub fn format_time(t: Option<SystemTime>) -> String {
    match t {
        Some(t) => {
            let dt: chrono::DateTime<chrono::Local> = t.into();
            dt.format("%Y-%m-%d %H:%M").to_string()
        }
        None => String::new(),
    }
}

/// Join two components of a remote / archive path that always uses `/`.
pub fn join_slash(base: &str, name: &str) -> String {
    if base.is_empty() {
        name.to_string()
    } else if base.ends_with('/') {
        format!("{base}{name}")
    } else {
        format!("{base}/{name}")
    }
}

/// Parent of a `/`-separated path. Returns `None` at the root.
pub fn parent_slash(path: &str) -> Option<String> {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    match trimmed.rfind('/') {
        Some(0) => Some("/".to_string()),
        Some(i) => Some(trimmed[..i].to_string()),
        None => Some(String::new()),
    }
}

/// Split a command line into arguments. Supports single and double quotes.
pub fn shell_split(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    let mut quote: Option<char> = None;
    for c in s.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => cur.push(c),
            None => match c {
                '"' | '\'' => {
                    quote = Some(c);
                    in_word = true;
                }
                c if c.is_whitespace() => {
                    if in_word {
                        out.push(std::mem::take(&mut cur));
                        in_word = false;
                    }
                }
                c => {
                    cur.push(c);
                    in_word = true;
                }
            },
        }
    }
    if in_word {
        out.push(cur);
    }
    out
}

/// Lower-cased extension of a file name (without dot). Empty for none.
pub fn ext_of(name: &str) -> String {
    match name.rfind('.') {
        Some(i) if i > 0 && i + 1 < name.len() => name[i + 1..].to_lowercase(),
        _ => String::new(),
    }
}

/// `path` with the home folder written as `~` (the home folder itself is `~/`).
pub fn tilde_path(path: &std::path::Path, home: Option<&std::path::Path>) -> String {
    if let Some(h) = home.filter(|h| !h.as_os_str().is_empty()) {
        if path == h {
            return "~/".to_string();
        }
        if let Ok(rest) = path.strip_prefix(h) {
            return format!("~/{}", rest.to_string_lossy().replace('\\', "/"));
        }
    }
    path.to_string_lossy().to_string()
}

/// Shorter forms of a `tilde_path`, longest first: the whole path, then with
/// the middle folders replaced by "…" (`~/…/parent/leaf`, `~/…/leaf`).
pub fn short_path_candidates(full: &str) -> Vec<String> {
    let mut out = vec![full.to_string()];
    let (root, rest) = if let Some(r) = full.strip_prefix("~/") {
        ("~/", r)
    } else if let Some(r) = full.strip_prefix('/') {
        ("/", r)
    } else {
        ("", full)
    };
    let parts: Vec<&str> = rest.split(['/', '\\']).filter(|p| !p.is_empty()).collect();
    for keep in [2usize, 1] {
        if parts.len() > keep {
            out.push(format!("{root}…/{}", parts[parts.len() - keep..].join("/")));
        }
    }
    out
}

/// Write `text` to `path` so only the user can read it (see `write_atomic`).
pub fn write_private(path: &std::path::Path, text: &str) -> std::io::Result<()> {
    write_atomic(path, text.as_bytes(), true)
}

/// Replace `path` with `bytes` so that a crash or power loss at any moment leaves either
/// the old file or the new one, never half of one: a new temporary file in the same
/// folder is written and flushed to the disk, then renamed over `path`. `private`: only
/// the user may read it (mode 0600); otherwise an existing file keeps its permissions.
pub fn write_atomic(path: &std::path::Path, bytes: &[u8], private: bool) -> std::io::Result<()> {
    use std::io::Write;
    let dir = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(std::path::Path::new("."));
    std::fs::create_dir_all(dir)?;
    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let old_perms = if private { None } else { std::fs::metadata(path).ok().map(|m| m.permissions()) };
    let mut attempt = 0;
    let (tmp, mut f) = loop {
        let tmp = dir.join(format!(".{name}.tmp-{}-{}", std::process::id(), random_hex(4)));
        let mut o = std::fs::OpenOptions::new();
        o.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut o, if private { 0o600 } else { 0o666 });
        match o.open(&tmp) {
            Ok(f) => break (tmp, f),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && attempt < 8 => attempt += 1,
            Err(e) => return Err(e),
        }
    };
    let mut written = f.write_all(bytes);
    if let (Ok(()), Some(p)) = (&written, old_perms) {
        written = f.set_permissions(p);
    }
    if written.is_ok() {
        written = sync_file(&f);
    }
    drop(f);
    if written.is_ok() {
        written = std::fs::rename(&tmp, path);
    }
    if let Err(e) = written {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    sync_dir(dir);
    Ok(())
}

/// Flush a file's data to the disk itself (on macOS `fsync` only reaches the drive's cache).
pub fn sync_file(f: &std::fs::File) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::io::AsRawFd;
        if unsafe { libc::fcntl(f.as_raw_fd(), libc::F_FULLFSYNC) } != -1 {
            return Ok(());
        }
    }
    f.sync_all()
}

/// Flush a folder, so that a file renamed into it stays there after a power loss.
pub fn sync_dir(dir: &std::path::Path) {
    #[cfg(unix)]
    if let Ok(d) = std::fs::File::open(dir) {
        let _ = sync_file(&d);
    }
    #[cfg(not(unix))]
    let _ = dir;
}

/// `n` random bytes as hex, from the operating system's random source.
pub fn random_hex(n: usize) -> String {
    use sha2::{Digest, Sha256};
    use std::hash::{BuildHasher, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let mut out = String::with_capacity(n * 2);
    while out.len() < n * 2 {
        let mut h = Sha256::new();
        #[cfg(unix)]
        {
            use std::io::Read;
            let mut seed = [0u8; 32];
            if std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut seed)).is_ok() {
                h.update(seed);
            }
        }
        // `RandomState` is seeded from the OS too (and is all there is on Windows).
        for i in 0..4u8 {
            let mut s = std::collections::hash_map::RandomState::new().build_hasher();
            s.write_u8(i);
            h.update(s.finish().to_le_bytes());
        }
        let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        h.update(now.to_le_bytes());
        h.update(std::process::id().to_le_bytes());
        h.update(NEXT.fetch_add(1, Ordering::Relaxed).to_le_bytes());
        for b in h.finalize() {
            out.push_str(&format!("{b:02x}"));
        }
    }
    out.truncate(n * 2);
    out
}

/// The SHA-256 of `bytes`.
pub fn sha256(bytes: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes).into()
}

/// Compare without stopping at the first difference.
pub fn same_bytes(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Whether process `pid` is running (true when that cannot be told).
pub fn pid_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        let Ok(pid) = libc::pid_t::try_from(pid) else { return false };
        if pid <= 0 {
            return false;
        }
        if unsafe { libc::kill(pid, 0) } == 0 {
            return true;
        }
        std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        true
    }
}

// ---------------------------------------------------------------- private temporary folders

/// Where this user's temporary files go: a folder only the user can open, so that on a
/// shared machine nobody else can read them or plant a file or link for them to write
/// through. Linux: `$XDG_RUNTIME_DIR/matrix_finder`, else in the user's cache folder.
fn private_base_candidates() -> Vec<std::path::PathBuf> {
    let mut v = Vec::new();
    #[cfg(all(unix, not(target_os = "macos")))]
    if let Some(r) = std::env::var_os("XDG_RUNTIME_DIR").map(std::path::PathBuf::from).filter(|p| p.is_absolute() && p.is_dir()) {
        v.push(r.join("matrix_finder"));
    }
    // macOS and Windows: the temporary folder is the user's own.
    #[cfg(not(all(unix, not(target_os = "macos"))))]
    v.push(std::env::temp_dir().join("matrix_finder"));
    if let Some(c) = dirs::cache_dir() {
        v.push(c.join("matrix_finder").join("tmp"));
    }
    v
}

/// Make `dir` (if needed) and check it is a real folder of this user that nobody else may
/// open; one of the user's own with wider permissions is narrowed.
pub fn ensure_private_dir(dir: &std::path::Path) -> std::io::Result<()> {
    if let Some(parent) = dir.parent() {
        std::fs::create_dir_all(parent)?;
    }
    #[cfg_attr(not(unix), allow(unused_mut))]
    let mut b = std::fs::DirBuilder::new();
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut b, 0o700);
    match b.create(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e),
    }
    // Not followed: a link here could point anywhere.
    let m = std::fs::symlink_metadata(dir)?;
    if !m.file_type().is_dir() {
        return Err(std::io::Error::other(format!("{} is not a folder", dir.display())));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        if m.uid() != unsafe { libc::geteuid() } {
            return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, format!("{} belongs to another user", dir.display())));
        }
        if m.mode() & 0o077 != 0 {
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        }
    }
    Ok(())
}

/// The private folder for temporary files (see `private_base_candidates`).
pub fn private_base() -> std::io::Result<std::path::PathBuf> {
    let mut last = std::io::Error::other("no folder for temporary files");
    for dir in private_base_candidates() {
        match ensure_private_dir(&dir) {
            Ok(()) => return Ok(dir),
            Err(e) => last = e,
        }
    }
    Err(last)
}

/// A new, empty folder for temporary files that only the user can open, with a name
/// nobody can guess: `<prefix>-<pid>-<random>` in `private_base`.
pub fn private_temp_dir(prefix: &str) -> std::io::Result<std::path::PathBuf> {
    let base = private_base()?;
    for _ in 0..16 {
        let dir = base.join(format!("{prefix}-{}-{}", std::process::id(), random_hex(8)));
        #[cfg_attr(not(unix), allow(unused_mut))]
        let mut b = std::fs::DirBuilder::new();
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut b, 0o700);
        // `create`, not `create_dir_all`: a folder that is already there is not ours.
        match b.create(&dir) {
            Ok(()) => return Ok(dir),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, "no free name for a temporary folder"))
}

/// Remove the temporary folders named `<prefix>-<pid>-…` (`prefix` one of `prefixes`)
/// left by Matrix Finders that are no longer running (after a crash).
pub fn clean_stale_temp_dirs(prefixes: &[&str]) {
    if cfg!(not(unix)) {
        return;
    }
    let Ok(base) = private_base() else { return };
    let Ok(rd) = std::fs::read_dir(&base) else { return };
    let me = std::process::id();
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let Some((prefix, rest)) = name.split_once('-') else { continue };
        let Some(pid) = rest.split('-').next().and_then(|p| p.parse::<u32>().ok()) else { continue };
        if !prefixes.contains(&prefix) || pid == me || pid_alive(pid) {
            continue;
        }
        if e.file_type().is_ok_and(|t| t.is_dir()) {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

// ---------------------------------------------------------------- reading files safely

/// What a path is when it is not a regular file or folder (after following links):
/// "Named pipe", "Socket", "Device" or "Special file" (English, translated by the caller).
pub fn special_kind(path: &std::path::Path) -> Option<&'static str> {
    let m = std::fs::metadata(path).ok()?;
    let ft = m.file_type();
    if ft.is_file() || ft.is_dir() {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        if ft.is_fifo() {
            return Some("Named pipe");
        }
        if ft.is_socket() {
            return Some("Socket");
        }
        if ft.is_block_device() || ft.is_char_device() {
            return Some("Device");
        }
    }
    Some("Special file")
}

/// Open `path` for reading only if it is a regular file (after following links). A named
/// pipe, socket or device is refused without blocking or taking data meant for another
/// program: opening one could wait forever for a writer, or read what is not ours.
pub fn open_regular(path: &std::path::Path) -> std::io::Result<std::fs::File> {
    let not_file = || std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("{}: not a regular file", path.display()));
    if !std::fs::metadata(path)?.is_file() {
        return Err(not_file());
    }
    let mut o = std::fs::OpenOptions::new();
    o.read(true);
    // Checked again on what was opened: it could have been swapped meanwhile.
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::custom_flags(&mut o, libc::O_NONBLOCK | libc::O_NOCTTY);
    let f = o.open(path)?;
    if !f.metadata()?.is_file() {
        return Err(not_file());
    }
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        let fd = f.as_raw_fd();
        unsafe {
            let flags = libc::fcntl(fd, libc::F_GETFL);
            if flags != -1 {
                libc::fcntl(fd, libc::F_SETFL, flags & !libc::O_NONBLOCK);
            }
        }
    }
    Ok(f)
}

/// `std::fs::read` for regular files only (see `open_regular`).
pub fn read_regular(path: &std::path::Path) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let mut f = open_regular(path)?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)?;
    Ok(buf)
}

/// `std::fs::read_to_string` for regular files only (see `open_regular`).
pub fn read_regular_string(path: &std::path::Path) -> std::io::Result<String> {
    String::from_utf8(read_regular(path)?).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

/// Compare strings treating digit runs as numbers (file2 < file10).
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    // Called for every pair a sort compares (hundreds of thousands in a big folder):
    // no allocation, and ASCII letters compared without Unicode tables.
    let (mut i, mut j) = (0usize, 0usize);
    loop {
        match (a[i..].chars().next(), b[j..].chars().next()) {
            (None, None) => return a.cmp(b),
            (None, _) => return Ordering::Less,
            (_, None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                // Runs of digits compare as numbers: by length without leading zeros, then digit by digit.
                let ie = i + a[i..].bytes().take_while(u8::is_ascii_digit).count();
                let je = j + b[j..].bytes().take_while(u8::is_ascii_digit).count();
                let (ta, tb) = (a[i..ie].trim_start_matches('0'), b[j..je].trim_start_matches('0'));
                let o = ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb));
                if o != Ordering::Equal {
                    return o;
                }
                (i, j) = (ie, je);
            }
            (Some(x), Some(y)) => {
                let o = if x.is_ascii() && y.is_ascii() { x.to_ascii_lowercase().cmp(&y.to_ascii_lowercase()) } else { x.to_lowercase().cmp(y.to_lowercase()) };
                if o != Ordering::Equal {
                    return o;
                }
                (i, j) = (i + x.len_utf8(), j + y.len_utf8());
            }
        }
    }
}

/// A program on PATH, or where package managers put it (apps started from the desktop
/// on macOS do not get the shell's PATH).
pub fn find_tool(name: &str) -> Option<std::path::PathBuf> {
    let on_path = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect::<Vec<_>>()).unwrap_or_default();
    let extra = ["/opt/homebrew/bin", "/usr/local/bin", "/opt/local/bin", "/usr/bin"].map(std::path::PathBuf::from);
    let names = if cfg!(windows) { vec![format!("{name}.exe"), name.to_string()] } else { vec![name.to_string()] };
    on_path.iter().chain(extra.iter()).find_map(|d| names.iter().map(|n| d.join(n)).find(|p| p.is_file()))
}

/// A private temporary folder, removed with everything in it when dropped. It is new,
/// only the user can open it and its name cannot be guessed, so files written in it
/// cannot be redirected by another user (see `private_temp_dir`).
pub struct TempDir(std::path::PathBuf);

/// The name the preview's temporary folders start with.
pub const TEMP_PREFIX: &str = "preview";

impl TempDir {
    pub fn new() -> Res<Self> {
        private_temp_dir(TEMP_PREFIX).map(Self).map_err(|e| e.to_string())
    }

    pub fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
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

/// A count with thousands separators: `1234567` gives `1,234,567`.
pub fn fmt_count(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// A glob set from the non-empty patterns, or `None` when there are none.
pub fn build_globset(patterns: &[String]) -> Res<Option<globset::GlobSet>> {
    let pats: Vec<&String> = patterns.iter().filter(|p| !p.trim().is_empty()).collect();
    if pats.is_empty() {
        return Ok(None);
    }
    let mut b = globset::GlobSetBuilder::new();
    for p in pats {
        b.add(globset::Glob::new(p.trim()).map_err(|e| format!("Bad pattern '{p}': {e}"))?);
    }
    b.build().map(Some).map_err(es)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn private_files_are_for_the_user_alone() {
        use std::os::unix::fs::PermissionsExt;
        let p = std::env::temp_dir().join(format!("matrix_finder-private-{}", std::process::id()));
        std::fs::write(&p, "old, readable by all").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
        write_private(&p, "secret").unwrap();
        assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "secret");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn atomic_writes_keep_permissions_and_leave_no_temp_files() {
        let dir = std::env::temp_dir().join(format!("matrix_finder-atomic-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("a.json");
        write_atomic(&p, b"one", false).unwrap();
        write_atomic(&p, b"two", false).unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"two");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o640)).unwrap();
            write_atomic(&p, b"three", false).unwrap();
            assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o640);
        }
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1, "no temporary file is left");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn random_names() {
        let a = random_hex(16);
        assert_eq!(a.len(), 32);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, random_hex(16));
        assert!(same_bytes(b"abc", b"abc") && !same_bytes(b"abc", b"abd") && !same_bytes(b"abc", b"ab"));
        assert!(pid_alive(std::process::id()));
    }

    #[cfg(unix)]
    #[test]
    fn private_temp_dirs() {
        use std::os::unix::fs::PermissionsExt;
        let a = private_temp_dir("test").unwrap();
        let b = private_temp_dir("test").unwrap();
        assert_ne!(a, b);
        for d in [&a, &b] {
            assert_eq!(std::fs::metadata(d).unwrap().permissions().mode() & 0o777, 0o700);
            assert_eq!(std::fs::metadata(d.parent().unwrap()).unwrap().permissions().mode() & 0o077, 0);
        }
        // A link in the place of the base folder is refused, not followed.
        let root = std::env::temp_dir().join(format!("matrix_finder-privbase-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("elsewhere")).unwrap();
        std::os::unix::fs::symlink(root.join("elsewhere"), root.join("base")).unwrap();
        assert!(ensure_private_dir(&root.join("base")).is_err());
        // One of ours that others could open is narrowed.
        std::fs::create_dir(root.join("open")).unwrap();
        std::fs::set_permissions(root.join("open"), std::fs::Permissions::from_mode(0o777)).unwrap();
        ensure_private_dir(&root.join("open")).unwrap();
        assert_eq!(std::fs::metadata(root.join("open")).unwrap().permissions().mode() & 0o777, 0o700);
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&a);
        let _ = std::fs::remove_dir_all(&b);
    }

    #[cfg(unix)]
    #[test]
    fn special_files_are_not_opened() {
        let dir = private_temp_dir("test").unwrap();
        let fifo = dir.join("pipe");
        let c = std::ffi::CString::new(fifo.to_string_lossy().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        // A plain open would wait forever for a writer.
        assert!(open_regular(&fifo).is_err());
        assert_eq!(special_kind(&fifo), Some("Named pipe"));
        assert_eq!(special_kind(std::path::Path::new("/dev/null")), Some("Device"));
        assert!(open_regular(std::path::Path::new("/dev/null")).is_err());
        std::fs::write(dir.join("f"), b"data").unwrap();
        assert_eq!(read_regular(&dir.join("f")).unwrap(), b"data");
        assert_eq!(special_kind(&dir.join("f")), None);
        // A link to a pipe is a pipe.
        std::os::unix::fs::symlink(&fifo, dir.join("link")).unwrap();
        assert!(open_regular(&dir.join("link")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sizes() {
        assert_eq!(human_size(10), "10 B");
        assert_eq!(human_size(1536), "1.5 KB");
        assert_eq!(human_size(1024 * 1024 * 200), "200 MB");
    }

    #[test]
    fn slash_paths() {
        assert_eq!(join_slash("/a", "b"), "/a/b");
        assert_eq!(join_slash("", "b"), "b");
        assert_eq!(parent_slash("/a/b"), Some("/a".into()));
        assert_eq!(parent_slash("/a"), Some("/".into()));
        assert_eq!(parent_slash("/"), None);
        assert_eq!(parent_slash("a"), Some(String::new()));
        assert_eq!(parent_slash(""), None);
    }

    #[test]
    fn split() {
        assert_eq!(shell_split(r#"open -a "My App" {dir}"#), vec!["open", "-a", "My App", "{dir}"]);
    }

    #[test]
    fn ext() {
        assert_eq!(ext_of("a.RS"), "rs");
        assert_eq!(ext_of(".bashrc"), "");
        assert_eq!(ext_of("noext"), "");
    }

    #[test]
    fn tilde_and_short_paths() {
        use std::path::Path;
        let home = Path::new("/home/me");
        assert_eq!(tilde_path(home, Some(home)), "~/");
        assert_eq!(tilde_path(Path::new("/home/me/Projects/app"), Some(home)), "~/Projects/app");
        assert_eq!(tilde_path(Path::new("/home/meow"), Some(home)), "/home/meow", "only whole folder names");
        assert_eq!(tilde_path(Path::new("/etc"), Some(home)), "/etc");
        assert_eq!(short_path_candidates("~/a/b/c/d"), ["~/a/b/c/d", "~/…/c/d", "~/…/d"]);
        assert_eq!(short_path_candidates("/usr/local/bin"), ["/usr/local/bin", "/…/local/bin", "/…/bin"]);
        assert_eq!(short_path_candidates("~/"), ["~/"]);
    }

    /// The comparison as it was first written (allocating), to check the fast one by.
    fn natural_cmp_before(a: &str, b: &str) -> Ordering {
        let (mut ai, mut bi) = (a.chars().peekable(), b.chars().peekable());
        loop {
            match (ai.peek().copied(), bi.peek().copied()) {
                (None, None) => return a.cmp(b),
                (None, _) => return Ordering::Less,
                (_, None) => return Ordering::Greater,
                (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                    let mut na = String::new();
                    while let Some(c) = ai.peek().copied().filter(|c| c.is_ascii_digit()) {
                        na.push(c);
                        ai.next();
                    }
                    let mut nb = String::new();
                    while let Some(c) = bi.peek().copied().filter(|c| c.is_ascii_digit()) {
                        nb.push(c);
                        bi.next();
                    }
                    let (ta, tb) = (na.trim_start_matches('0'), nb.trim_start_matches('0'));
                    let o = ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb));
                    if o != Ordering::Equal {
                        return o;
                    }
                }
                (Some(x), Some(y)) => {
                    let o = x.to_lowercase().cmp(y.to_lowercase());
                    if o != Ordering::Equal {
                        return o;
                    }
                    ai.next();
                    bi.next();
                }
            }
        }
    }

    /// The fast comparison orders exactly as the first one did.
    #[test]
    fn natural_cmp_is_unchanged() {
        let words = [
            "",
            "a",
            "A",
            "b",
            "a1",
            "a01",
            "a001",
            "a1b",
            "a10",
            "a2",
            "a02b",
            "file10",
            "file2",
            "File1",
            "file-2",
            "file_2",
            "0",
            "00",
            "007",
            "7",
            "x9y10",
            "x9y9",
            "x09y10",
            "é",
            "E",
            "e",
            "Éa",
            "ä1",
            "a\u{0301}",
            "ß",
            "SS",
            "İ",
            "i",
            "Ω2",
            "ω10",
            "日本1",
            "日本01",
            "12a",
            "12",
            "1.2.10",
            "1.2.9",
            "v1.10.0",
            "v1.9.0",
            "a b",
            "a-b",
            "a_b",
            "Z",
            "z",
            "~",
            "_",
            "-1",
            "99999999999999999999",
            "100000000000000000000",
        ];
        for a in words {
            for b in words {
                assert_eq!(natural_cmp(a, b), natural_cmp_before(a, b), "{a:?} vs {b:?}");
            }
        }
    }

    #[test]
    fn natural() {
        let mut v = vec!["file10", "file2", "File1", "a"];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(v, vec!["a", "File1", "file2", "file10"]);
    }

    #[test]
    fn durations() {
        assert_eq!(format_duration(83.0), "1:23");
        assert_eq!(format_duration(3723.5), "1:02:04");
        assert_eq!(format_duration(0.2), "0:00");
    }

    #[test]
    fn counts() {
        assert_eq!(fmt_count(1234567), "1,234,567");
        assert_eq!(fmt_count(999), "999");
    }
}
