//! Remote file systems: FTP (suppaftp) and SFTP (libssh2).
use crate::config::{ConnectionProfile, Protocol};
use crate::files::vfs::Entry;
use crate::i18n::t;
use crate::util::{es, join_slash, Res};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, TryLockError};
use std::time::Duration;

/// Called with the number of bytes transferred since the previous call.
pub type ProgressFn<'a> = &'a dyn Fn(u64);

pub trait RemoteFs: Send {
    fn home(&mut self) -> String;
    fn list(&mut self, path: &str) -> Res<Vec<Entry>>;
    fn download(&mut self, remote: &str, local: &Path, progress: ProgressFn, cancel: &AtomicBool) -> Res<()>;
    fn upload(&mut self, local: &Path, remote: &str, progress: ProgressFn, cancel: &AtomicBool) -> Res<()>;
    /// The first `max` bytes of a file; stops early once `cancel` is set.
    fn read_head(&mut self, remote: &str, max: usize, cancel: &AtomicBool) -> Res<Vec<u8>>;
    fn mkdir(&mut self, path: &str) -> Res<()>;
    fn remove_file(&mut self, path: &str) -> Res<()>;
    fn remove_dir(&mut self, path: &str) -> Res<()>;
    /// Rename; as far as the protocol allows, never over an existing `to` (SFTP refuses,
    /// but many FTP servers replace it: callers check the name first).
    fn rename(&mut self, from: &str, to: &str) -> Res<()>;
    /// Whether `path` itself is a symbolic link. Servers that list links as plain
    /// files (FTP) never report a link as a folder, so they need no check.
    fn is_symlink(&mut self, _path: &str) -> Res<bool> {
        Ok(false)
    }
    /// Whether anything is at `path` now.
    fn exists(&mut self, path: &str) -> Res<bool> {
        Ok(listed(self, path)?.is_some())
    }
    /// The size of the file at `path`, when the server tells it.
    fn file_size(&mut self, path: &str) -> Res<Option<u64>> {
        Ok(listed(self, path)?.filter(|e| !e.is_dir).map(|e| e.size))
    }
    /// Create an empty file, failing when the name is taken (an existing file is never emptied).
    fn create_file(&mut self, _path: &str) -> Res<()> {
        Err(t("Not supported by this connection"))
    }
    /// Give `to` the permissions of `from` (an overwritten file keeps them). Best effort.
    fn copy_mode(&mut self, _from: &str, _to: &str) {}
}

/// The entry for `path` in its folder's listing.
fn listed<F: RemoteFs + ?Sized>(fs: &mut F, path: &str) -> Res<Option<Entry>> {
    let (dir, name) = match path.trim_end_matches('/').rsplit_once('/') {
        Some(("", name)) => ("/", name),
        Some((dir, name)) => (dir, name),
        None => (".", path),
    };
    Ok(fs.list(dir)?.into_iter().find(|e| e.name == name))
}

pub type SharedRemote = Arc<Mutex<Box<dyn RemoteFs>>>;

/// Shown when a connection is in use by a transfer and the UI cannot wait for it.
pub fn busy_text() -> String {
    t("The connection is busy with a transfer; try again when it finishes")
}

/// The connection, if nothing else is using it. The UI thread uses this instead of
/// `lock()`: a transfer holds the connection for a whole file, and waiting for it
/// would freeze the window.
pub fn try_use(fs: &SharedRemote) -> Res<MutexGuard<'_, Box<dyn RemoteFs>>> {
    match fs.try_lock() {
        Ok(g) => Ok(g),
        Err(TryLockError::WouldBlock) => Err(busy_text()),
        Err(TryLockError::Poisoned(e)) => Err(e.to_string()),
    }
}

/// Like `try_use`, but waits up to `patience` for a short operation (a listing) to finish,
/// and gives up early once `cancel` is set. For background threads that must not wait for
/// a whole transfer.
pub fn use_soon<'a>(fs: &'a SharedRemote, patience: Duration, cancel: &dyn Fn() -> bool) -> Res<MutexGuard<'a, Box<dyn RemoteFs>>> {
    let start = std::time::Instant::now();
    loop {
        match fs.try_lock() {
            Ok(g) => return Ok(g),
            Err(TryLockError::Poisoned(e)) => return Err(e.to_string()),
            Err(TryLockError::WouldBlock) => {
                if cancel() {
                    return Err(t("Cancelled"));
                }
                if start.elapsed() >= patience {
                    return Err(busy_text());
                }
                std::thread::sleep(Duration::from_millis(40));
            }
        }
    }
}

