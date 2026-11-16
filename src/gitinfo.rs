//! Git repositories in the file list: which folders are one, their current branch,
//! and whether everything in them is committed.
//!
//! The branch is read from `.git/HEAD`, which runs nothing. Whether there are changes
//! needs `git status`, and a repository's own config can make git run a program of its
//! choice (`core.fsmonitor`, filter drivers): a folder unpacked from the internet would
//! then run code just by being shown. So git runs with those turned off, without going
//! into submodules (each its own config) and without writing to the repository.
//!
//! All of it happens on background threads, cached per folder, and only for folders
//! that are on screen; the list asks every frame and gets what is known so far.
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Condvar, LazyLock, Mutex};
use std::time::{Duration, Instant};

/// How often a shown folder is looked at again for a new branch, and for changes.
const HEAD_TTL: Duration = Duration::from_secs(3);
const STATUS_TTL: Duration = Duration::from_secs(10);
/// A `git status` taking longer than this is stopped; the folder is then left alone a while.
const STATUS_TIMEOUT: Duration = Duration::from_secs(10);
const SLOW_RETRY: Duration = Duration::from_secs(120);
/// Folders waiting to be looked at; the most recently shown go first.
const MAX_QUEUE: usize = 256;
const WORKERS: usize = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// Not known (yet): git is missing, still checking or too slow.
    Unknown,
    /// Everything is committed.
    Clean,
    /// Changed, staged or new files.
    Dirty,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Repo {
    /// The branch, or the commit (short) when no branch is checked out.
    pub branch: String,
    pub detached: bool,
    pub state: State,
    /// Changed or staged files, and files not tracked yet.
    pub changed: u32,
    pub untracked: u32,
    /// Commits not yet pushed, and not yet pulled (when there is an upstream).
    pub ahead: u32,
    pub behind: u32,
}

#[derive(Default)]
struct Slot {
    /// `None` when the folder is not a repository.
    repo: Option<Repo>,
    head_at: Option<Instant>,
    status_at: Option<Instant>,
    queued: bool,
}

#[derive(Clone, Copy, PartialEq)]
struct Job {
    status: bool,
}

struct Shared {
    slots: HashMap<PathBuf, Slot>,
    queue: Vec<(PathBuf, Job)>,
    ctx: Option<egui::Context>,
    started: bool,
}

static SHARED: LazyLock<(Mutex<Shared>, Condvar)> = LazyLock::new(|| (Mutex::new(Shared { slots: HashMap::new(), queue: vec![], ctx: None, started: false }), Condvar::new()));
/// The `git` program, found once.
static GIT: LazyLock<Option<PathBuf>> = LazyLock::new(|| crate::media::find_tool("git"));

/// What is known of `dir` (a local folder on screen); asks for it to be looked at if
/// that is due. `status`: also find out whether everything is committed.
pub fn lookup(ctx: &egui::Context, dir: &Path, status: bool) -> Option<Repo> {
    let (lock, wake) = &*SHARED;
    let mut s = lock.lock().ok()?;
    if s.ctx.is_none() {
        s.ctx = Some(ctx.clone());
    }
    if !s.started {
        s.started = true;
        for _ in 0..WORKERS {
            let _ = std::thread::Builder::new().name("git-info".into()).spawn(worker);
        }
    }
    let now = Instant::now();
    let slot = s.slots.entry(dir.to_path_buf()).or_default();
    let head_due = slot.head_at.is_none_or(|t| now.duration_since(t) > HEAD_TTL);
    let status_due = status && slot.repo.is_some() && slot.status_at.is_none_or(|t| now > t && now.duration_since(t) > STATUS_TTL);
    let mut repo = slot.repo.clone();
    if !status {
        if let Some(r) = repo.as_mut() {
            r.state = State::Unknown;
        }
    }
    if (head_due || status_due) && !slot.queued {
        slot.queued = true;
        let job = Job { status: status_due || (status && slot.status_at.is_none()) };
        s.queue.push((dir.to_path_buf(), job));
        if s.queue.len() > MAX_QUEUE {
            let (old, _) = s.queue.remove(0);
            if let Some(slot) = s.slots.get_mut(&old) {
                slot.queued = false;
            }
        }
        wake.notify_one();
    }
    repo
}

/// Forget what is known of the folders in `dir`, so they are looked at again (after a reload).
pub fn invalidate_children(dir: &Path) {
    if let Ok(mut s) = SHARED.0.lock() {
        for (path, slot) in s.slots.iter_mut() {
            if path.parent() == Some(dir) {
                slot.head_at = None;
                slot.status_at = None;
            }
        }
    }
}

