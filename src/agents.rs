//! AI coding agents (Claude Code, Codex, …): which are installed, and their logos,
//! for the AI button in a terminal's info bar (Settings → Terminal → AI button).
use egui::{pos2, Color32, Painter, Rect};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

pub struct Agent {
    /// Stable id, kept in the settings (`term_agent`).
    pub id: &'static str,
    pub name: &'static str,
    /// The command that starts it.
    pub program: &'static str,
    /// Its logo, one colour (`currentColor`), drawn in the colour of the other buttons.
    pub svg: &'static [u8],
}

/// Every agent, the most widely used first: unless one is chosen, the AI button
/// starts the first one found.
pub const ALL: [Agent; 5] = [
    Agent { id: "claude", name: "Claude Code", program: "claude", svg: include_bytes!("../assets/agents/claude.svg") },
    Agent { id: "codex", name: "Codex", program: "codex", svg: include_bytes!("../assets/agents/codex.svg") },
    Agent { id: "gemini", name: "Gemini CLI", program: "gemini", svg: include_bytes!("../assets/agents/geminicli.svg") },
    Agent { id: "qwen", name: "Qwen Code", program: "qwen", svg: include_bytes!("../assets/agents/qwen.svg") },
    Agent { id: "opencode", name: "OpenCode", program: "opencode", svg: include_bytes!("../assets/agents/opencode.svg") },
];

pub fn get(id: &str) -> Option<&'static Agent> {
    ALL.iter().find(|a| a.id == id)
}

/// An installed agent.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    pub id: &'static str,
    /// What to type to start it: the command when the user's shell finds it,
    /// else the full path of the program.
    pub run: String,
}

/// What a click on the AI button (or its menu) asks for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AgentAct {
    /// Start it at this terminal's prompt.
    Run(&'static str),
    /// Make this agent the one the button starts.
    Choose(&'static str),
    /// Turn the AI button off.
    HideButton,
    /// Open Settings → Terminal.
    Settings,
}

/// The agent the AI button starts: the chosen one (`chosen` is an id, empty for
/// automatic) when it is installed, else the first one found.
pub fn pick<'a>(found: &'a [Found], chosen: &str) -> Option<&'a Found> {
    found.iter().find(|f| f.id == chosen).or_else(|| ALL.iter().find_map(|a| found.iter().find(|f| f.id == a.id)))
}

/// Look for the agents on a thread; the result arrives on the receiver.
pub fn detect_in_background(ctx: &egui::Context) -> Receiver<Vec<Found>> {
    let (tx, rx) = channel();
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        let _ = tx.send(detect());
        ctx.request_repaint();
    });
    rx
}

/// Which agents are installed. Asks the user's shell first, since that is where
/// the built-in terminal starts them (an app opened from Finder or a desktop menu
/// often has a much shorter PATH), then looks in PATH and the usual install folders.
pub fn detect() -> Vec<Found> {
    let programs: Vec<&str> = ALL.iter().map(|a| a.program).collect();
    let in_shell = if cfg!(windows) || cfg!(test) { None } else { shell_finds(&crate::terminal::default_shell(), &programs) };
    let dirs = search_dirs();
    ALL.iter()
        .filter_map(|a| {
            let run = if in_shell.as_ref().is_some_and(|s| s.contains(a.program)) {
                a.program.to_string()
            } else {
                let (path, on_path) = find_in(a.program, &dirs)?;
                // In PATH the shell finds it by name too, unless it was asked and said no.
                if on_path && in_shell.is_none() { a.program.to_string() } else { path.to_string_lossy().to_string() }
            };
            Some(Found { id: a.id, run })
        })
        .collect()
}