/// Read up to `max` bytes, checking `cancel` between blocks.
fn read_limited(r: &mut dyn Read, max: usize, cancel: &AtomicBool) -> Res<Vec<u8>> {
    let mut buf = Vec::new();
    let mut block = vec![0u8; 64 * 1024];
    while buf.len() < max {
        if cancel.load(Ordering::Relaxed) {
            return Err(t("Cancelled"));
        }
        let want = block.len().min(max - buf.len());
        let n = match r.read(&mut block[..want]) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(es(e)),
        };
        buf.extend_from_slice(&block[..n]);
    }
    Ok(buf)
}

#[derive(Debug)]
pub enum ConnectError {
    /// The server's host key is in no known_hosts file. Nothing was sent to the server
    /// yet; connect again with this fingerprint once the user has accepted it.
    #[cfg_attr(not(feature = "sftp"), allow(dead_code))]
    UnknownHostKey {
        fingerprint: String,
    },
    Other(String),
}

impl From<String> for ConnectError {
    fn from(e: String) -> Self {
        ConnectError::Other(e)
    }
}

impl From<&str> for ConnectError {
    fn from(e: &str) -> Self {
        ConnectError::Other(e.to_string())
    }
}

/// Connect to a server. `trusted_key` is an SFTP host key fingerprint the user has accepted:
/// a server with that unknown key is remembered and connected to.
pub fn connect(p: &ConnectionProfile, trusted_key: Option<&str>) -> Result<Box<dyn RemoteFs>, ConnectError> {
    match p.protocol {
        Protocol::Ftp => Ok(Box::new(FtpFs::connect(p)?)),
        Protocol::Sftp => {
            #[cfg(feature = "sftp")]
            {
                Ok(Box::new(SftpFs::connect(p, trusted_key)?))
            }
            #[cfg(not(feature = "sftp"))]
            {
                let _ = trusted_key;
                Err("SFTP support was not compiled in (enable the `sftp` feature)".into())
            }
        }
    }
}

/// Recursively delete a remote path. A link to a folder is removed itself, never what it
/// points to.
pub fn remove_recursive(fs: &mut dyn RemoteFs, path: &str, is_dir: bool) -> Res<()> {
    let is_dir = is_dir && !fs.is_symlink(path)?;
    remove_tree(fs, path, is_dir)
}

fn remove_tree(fs: &mut dyn RemoteFs, path: &str, is_dir: bool) -> Res<()> {
    if is_dir {
        for e in fs.list(path)? {
            remove_tree(fs, &join_slash(path, &e.name), e.is_dir && !e.is_link)?;
        }
        fs.remove_dir(path)
    } else {
        fs.remove_file(path)
    }
}

fn copy_stream(r: &mut dyn Read, w: &mut dyn Write, progress: ProgressFn, cancel: &AtomicBool) -> Res<()> {
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(t("Cancelled"));
        }
        let n = r.read(&mut buf).map_err(es)?;
        if n == 0 {
            break;
        }
        w.write_all(&buf[..n]).map_err(es)?;
        progress(n as u64);
    }
    Ok(())
}

// ---------------------------------------------------------------- FTP

pub struct FtpFs {
    ftp: suppaftp::FtpStream,
}

impl FtpFs {
    pub fn connect(p: &ConnectionProfile) -> Res<Self> {
        use std::net::ToSocketAddrs;
        let addr = (p.host.as_str(), p.port).to_socket_addrs().map_err(es)?.next().ok_or("Cannot resolve host")?;
        let mut ftp = suppaftp::FtpStream::connect_timeout(addr, Duration::from_secs(15)).map_err(es)?;
        let user = if p.user.is_empty() { "anonymous" } else { p.user.as_str() };
        ftp.login(ftp_arg(user)?, ftp_arg(&p.password)?).map_err(es)?;
        ftp.transfer_type(suppaftp::types::FileType::Binary).map_err(es)?;
        if let Ok(path) = ftp_arg(&p.initial_path) {
            if !path.is_empty() {
                let _ = ftp.cwd(path);
            }
        }
        Ok(Self { ftp })
    }
}

