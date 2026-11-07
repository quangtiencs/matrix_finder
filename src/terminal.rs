//! Launch terminals and external applications.
use crate::util::{es, shell_split, Res};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::process::Command;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum TerminalKind {
    /// The terminal built into Matrix Finder (opens in a tab).
    BuiltIn,
    Native,
    Ghostty,
    ITerm2,
    WindowsTerminal,
    PowerShell,
    Alacritty,
    Kitty,
    WezTerm,
    Konsole,
    GnomeTerminal,
    Custom,
}

impl TerminalKind {
    pub fn label(self) -> &'static str {
        match self {
            TerminalKind::BuiltIn => "Built-in (tab)",
            TerminalKind::Native => "Native (OS default)",
            TerminalKind::Ghostty => "Ghostty",
            TerminalKind::ITerm2 => "iTerm2",
            TerminalKind::WindowsTerminal => "Windows Terminal",
            TerminalKind::PowerShell => "PowerShell",
            TerminalKind::Alacritty => "Alacritty",
            TerminalKind::Kitty => "Kitty",
            TerminalKind::WezTerm => "WezTerm",
            TerminalKind::Konsole => "Konsole",
            TerminalKind::GnomeTerminal => "GNOME Terminal",
            TerminalKind::Custom => "Custom command…",
        }
    }

    /// Terminals that make sense on the current OS.
    pub fn available() -> Vec<TerminalKind> {
        use TerminalKind::*;
        if cfg!(target_os = "macos") {
            vec![BuiltIn, Native, Ghostty, ITerm2, Alacritty, Kitty, WezTerm, Custom]
        } else if cfg!(windows) {
            vec![BuiltIn, Native, WindowsTerminal, PowerShell, Alacritty, WezTerm, Custom]
        } else {
            vec![BuiltIn, Native, Ghostty, GnomeTerminal, Konsole, Alacritty, Kitty, WezTerm, Custom]
        }
    }
}

/// A saved way to open a terminal: which app, which shell, and a startup script.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct TerminalProfile {
    pub name: String,
    pub terminal: TerminalKind,
    /// Command template for `TerminalKind::Custom` (`{dir}`, `{cmd}`).
    pub custom: String,
    /// Shell to start (empty = the user's default shell).
    pub shell: String,
    /// Commands run when the terminal starts (one per line), then the shell stays open.
    pub startup: String,
    /// Colour theme of the built-in terminal (empty = same as the app).
    pub theme: String,
}

/// Colour theme of a new built-in terminal profile.
pub const DEFAULT_TERM_THEME: &str = "Breeze Light";

impl Default for TerminalProfile {
    fn default() -> Self {
        Self { name: "Default".into(), terminal: TerminalKind::BuiltIn, custom: String::new(), shell: String::new(), startup: String::new(), theme: DEFAULT_TERM_THEME.into() }
    }
}

/// The user's login shell (or the platform default).
pub fn default_shell() -> String {
    if cfg!(windows) {
        if which("pwsh") { "pwsh".into() } else { "powershell".into() }
    } else {
        std::env::var("SHELL").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "/bin/sh".into())
    }
}

/// Shells offered in the profile editor (from /etc/shells on Unix).
pub fn known_shells() -> Vec<String> {
    if cfg!(windows) {
        return ["pwsh", "powershell", "cmd", "bash", "nu"].iter().filter(|s| which(s)).map(|s| s.to_string()).collect();
    }
    let mut v: Vec<String> = std::fs::read_to_string("/etc/shells")
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#') && !l.contains("nologin") && !l.ends_with("/false"))
        .map(String::from)
        .collect();
    for extra in ["fish", "nu", "pwsh"] {
        if which(extra) && !v.iter().any(|s| s.ends_with(&format!("/{extra}"))) {
            v.push(extra.into());
        }
    }
    v.dedup();
    v
}

/// Command line (argv) that starts the profile's shell, runs its startup
/// script, and leaves an interactive shell open. `None` = terminal default.
pub fn startup_argv(p: &TerminalProfile) -> Option<Vec<String>> {
    let script = p.startup.trim();
    let shell = p.shell.trim();
    if script.is_empty() && shell.is_empty() {
        return None;
    }
    let shell = if shell.is_empty() { default_shell() } else { shell.to_string() };
    // Accept both Unix and Windows separators (profiles may be shared across OSes).
    let base = shell.rsplit(['/', '\\']).next().unwrap_or(&shell).to_lowercase();
    let base = base.trim_end_matches(".exe");
    Some(match base {
        "pwsh" | "powershell" => {
            let mut v = vec![shell.clone(), "-NoExit".into()];
            if !script.is_empty() {
                v.push("-Command".into());
                v.push(script.lines().collect::<Vec<_>>().join("; "));
            }
            v
        }
        "cmd" => {
            let mut v = vec![shell.clone()];
            if !script.is_empty() {
                v.push("/K".into());
                v.push(script.lines().collect::<Vec<_>>().join(" & "));
            }
            v
        }
        "nu" | "nushell" => {
            let mut v = vec![shell.clone()];
            if !script.is_empty() {
                v.push("-e".into());
                v.push(script.to_string());
            }
            v
        }
        // POSIX shells and fish: run the script, then replace with an interactive shell.
        _ => {
            if script.is_empty() {
                vec![shell]
            } else {
                vec![shell.clone(), "-c".into(), format!("{script}\nexec {} -i", sh_quote(&shell))]
            }
        }
    })
}

