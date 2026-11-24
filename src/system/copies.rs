//! Linux: a copy installed with `scripts/install.sh` (in `~/.local` or `/usr/local`)
//! running while the .deb or .rpm package installed another one in `/usr`. The
//! script's copy comes first on PATH and in the applications menu, so the package's
//! copy, and every update of it, would never start; the app says so at start.
//!
//! Only reads: whether files exist, and the package copy's `--version`.
use std::path::{Path, PathBuf};

/// Where the .deb and .rpm packages put the program.
pub const PACKAGE_BIN: &str = "/usr/bin/matrix_finder";

/// The raw address of the install script, for the command that removes its copy.
const SCRIPT_URL: &str = "https://raw.githubusercontent.com/quangtiencs/matrix_finder/main/scripts/install.sh";

/// The script's copy that is running, and the package's.
#[derive(Debug, PartialEq)]
pub struct ScriptOverPackage {
    /// The running program, in `<prefix>/bin`.
    pub running: PathBuf,
    /// The script's install prefix (`~/.local`, `/usr/local`, or a `--prefix`).
    pub prefix: PathBuf,
    /// The package's program (`/usr/bin/matrix_finder`).
    pub package: PathBuf,
}

/// The running program is the install script's copy and the package's copy exists too.
pub fn script_over_package() -> Option<ScriptOverPackage> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let exe = std::env::current_exe().ok()?;
    check(&exe, Path::new(PACKAGE_BIN))
}

fn check(exe: &Path, package: &Path) -> Option<ScriptOverPackage> {
    if exe == package || !package.is_file() {
        return None;
    }
    let bin = exe.parent()?;
    if bin.file_name()? != "bin" {
        return None;
    }
    let prefix = bin.parent()?;
    // Written by the install script next to what it installed; a build run from a
    // source folder (cargo run) has none.
    prefix.join("share/matrix_finder/install-manifest.txt").is_file().then(|| ScriptOverPackage {
        running: exe.to_path_buf(),
        prefix: prefix.to_path_buf(),
        package: package.to_path_buf(),
    })
}

impl ScriptOverPackage {
    /// The package copy's version ("matrix_finder 0.14.0"), if it tells.
    pub fn package_version(&self) -> Option<String> {
        let out = std::process::Command::new(&self.package).arg("--version").output().ok()?;
        let v = String::from_utf8_lossy(&out.stdout).trim().to_string();
        (out.status.success() && !v.is_empty()).then_some(v)
    }

    /// The command that removes the script's copy (settings are kept).
    pub fn uninstall_command(&self) -> String {
        let home_local = dirs::home_dir().map(|h| h.join(".local"));
        let args = if self.prefix == Path::new("/usr/local") {
            return format!("curl -fsSL {SCRIPT_URL} | sudo bash -s -- --system --uninstall");
        } else if home_local.as_deref() == Some(self.prefix.as_path()) {
            "--uninstall".to_string()
        } else {
            format!("--prefix {} --uninstall", crate::terminal::sh_quote(&self.prefix.to_string_lossy()))
        };
        format!("curl -fsSL {SCRIPT_URL} | bash -s -- {args}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_script_copy_over_the_package() {
        let tmp = crate::util::TempDir::new().unwrap();
        let root = tmp.path();
        let package = root.join("usr/bin/matrix_finder");
        let prefix = root.join("home/me/.local");
        let exe = prefix.join("bin/matrix_finder");
        for f in [&package, &exe] {
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(f, b"").unwrap();
        }
        // No manifest: a build from a source folder, not the script's copy.
        assert_eq!(check(&exe, &package), None);
        let manifest = prefix.join("share/matrix_finder/install-manifest.txt");
        std::fs::create_dir_all(manifest.parent().unwrap()).unwrap();
        std::fs::write(&manifest, b"# Matrix Finder install manifest\n").unwrap();
        let found = check(&exe, &package).expect("the script's copy");
        assert_eq!((found.running.as_path(), found.prefix.as_path()), (exe.as_path(), prefix.as_path()));
        // The package's copy running, or no package: nothing to say.
        assert_eq!(check(&package, &package), None);
        assert_eq!(check(&exe, &root.join("usr/bin/none")), None);
        // Not in a bin folder.
        let elsewhere = prefix.join("target/matrix_finder");
        assert_eq!(check(&elsewhere, &package), None);
    }

    #[test]
    fn uninstall_commands() {
        let copy = |prefix: &str| ScriptOverPackage { running: PathBuf::new(), prefix: PathBuf::from(prefix), package: PathBuf::from(PACKAGE_BIN) };
        assert!(copy("/usr/local").uninstall_command().ends_with("| sudo bash -s -- --system --uninstall"));
        assert!(copy("/opt/my apps").uninstall_command().ends_with("| bash -s -- --prefix '/opt/my apps' --uninstall"));
        if let Some(home) = dirs::home_dir() {
            assert!(copy(&home.join(".local").to_string_lossy()).uninstall_command().ends_with("| bash -s -- --uninstall"));
        }
    }
}