/// A name or path for the FTP control channel. A line break there would end the command
/// and start another one, so such names are refused before anything is sent.
fn ftp_arg(s: &str) -> Res<&str> {
    if s.contains(['\r', '\n', '\0']) {
        return Err(t("This name cannot be used over FTP: it contains a line break or a null character."));
    }
    Ok(s)
}

fn parse_ftp_line(line: &str) -> Option<Entry> {
    use std::str::FromStr;
    let f = suppaftp::list::File::from_str(line).ok()?;
    let name = f.name().to_string();
    if name == "." || name == ".." {
        return None;
    }
    let mut e = Entry::new(name, f.is_directory(), f.size() as u64, Some(f.modified()));
    e.is_link = f.is_symlink();
    Some(e)
}

impl RemoteFs for FtpFs {
    fn home(&mut self) -> String {
        self.ftp.pwd().unwrap_or_else(|_| "/".into())
    }

    fn list(&mut self, path: &str) -> Res<Vec<Entry>> {
        let lines = self.ftp.list(Some(ftp_arg(path)?)).map_err(es)?;
        Ok(lines.iter().filter_map(|l| parse_ftp_line(l)).collect())
    }

    fn download(&mut self, remote: &str, local: &Path, progress: ProgressFn, cancel: &AtomicBool) -> Res<()> {
        let remote = ftp_arg(remote)?;
        let mut out = std::fs::File::create(local).map_err(es)?;
        let mut stream = self.ftp.retr_as_stream(remote).map_err(es)?;
        let res = copy_stream(&mut stream, &mut out, progress, cancel);
        let fin = stream.finish().map_err(es);
        res.and(fin)
    }

    /// STOR replaces a file of that name: callers upload to a fresh temporary name.
    fn upload(&mut self, local: &Path, remote: &str, progress: ProgressFn, cancel: &AtomicBool) -> Res<()> {
        let remote = ftp_arg(remote)?;
        let mut inp = std::fs::File::open(local).map_err(es)?;
        let mut stream = self.ftp.put_with_stream(remote).map_err(es)?;
        let res = copy_stream(&mut inp, &mut stream, progress, cancel);
        let fin = stream.finish().map_err(es);
        res.and(fin)
    }

    fn read_head(&mut self, remote: &str, max: usize, cancel: &AtomicBool) -> Res<Vec<u8>> {
        let mut stream = self.ftp.retr_as_stream(ftp_arg(remote)?).map_err(es)?;
        let r = read_limited(&mut stream, max, cancel);
        if r.as_ref().map_or(true, |buf| buf.len() >= max) {
            // Stop the transfer early; ignore errors from the aborted stream.
            let _ = self.ftp.abort(stream);
        } else {
            let _ = stream.finish();
        }
        r
    }

    fn mkdir(&mut self, path: &str) -> Res<()> {
        self.ftp.mkdir(ftp_arg(path)?).map_err(es)
    }
    fn remove_file(&mut self, path: &str) -> Res<()> {
        self.ftp.rm(ftp_arg(path)?).map_err(es)
    }
    fn remove_dir(&mut self, path: &str) -> Res<()> {
        self.ftp.rmdir(ftp_arg(path)?).map_err(es)
    }
    fn rename(&mut self, from: &str, to: &str) -> Res<()> {
        self.ftp.rename(ftp_arg(from)?, ftp_arg(to)?).map_err(es)
    }
    fn exists(&mut self, path: &str) -> Res<bool> {
        // SIZE also finds hidden files, which many servers leave out of listings.
        if self.ftp.size(ftp_arg(path)?).is_ok() {
            return Ok(true);
        }
        Ok(listed(self, path)?.is_some())
    }
    fn file_size(&mut self, path: &str) -> Res<Option<u64>> {
        match self.ftp.size(ftp_arg(path)?) {
            Ok(n) => Ok(Some(n as u64)),
            Err(_) => Ok(listed(self, path)?.filter(|e| !e.is_dir).map(|e| e.size)),
        }
    }
    fn create_file(&mut self, path: &str) -> Res<()> {
        if self.exists(path)? {
            return Err(format!("{}: {path}", t("Already exists")));
        }
        // APPE adds nothing to a file that appeared meanwhile, where STOR would empty it.
        self.ftp.append_file(ftp_arg(path)?, &mut std::io::empty()).map(|_| ()).map_err(es)
    }
}