/// Folders to look in, each with whether it is in this process's PATH.
fn search_dirs() -> Vec<(PathBuf, bool)> {
    let mut v: Vec<(PathBuf, bool)> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).map(|d| (d, true)).collect()).unwrap_or_default();
    let mut extra: Vec<PathBuf> = Vec::new();
    if let Some(h) = dirs::home_dir() {
        if cfg!(windows) {
            for k in ["APPDATA", "LOCALAPPDATA"] {
                if let Some(d) = std::env::var_os(k) {
                    let d = PathBuf::from(d);
                    extra.push(d.join("npm"));
                    extra.push(d.join("Programs").join("claude"));
                }
            }
            extra.push(h.join(".local").join("bin"));
            extra.push(h.join(".bun").join("bin"));
            extra.push(h.join(".opencode").join("bin"));
        } else {
            for rel in [
                ".local/bin", ".claude/local", ".npm-global/bin", ".npm/bin", ".bun/bin", ".opencode/bin", ".volta/bin", ".cargo/bin", ".yarn/bin", ".deno/bin",
                ".asdf/shims", ".local/share/mise/shims", ".local/share/pnpm", "Library/pnpm", "bin",
            ] {
                extra.push(h.join(rel));
            }
            // Node version managers keep one bin folder per version.
            for base in [".nvm/versions/node", ".local/share/fnm/node-versions", "Library/Application Support/fnm/node-versions"] {
                if let Ok(rd) = std::fs::read_dir(h.join(base)) {
                    for e in rd.flatten() {
                        let p = e.path();
                        extra.push(if base.contains("fnm") { p.join("installation").join("bin") } else { p.join("bin") });
                    }
                }
            }
            for abs in ["/opt/homebrew/bin", "/usr/local/bin", "/home/linuxbrew/.linuxbrew/bin", "/snap/bin"] {
                extra.push(abs.into());
            }
        }
    }
    for d in extra {
        if !v.iter().any(|(p, _)| *p == d) {
            v.push((d, false));
        }
    }
    v
}

/// The first `program` in `dirs` that can be run, and whether its folder is in PATH.
fn find_in(program: &str, dirs: &[(PathBuf, bool)]) -> Option<(PathBuf, bool)> {
    let names: Vec<String> = if cfg!(windows) { ["exe", "cmd", "bat", "ps1"].iter().map(|e| format!("{program}.{e}")).collect() } else { vec![program.to_string()] };
    dirs.iter().find_map(|(d, on_path)| names.iter().map(|n| d.join(n)).find(|p| runnable(p)).map(|p| (p, *on_path)))
}

fn runnable(p: &Path) -> bool {
    let Ok(m) = std::fs::metadata(p) else { return false };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        m.is_file() && m.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        m.is_file()
    }
}

const MARK: &str = "matrix-finder-agent:";
const DONE: &str = "matrix-finder-agents-done";

/// The programs the user's shell finds (as a login, interactive shell, like the
/// built-in terminal starts it, so aliases and PATH set in its startup files count).
/// `None` when the shell is not one this knows how to ask, or it did not answer.
fn shell_finds(shell: &str, programs: &[&str]) -> Option<HashSet<String>> {
    let script = probe_script(shell, programs)?;
    let child = std::process::Command::new(shell)
        .args(["-i", "-l", "-c", &script])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn();
    let mut child = child.ok()?;
    let out = child.stdout.take()?;
    // Read on a thread: a startup file may start something that keeps the pipe open.
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        use std::io::BufRead;
        for line in std::io::BufReader::new(out).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let deadline = Instant::now() + Duration::from_secs(8);
    let mut lines = Vec::new();
    let mut done = false;
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        match rx.recv_timeout(left) {
            Ok(l) if l.trim() == DONE => {
                done = true;
                break;
            }
            Ok(l) => lines.push(l),
            Err(_) => break,
        }
    }
    if !matches!(child.try_wait(), Ok(Some(_))) {
        let _ = child.kill();
    }
    let _ = child.wait();
    // No end mark: the shell did not finish, so its answer says nothing.
    done.then(|| parse_probe(&lines))
}

/// A script for `shell` that prints a mark line for each program it finds.
fn probe_script(shell: &str, programs: &[&str]) -> Option<String> {
    let base = shell.rsplit('/').next().unwrap_or(shell);
    let list = programs.join(" ");
    match base {
        "fish" => Some(format!("for c in {list}; if command -q $c; echo {MARK}$c; end; end; echo {DONE}")),
        "sh" | "bash" | "zsh" | "ksh" | "mksh" | "dash" | "yash" => {
            Some(format!("for c in {list}; do command -v \"$c\" >/dev/null 2>&1 && echo \"{MARK}$c\"; done; echo {DONE}"))
        }
        _ => None,
    }
}

fn parse_probe(lines: &[String]) -> HashSet<String> {
    lines.iter().filter_map(|l| l.trim().strip_prefix(MARK)).map(String::from).collect()
}

// ---------------------------------------------------------------- icons

