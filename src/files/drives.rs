//! Drives for the sidebar: the mounted file systems, a watcher that keeps the list up
//! to date off the UI thread, and ejecting a drive.

use crate::i18n::t;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

/// A mounted file system.
#[derive(Clone, Debug, PartialEq)]
pub struct Drive {
    /// Mount point and volume name, e.g. "/Volumes/USB (USB)".
    pub label: String,
    /// What the sidebar shows: the volume's name ("Macintosh HD", "Windows (C:)").
    pub name: String,
    /// A mount of the system itself (macOS's Data volume, Linux's /boot): not shown in the
    /// sidebar, but still used for free space.
    pub system: bool,
    pub mount: PathBuf,
    pub total: u64,
    pub avail: u64,
    /// File system type ("apfs", "smbfs", "ext4", "NTFS"…).
    pub fs: String,
    /// The device ("/dev/sdb1" on Linux; the volume name on macOS).
    pub device: String,
    pub removable: bool,
    /// The sidebar offers to eject it.
    pub ejectable: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Os {
    Mac,
    Linux,
    Windows,
}

impl Os {
    pub fn current() -> Os {
        if cfg!(target_os = "macos") {
            Os::Mac
        } else if cfg!(windows) {
            Os::Windows
        } else {
            Os::Linux
        }
    }
}

/// The mounted drives, sorted by mount point.
pub fn list() -> Vec<Drive> {
    let os = Os::current();
    let labels = if os == Os::Linux { linux_labels() } else { vec![] };
    let disks = sysinfo::Disks::new_with_refreshed_list();
    let mut out: Vec<Drive> = disks
        .list()
        .iter()
        .map(|d| {
            let mount = d.mount_point().to_path_buf();
            let name = d.name().to_string_lossy().to_string();
            let label = if cfg!(windows) || name.is_empty() { mount.to_string_lossy().to_string() } else { format!("{} ({})", mount.to_string_lossy(), name) };
            let fs = d.file_system().to_string_lossy().to_string();
            let removable = d.is_removable();
            Drive {
                label,
                name: display_name(os, &mount, &name, removable, &labels),
                system: is_system(os, &mount, &fs),
                ejectable: is_ejectable(os, &mount, &fs, removable),
                mount,
                total: d.total_space(),
                avail: d.available_space(),
                fs,
                device: name,
                removable,
            }
        })
        .collect();
    out.sort_by(|a, b| a.mount.cmp(&b.mount));
    out.dedup_by(|a, b| a.mount == b.mount);
    unique_names(&mut out);
    if out.is_empty() {
        let mount = PathBuf::from("/");
        out.push(Drive {
            label: "/".into(),
            name: "/".into(),
            system: false,
            mount,
            total: 0,
            avail: 0,
            fs: String::new(),
            device: String::new(),
            removable: false,
            ejectable: false,
        });
    }
    out
}

/// The name the sidebar shows. `volume` is what the system calls it (the volume name;
/// the device on Linux); `labels` are Linux's (device, label) pairs.
pub fn display_name(os: Os, mount: &Path, volume: &str, removable: bool, labels: &[(String, String)]) -> String {
    let folder = || mount.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| mount.to_string_lossy().to_string());
    let volume = volume.trim();
    match os {
        Os::Mac if !volume.is_empty() => volume.to_string(),
        Os::Mac => folder(),
        Os::Linux => match labels.iter().find(|(dev, _)| dev == volume) {
            Some((_, label)) if !label.trim().is_empty() => label.clone(),
            _ if mount == Path::new("/") => t("Computer"),
            _ => folder(),
        },
        Os::Windows => {
            let name = if !volume.is_empty() {
                volume.to_string()
            } else if removable {
                t("USB Drive")
            } else {
                t("Local Disk")
            };
            match drive_letter(mount) {
                Some(l) => format!("{name} ({l}:)"),
                None => name,
            }
        }
    }
}

/// Linux's volume labels: (device, label) from /dev/disk/by-label. Empty elsewhere.
fn linux_labels() -> Vec<(String, String)> {
    let Ok(rd) = std::fs::read_dir("/dev/disk/by-label") else { return vec![] };
    rd.flatten()
        .filter_map(|e| {
            let device = std::fs::canonicalize(e.path()).ok()?.to_string_lossy().to_string();
            Some((device, unescape_label(&e.file_name().to_string_lossy())))
        })
        .collect()
}