fn worker() {
    let (lock, wake) = &*SHARED;
    loop {
        let (dir, job) = {
            let Ok(mut s) = lock.lock() else { return };
            loop {
                if let Some(next) = s.queue.pop() {
                    break next;
                }
                let Ok(guard) = wake.wait(s) else { return };
                s = guard;
            }
        };
        let head = read_head(&dir);
        let status = match (&head, job.status) {
            (Some(_), true) => Some(status(&dir)),
            _ => None,
        };
        let Ok(mut s) = lock.lock() else { return };
        let now = Instant::now();
        let slot = s.slots.entry(dir).or_default();
        slot.queued = false;
        slot.head_at = Some(now);
        let previous = slot.repo.take();
        slot.repo = head.map(|(branch, detached)| {
            let mut r = previous.filter(|p| p.branch == branch).unwrap_or(Repo { branch, detached, state: State::Unknown, changed: 0, untracked: 0, ahead: 0, behind: 0 });
            r.detached = detached;
            match &status {
                Some(Some(st)) => {
                    r.state = if st.changed + st.untracked > 0 { State::Dirty } else { State::Clean };
                    (r.changed, r.untracked, r.ahead, r.behind) = (st.changed, st.untracked, st.ahead, st.behind);
                }
                Some(None) => r.state = State::Unknown,
                None => {}
            }
            r
        });
        match status {
            Some(Some(_)) => slot.status_at = Some(now),
            // Too slow or failed: not again for a while.
            Some(None) => slot.status_at = Some(now + SLOW_RETRY),
            None => {}
        }
        if let Some(ctx) = &s.ctx {
            ctx.request_repaint();
        }
    }
}

/// The git folder of a working tree: `.git`, or where a `.git` file points (worktrees, submodules).
fn git_dir(dir: &Path) -> Option<PathBuf> {
    let dot = dir.join(".git");
    let meta = std::fs::metadata(&dot).ok()?;
    if meta.is_dir() {
        return Some(dot);
    }
    if meta.is_file() && meta.len() < 4096 {
        let text = std::fs::read_to_string(&dot).ok()?;
        let target = text.lines().find_map(|l| l.strip_prefix("gitdir:"))?.trim();
        let target = Path::new(target);
        return Some(if target.is_absolute() { target.to_path_buf() } else { dir.join(target) });
    }
    None
}

/// The checked-out branch of the repository in `dir` (and whether HEAD is detached),
/// or `None` if `dir` is not a repository.
fn read_head(dir: &Path) -> Option<(String, bool)> {
    let head = std::fs::read(git_dir(dir)?.join("HEAD")).ok()?;
    parse_head(&head[..head.len().min(4096)])
}

fn parse_head(head: &[u8]) -> Option<(String, bool)> {
    let text = std::str::from_utf8(head).ok()?.trim();
    if let Some(r) = text.strip_prefix("ref:") {
        let r = r.trim();
        let branch = r.strip_prefix("refs/heads/").unwrap_or(r);
        return (!branch.is_empty()).then(|| (branch.to_string(), false));
    }
    // A commit id: show its start.
    (text.len() >= 7 && text.bytes().all(|b| b.is_ascii_hexdigit())).then(|| (text[..7].to_string(), true))
}

#[derive(Debug, PartialEq)]
struct Status {
    changed: u32,
    untracked: u32,
    ahead: u32,
    behind: u32,
}

/// `git status --porcelain=v2 --branch`: changed and new files, and how far from upstream.
fn parse_status(out: &str) -> Status {
    let mut s = Status { changed: 0, untracked: 0, ahead: 0, behind: 0 };
    for line in out.lines() {
        if let Some(ab) = line.strip_prefix("# branch.ab ") {
            for part in ab.split_whitespace() {
                if let Some(n) = part.strip_prefix('+') {
                    s.ahead = n.parse().unwrap_or(0);
                } else if let Some(n) = part.strip_prefix('-') {
                    s.behind = n.parse().unwrap_or(0);
                }
            }
        } else if line.starts_with("? ") {
            s.untracked += 1;
        } else if line.starts_with("1 ") || line.starts_with("2 ") || line.starts_with("u ") {
            s.changed += 1;
        }
    }
    s
}

/// The repository config's filter drivers (their names), which git would otherwise run.
fn filter_drivers(git: &Path, dir: &Path) -> Vec<String> {
    let out = Command::new(git)
        .arg("-C")
        .arg(dir)
        .args(["config", "--null", "--name-only", "--get-regexp", r"^filter\."])
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output();
    let Ok(out) = out else { return vec![] };
    let mut names: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .split('\0')
        .filter_map(|k| k.strip_prefix("filter.")?.rsplit_once('.').map(|(name, _)| name.to_string()))
        .collect();
    names.sort();
    names.dedup();
    names
}

