//! Single instance: a second launch (e.g. `matrix_finder ~/src` in a terminal) hands
//! its folder to the Matrix Finder that is already running, which opens it in a new
//! tab, and then exits.
//!
//! The running app listens on a localhost TCP port. The port and a random token are
//! kept in `instance.json` in the config folder (readable only by the user); a
//! request without the token is ignored.
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};
use std::time::Duration;

/// The longest request accepted (a path and a token).
const MAX_REQUEST: u64 = 64 * 1024;

#[derive(Serialize, Deserialize)]
struct Info {
    port: u16,
    token: String,
    pid: u32,
}

#[derive(Serialize, Deserialize)]
struct Request {
    token: String,
    /// The folder (or file, archive, URL) to open; `None` only asks "are you there?".
    open: Option<String>,
}

fn info_file() -> PathBuf {
    crate::config::config_dir().join("instance.json")
}

fn read_info() -> Option<Info> {
    serde_json::from_str(&std::fs::read_to_string(info_file()).ok()?).ok()
}

/// Send one request to the running instance; true when it answered "ok".
fn send(open: Option<&str>) -> bool {
    let Some(info) = read_info() else { return false };
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, info.port));
    let Ok(mut s) = TcpStream::connect_timeout(&addr, Duration::from_millis(500)) else { return false };
    let _ = s.set_read_timeout(Some(Duration::from_secs(3)));
    let _ = s.set_write_timeout(Some(Duration::from_secs(3)));
    let req = Request { token: info.token, open: open.map(str::to_string) };
    let Ok(mut line) = serde_json::to_string(&req) else { return false };
    line.push('\n');
    if s.write_all(line.as_bytes()).is_err() {
        return false;
    }
    let mut reply = String::new();
    BufReader::new(s).read_line(&mut reply).is_ok() && reply.trim() == "ok"
}

/// Ask the running Matrix Finder to open `target` in a new tab. False when none
/// is running (or it did not answer): start normally then.
pub fn hand_off(target: &str) -> bool {
    send(Some(target))
}

/// Requests from later launches, for the running app.
pub struct Server {
    pub rx: Receiver<String>,
    token: String,
}

impl Server {
    /// Start listening, unless another Matrix Finder already does (then `None`).
    /// `ctx` is woken for every request.
    pub fn start(ctx: egui::Context) -> Option<Server> {
        if send(None) {
            return None;
        }
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).ok()?;
        let port = listener.local_addr().ok()?.port();
        let token = new_token();
        let info = Info { port, token: token.clone(), pid: std::process::id() };
        write_private(&info_file(), &serde_json::to_string(&info).ok()?).ok()?;
        let (tx, rx) = channel();
        let want = token.clone();
        std::thread::Builder::new()
            .name("single-instance".into())
            .spawn(move || {
                for s in listener.incoming().flatten() {
                    let Some(open) = serve(s, &want) else { continue };
                    if let Some(open) = open {
                        if tx.send(open).is_err() {
                            return;
                        }
                        ctx.request_repaint();
                    }
                }
            })
            .ok()?;
        Some(Server { rx, token })
    }

    /// Remove `instance.json` when it is still ours (call on exit).
    pub fn stop(&self) {
        if read_info().is_some_and(|i| i.token == self.token) {
            let _ = std::fs::remove_file(info_file());
        }
    }
}

/// Answer one connection. `Some(open)` for a request with the right token.
fn serve(s: TcpStream, token: &str) -> Option<Option<String>> {
    let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = s.set_write_timeout(Some(Duration::from_secs(2)));
    let mut line = String::new();
    BufReader::new((&s).take(MAX_REQUEST)).read_line(&mut line).ok()?;
    let req: Request = serde_json::from_str(&line).ok()?;
    if !same(req.token.as_bytes(), token.as_bytes()) {
        return None;
    }
    let _ = (&s).write_all(b"ok\n");
    Some(req.open)
}

/// Compare without stopping at the first difference.
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// 128 random bits as hex (`RandomState` is seeded from the OS).
fn new_token() -> String {
    use std::hash::{BuildHasher, Hasher};
    let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    (0..2u8)
        .map(|i| {
            let mut h = std::collections::hash_map::RandomState::new().build_hasher();
            h.write_u8(i);
            h.write_u128(seed);
            h.write_u32(std::process::id());
            format!("{:016x}", h.finish())
        })
        .collect()
}

/// Write `text` to `path` (via a temporary file) so only the user can read it.
fn write_private(path: &Path, text: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(path.parent().unwrap_or(Path::new(".")))?;
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
    o.open(&tmp)?.write_all(text.as_bytes())?;
    std::fs::rename(&tmp, path)
}

/// Command line: `matrix_finder [--new-tab | --new-window] [FOLDER]`.
#[derive(Debug, Default, PartialEq)]
pub struct Args {
    /// The folder (or file, archive, `sftp://…` URL) to open.
    pub open: Option<String>,
    /// `--new-tab` / `--new-window`: overrides Settings for this launch.
    pub mode: Option<crate::config::InstanceMode>,
}