/// A name from /dev/disk/by-label, where "My Disk" is "My\x20Disk".
pub fn unescape_label(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = |c: u8| (c as char).to_digit(16);
        if b[i] == b'\\' && b.get(i + 1) == Some(&b'x') {
            if let (Some(h), Some(l)) = (b.get(i + 2).and_then(|&c| hex(c)), b.get(i + 3).and_then(|&c| hex(c))) {
                out.push((h * 16 + l) as u8);
                i += 4;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

/// Whether a mount belongs to the system itself and is not shown as a drive.
pub fn is_system(os: Os, mount: &Path, fs: &str) -> bool {
    let under = |p: &str| mount.starts_with(p);
    match os {
        // The Data, VM, Preboot… volumes behind the one system disk.
        Os::Mac => under("/System/Volumes") || under("/private/var/vm"),
        // Boot partitions, snap and AppImage images, container layers.
        Os::Linux => under("/boot") || under("/snap") || under("/var/snap") || under("/var/lib/docker") || matches!(fs, "squashfs" | "overlay"),
        Os::Windows => false,
    }
}

/// Two shown drives with the same name get their mount point after it (the system's own
/// mounts, which are not shown, do not count).
pub fn unique_names(drives: &mut [Drive]) {
    let names: Vec<String> = drives.iter().filter(|d| !d.system).map(|d| d.name.clone()).collect();
    for d in drives.iter_mut().filter(|d| !d.system) {
        if names.iter().filter(|n| **n == d.name).count() > 1 {
            d.name = format!("{} ({})", d.name, d.mount.display());
        }
    }
}

/// The drives the sidebar lists, by the "Drives in the sidebar" setting.
pub fn sidebar(all: &[Drive], mode: crate::config::SidebarDrives) -> Vec<&Drive> {
    use crate::config::SidebarDrives;
    let shown: Vec<&Drive> = all.iter().filter(|d| !d.system).collect();
    match mode {
        SidebarDrives::Always => shown,
        SidebarDrives::WhenSeveral if shown.len() > 1 => shown,
        SidebarDrives::WhenSeveral | SidebarDrives::Never => vec![],
    }
}

/// Network file systems on macOS: unmounted, as there is no disk to eject.
const MAC_NETWORK_FS: &[&str] = &["smbfs", "nfs", "afpfs", "webdav", "cifs", "ftp"];

/// "E:\" and nothing else: a drive letter (Windows).
fn drive_letter(mount: &Path) -> Option<char> {
    let s = mount.to_string_lossy();
    let mut c = s.chars();
    match (c.next(), c.next(), c.next(), c.next()) {
        (Some(l), Some(':'), Some('\\'), None) if l.is_ascii_alphabetic() => Some(l.to_ascii_uppercase()),
        _ => None,
    }
}

/// Whether a drive can be ejected from the sidebar: external and network volumes, never
/// the system's own.
/// `_fs` is the file system type; network volumes follow the same rules.
pub fn is_ejectable(os: Os, mount: &Path, _fs: &str, removable: bool) -> bool {
    let depth = mount.components().count();
    match os {
        Os::Mac => mount.starts_with("/Volumes") && depth > 2,
        Os::Linux => {
            if mount == Path::new("/") {
                return false;
            }
            let desktop = (mount.starts_with("/media") && depth > 2) || (mount.starts_with("/run/media") && depth > 4);
            desktop || removable
        }
        Os::Windows => removable && drive_letter(mount).is_some(),
    }
}

/// The commands that eject `d`, tried in order until one works. Each is a program and
/// its arguments, run without a shell.
pub fn eject_commands(os: Os, d: &Drive) -> Vec<Vec<String>> {
    if !is_ejectable(os, &d.mount, &d.fs, d.removable) {
        return vec![];
    }
    let mp = d.mount.to_string_lossy().to_string();
    let cmd = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<String>>();
    match os {
        Os::Mac if MAC_NETWORK_FS.contains(&d.fs.to_lowercase().as_str()) => vec![cmd(&["diskutil", "unmount", &mp])],
        // Eject takes the whole disk away, as Finder does; a volume of an internal disk
        // cannot be ejected, only unmounted.
        Os::Mac => vec![cmd(&["diskutil", "eject", &mp]), cmd(&["diskutil", "unmount", &mp])],
        Os::Linux => {
            let mut v = vec![cmd(&["gio", "mount", "-e", &mp])];
            if d.device.starts_with("/dev/") {
                v.push(cmd(&["udisksctl", "unmount", "-b", &d.device]));
            }
            v.push(cmd(&["umount", &mp]));
            v
        }
        Os::Windows => {
            let Some(letter) = drive_letter(&d.mount) else { return vec![] };
            // The Eject of Explorer's context menu. The letter is the only thing put in.
            let script = format!(
                "$d = (New-Object -ComObject Shell.Application).Namespace(17).ParseName('{letter}:'); if ($d) {{ $d.InvokeVerb('Eject') }} else {{ exit 2 }}"
            );
            vec![cmd(&["powershell", "-NoProfile", "-NonInteractive", "-Command", &script])]
        }
    }
}

fn is_busy(output: &str) -> bool {
    let l = output.to_lowercase();
    l.contains("dissented by pid") || l.contains("target is busy") || l.contains("devicebusy") || l.contains("resource busy")
}

/// A short message for what an eject command printed when it failed.
pub fn friendly_error(output: &str, own_pid: u32) -> String {
    let lower = output.to_lowercase();
    if let Some(i) = lower.find("dissented by pid ") {
        // "dissented by PID 50149 (/bin/sleep)"
        let rest = &output[i + "dissented by pid ".len()..];
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        if let Ok(pid) = digits.parse::<u32>() {
            if pid == own_pid {
                return t("Matrix Finder is still reading files on this drive. Try again in a moment.");
            }
            let program = rest[digits.len()..]
                .trim_start()
                .strip_prefix('(')
                .and_then(|r| r.split(')').next())
                .map(|p| p.rsplit('/').next().unwrap_or(p).to_string())
                .filter(|p| !p.is_empty());
            return match program {
                Some(p) => format!("{}: {p} (PID {pid})", t("A program is using this drive")),
                None => format!("{}: PID {pid}", t("A program is using this drive")),
            };
        }
    }
    if is_busy(output) {
        return t("A program is using this drive.");
    }
    output.lines().map(str::trim).find(|l| !l.is_empty()).map(str::to_string).unwrap_or_else(|| t("The drive could not be ejected."))
}

/// Whether `mount` is the root file system ("/"), which holds every path. A Windows
/// drive letter (`E:\`) has no parent either, but is a drive of its own.
fn is_root(mount: &Path) -> bool {
    mount.parent().is_none() && drive_letter(mount).is_none()
}

/// Whether `path` is on the drive mounted at `mount` (never for the root file system).
pub fn on_drive(path: &Path, mount: &Path) -> bool {
    !is_root(mount) && path.starts_with(mount)
}

/// Whether two lists have the same drives (free space aside).
pub fn same_drives(a: &[Drive], b: &[Drive]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(x, y)| {
            x.mount == y.mount && x.label == y.label && x.name == y.name && x.system == y.system && x.ejectable == y.ejectable && x.total == y.total
        })
}

/// Mount points in `old` that are gone from `new`.
pub fn removed(old: &[Drive], new: &[Drive]) -> Vec<PathBuf> {
    old.iter().filter(|o| !is_root(&o.mount) && !new.iter().any(|n| n.mount == o.mount)).map(|o| o.mount.clone()).collect()
}

/// The outcome of running one command: it ran and exited (`Ok((success, output))`), or it
/// could not be started (e.g. the program is not installed).
pub type RunResult = std::io::Result<(bool, String)>;

/// Ejects `d` with `commands`, run by `run`; `mounted` tells whether the drive is still
/// mounted after a command reported success.
pub fn eject_with(d: &Drive, commands: Vec<Vec<String>>, mut run: impl FnMut(&[String]) -> RunResult, mut mounted: impl FnMut() -> bool) -> Result<(), String> {
    if commands.is_empty() {
        return Err(format!("{}: {}", t("This drive cannot be ejected"), d.mount.display()));
    }
    let (mut failures, mut missing) = (Vec::new(), 0);
    for c in &commands {
        match run(c) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => missing += 1,
            Err(e) => failures.push(e.to_string()),
            Ok((true, _)) if !mounted() => return Ok(()),
            // It said yes, yet the drive is still there: the next command.
            Ok((true, _)) => {}
            Ok((false, out)) => failures.push(out),
        }
    }
    if missing == commands.len() {
        return Err(t("No program to eject drives was found."));
    }
    let pid = std::process::id();
    if let Some(busy) = failures.iter().find(|o| is_busy(o)) {
        return Err(friendly_error(busy, pid));
    }
    if let Some(first) = failures.first() {
        return Err(friendly_error(first, pid));
    }
    Err(t("The drive is still mounted."))
}

/// Whether something is still mounted at `mount`.
fn still_mounted(mount: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let parent = mount.parent().unwrap_or(mount);
        match (std::fs::metadata(mount), std::fs::metadata(parent)) {
            (Ok(m), Ok(p)) => m.dev() != p.dev(),
            _ => false,
        }
    }
    #[cfg(not(unix))]
    {
        mount.exists()
    }
}