// ---------------------------------------------------------------- SFTP

#[cfg(feature = "sftp")]
pub struct SftpFs {
    _session: ssh2::Session,
    sftp: ssh2::Sftp,
    home: String,
}

/// The app's own known_hosts file, which holds the keys the user accepted here.
/// `~/.ssh/known_hosts` is also trusted, but belongs to OpenSSH and is never written.
#[cfg(feature = "sftp")]
fn app_known_hosts() -> std::path::PathBuf {
    crate::config::config_dir().join("known_hosts")
}

/// Check the server's host key before any credentials are sent to it.
#[cfg(feature = "sftp")]
fn verify_host_key(sess: &ssh2::Session, host: &str, port: u16, trusted_key: Option<&str>) -> Result<(), ConnectError> {
    use base64::Engine;
    use ssh2::{CheckResult, HashType, KnownHostFileKind};
    let (key, key_type) = sess.host_key().ok_or("The server sent no host key")?;
    let hash = sess.host_key_hash(HashType::Sha256).ok_or("Cannot hash the server's host key")?;
    let fingerprint = format!("SHA256:{}", base64::engine::general_purpose::STANDARD_NO_PAD.encode(hash));
    let app_file = app_known_hosts();
    let mut known = sess.known_hosts().map_err(es)?;
    // A missing file, or a line libssh2 cannot parse, only makes a key unknown: the user is asked.
    if let Some(home) = dirs::home_dir() {
        let _ = known.read_file(&home.join(".ssh/known_hosts"), KnownHostFileKind::OpenSSH);
    }
    let _ = known.read_file(&app_file, KnownHostFileKind::OpenSSH);
    match known.check_port(host, port, key) {
        CheckResult::Match => Ok(()),
        CheckResult::Mismatch => Err(ConnectError::Other(format!(
            "The host key of {host} has CHANGED ({fingerprint}). Someone may be impersonating the server, so the connection was refused. If the change is expected, remove the old key from ~/.ssh/known_hosts or {}.",
            app_file.display()
        ))),
        CheckResult::Failure => Err("Could not check the server's host key".into()),
        CheckResult::NotFound if trusted_key == Some(fingerprint.as_str()) => {
            if let Some(dir) = app_file.parent() {
                std::fs::create_dir_all(dir).map_err(es)?;
            }
            // Only this app's own entries are read here, so only they are written back.
            let mut own = sess.known_hosts().map_err(es)?;
            if app_file.exists() {
                own.read_file(&app_file, KnownHostFileKind::OpenSSH).map_err(es)?;
            }
            let entry = if port == 22 { host.to_string() } else { format!("[{host}]:{port}") };
            own.add(&entry, key, "added by Matrix Finder", key_type.into()).map_err(es)?;
            own.write_file(&app_file, KnownHostFileKind::OpenSSH).map_err(es)?;
            Ok(())
        }
        CheckResult::NotFound => Err(ConnectError::UnknownHostKey { fingerprint }),
    }
}

#[cfg(feature = "sftp")]
impl SftpFs {
    pub fn connect(p: &ConnectionProfile, trusted_key: Option<&str>) -> Result<Self, ConnectError> {
        use std::net::{TcpStream, ToSocketAddrs};
        let addr = (p.host.as_str(), p.port).to_socket_addrs().map_err(es)?.next().ok_or("Cannot resolve host")?;
        let tcp = TcpStream::connect_timeout(&addr, Duration::from_secs(15)).map_err(es)?;
        let mut sess = ssh2::Session::new().map_err(es)?;
        sess.set_tcp_stream(tcp);
        sess.set_timeout(30_000);
        sess.handshake().map_err(es)?;
        verify_host_key(&sess, &p.host, p.port, trusted_key)?;
        let user = if p.user.is_empty() { whoami() } else { p.user.clone() };
        if !p.key_file.is_empty() {
            let pass = if p.password.is_empty() { None } else { Some(p.password.as_str()) };
            let key = match p.key_file.strip_prefix("~/") {
                Some(rest) => dirs::home_dir().map(|h| h.join(rest)).unwrap_or_else(|| p.key_file.clone().into()),
                None => std::path::PathBuf::from(&p.key_file),
            };
            sess.userauth_pubkey_file(&user, None, &key, pass).map_err(es)?;
        } else if !p.password.is_empty() {
            sess.userauth_password(&user, &p.password).map_err(es)?;
        } else {
            sess.userauth_agent(&user).map_err(es)?;
        }
        if !sess.authenticated() {
            return Err("Authentication failed".into());
        }
        let sftp = sess.sftp().map_err(es)?;
        let home = if p.initial_path.is_empty() {
            sftp.realpath(Path::new(".")).map(|p| p.to_string_lossy().to_string()).unwrap_or_else(|_| "/".into())
        } else {
            p.initial_path.clone()
        };
        Ok(Self { _session: sess, sftp, home })
    }
}