impl Args {
    /// Parse; `Err` is the text to print (help, version or a usage error) before exiting.
    pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Args, (String, i32)> {
        use crate::config::InstanceMode;
        let mut a = Args::default();
        let mut only_paths = false;
        for arg in args {
            match arg.as_str() {
                _ if only_paths || !arg.starts_with('-') || arg == "-" => {
                    if a.open.is_some() {
                        return Err((format!("matrix_finder: only one folder can be given\n\n{}", usage()), 2));
                    }
                    a.open = Some(arg);
                }
                "--" => only_paths = true,
                "--new-tab" => a.mode = Some(InstanceMode::NewTab),
                "--new-window" => a.mode = Some(InstanceMode::NewWindow),
                "-h" | "--help" => return Err((usage(), 0)),
                "-V" | "--version" => return Err((format!("matrix_finder {}", env!("CARGO_PKG_VERSION")), 0)),
                // macOS adds this when the app is started from Finder.
                _ if arg.starts_with("-psn_") => {}
                _ => return Err((format!("matrix_finder: unknown option {arg}\n\n{}", usage()), 2)),
            }
        }
        Ok(a)
    }

    /// What to open, as an absolute path (`sftp://…` and `~` are kept as typed).
    /// No folder given: the terminal's current folder when started from one.
    pub fn target(&self, cwd: Option<&Path>, from_terminal: bool) -> Option<String> {
        let Some(open) = &self.open else {
            return cwd.filter(|_| from_terminal).map(|d| d.to_string_lossy().to_string());
        };
        if open.contains("://") || open.starts_with('~') {
            return Some(open.clone());
        }
        let p = Path::new(open);
        let abs = if p.is_absolute() { p.to_path_buf() } else { cwd.map(|d| d.join(p)).unwrap_or_else(|| p.to_path_buf()) };
        Some(std::fs::canonicalize(&abs).unwrap_or(abs).to_string_lossy().to_string())
    }
}

fn usage() -> String {
    "Usage: matrix_finder [OPTIONS] [FOLDER]\n\n\
     Opens FOLDER (default: the current folder when started from a terminal).\n\
     When Matrix Finder is already running, it opens there in a new tab\n\
     (Settings → Files & layout → Window).\n\n\
     Options:\n  \
       --new-tab      Open in a new tab of the running Matrix Finder\n  \
       --new-window   Start a separate Matrix Finder window\n  \
       -h, --help     Show this help\n  \
       -V, --version  Show the version"
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::InstanceMode;

    fn parse(v: &[&str]) -> Result<Args, (String, i32)> {
        Args::parse(v.iter().map(|s| s.to_string()))
    }

    #[test]
    fn parses_flags_and_folder() {
        assert_eq!(parse(&[]).unwrap(), Args::default());
        let a = parse(&["--new-window", "src"]).unwrap();
        assert_eq!(a.mode, Some(InstanceMode::NewWindow));
        assert_eq!(a.open.as_deref(), Some("src"));
        assert_eq!(parse(&["--", "--odd"]).unwrap().open.as_deref(), Some("--odd"));
        assert_eq!(parse(&["-psn_0_12345"]).unwrap(), Args::default());
        assert_eq!(parse(&["--bogus"]).unwrap_err().1, 2);
        assert_eq!(parse(&["a", "b"]).unwrap_err().1, 2);
        assert_eq!(parse(&["--help"]).unwrap_err().1, 0);
    }

    #[test]
    fn resolves_target() {
        let cwd = std::env::temp_dir();
        let none = Args::default();
        assert_eq!(none.target(Some(&cwd), false), None);
        assert_eq!(none.target(Some(&cwd), true), Some(cwd.to_string_lossy().to_string()));
        let url = Args { open: Some("sftp://me@host/x".into()), mode: None };
        assert_eq!(url.target(Some(&cwd), true).as_deref(), Some("sftp://me@host/x"));
        let rel = Args { open: Some("does-not-exist-mf".into()), mode: None };
        assert_eq!(rel.target(Some(&cwd), true), Some(cwd.join("does-not-exist-mf").to_string_lossy().to_string()));
    }

    #[test]
    fn token_compare() {
        assert!(same(b"abc", b"abc"));
        assert!(!same(b"abc", b"abd"));
        assert!(!same(b"abc", b"ab"));
        assert_eq!(new_token().len(), 32);
        assert_ne!(new_token(), new_token());
    }

    /// A request reaches the server and is answered; a wrong token is ignored.
    #[test]
    fn round_trip() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let srv = std::thread::spawn(move || {
            let mut got = vec![];
            for s in listener.incoming().take(2).flatten() {
                got.push(serve(s, "secret"));
            }
            got
        });
        let ask = |token: &str| {
            let mut s = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
            let req = serde_json::to_string(&Request { token: token.into(), open: Some("/tmp".into()) }).unwrap();
            s.write_all(format!("{req}\n").as_bytes()).unwrap();
            let mut r = String::new();
            let _ = BufReader::new(s).read_line(&mut r);
            r
        };
        assert_eq!(ask("secret"), "ok\n");
        assert_eq!(ask("wrong"), "");
        assert_eq!(srv.join().unwrap(), vec![Some(Some("/tmp".to_string())), None]);
    }
}