/// Runs a command without a shell; gives up after a minute.
fn run_command(c: &[String]) -> RunResult {
    use std::io::Read;
    let (prog, args) = c.split_first().ok_or(std::io::ErrorKind::InvalidInput)?;
    let mut cmd = std::process::Command::new(prog);
    cmd.args(args).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = cmd.spawn()?;
    let pipe = |r: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut s = String::new();
            if let Some(r) = r {
                let mut buf = Vec::new();
                let _ = r.take(64 * 1024).read_to_end(&mut buf);
                s = String::from_utf8_lossy(&buf).to_string();
            }
            s
        })
    };
    let out = pipe(child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
    let err = pipe(child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
    let start = Instant::now();
    let status = loop {
        if let Some(s) = child.try_wait()? {
            break Some(s);
        }
        if start.elapsed() > Duration::from_secs(60) {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let text = format!("{}\n{}", err.join().unwrap_or_default(), out.join().unwrap_or_default());
    match status {
        Some(s) => Ok((s.success(), text)),
        None => Ok((false, t("The drive did not answer in time."))),
    }
}

/// Ejects `d`: runs the system's commands and waits until it is gone. Blocks; call it
/// off the UI thread.
pub fn eject(d: &Drive) -> Result<(), String> {
    let mount = d.mount.clone();
    eject_with(d, eject_commands(Os::current(), d), run_command, || {
        // Windows ejects in the background: give it a few seconds.
        let start = Instant::now();
        while still_mounted(&mount) {
            if start.elapsed() > Duration::from_secs(10) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        false
    })
}

/// Keeps the drive list up to date on a thread: a drive that is plugged in, ejected or
/// unmounted elsewhere shows up within a couple of seconds, and the UI never waits for
/// a slow mount.
pub struct Watcher {
    rx: Receiver<Vec<Drive>>,
    wake: Sender<()>,
}

impl Watcher {
    pub fn spawn(ctx: &egui::Context) -> Watcher {
        Watcher::spawn_with(list, Duration::from_secs(2), Duration::from_secs(10), Some(ctx.clone()))
    }

    /// With its own lister and timings (tests). A changed list is sent at once; free
    /// space only every `space_every`.
    pub fn spawn_with(lister: impl Fn() -> Vec<Drive> + Send + 'static, every: Duration, space_every: Duration, ctx: Option<egui::Context>) -> Watcher {
        let (tx, rx) = channel();
        let (wake, woken) = channel::<()>();
        std::thread::spawn(move || {
            let mut last: Option<Vec<Drive>> = None;
            let mut sent = Instant::now();
            let mut forced = false;
            loop {
                let now = lister();
                let changed = last.as_ref().is_none_or(|l| !same_drives(l, &now));
                let space = sent.elapsed() >= space_every && last.as_ref() != Some(&now);
                if changed || space || forced {
                    if tx.send(now.clone()).is_err() {
                        return;
                    }
                    sent = Instant::now();
                    if let Some(c) = &ctx {
                        c.request_repaint();
                    }
                }
                last = Some(now);
                forced = match woken.recv_timeout(every) {
                    Ok(()) => {
                        while woken.try_recv().is_ok() {}
                        true
                    }
                    Err(RecvTimeoutError::Timeout) => false,
                    Err(RecvTimeoutError::Disconnected) => return,
                };
            }
        });
        Watcher { rx, wake }
    }

    /// The newest list, if it changed since the last call.
    pub fn poll(&self) -> Option<Vec<Drive>> {
        self.rx.try_iter().last()
    }

    /// Read the drives again now (after an eject, or on Refresh).
    pub fn refresh(&self) {
        let _ = self.wake.send(());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    fn drive(mount: &str, fs: &str, device: &str, removable: bool, os: Os) -> Drive {
        let mount = PathBuf::from(mount);
        Drive {
            label: mount.to_string_lossy().to_string(),
            name: mount.file_name().map_or("/".into(), |n| n.to_string_lossy().to_string()),
            system: is_system(os, &mount, fs),
            ejectable: is_ejectable(os, &mount, fs, removable),
            mount,
            total: 100,
            avail: 50,
            fs: fs.into(),
            device: device.into(),
            removable,
        }
    }

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn which_drives_can_be_ejected() {
        let p = Path::new;
        // macOS: volumes under /Volumes, never the system's.
        for (mount, fs, removable, yes) in [
            ("/", "apfs", false, false),
            ("/System/Volumes/Data", "apfs", false, false),
            ("/Volumes", "apfs", false, false),
            ("/Volumes/MFTest", "hfs", true, true),
            ("/Volumes/Backup", "apfs", false, true),
            ("/Volumes/Share", "smbfs", false, true),
            ("/Users/me/remote", "macfuse", false, false),
        ] {
            assert_eq!(is_ejectable(Os::Mac, p(mount), fs, removable), yes, "mac {mount}");
        }
        // Linux: removable drives, and what the desktop mounted under /media or /run/media.
        for (mount, fs, removable, yes) in [
            ("/", "ext4", false, false),
            ("/home", "ext4", false, false),
            ("/boot/efi", "vfat", false, false),
            ("/media", "ext4", false, false),
            ("/media/me/USB", "vfat", false, true),
            ("/run/media/me/USB", "exfat", false, true),
            ("/mnt/usb", "vfat", true, true),
            ("/mnt/data", "ext4", false, false),
        ] {
            assert_eq!(is_ejectable(Os::Linux, p(mount), fs, removable), yes, "linux {mount}");
        }
        // Windows: removable drives with a drive letter.
        for (mount, removable, yes) in
            [("C:\\", false, false), ("E:\\", true, true), ("D:\\", false, false), ("\\\\?\\Volume{1234}\\", true, false), ("E:\\x\\", true, false)]
        {
            assert_eq!(is_ejectable(Os::Windows, p(mount), "NTFS", removable), yes, "windows {mount}");
        }
    }

    #[test]
    fn eject_commands_per_system() {
        // macOS: eject (the whole disk, as Finder does), else unmount (a volume of an
        // internal disk); network volumes are unmounted. One argument each, spaces and all.
        let d = drive("/Volumes/My Disk", "hfs", "My Disk", true, Os::Mac);
        assert_eq!(eject_commands(Os::Mac, &d), [strings(&["diskutil", "eject", "/Volumes/My Disk"]), strings(&["diskutil", "unmount", "/Volumes/My Disk"])]);
        let d = drive("/Volumes/Share", "smbfs", "//me@nas/Share", false, Os::Mac);
        assert_eq!(eject_commands(Os::Mac, &d), [strings(&["diskutil", "unmount", "/Volumes/Share"])]);
        // Linux: the desktop's way first, then udisks with the device, then umount.
        let d = drive("/media/me/USB", "vfat", "/dev/sdb1", true, Os::Linux);
        assert_eq!(
            eject_commands(Os::Linux, &d),
            [strings(&["gio", "mount", "-e", "/media/me/USB"]), strings(&["udisksctl", "unmount", "-b", "/dev/sdb1"]), strings(&["umount", "/media/me/USB"])]
        );
        let d = drive("/media/me/Phone", "fuse", "jmtpfs", false, Os::Linux);
        assert_eq!(eject_commands(Os::Linux, &d), [strings(&["gio", "mount", "-e", "/media/me/Phone"]), strings(&["umount", "/media/me/Phone"])]);
        // Windows: the shell's Eject verb on the drive letter, and nothing else in the script.
        let d = drive("E:\\", "FAT32", "USB", true, Os::Windows);
        let c = eject_commands(Os::Windows, &d);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0][0], "powershell");
        let script = c[0].last().unwrap();
        assert!(script.contains("ParseName('E:')"), "{script}");
        assert_eq!(script.matches('\'').count(), 4, "only the drive letter and the verb are quoted: {script}");
        // Never for a drive that cannot be ejected.
        for (os, d) in [
            (Os::Mac, drive("/", "apfs", "Macintosh HD", false, Os::Mac)),
            (Os::Linux, drive("/home", "ext4", "/dev/sda2", false, Os::Linux)),
            (Os::Windows, drive("C:\\", "NTFS", "", false, Os::Windows)),
        ] {
            assert!(eject_commands(os, &d).is_empty(), "{os:?} {}", d.mount.display());
        }
    }

    #[test]
    fn eject_errors_in_plain_words() {
        // What diskutil prints when a program has a file or folder open there.
        let busy = "Unmount of disk4 failed: at least one volume could not be unmounted\nUnmount was dissented by PID 50149 (/bin/sleep)\nDissenter parent PPID 1 (/sbin/launchd)\n";
        let e = friendly_error(busy, 1);
        assert!(e.contains("sleep") && e.contains("50149"), "{e}");
        let e = friendly_error("Volume MFTest on disk4s1 failed to unmount: dissented by PID 77 (/bin/zsh)", 1);
        assert!(e.contains("zsh") && e.contains("77"), "{e}");
        // This app itself still reading something there.
        let e = friendly_error("Unmount was dissented by PID 4242 (/Applications/Matrix Finder.app/Contents/MacOS/matrix_finder)", 4242);
        assert_eq!(e, crate::i18n::t("Matrix Finder is still reading files on this drive. Try again in a moment."));
        // Linux.
        for out in [
            "umount: /media/me/USB: target is busy.",
            "Error unmounting /dev/sdb1: GDBus.Error:org.freedesktop.UDisks2.Error.DeviceBusy: Error unmounting /dev/sdb1: target is busy",
        ] {
            assert_eq!(friendly_error(out, 1), crate::i18n::t("A program is using this drive."), "{out}");
        }
        // Anything else: its first line.
        assert_eq!(friendly_error("\nFailed to find disk /Volumes/NoSuch\nmore", 1), "Failed to find disk /Volumes/NoSuch");
        assert_eq!(friendly_error("  \n", 1), crate::i18n::t("The drive could not be ejected."));
    }

    #[test]
    fn paths_on_a_drive() {
        let p = Path::new;
        assert!(on_drive(p("/Volumes/A/x/y"), p("/Volumes/A")));
        assert!(on_drive(p("/Volumes/A"), p("/Volumes/A")));
        assert!(!on_drive(p("/Volumes/AB/x"), p("/Volumes/A")), "whole names only");
        assert!(!on_drive(p("/Volumes/B"), p("/Volumes/A")));
        assert!(!on_drive(p("/Users/me"), p("/")), "the root holds everything; it is never ejected");
        // Windows: a drive letter is a drive of its own (its path has no parent).
        #[cfg(windows)]
        {
            assert!(on_drive(p("E:\\photos\\a.jpg"), p("E:\\")));
            assert!(!on_drive(p("C:\\Users"), p("E:\\")));
        }
    }

    #[test]
    fn comparing_lists() {
        let a = vec![drive("/", "apfs", "", false, Os::Mac), drive("/Volumes/USB", "hfs", "USB", true, Os::Mac)];
        let mut b = a.clone();
        b[1].avail = 1;
        assert!(same_drives(&a, &b), "free space alone");
        assert!(removed(&a, &b).is_empty());
        let c = vec![a[0].clone()];
        assert!(!same_drives(&a, &c));
        assert_eq!(removed(&a, &c), [PathBuf::from("/Volumes/USB")]);
        assert!(removed(&c, &a).is_empty(), "added, not removed");
        let mut d = a.clone();
        d[1].label = "/Volumes/USB (Renamed)".into();
        assert!(!same_drives(&a, &d), "renamed");
        // The root is never reported as removed.
        assert!(removed(&a, &[]).iter().all(|m| m != Path::new("/")));
        // A USB stick pulled out on Windows is.
        #[cfg(windows)]
        {
            let c = drive("C:\\", "NTFS", "", false, Os::Windows);
            let e = drive("E:\\", "FAT32", "", true, Os::Windows);
            assert_eq!(removed(&[c.clone(), e], &[c]), [PathBuf::from("E:\\")]);
        }
    }

    /// The commands a pretend system was asked to run.
    type Ran = Arc<Mutex<Vec<String>>>;

    /// A pretend system for `eject_with`: each command's result in turn, and the commands run.
    fn fake(results: Vec<RunResult>) -> (impl FnMut(&[String]) -> RunResult, Ran) {
        let ran = Arc::new(Mutex::new(Vec::new()));
        let r2 = ran.clone();
        let mut results = results.into_iter();
        (
            move |cmd: &[String]| {
                r2.lock().unwrap().push(cmd.join(" "));
                results.next().unwrap_or(Ok((false, "unexpected".into())))
            },
            ran,
        )
    }

    fn usb() -> Drive {
        drive("/Volumes/USB", "hfs", "USB", true, Os::Mac)
    }

    #[test]
    fn eject_tries_commands_in_order() {
        let cmds = eject_commands(Os::Mac, &usb());
        // The first one works and the drive is gone: nothing else runs.
        let (run, ran) = fake(vec![Ok((true, "Disk /Volumes/USB ejected".into()))]);
        assert_eq!(eject_with(&usb(), cmds.clone(), run, || false), Ok(()));
        assert_eq!(ran.lock().unwrap().len(), 1);
        // The first fails, the second works.
        let (run, ran) = fake(vec![Ok((false, "Unable to eject".into())), Ok((true, "Volume USB unmounted".into()))]);
        assert_eq!(eject_with(&usb(), cmds.clone(), run, || false), Ok(()));
        assert_eq!(*ran.lock().unwrap(), ["diskutil eject /Volumes/USB", "diskutil unmount /Volumes/USB"]);
        // A program that is not installed: the next one.
        let (run, _) = fake(vec![Err(std::io::ErrorKind::NotFound.into()), Ok((true, String::new()))]);
        assert_eq!(eject_with(&usb(), cmds.clone(), run, || false), Ok(()));
        // None installed.
        let (run, _) = fake(vec![Err(std::io::ErrorKind::NotFound.into()), Err(std::io::ErrorKind::NotFound.into())]);
        assert_eq!(eject_with(&usb(), cmds.clone(), run, || false), Err(crate::i18n::t("No program to eject drives was found.")));
        // All fail: the busy message wins over a vaguer one.
        let (run, _) =
            fake(vec![Ok((false, "Unmount was dissented by PID 9 (/bin/zsh)".into())), Ok((false, "Volume USB on disk4s1 failed to unmount".into()))]);
        let e = eject_with(&usb(), cmds.clone(), run, || true).unwrap_err();
        assert!(e.contains("zsh"), "{e}");
        let (run, _) = fake(vec![Ok((false, "Unable to eject".into())), Ok((false, "Volume USB on disk4s1 failed to unmount".into()))]);
        assert_eq!(eject_with(&usb(), cmds.clone(), run, || true), Err("Unable to eject".to_string()));
        // Success reported, but still mounted: the next command, then an honest error.
        let (run, ran) = fake(vec![Ok((true, String::new())), Ok((true, String::new()))]);
        assert_eq!(eject_with(&usb(), cmds.clone(), run, || true), Err(crate::i18n::t("The drive is still mounted.")));
        assert_eq!(ran.lock().unwrap().len(), 2);
        // A drive that cannot be ejected: nothing runs.
        let root = drive("/", "apfs", "", false, Os::Mac);
        let (run, ran) = fake(vec![]);
        assert!(eject_with(&root, vec![], run, || true).is_err());
        assert!(ran.lock().unwrap().is_empty());
    }

    #[test]
    fn watcher_follows_changes() {
        let now = Arc::new(Mutex::new(vec![drive("/", "apfs", "", false, Os::Mac)]));
        let calls = Arc::new(Mutex::new(0usize));
        let (n2, c2) = (now.clone(), calls.clone());
        let w = Watcher::spawn_with(
            move || {
                *c2.lock().unwrap() += 1;
                n2.lock().unwrap().clone()
            },
            Duration::from_millis(50),
            Duration::from_secs(3600),
            None,
        );
        let wait = |w: &Watcher| {
            let start = Instant::now();
            while start.elapsed() < Duration::from_secs(5) {
                if let Some(v) = w.poll() {
                    return Some(v);
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            None
        };
        // The first list, then nothing while nothing changes (free space aside).
        assert_eq!(wait(&w).unwrap().len(), 1);
        now.lock().unwrap()[0].avail = 7;
        std::thread::sleep(Duration::from_millis(200));
        assert!(w.poll().is_none());
        // A drive comes and goes.
        now.lock().unwrap().push(usb());
        assert_eq!(wait(&w).unwrap().len(), 2);
        now.lock().unwrap().pop();
        assert_eq!(wait(&w).unwrap().len(), 1);
        // Refresh reads at once, without waiting for the next turn.
        let w = Watcher::spawn_with(
            {
                let n = now.clone();
                move || n.lock().unwrap().clone()
            },
            Duration::from_secs(3600),
            Duration::from_secs(3600),
            None,
        );
        assert!(wait(&w).is_some());
        now.lock().unwrap().push(usb());
        w.refresh();
        let start = Instant::now();
        assert_eq!(wait(&w).unwrap().len(), 2);
        assert!(start.elapsed() < Duration::from_secs(2));
        // The thread ends with the watcher.
        let before = *calls.lock().unwrap();
        drop(w);
        assert!(before > 0);
    }

    #[test]
    fn lists_this_system() {
        let v = list();
        assert!(!v.is_empty());
        assert!(v.windows(2).all(|w| w[0].mount < w[1].mount), "sorted, no duplicates");
        for d in &v {
            assert_eq!(d.ejectable, is_ejectable(Os::current(), &d.mount, &d.fs, d.removable));
            assert_eq!(d.system, is_system(Os::current(), &d.mount, &d.fs));
            assert!(!d.name.is_empty());
        }
        if cfg!(unix) {
            assert!(v.iter().any(|d| d.mount == Path::new("/") && !d.ejectable));
        }
    }

    #[test]
    fn drive_names() {
        let p = Path::new;
        let none: &[(String, String)] = &[];
        // macOS: the volume's name.
        assert_eq!(display_name(Os::Mac, p("/"), "Macintosh HD", false, none), "Macintosh HD");
        assert_eq!(display_name(Os::Mac, p("/Volumes/USB"), "USB", true, none), "USB");
        assert_eq!(display_name(Os::Mac, p("/Volumes/Untitled 1"), "", true, none), "Untitled 1", "no name: the folder");
        assert_eq!(display_name(Os::Mac, p("/"), "", false, none), "/");
        // Linux: the label of the device, else the folder; the root is the computer.
        let labels = vec![("/dev/sdb1".to_string(), "KINGSTON".to_string())];
        assert_eq!(display_name(Os::Linux, p("/media/me/sdb1"), "/dev/sdb1", true, &labels), "KINGSTON");
        assert_eq!(display_name(Os::Linux, p("/media/me/USB"), "/dev/sdc1", true, &labels), "USB");
        assert_eq!(display_name(Os::Linux, p("/"), "/dev/sda2", false, &labels), crate::i18n::t("Computer"));
        assert_eq!(display_name(Os::Linux, p("/home"), "/dev/sda3", false, none), "home");
        // Windows: as Explorer shows them.
        assert_eq!(display_name(Os::Windows, p("C:\\"), "Windows", false, none), "Windows (C:)");
        assert_eq!(display_name(Os::Windows, p("E:\\"), "", true, none), format!("{} (E:)", crate::i18n::t("USB Drive")));
        assert_eq!(display_name(Os::Windows, p("D:\\"), "", false, none), format!("{} (D:)", crate::i18n::t("Local Disk")));
        // Labels as /dev/disk/by-label writes them.
        assert_eq!(unescape_label("My\\x20Disk"), "My Disk");
        assert_eq!(unescape_label("D\\xc3\\xa9j\\xc3\\xa0"), "Déjà");
        assert_eq!(unescape_label("plain"), "plain");
        assert_eq!(unescape_label("bad\\xZZ"), "bad\\xZZ");
    }

    #[test]
    fn system_mounts_are_not_drives() {
        let p = Path::new;
        for (mount, fs, yes) in [
            ("/", "apfs", false),
            ("/System/Volumes/Data", "apfs", true),
            ("/System/Volumes/VM", "apfs", true),
            ("/private/var/vm", "apfs", true),
            ("/Volumes/USB", "hfs", false),
            ("/Volumes/Recovery", "apfs", false),
        ] {
            assert_eq!(is_system(Os::Mac, p(mount), fs), yes, "mac {mount}");
        }
        for (mount, fs, yes) in [
            ("/", "ext4", false),
            ("/home", "ext4", false),
            ("/boot", "ext4", true),
            ("/boot/efi", "vfat", true),
            ("/snap/core22/1380", "squashfs", true),
            ("/opt/app", "squashfs", true),
            ("/media/me/USB", "vfat", false),
            ("/var/lib/docker/overlay2/x/merged", "overlay", true),
        ] {
            assert_eq!(is_system(Os::Linux, p(mount), fs), yes, "linux {mount}");
        }
        assert!(!is_system(Os::Windows, p("C:\\"), "NTFS"));
    }

    #[test]
    fn same_names_are_told_apart() {
        let mut v = vec![
            drive("/Volumes/Untitled", "hfs", "", true, Os::Mac),
            drive("/Volumes/Untitled 1", "hfs", "", true, Os::Mac),
            drive("/Volumes/USB", "hfs", "", true, Os::Mac),
        ];
        for d in &mut v[..2] {
            d.name = "Untitled".into();
        }
        unique_names(&mut v);
        assert_eq!(v.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(), ["Untitled (/Volumes/Untitled)", "Untitled (/Volumes/Untitled 1)", "USB"]);
        // The system's own mounts are not shown, so they take no part: macOS's Data
        // volume has the system disk's name too.
        let mut v = vec![drive("/", "apfs", "", false, Os::Mac), drive("/System/Volumes/Data", "apfs", "", false, Os::Mac)];
        for d in &mut v {
            d.name = "Macintosh HD".into();
        }
        unique_names(&mut v);
        assert_eq!(v[0].name, "Macintosh HD");
    }

    #[test]
    fn drives_section_setting() {
        use crate::config::SidebarDrives;
        let mounts = |v: Vec<&Drive>| v.into_iter().map(|d| d.mount.to_string_lossy().to_string()).collect::<Vec<_>>();
        let one = vec![drive("/", "apfs", "", false, Os::Mac), drive("/System/Volumes/Data", "apfs", "", false, Os::Mac)];
        let two = vec![one[0].clone(), one[1].clone(), usb()];
        // By default only with more than one drive; the system's own mounts do not count.
        assert_eq!(SidebarDrives::default(), SidebarDrives::WhenSeveral);
        assert!(sidebar(&one, SidebarDrives::WhenSeveral).is_empty());
        assert_eq!(mounts(sidebar(&two, SidebarDrives::WhenSeveral)), ["/", "/Volumes/USB"]);
        assert_eq!(mounts(sidebar(&one, SidebarDrives::Always)), ["/"]);
        assert!(sidebar(&two, SidebarDrives::Never).is_empty());
    }

    /// Mount a small disk image, eject it while a program has its folder open (refused,
    /// with that program named), then eject it. macOS only; it really mounts a disk.
    #[test]
    #[ignore]
    fn live_eject_disk_image() {
        if Os::current() != Os::Mac {
            return;
        }
        let dir = crate::util::TempDir::new().unwrap();
        let img = dir.path().join("t.dmg");
        let name = format!("MFEject{}", std::process::id());
        let ok = std::process::Command::new("hdiutil").args(["create", "-size", "4m", "-fs", "HFS+", "-volname", &name]).arg(&img).output().unwrap();
        assert!(ok.status.success());
        let ok = std::process::Command::new("hdiutil").arg("attach").arg(&img).output().unwrap();
        assert!(ok.status.success());
        let mount = PathBuf::from("/Volumes").join(&name);
        let find = || list().into_iter().find(|d| d.mount == mount);
        let d = find().expect("listed once mounted");
        assert!(d.ejectable && d.removable);

        let mut busy = std::process::Command::new("sleep").arg("30").current_dir(&mount).spawn().unwrap();
        let e = eject(&d).unwrap_err();
        assert!(e.contains("sleep"), "{e}");
        assert!(find().is_some(), "still mounted");
        busy.kill().unwrap();
        busy.wait().unwrap();

        assert_eq!(eject(&d), Ok(()));
        assert!(find().is_none());
        assert!(!mount.exists());
    }
}