type Textures = HashMap<(&'static str, u32), Option<egui::TextureHandle>>;
static TEXTURES: std::sync::LazyLock<std::sync::Mutex<Textures>> = std::sync::LazyLock::new(Default::default);

/// The agent's logo as a white mask `px` pixels square (straight alpha).
fn mask(a: &Agent, px: u32) -> Option<Vec<u8>> {
    let svg = String::from_utf8_lossy(a.svg).replace("currentColor", "#fff");
    let mut rgba = crate::logo::render(svg.as_bytes(), px)?;
    for c in rgba.chunks_exact_mut(4) {
        c[..3].fill(255);
    }
    Some(rgba)
}

/// Draw the agent's logo in `r`, in `col` (like the icons of the buttons beside it).
pub fn paint_logo(p: &Painter, r: Rect, a: &'static Agent, col: Color32) {
    let side = r.width().min(r.height());
    let px = (side * p.ctx().pixels_per_point()).round().clamp(8.0, 256.0) as u32;
    let tex = {
        let Ok(mut cache) = TEXTURES.lock() else { return };
        cache
            .entry((a.id, px))
            .or_insert_with(|| {
                let img = egui::ColorImage::from_rgba_unmultiplied([px as usize, px as usize], &mask(a, px)?);
                Some(p.ctx().load_texture(format!("agent:{}:{px}", a.id), img, egui::TextureOptions::LINEAR))
            })
            .clone()
    };
    if let Some(tex) = tex {
        let sq = Rect::from_center_size(r.center(), egui::vec2(side, side));
        p.image(tex.id(), sq, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), col);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(ids: &[&'static str]) -> Vec<Found> {
        ids.iter().map(|id| Found { id, run: id.to_string() }).collect()
    }

    #[test]
    fn picks_the_chosen_one_or_the_first_found() {
        let f = found(&["opencode", "qwen", "codex"]);
        let id = |chosen: &str| pick(&f, chosen).map(|f| f.id);
        assert_eq!(id(""), Some("codex"), "automatic: the first in ALL that is installed");
        assert_eq!(id("qwen"), Some("qwen"));
        assert_eq!(id("claude"), Some("codex"), "chosen but not installed");
        assert_eq!(pick(&[], "claude"), None);
    }

    #[test]
    fn logos_render() {
        for a in &ALL {
            let m = mask(a, 32).unwrap_or_else(|| panic!("{} logo renders", a.id));
            let ink = m.chunks_exact(4).filter(|c| c[3] > 128).count();
            assert!(ink > 32 * 32 / 10, "{}: {ink} pixels drawn", a.id);
        }
    }

    #[test]
    fn probe_script_and_output() {
        let s = probe_script("/bin/zsh", &["claude", "codex"]).unwrap();
        assert!(s.starts_with("for c in claude codex; do command -v"));
        assert!(probe_script("/opt/homebrew/bin/fish", &["claude"]).unwrap().contains("command -q $c"));
        assert_eq!(probe_script("/usr/bin/nu", &["claude"]), None);
        let lines: Vec<String> = ["Welcome!", "matrix-finder-agent:claude", "  matrix-finder-agent:qwen  "].iter().map(|s| s.to_string()).collect();
        let got = parse_probe(&lines);
        assert_eq!(got, ["claude", "qwen"].iter().map(|s| s.to_string()).collect());
    }

    /// A real shell, end to end: it finds `sh` and says so, and not a made-up name.
    #[cfg(unix)]
    #[test]
    fn real_shell_probe() {
        let got = shell_finds("/bin/sh", &["sh", "mf-no-such-program"]).expect("the shell answers");
        assert_eq!(got, ["sh".to_string()].into_iter().collect());
        assert_eq!(shell_finds("/bin/no-such-shell", &["sh"]), None);
    }

    /// What this computer has: cargo test agents::tests::detect_here -- --ignored --nocapture
    #[test]
    #[ignore]
    fn detect_here() {
        let programs: Vec<&str> = ALL.iter().map(|a| a.program).collect();
        let at = Instant::now();
        let shell = crate::terminal::default_shell();
        println!("{shell} finds {:?} ({:?})", shell_finds(&shell, &programs), at.elapsed());
        let dirs = search_dirs();
        for a in &ALL {
            println!("{:>8}: {:?}", a.id, find_in(a.program, &dirs));
        }
    }

    #[cfg(unix)]
    #[test]
    fn finds_runnable_files() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("mf-agents-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let prog = dir.join("mf-fake-agent");
        std::fs::write(&prog, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&prog, std::fs::Permissions::from_mode(0o755)).unwrap();
        let dirs = vec![(PathBuf::from("/nonexistent"), true), (dir.clone(), false)];
        assert_eq!(find_in("mf-fake-agent", &dirs), Some((prog.clone(), false)));
        std::fs::set_permissions(&prog, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(find_in("mf-fake-agent", &dirs), None, "not executable");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
