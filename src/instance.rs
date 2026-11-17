//! Single instance: a second launch (e.g. `matrix_finder ~/src` in a terminal) hands
//! its folder to the Matrix Finder that is already running, which opens it in a new
//! tab, and then exits.
//!
//! The running app listens on a localhost TCP port. The port and a random token are
//! kept in `instance.json` in the config folder (readable only by the user); a
//! request without the token is ignored. Before a later launch sends anything, the
//! listener proves it knows the token too (it answers a random challenge with
//! SHA-256 of the token and the challenge): after a crash another program may hold
//! that port, and it must not be told which folder is being opened.
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

/// The first line from a later launch: a random challenge.
#[derive(Serialize, Deserialize)]
struct Hello {
    nonce: String,
}

/// The running app's answer to `Hello`.
#[derive(Serialize, Deserialize)]
struct Proof {
    proof: String,
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
    // Left behind by a Matrix Finder that is gone (a crash).
    if !crate::util::pid_alive(info.pid) {
        return false;
    }
    ask(info.port, &info.token, open)
}

/// What the running app answers to challenge `nonce`.
fn proof(token: &str, nonce: &str) -> String {
    let mut bytes = token.as_bytes().to_vec();
    bytes.push(b'\n');
    bytes.extend_from_slice(nonce.as_bytes());
    crate::util::sha256(&bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// Send one JSON line.
fn send_line(s: &TcpStream, v: &impl Serialize) -> bool {
    let Ok(mut line) = serde_json::to_string(v) else { return false };
    line.push('\n');
    (&*s).write_all(line.as_bytes()).is_ok()
}

/// Talk to the app at `port`: check that it knows `token`, then send the request.
fn ask(port: u16, token: &str, open: Option<&str>) -> bool {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let Ok(s) = TcpStream::connect_timeout(&addr, Duration::from_millis(500)) else { return false };
    let _ = s.set_read_timeout(Some(Duration::from_secs(3)));
    let _ = s.set_write_timeout(Some(Duration::from_secs(3)));
    let nonce = crate::util::random_hex(16);
    if !send_line(&s, &Hello { nonce: nonce.clone() }) {
        return false;
    }
    let mut r = BufReader::new((&s).take(MAX_REQUEST));
    let mut line = String::new();
    if r.read_line(&mut line).is_err() {
        return false;
    }
    // Not the app (or another version of it): nothing is told, a new window opens.
    let Ok(answer) = serde_json::from_str::<Proof>(&line) else { return false };
    if !crate::util::same_bytes(answer.proof.as_bytes(), proof(token, &nonce).as_bytes()) {
        return false;
    }
    if !send_line(&s, &Request { token: token.to_string(), open: open.map(str::to_string) }) {
        return false;
    }
    let mut reply = String::new();
    r.read_line(&mut reply).is_ok() && reply.trim() == "ok"
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
        crate::util::write_private(&info_file(), &serde_json::to_string(&info).ok()?).ok()?;
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

/// Answer one connection: prove the token is known, then take the request. `Some(open)`
/// for a request with the right token.
fn serve(s: TcpStream, token: &str) -> Option<Option<String>> {
    let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = s.set_write_timeout(Some(Duration::from_secs(2)));
    let mut r = BufReader::new((&s).take(MAX_REQUEST));
    let mut line = String::new();
    r.read_line(&mut line).ok()?;
    let hello: Hello = serde_json::from_str(&line).ok()?;
    // A real challenge: long enough not to be one seen before.
    if !(16..=256).contains(&hello.nonce.len()) {
        return None;
    }
    if !send_line(&s, &Proof { proof: proof(token, &hello.nonce) }) {
        return None;
    }
    line.clear();
    r.read_line(&mut line).ok()?;
    let req: Request = serde_json::from_str(&line).ok()?;
    if !crate::util::same_bytes(req.token.as_bytes(), token.as_bytes()) {
        return None;
    }
    let _ = (&s).write_all(b"ok\n");
    Some(req.open)
}

/// 128 random bits as hex.
fn new_token() -> String {
    crate::util::random_hex(16)
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
    fn tokens() {
        assert_eq!(new_token().len(), 32);
        assert_ne!(new_token(), new_token());
        assert_eq!(proof("t", "n"), proof("t", "n"));
        assert_ne!(proof("t", "n"), proof("t", "m"));
        assert_ne!(proof("t", "n"), proof("u", "n"));
    }

    /// A request reaches the server and is answered; one that does not know the token
    /// sends nothing; the request of an old version (no challenge) is ignored.
    #[test]
    fn round_trip() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let srv = std::thread::spawn(move || {
            let mut got = vec![];
            for s in listener.incoming().take(3).flatten() {
                got.push(serve(s, "secret"));
            }
            got
        });
        assert!(ask(port, "secret", Some("/tmp")));
        // The server's proof does not match this token: the request is never sent.
        assert!(!ask(port, "wrong", Some("/private")));
        let mut s = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
        let req = serde_json::to_string(&Request { token: "secret".into(), open: Some("/old".into()) }).unwrap();
        s.write_all(format!("{req}\n").as_bytes()).unwrap();
        let mut r = String::new();
        let _ = BufReader::new(s).read_line(&mut r);
        assert_eq!(r, "");
        assert_eq!(srv.join().unwrap(), vec![Some(Some("/tmp".to_string())), None, None]);
    }

    /// Another program on the recorded port (after a crash) cannot prove the token, so
    /// it is never told the folder or the token.
    #[test]
    fn impostor_learns_nothing() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let srv = std::thread::spawn(move || {
            let (s, _) = listener.accept().unwrap();
            let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
            let mut r = BufReader::new(&s);
            let mut first = String::new();
            r.read_line(&mut first).unwrap();
            (&s).write_all(b"{\"proof\":\"0000\"}\n").unwrap();
            let mut rest = String::new();
            let _ = r.read_to_string(&mut rest);
            first + &rest
        });
        assert!(!ask(port, "secret", Some("/home/me/private")));
        let heard = srv.join().unwrap();
        assert!(!heard.contains("private") && !heard.contains("secret"), "{heard}");
        assert!(!crate::util::pid_alive(u32::MAX));
    }
}
