//! Which app opens which file type. Files open with the operating system's default
//! app unless the user chose another one for that type (`Settings::default_apps`);
//! this never changes the system's own settings.
use crate::util::shell_split;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{channel, Sender};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// File types listed in Settings even when nobody has chosen an app for them.
pub const COMMON: [&str; 14] = ["pdf", "txt", "md", "html", "jpg", "png", "mp4", "mp3", "docx", "xlsx", "pptx", "csv", "json", "zip"];

/// `.PDF`, `*.pdf` and ` pdf ` all mean the type `pdf`.
pub fn normalize_ext(text: &str) -> String {
    text.trim().trim_start_matches('*').trim_start_matches('.').trim().to_lowercase()
}

/// The app chosen for a file, trying longer extensions first (`tar.gz` before `gz`).
pub fn program_for<'a>(apps: &'a BTreeMap<String, String>, file_name: &str) -> Option<&'a str> {
    extensions(file_name).find_map(|ext| apps.get(&ext)).map(String::as_str)
}

/// A file's extensions in lower case, longest first: `a.tar.gz` gives `tar.gz`, then `gz`.
fn extensions(file_name: &str) -> impl Iterator<Item = String> {
    let name = file_name.to_lowercase();
    // A leading dot marks a hidden file, not an extension.
    let body_start = usize::from(name.starts_with('.'));
    let starts: Vec<usize> = name[body_start..].match_indices('.').map(|(i, _)| body_start + i + 1).collect();
    starts.into_iter().map(move |i| name[i..].to_string()).filter(|e| !e.is_empty())
}

/// The type "always open with" applies to: the file's last extension.
pub fn type_of(file_name: &str) -> Option<String> {
    extensions(file_name).last()
}

/// Whether a chosen app certainly cannot start. `.app` names are left to macOS to find.
pub fn program_missing(program: &str) -> bool {
    let parts = shell_split(program);
    let Some(first) = parts.first() else { return false };
    let path = Path::new(first);
    if path.components().count() > 1 {
        return !path.exists();
    }
    if cfg!(target_os = "macos") && first.ends_with(".app") {
        return false;
    }
    !crate::terminal::which(first)
}

// ------------------------------------------------------------ the system's choice

/// The system's default app for a type, looked up in the background. `None` while
/// that first lookup runs; inner `None` when the system has none or will not say.
pub fn system_app(ctx: &egui::Context, ext: &str) -> Option<Option<String>> {
    /// How long an answer is shown before it is checked again (it can change in the system's settings).
    const FRESH: Duration = Duration::from_secs(30);
    struct Lookups {
        known: HashMap<String, (Instant, Option<String>)>,
        pending: HashSet<String>,
        tx: Sender<String>,
    }
    static LOOKUPS: OnceLock<Mutex<Lookups>> = OnceLock::new();
    let lookups = LOOKUPS.get_or_init(|| {
        let (tx, rx) = channel::<String>();
        let ctx = ctx.clone();
        let _ = std::thread::Builder::new().name("default-apps".into()).spawn(move || {
            while let Ok(ext) = rx.recv() {
                let app = system_app_name(&ext);
                if let Some(Ok(mut l)) = LOOKUPS.get().map(Mutex::lock) {
                    l.pending.remove(&ext);
                    l.known.insert(ext, (Instant::now(), app));
                }
                ctx.request_repaint();
            }
        });
        Mutex::new(Lookups { known: HashMap::new(), pending: HashSet::new(), tx })
    });
    let mut l = lookups.lock().ok()?;
    let answer = l.known.get(ext).cloned();
    if answer.as_ref().is_none_or(|(at, _)| at.elapsed() > FRESH) && l.pending.insert(ext.to_string()) {
        let _ = l.tx.send(ext.to_string());
    }
    answer.map(|(_, app)| app)
}

fn system_app_name(ext: &str) -> Option<String> {
    if cfg!(target_os = "macos") {
        macos_app(ext)
    } else if cfg!(windows) {
        windows_app(ext)
    } else {
        linux_app(ext)
    }
}