#[cfg(feature = "sftp")]
fn whoami() -> String {
    std::env::var("USER").or_else(|_| std::env::var("USERNAME")).unwrap_or_else(|_| "root".into())
}

#[cfg(feature = "sftp")]
impl RemoteFs for SftpFs {
    fn home(&mut self) -> String {
        self.home.clone()
    }

    fn list(&mut self, path: &str) -> Res<Vec<Entry>> {
        let items = self.sftp.readdir(Path::new(path)).map_err(es)?;
        Ok(items
            .into_iter()
            .filter_map(|(p, st)| {
                let name = p.file_name()?.to_string_lossy().to_string();
                if name == "." || name == ".." {
                    return None;
                }
                let mtime = st.mtime.map(|t| std::time::UNIX_EPOCH + Duration::from_secs(t));
                let is_link = st.file_type().is_symlink();
                let mut is_dir = st.is_dir();
                if is_link {
                    if let Ok(target) = self.sftp.stat(&p) {
                        is_dir = target.is_dir();
                    }
                }
                let mut e = Entry::new(name, is_dir, st.size.unwrap_or(0), mtime);
                e.is_link = is_link;
                Some(e)
            })
            .collect())
    }

    fn download(&mut self, remote: &str, local: &Path, progress: ProgressFn, cancel: &AtomicBool) -> Res<()> {
        let mut r = self.sftp.open(Path::new(remote)).map_err(es)?;
        let mut out = std::fs::File::create(local).map_err(es)?;
        let res = copy_stream(&mut r, &mut out, progress, cancel);
        let _ = r.close();
        res
    }

    /// Never replaces anything: the file is created new (callers use a temporary name).
    fn upload(&mut self, local: &Path, remote: &str, progress: ProgressFn, cancel: &AtomicBool) -> Res<()> {
        use ssh2::{OpenFlags, OpenType};
        let mut inp = std::fs::File::open(local).map_err(es)?;
        let mut w = self.sftp.open_mode(Path::new(remote), OpenFlags::WRITE | OpenFlags::EXCLUSIVE, 0o644, OpenType::File).map_err(es)?;
        let res = copy_stream(&mut inp, &mut w, progress, cancel).and_then(|_| w.flush().map_err(es));
        // Servers without fsync@openssh.com cannot sync; the size is checked afterwards anyway.
        if res.is_ok() {
            let _ = w.fsync();
        }
        // The close reports a failed write too, so it is checked.
        let closed = w.close().map_err(es);
        res.and(closed)
    }

    fn read_head(&mut self, remote: &str, max: usize, cancel: &AtomicBool) -> Res<Vec<u8>> {
        let mut r = self.sftp.open(Path::new(remote)).map_err(es)?;
        read_limited(&mut r, max, cancel)
    }