/// Quote for POSIX sh.
pub fn sh_quote(s: &str) -> String {
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "/._-+=:,@".contains(c)) {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn sh_join(argv: &[String]) -> String {
    argv.iter().map(|a| sh_quote(a)).collect::<Vec<_>>().join(" ")
}

/// AppleScript string literal.
fn as_quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

fn spawn(mut cmd: Command) -> Res<()> {
    cmd.spawn().map(|_| ()).map_err(|e| format!("{}: {}", cmd.get_program().to_string_lossy(), e))
}

fn command(program: &str, args: &[String], dir: &Path) -> Command {
    let mut c = Command::new(program);
    c.args(args).current_dir(dir);
    c
}

fn sv(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

pub fn open_profile(p: &TerminalProfile, dir: &Path) -> Res<()> {
    open_terminal_with(p.terminal, &p.custom, dir, startup_argv(p))
}

pub fn open_terminal_with(kind: TerminalKind, custom: &str, dir: &Path, run: Option<Vec<String>>) -> Res<()> {
    let d = dir.to_string_lossy().to_string();
    let wd = format!("--working-directory={d}");
    let with = |mut base: Vec<String>, sep: &[&str]| -> Vec<String> {
        if let Some(r) = &run {
            base.extend(sv(sep));
            base.extend(r.iter().cloned());
        }
        base
    };
    match kind {
        TerminalKind::BuiltIn => return Err("the built-in terminal opens in a tab".into()),
        TerminalKind::Custom => {
            let mut argv = Vec::new();
            for tok in shell_split(custom) {
                if tok == "{cmd}" {
                    argv.extend(run.clone().unwrap_or_default());
                } else {
                    argv.push(tok.replace("{dir}", &d).replace("{cmd}", &run.as_deref().map(sh_join).unwrap_or_default()));
                }
            }
            let (prog, args) = argv.split_first().ok_or("Custom terminal command is empty")?;
            spawn(command(prog, args, dir))
        }
        TerminalKind::Ghostty if cfg!(target_os = "macos") => {
            spawn(command("open", &with(sv(&["-na", "Ghostty", "--args", &wd]), &["-e"]), dir))
        }
        TerminalKind::Ghostty => spawn(command("ghostty", &with(sv(&[&wd]), &["-e"]), dir)),
        TerminalKind::ITerm2 => match &run {
            None => spawn(command("open", &sv(&["-a", "iTerm", &d]), dir)),
            Some(r) => {
                let line = format!("/bin/sh -c {}", sh_quote(&format!("cd {} && exec {}", sh_quote(&d), sh_join(r))));
                let script = format!("tell application \"iTerm\" to create window with default profile command {}", as_quote(&line));
                spawn(command("osascript", &sv(&["-e", &script]), dir))
            }
        },
        TerminalKind::WindowsTerminal => spawn(command("wt", &with(sv(&["-d", &d]), &[]), dir)),
        TerminalKind::PowerShell => {
            let ps = if which("pwsh") { "pwsh" } else { "powershell" };
            let args = match &run {
                Some(r) => [sv(&["/C", "start", "\"\""]), r.clone()].concat(),
                None => sv(&["/C", "start", "\"\"", ps, "-NoExit"]),
            };
            spawn(command("cmd", &args, dir))
        }
        TerminalKind::Alacritty => spawn(command("alacritty", &with(sv(&["--working-directory", &d]), &["-e"]), dir)),
        TerminalKind::Kitty => spawn(command("kitty", &with(sv(&["--directory", &d]), &[]), dir)),
        TerminalKind::WezTerm => spawn(command("wezterm", &with(sv(&["start", "--cwd", &d]), &["--"]), dir)),
        TerminalKind::Konsole => spawn(command("konsole", &with(sv(&["--workdir", &d]), &["-e"]), dir)),
        TerminalKind::GnomeTerminal => spawn(command("gnome-terminal", &with(sv(&[&wd]), &["--"]), dir)),
        TerminalKind::Native => native_terminal(dir, run),
    }
}

#[cfg(target_os = "macos")]
fn native_terminal(dir: &Path, run: Option<Vec<String>>) -> Res<()> {
    let d = dir.to_string_lossy().to_string();
    match run {
        None => spawn(command("open", &sv(&["-a", "Terminal", &d]), dir)),
        Some(r) => {
            let line = format!("cd {} && exec {}", sh_quote(&d), sh_join(&r));
            let script = format!("tell application \"Terminal\"\nactivate\ndo script {}\nend tell", as_quote(&line));
            spawn(command("osascript", &sv(&["-e", &script]), dir))
        }
    }
}

#[cfg(windows)]
fn native_terminal(dir: &Path, run: Option<Vec<String>>) -> Res<()> {
    let d = dir.to_string_lossy().to_string();
    if which("wt") {
        let mut args = sv(&["-d", &d]);
        args.extend(run.unwrap_or_default());
        return spawn(command("wt", &args, dir));
    }
    let args = match run {
        Some(r) => [sv(&["/C", "start", "\"\""]), r].concat(),
        None => sv(&["/C", "start", "cmd", "/K"]),
    };
    spawn(command("cmd", &args, dir))
}

#[cfg(all(unix, not(target_os = "macos")))]
fn native_terminal(dir: &Path, run: Option<Vec<String>>) -> Res<()> {
    let d = dir.to_string_lossy().to_string();
    let wd = format!("--working-directory={d}");
    // (program, args before the command, separator before the command)
    let candidates: Vec<(String, Vec<String>, Vec<String>)> = {
        let mut v = Vec::new();
        if let Ok(t) = std::env::var("TERMINAL") {
            if !t.is_empty() {
                v.push((t, vec![], sv(&["-e"])));
            }
        }
        v.extend([
            ("x-terminal-emulator".to_string(), vec![], sv(&["-e"])),
            ("gnome-terminal".into(), vec![wd.clone()], sv(&["--"])),
            ("konsole".into(), sv(&["--workdir", &d]), sv(&["-e"])),
            ("xfce4-terminal".into(), vec![wd.clone()], sv(&["-x"])),
            ("tilix".into(), vec![wd.clone()], sv(&["-e"])),
            ("ghostty".into(), vec![wd.clone()], sv(&["-e"])),
            ("kitty".into(), sv(&["--directory", &d]), vec![]),
            ("alacritty".into(), sv(&["--working-directory", &d]), sv(&["-e"])),
            ("wezterm".into(), sv(&["start", "--cwd", &d]), sv(&["--"])),
            ("xterm".into(), vec![], sv(&["-e"])),
        ]);
        v
    };
    for (prog, mut args, sep) in candidates {
        if which(&prog) {
            if let Some(r) = &run {
                args.extend(sep);
                args.extend(r.iter().cloned());
            }
            return spawn(command(&prog, &args, dir));
        }
    }
    Err("No terminal emulator found. Choose one in Settings.".into())
}

pub fn which(prog: &str) -> bool {
    let Some(paths) = std::env::var_os("PATH") else { return false };
    std::env::split_paths(&paths).any(|d| {
        let p = d.join(prog);
        p.is_file() || (cfg!(windows) && d.join(format!("{prog}.exe")).is_file())
    })
}

/// Open with the operating system's default application.
pub fn open_default(path: &Path) -> Res<()> {
    open::that_detached(path).map_err(es)
}

/// Open with a specific program (path or command name).
pub fn open_with(program: &str, path: &Path) -> Res<()> {
    let parts = shell_split(program);
    let (prog, args) = parts.split_first().ok_or("No program given")?;
    if cfg!(target_os = "macos") && prog.ends_with(".app") {
        return spawn({
            let mut c = Command::new("open");
            c.arg("-a").arg(prog).arg(path);
            c
        });
    }
    let mut c = Command::new(prog);
    c.args(args).arg(path);
    if let Some(parent) = path.parent() {
        c.current_dir(parent);
    }
    spawn(c)
}

/// Show the file in the OS file manager.
pub fn reveal(path: &Path) -> Res<()> {
    if cfg!(target_os = "macos") {
        spawn({
            let mut c = Command::new("open");
            c.arg("-R").arg(path);
            c
        })
    } else if cfg!(windows) {
        spawn({
            let mut c = Command::new("explorer");
            c.arg(format!("/select,{}", path.to_string_lossy()));
            c
        })
    } else {
        open::that_detached(path.parent().unwrap_or(path)).map_err(es)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_commands_per_shell() {
        let mut p = TerminalProfile::default();
        assert_eq!(startup_argv(&p), None);
        p.shell = "/bin/zsh".into();
        assert_eq!(startup_argv(&p), Some(vec!["/bin/zsh".to_string()]));
        p.startup = "source .venv/bin/activate\nclear".into();
        assert_eq!(
            startup_argv(&p),
            Some(vec!["/bin/zsh".into(), "-c".into(), "source .venv/bin/activate\nclear\nexec /bin/zsh -i".into()])
        );
        p.shell = "pwsh".into();
        p.startup = "Set-Location src\nGet-ChildItem".into();
        assert_eq!(startup_argv(&p), Some(vec!["pwsh".into(), "-NoExit".into(), "-Command".into(), "Set-Location src; Get-ChildItem".into()]));
        p.shell = "C:\\Windows\\System32\\cmd.exe".into();
        p.startup = "cd src\ndir".into();
        assert_eq!(startup_argv(&p).unwrap()[1..], ["/K".to_string(), "cd src & dir".to_string()]);
    }

    #[test]
    fn quoting() {
        assert_eq!(sh_quote("/usr/bin/fish"), "/usr/bin/fish");
        assert_eq!(sh_quote("it's here"), "'it'\\''s here'");
    }
}