#[cfg(target_os = "macos")]
fn macos_app(ext: &str) -> Option<String> {
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};
    use std::ffi::{c_char, CStr, CString};
    // Launch Services answers for a file, so ask about an empty one of this type.
    let dir = std::env::temp_dir().join(format!("matrix_finder-probe-{}", std::process::id()));
    std::fs::create_dir_all(&dir).ok()?;
    let probe = dir.join(format!("probe.{ext}"));
    std::fs::write(&probe, b"").ok()?;
    let c_path = CString::new(probe.to_string_lossy().as_bytes()).ok()?;
    // SAFETY: plain Foundation/AppKit calls; every returned pointer is checked before use,
    // and the strings stay valid inside the pool.
    let app = objc2::rc::autoreleasepool(|_| unsafe {
        let ns_path: *mut AnyObject = msg_send![class!(NSString), stringWithUTF8String: c_path.as_ptr()];
        if ns_path.is_null() {
            return None;
        }
        let url: *mut AnyObject = msg_send![class!(NSURL), fileURLWithPath: ns_path];
        let workspace: *mut AnyObject = msg_send![class!(NSWorkspace), sharedWorkspace];
        if url.is_null() || workspace.is_null() {
            return None;
        }
        let app_url: *mut AnyObject = msg_send![workspace, URLForApplicationToOpenURL: url];
        if app_url.is_null() {
            return None;
        }
        let app_path: *mut AnyObject = msg_send![app_url, path];
        if app_path.is_null() {
            return None;
        }
        let bytes: *const c_char = msg_send![app_path, UTF8String];
        (!bytes.is_null()).then(|| CStr::from_ptr(bytes).to_string_lossy().into_owned())
    });
    let _ = std::fs::remove_file(&probe);
    let _ = std::fs::remove_dir(&dir);
    Path::new(&app?).file_stem().map(|s| s.to_string_lossy().into_owned())
}

#[cfg(not(target_os = "macos"))]
fn macos_app(_ext: &str) -> Option<String> {
    None
}

/// Run a helper program quietly and return what it printed.
fn output_of(program: &str, args: &[&str]) -> Option<String> {
    let mut c = Command::new(program);
    c.args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let out = c.output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

fn windows_app(ext: &str) -> Option<String> {
    // The user's choice, then the machine's association.
    let choice = format!("HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\FileExts\\.{ext}\\UserChoice");
    let prog_id = output_of("reg", &["query", &choice, "/v", "ProgId"])
        .and_then(|o| reg_value(&o))
        .or_else(|| output_of("reg", &["query", &format!("HKCR\\.{ext}"), "/ve"]).and_then(|o| reg_value(&o)))?;
    let command = output_of("reg", &["query", &format!("HKCR\\{prog_id}\\shell\\open\\command"), "/ve"]).and_then(|o| reg_value(&o));
    Some(command.and_then(|c| exe_stem(&c)).unwrap_or(prog_id))
}

/// The value in `reg query` output: `    ProgId    REG_SZ    AcroExch.Document`.
fn reg_value(output: &str) -> Option<String> {
    output.lines().find_map(|l| {
        let (_, rest) = l.split_once("REG_")?;
        let (_, value) = rest.split_once(char::is_whitespace)?;
        let value = value.trim();
        (!value.is_empty()).then(|| value.to_string())
    })
}

/// The program's name in a registry command: `"C:\Apps\foo.exe" "%1"` gives `foo`.
fn exe_stem(command: &str) -> Option<String> {
    let command = command.trim();
    let program = match command.strip_prefix('"') {
        Some(rest) => rest.split('"').next()?,
        None => command.split_whitespace().next()?,
    };
    let file = program.rsplit(['\\', '/']).next()?;
    let stem = file.rsplit_once('.').map_or(file, |(s, _)| s);
    (!stem.is_empty()).then(|| stem.to_string())
}

fn linux_app(ext: &str) -> Option<String> {
    let dirs = xdg_data_dirs();
    let mime = dirs.iter().find_map(|d| std::fs::read_to_string(d.join("mime/globs2")).ok().and_then(|g| mime_of(&g, ext)))?;
    let desktop = output_of("xdg-mime", &["query", "default", &mime])?;
    let desktop = desktop.trim();
    if desktop.is_empty() {
        return None;
    }
    let name = dirs.iter().find_map(|d| std::fs::read_to_string(d.join("applications").join(desktop)).ok().and_then(|f| desktop_name(&f)));
    Some(name.unwrap_or_else(|| desktop.trim_end_matches(".desktop").to_string()))
}

fn xdg_data_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = vec![];
    if let Some(home) = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).or_else(|| dirs::home_dir().map(|h| h.join(".local/share"))) {
        dirs.push(home);
    }
    let system = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    dirs.extend(system.split(':').filter(|d| !d.is_empty()).map(PathBuf::from));
    dirs
}