    fn mkdir(&mut self, path: &str) -> Res<()> {
        self.sftp.mkdir(Path::new(path), 0o755).map_err(es)
    }
    fn remove_file(&mut self, path: &str) -> Res<()> {
        self.sftp.unlink(Path::new(path)).map_err(es)
    }
    fn remove_dir(&mut self, path: &str) -> Res<()> {
        self.sftp.rmdir(Path::new(path)).map_err(es)
    }
    fn rename(&mut self, from: &str, to: &str) -> Res<()> {
        use ssh2::RenameFlags;
        // Without OVERWRITE: the server refuses a name that is taken.
        self.sftp.rename(Path::new(from), Path::new(to), Some(RenameFlags::ATOMIC | RenameFlags::NATIVE)).map_err(es)
    }
    fn is_symlink(&mut self, path: &str) -> Res<bool> {
        let st = self.sftp.lstat(Path::new(path)).map_err(es)?;
        Ok(st.file_type().is_symlink())
    }
    fn exists(&mut self, path: &str) -> Res<bool> {
        match self.sftp.lstat(Path::new(path)) {
            Ok(_) => Ok(true),
            // SSH_FX_NO_SUCH_FILE; anything else (no permission) may hide a file.
            Err(e) if e.code() == ssh2::ErrorCode::SFTP(2) => Ok(false),
            Err(e) => Err(es(e)),
        }
    }
    fn file_size(&mut self, path: &str) -> Res<Option<u64>> {
        Ok(self.sftp.stat(Path::new(path)).map_err(es)?.size)
    }
    fn create_file(&mut self, path: &str) -> Res<()> {
        use ssh2::{OpenFlags, OpenType};
        if self.exists(path)? {
            return Err(format!("{}: {path}", t("Already exists")));
        }
        let mut f = self.sftp.open_mode(Path::new(path), OpenFlags::WRITE | OpenFlags::EXCLUSIVE, 0o644, OpenType::File).map_err(es)?;
        f.close().map_err(es)
    }
    fn copy_mode(&mut self, from: &str, to: &str) {
        if let Ok(st) = self.sftp.stat(Path::new(from)) {
            if let Some(perm) = st.perm {
                let mode = ssh2::FileStat { size: None, uid: None, gid: None, perm: Some(perm & 0o777), atime: None, mtime: None };
                let _ = self.sftp.setstat(Path::new(to), mode);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ftp_unix_listing() {
        let e = parse_ftp_line("drwxr-xr-x    2 1000     1000         4096 Jan 01 12:00 docs").unwrap();
        assert!(e.is_dir);
        assert_eq!(e.name, "docs");
        let f = parse_ftp_line("-rw-r--r--    1 1000     1000       123456 Jan 01  2024 file.txt").unwrap();
        assert!(!f.is_dir);
        assert_eq!(f.size, 123456);
    }

    #[test]
    fn ftp_names_with_line_breaks_are_never_sent() {
        use std::io::{BufRead, BufReader};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (sock, _) = listener.accept().unwrap();
            let mut out = sock.try_clone().unwrap();
            out.write_all(b"220 ready\r\n").unwrap();
            let mut seen = vec![];
            for line in BufReader::new(sock).lines() {
                let line = line.unwrap();
                let reply: &[u8] = match line.split(' ').next().unwrap() {
                    "USER" => b"331 password\r\n",
                    "PASS" => b"230 in\r\n",
                    "TYPE" => b"200 ok\r\n",
                    "QUIT" => b"221 bye\r\n",
                    _ => b"250 ok\r\n",
                };
                out.write_all(reply).unwrap();
                seen.push(line);
                if seen.last().is_some_and(|l| l == "QUIT") {
                    break;
                }
            }
            seen
        });
        let p = ConnectionProfile { protocol: Protocol::Ftp, host: "127.0.0.1".into(), port, user: "me".into(), password: "pw".into(), ..Default::default() };
        let mut fs = FtpFs::connect(&p).unwrap();
        for bad in ["x\r\nDELE victim", "x\nDELE victim", "x\0y"] {
            assert!(fs.remove_file(bad).is_err());
            assert!(fs.mkdir(bad).is_err());
            assert!(fs.rename("a", bad).is_err());
            assert!(fs.list(bad).is_err());
        }
        fs.remove_file("fine.txt").unwrap();
        fs.ftp.quit().unwrap();
        let seen = server.join().unwrap();
        assert_eq!(seen, ["USER me", "PASS pw", "TYPE I", "DELE fine.txt", "QUIT"]);
    }
}

#[cfg(test)]
mod fake_ftp_tests {
    use super::*;
    use std::io::{BufRead, BufReader};