/// Run git safely in a repository that may not be trusted: nothing it configures runs.
fn safe_git(git: &Path, dir: &Path) -> Command {
    let mut cmd = Command::new(git);
    cmd.arg("-C").arg(dir).args(["-c", "core.fsmonitor=false", "-c", "core.hooksPath=/dev/null", "-c", "protocol.allow=never"]);
    for d in filter_drivers(git, dir) {
        for key in ["clean", "smudge", "process"] {
            cmd.arg("-c").arg(format!("filter.{d}.{key}="));
        }
        cmd.arg("-c").arg(format!("filter.{d}.required=false"));
    }
    // Never write (refresh the index), never ask anything.
    cmd.env("GIT_OPTIONAL_LOCKS", "0").env("GIT_TERMINAL_PROMPT", "0").env_remove("GIT_DIR").env_remove("GIT_WORK_TREE").stdin(Stdio::null()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    cmd
}

/// Whether everything in the repository at `dir` is committed; `None` if git is missing,
/// fails or takes too long.
fn status(dir: &Path) -> Option<Status> {
    let git = GIT.as_ref()?;
    let tmp = crate::media::TempDir::new().ok()?;
    let out_path = tmp.path().join("status");
    let mut cmd = safe_git(git, dir);
    cmd.args(["status", "--porcelain=v2", "--branch", "--ignore-submodules=all", "--untracked-files=normal"]);
    cmd.stdout(std::fs::File::create(&out_path).ok()?);
    let mut child = cmd.spawn().ok()?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(st)) if st.success() => break,
            Ok(None) if started.elapsed() < STATUS_TIMEOUT => std::thread::sleep(Duration::from_millis(15)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            _ => return None,
        }
    }
    let out = std::fs::read(&out_path).ok()?;
    Some(parse_status(&String::from_utf8_lossy(&out)))
}

/// `name` cut to at most `max` characters, ending in "…" when cut.
pub fn shorten(name: &str, max: usize) -> String {
    if name.chars().count() <= max {
        return name.to_string();
    }
    let keep: String = name.chars().take(max.saturating_sub(1)).collect();
    format!("{keep}…")
}

/// A shared handle used by tests to wait for the workers.
#[cfg(test)]
pub fn wait_for(dir: &Path, status: bool, ctx: &egui::Context) -> Option<Repo> {
    let until = Instant::now() + Duration::from_secs(20);
    loop {
        let r = lookup(ctx, dir, status);
        let done = match &r {
            Some(r) => !status || r.state != State::Unknown,
            None => SHARED.0.lock().ok()?.slots.get(dir).is_some_and(|s| s.head_at.is_some()),
        };
        if done || Instant::now() > until {
            return r;
        }
        std::thread::sleep(Duration::from_millis(30));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heads_and_status_lines() {
        assert_eq!(parse_head(b"ref: refs/heads/feature/x\n"), Some(("feature/x".into(), false)));
        assert_eq!(parse_head(b"0123456789abcdef0123456789abcdef01234567\n"), Some(("0123456".into(), true)));
        assert_eq!(parse_head(b"garbage"), None);
        let st = parse_status("# branch.oid abc\n# branch.head main\n# branch.ab +2 -1\n1 .M N... 100644 100644 100644 a a f.txt\n2 R. N... 100644 100644 100644 a a R100 new\told\n? new.txt\n? b\n! ignored\n");
        assert_eq!(st, Status { changed: 2, untracked: 2, ahead: 2, behind: 1 });
        assert_eq!(shorten("main", 12), "main");
        assert_eq!(shorten("feature/very-long-branch", 10), "feature/v…");
        assert_eq!(shorten("tính-năng-mới", 5), "tính…");
    }

    fn git(dir: &Path, args: &[&str]) {
        let ok = Command::new("git").arg("-C").arg(dir).args(args).stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success());
        assert!(ok, "git {args:?}");
    }

    #[test]
    fn repositories_branches_and_changes() {
        if GIT.is_none() {
            eprintln!("skipped: no git");
            return;
        }
        let tmp = crate::media::TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "work"]);
        std::fs::write(repo.join("a.txt"), "a").unwrap();
        git(&repo, &["add", "-A"]);
        git(&repo, &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-qm", "init"]);
        let ctx = egui::Context::default();
        let r = wait_for(&repo, true, &ctx).expect("a repository");
        assert_eq!((r.branch.as_str(), r.state), ("work", State::Clean));
        // Not a repository.
        assert_eq!(wait_for(tmp.path(), true, &ctx), None);

        // A repository that would run a program when git looks at it: nothing runs.
        let marker = tmp.path().join("ran");
        let evil = format!("[core]\n\tfsmonitor = \"touch '{}'; false\"\n[filter \"x\"]\n\tclean = touch '{}'\n", marker.display(), marker.display());
        let config = std::fs::read_to_string(repo.join(".git/config")).unwrap();
        std::fs::write(repo.join(".git/config"), config + &evil).unwrap();
        std::fs::write(repo.join(".gitattributes"), "*.txt filter=x\n").unwrap();
        std::fs::write(repo.join("b.txt"), "new").unwrap();
        std::fs::write(repo.join("a.txt"), "b").unwrap();
        let st = status(&repo).expect("status");
        assert_eq!((st.changed, st.untracked), (1, 2), "{st:?}");
        assert!(!marker.exists(), "git ran the repository's program");
    }
}