/// The MIME type of an extension in a `globs2` file (`50:application/pdf:*.pdf`, heaviest first).
fn mime_of(globs2: &str, ext: &str) -> Option<String> {
    let glob = format!("*.{ext}");
    globs2.lines().filter(|l| !l.starts_with('#')).find_map(|l| {
        let mut parts = l.split(':');
        let (_weight, mime, pattern) = (parts.next()?, parts.next()?, parts.next()?);
        pattern.eq_ignore_ascii_case(&glob).then(|| mime.to_string())
    })
}

/// The `Name=` of a `.desktop` file's main section.
fn desktop_name(file: &str) -> Option<String> {
    file.lines().skip_while(|l| l.trim() != "[Desktop Entry]").skip(1).take_while(|l| !l.starts_with('[')).find_map(|l| l.strip_prefix("Name=")).map(|n| n.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_types() {
        assert_eq!(normalize_ext(" *.PDF "), "pdf");
        assert_eq!(normalize_ext(".Tar.GZ"), "tar.gz");
        assert_eq!(type_of("Report.final.PDF").as_deref(), Some("pdf"));
        assert_eq!(type_of("Makefile"), None);
        assert_eq!(type_of(".zshrc"), None, "a hidden file has no extension");
        assert_eq!(type_of(".config.json").as_deref(), Some("json"));
    }

    #[test]
    fn chosen_apps_win_by_longest_extension() {
        let apps: BTreeMap<String, String> = [("gz".to_string(), "gunzip-app".to_string()), ("tar.gz".to_string(), "tar-app".to_string()), ("pdf".to_string(), "reader".to_string())].into();
        assert_eq!(program_for(&apps, "a.tar.gz"), Some("tar-app"));
        assert_eq!(program_for(&apps, "notes.GZ"), Some("gunzip-app"));
        assert_eq!(program_for(&apps, "x.y.PDF"), Some("reader"));
        assert_eq!(program_for(&apps, "photo.jpg"), None);
        assert_eq!(program_for(&apps, "pdf"), None, "a file called pdf is not of type pdf");
    }

    #[test]
    fn missing_programs() {
        assert!(program_missing("/no/such/program --flag"));
        assert!(program_missing("no-such-program-123"));
        assert!(!program_missing(""));
        if cfg!(unix) {
            assert!(!program_missing("sh -c true"));
            assert!(!program_missing("\"/bin/sh\""));
        }
    }

    #[test]
    fn system_answers_are_parsed() {
        let globs = "# comment\n80:application/x-foo:*.foo\n50:application/pdf:*.pdf\n10:text/plain:*.PDF\n";
        assert_eq!(mime_of(globs, "pdf").as_deref(), Some("application/pdf"));
        assert_eq!(mime_of(globs, "nope"), None);
        let desktop = "[Other]\nName=Wrong\n[Desktop Entry]\nType=Application\nName=Evince\nName[vi]=Khac\n[Desktop Action x]\nName=Later\n";
        assert_eq!(desktop_name(desktop).as_deref(), Some("Evince"));
        assert_eq!(reg_value("\r\nHKEY_CURRENT_USER\\x\r\n    ProgId    REG_SZ    MSEdgePDF\r\n\r\n").as_deref(), Some("MSEdgePDF"));
        assert_eq!(reg_value("HKEY\\x\r\n"), None);
        assert_eq!(exe_stem("\"C:\\Program Files\\Foo Bar\\foo.exe\" \"%1\"").as_deref(), Some("foo"));
        assert_eq!(exe_stem("C:\\Windows\\notepad.exe %1").as_deref(), Some("notepad"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_reports_its_default_app() {
        let app = macos_app("txt").expect("macOS has an app for text files");
        assert!(!app.is_empty() && !app.ends_with(".app"), "{app}");
        assert_eq!(macos_app("zzqqxx-no-such-type"), None);
    }
}