    /// A tiny FTP server holding files of the given sizes; returns its port and, once the
    /// client quits, every command it received.
    fn serve(files: &'static [(&'static str, u64)]) -> (u16, std::thread::JoinHandle<Vec<String>>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (sock, _) = listener.accept().unwrap();
            let mut out = sock.try_clone().unwrap();
            out.write_all(b"220 ready\r\n").unwrap();
            let mut seen = vec![];
            let mut data: Option<std::net::TcpListener> = None;
            for line in BufReader::new(sock).lines() {
                let line = line.unwrap();
                let (cmd, arg) = line.split_once(' ').unwrap_or((line.as_str(), ""));
                let reply = match cmd {
                    "USER" => "331 password".to_string(),
                    "PASS" => "230 in".into(),
                    "TYPE" => "200 ok".into(),
                    "QUIT" => "221 bye".into(),
                    "SIZE" => match files.iter().find(|(n, _)| *n == arg) {
                        Some((_, size)) => format!("213 {size}"),
                        None => "550 no such file".into(),
                    },
                    "PASV" => {
                        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
                        let p = l.local_addr().unwrap().port();
                        data = Some(l);
                        format!("227 Entering Passive Mode (127,0,0,1,{},{})", p >> 8, p & 255)
                    }
                    "LIST" | "APPE" | "STOR" => {
                        out.write_all(b"150 go\r\n").unwrap();
                        let (mut conn, _) = data.take().unwrap().accept().unwrap();
                        if cmd == "LIST" {
                            for (n, size) in files {
                                conn.write_all(format!("-rw-r--r--    1 1000     1000 {size:>12} Jan 01 12:00 {n}\r\n").as_bytes()).unwrap();
                            }
                        } else {
                            std::io::copy(&mut conn, &mut std::io::sink()).unwrap();
                        }
                        drop(conn);
                        "226 done".into()
                    }
                    _ => "250 ok".into(),
                };
                out.write_all(format!("{reply}\r\n").as_bytes()).unwrap();
                let quit = cmd == "QUIT";
                seen.push(line.clone());
                if quit {
                    break;
                }
            }
            seen
        });
        (port, server)
    }

    #[test]
    fn a_new_file_on_ftp_never_empties_an_existing_one() {
        let (port, server) = serve(&[("taken.txt", 5), (".hidden", 3)]);
        let p = ConnectionProfile { protocol: Protocol::Ftp, host: "127.0.0.1".into(), port, user: "me".into(), password: "pw".into(), ..Default::default() };
        let mut fs = FtpFs::connect(&p).unwrap();
        assert!(fs.create_file("taken.txt").unwrap_err().contains("Already exists"));
        assert!(fs.exists(".hidden").unwrap());
        assert_eq!(fs.file_size("taken.txt").unwrap(), Some(5));
        fs.create_file("new.txt").unwrap();
        fs.ftp.quit().unwrap();
        let seen = server.join().unwrap();
        assert!(!seen.iter().any(|l| l.starts_with("STOR")), "{seen:?}");
        assert!(!seen.iter().any(|l| l.contains("APPE taken.txt")), "{seen:?}");
        assert!(seen.iter().any(|l| l == "APPE new.txt"), "{seen:?}");
    }
}

/// Live round-trip tests against real servers. Run with e.g.
/// `MATRIX_FINDER_TEST_FTP=127.0.0.1:2121:user:pass cargo test live_ -- --ignored`
/// `MATRIX_FINDER_TEST_SFTP=127.0.0.1:2222:user:/path/to/key cargo test live_ -- --ignored`
#[cfg(test)]
mod live_tests {
    use super::*;

    fn roundtrip(p: ConnectionProfile) {
        // These tests run against the tester's own server, so its key is accepted.
        let mut fs = match connect(&p, None) {
            Err(ConnectError::UnknownHostKey { fingerprint }) => connect(&p, Some(&fingerprint)),
            r => r,
        }
        .expect("connect");
        let home = fs.home();
        let dir = join_slash(&home, "matrix_finder-live-test");
        let _ = remove_recursive(&mut *fs, &dir, true);
        fs.mkdir(&dir).unwrap();
        let local = std::env::temp_dir().join(format!("matrix_finder-live-{}", std::process::id()));
        std::fs::write(&local, b"hello remote").unwrap();
        let cancel = AtomicBool::new(false);
        fs.upload(&local, &join_slash(&dir, "a.txt"), &|_| {}, &cancel).unwrap();
        let names: Vec<String> = fs.list(&dir).unwrap().into_iter().map(|e| e.name).collect();
        assert_eq!(names, vec!["a.txt"]);
        assert_eq!(fs.read_head(&join_slash(&dir, "a.txt"), 5, &cancel).unwrap(), b"hello");
        fs.rename(&join_slash(&dir, "a.txt"), &join_slash(&dir, "b.txt")).unwrap();
        let back = local.with_extension("back");
        fs.download(&join_slash(&dir, "b.txt"), &back, &|_| {}, &cancel).unwrap();
        assert_eq!(std::fs::read(&back).unwrap(), b"hello remote");
        remove_recursive(&mut *fs, &dir, true).unwrap();
        let _ = std::fs::remove_file(local);
        let _ = std::fs::remove_file(back);
    }

    fn parse(var: &str, proto: Protocol) -> Option<ConnectionProfile> {
        let v = std::env::var(var).ok()?;
        let parts: Vec<&str> = v.splitn(4, ':').collect();
        let mut p = ConnectionProfile { protocol: proto, host: parts[0].into(), port: parts[1].parse().ok()?, user: parts[2].into(), ..Default::default() };
        let secret = parts.get(3).copied().unwrap_or("");
        if proto == Protocol::Sftp && std::path::Path::new(secret).is_file() {
            p.key_file = secret.into();
        } else {
            p.password = secret.into();
        }
        Some(p)
    }

    #[test]
    #[ignore]
    fn live_ftp() {
        roundtrip(parse("MATRIX_FINDER_TEST_FTP", Protocol::Ftp).expect("set MATRIX_FINDER_TEST_FTP"));
    }

    #[test]
    #[ignore]
    fn live_sftp() {
        roundtrip(parse("MATRIX_FINDER_TEST_SFTP", Protocol::Sftp).expect("set MATRIX_FINDER_TEST_SFTP"));
    }

    #[cfg(feature = "sftp")]
    #[test]
    #[ignore]
    fn live_sftp_delete_keeps_what_links_point_to() {
        let p = parse("MATRIX_FINDER_TEST_SFTP", Protocol::Sftp).expect("set MATRIX_FINDER_TEST_SFTP");
        let mut fs = match SftpFs::connect(&p, None) {
            Err(ConnectError::UnknownHostKey { fingerprint }) => SftpFs::connect(&p, Some(&fingerprint)),
            r => r,
        }
        .expect("connect");
        let dir = join_slash(&fs.home(), "matrix_finder-live-links");
        let _ = remove_recursive(&mut fs, &dir, true);
        let (target, nested) = (join_slash(&dir, "target"), join_slash(&dir, "nested"));
        fs.mkdir(&dir).unwrap();
        fs.mkdir(&target).unwrap();
        fs.mkdir(&nested).unwrap();
        let local = std::env::temp_dir().join(format!("matrix_finder-live-links-{}", std::process::id()));
        std::fs::write(&local, b"keep me").unwrap();
        fs.upload(&local, &join_slash(&target, "a.txt"), &|_| {}, &AtomicBool::new(false)).unwrap();
        let _ = std::fs::remove_file(&local);
        let (top, inner) = (join_slash(&dir, "link"), join_slash(&nested, "link"));
        fs.sftp.symlink(Path::new(&target), Path::new(&top)).unwrap();
        fs.sftp.symlink(Path::new(&target), Path::new(&inner)).unwrap();
        // Listed as a folder, as the app would pass it.
        let listed = fs.list(&dir).unwrap();
        let link = listed.iter().find(|e| e.name == "link").unwrap();
        assert!(link.is_dir && link.is_link);
        remove_recursive(&mut fs, &top, true).unwrap();
        remove_recursive(&mut fs, &nested, true).unwrap();
        let names: Vec<String> = fs.list(&dir).unwrap().into_iter().map(|e| e.name).collect();
        assert_eq!(names, vec!["target"]);
        let kept: Vec<String> = fs.list(&target).unwrap().into_iter().map(|e| e.name).collect();
        assert_eq!(kept, vec!["a.txt"]);
        remove_recursive(&mut fs, &dir, true).unwrap();
    }
}
