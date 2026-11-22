//! Checking for a newer version: GitHub is asked for the latest release, only when the
//! user asks (the menu), or at start when that is turned on in Settings (it is off by
//! default). Nothing is downloaded or installed here: the release page or its download
//! opens in the browser, and the user installs it as before.
use crate::i18n::t;
use crate::util::Res;
use std::process::{Command, Stdio};

pub const REPO: &str = "quangtiencs/matrix_finder";
/// At most one automatic check a day.
pub const AUTO_EVERY_SECS: i64 = 24 * 3600;

pub type Version = (u64, u64, u64);

#[derive(Clone, Debug, PartialEq)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub size: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub version: Version,
    pub tag: String,
    pub title: String,
    /// The release page.
    pub url: String,
    /// Release notes (Markdown).
    pub notes: String,
    /// `YYYY-MM-DD`.
    pub date: String,
    pub assets: Vec<Asset>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    Newer(Release),
    UpToDate(Version),
}

pub fn version_text(v: Version) -> String {
    format!("{}.{}.{}", v.0, v.1, v.2)
}

/// The first `x.y[.z]` in `s`: `v0.7.0`, `Matrix Finder 0.7.0`, `0.8`.
pub fn parse_version(s: &str) -> Option<Version> {
    let start = s.find(|c: char| c.is_ascii_digit())?;
    let mut nums = s[start..].split(|c: char| !c.is_ascii_digit()).take(3).map_while(|p| p.parse::<u64>().ok());
    let v = (nums.next()?, nums.next()?, nums.next().unwrap_or(0));
    Some(v)
}

pub fn current() -> Version {
    parse_version(env!("CARGO_PKG_VERSION")).unwrap_or((0, 0, 0))
}

/// The answer of GitHub's `releases/latest`.
pub fn parse_release(json: &[u8]) -> Res<Release> {
    let v: serde_json::Value = serde_json::from_slice(json).map_err(|e| format!("{} {e}", t("GitHub sent an answer that could not be read:")))?;
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or_default().to_string();
    let tag = s("tag_name");
    let version = parse_version(&tag).ok_or_else(|| format!("{} {tag:?}", t("The latest release has no version number:")))?;
    let assets = v
        .get("assets")
        .and_then(|a| a.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| {
                    Some(Asset {
                        name: x.get("name")?.as_str()?.to_string(),
                        url: x.get("browser_download_url")?.as_str()?.to_string(),
                        size: x.get("size").and_then(|n| n.as_u64()).unwrap_or(0),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let url = match s("html_url") {
        u if u.starts_with("https://github.com/") => u,
        _ => format!("https://github.com/{REPO}/releases/tag/{tag}"),
    };
    Ok(Release { version, title: s("name"), url, notes: s("body"), date: s("published_at").chars().take(10).collect(), tag, assets })
}

/// The download made for this system, if the release has one (`os`, `arch` as in
/// `std::env::consts`): on a Mac with Apple silicon the arm64 DMG. Since 0.8.0 releases
/// have no DMG for Intel Macs (an older universal one is still taken): those build from source.
pub fn asset_for<'a>(r: &'a Release, os: &str, arch: &str) -> Option<&'a Asset> {
    let find = |part: &str| r.assets.iter().find(|a| a.name.to_lowercase().contains(part) && a.url.starts_with("https://"));
    match os {
        "macos" if arch == "aarch64" => find("macos-arm64.dmg").or_else(|| find("macos-universal.dmg")),
        "macos" => find("macos-universal.dmg").or_else(|| find("macos-x86_64.dmg")),
        "windows" => find(".msi").or_else(|| find("windows")),
        _ => None,
    }
}

/// What a check found: a newer release than `have`, or that `have` is the newest.
pub fn outcome(release: Release, have: Version) -> Outcome {
    if release.version > have {
        Outcome::Newer(release)
    } else {
        Outcome::UpToDate(release.version.max(have))
    }
}

/// Ask GitHub (in the background: this waits for the network).
pub fn check() -> Res<Outcome> {
    Ok(outcome(parse_release(&fetch()?)?, current()))
}

#[cfg(test)]
pub static FAKE_ANSWER: std::sync::Mutex<Option<Res<String>>> = std::sync::Mutex::new(None);

/// The latest release, as GitHub's API gives it, fetched with curl (or wget). Only the
/// app's name and version go with the request.
fn fetch() -> Res<Vec<u8>> {
    #[cfg(test)]
    {
        // Tests never go online.
        return FAKE_ANSWER.lock().unwrap().clone().unwrap_or_else(|| Err("offline".into())).map(String::into_bytes);
    }
    #[allow(unreachable_code)]
    {
        let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
        let agent = format!("MatrixFinder/{}", env!("CARGO_PKG_VERSION"));
        let accept = "Accept: application/vnd.github+json";
        let tools: [(&str, Vec<String>); 2] = [
            ("curl", vec!["-fsSL".into(), "--max-time".into(), "20".into(), "-A".into(), agent.clone(), "-H".into(), accept.into(), url.clone()]),
            (
                "wget",
                vec!["-q".into(), "-O".into(), "-".into(), "--timeout=20".into(), format!("--user-agent={agent}"), format!("--header={accept}"), url.clone()],
            ),
        ];
        for (tool, args) in tools {
            let mut cmd = Command::new(tool);
            cmd.args(&args).stdin(Stdio::null());
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
            }
            match cmd.output() {
                Ok(out) if out.status.success() => return Ok(out.stdout),
                Ok(out) => {
                    let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
                    let detail = if err.is_empty() { format!("{tool} {}", out.status) } else { err };
                    return Err(format!("{} {detail}", t("Could not reach GitHub:")));
                }
                // Not installed: try the next one.
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(format!("{} {e}", t("Could not reach GitHub:"))),
            }
        }
        Err(t("Checking for updates needs curl or wget, and neither was found."))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub const SAMPLE: &str = r#"{
        "tag_name": "v0.9.1", "name": "Matrix Finder 0.9.1",
        "html_url": "https://github.com/quangtiencs/matrix_finder/releases/tag/v0.9.1",
        "published_at": "2026-12-01T08:30:00Z", "draft": false, "prerelease": false,
        "body": "Faster **previews**.\n\n- Fix one\n- Fix two",
        "assets": [
            {"name": "MatrixFinder-0.9.1-macos-universal.dmg", "size": 39000000, "browser_download_url": "https://github.com/quangtiencs/matrix_finder/releases/download/v0.9.1/MatrixFinder-0.9.1-macos-universal.dmg"},
            {"name": "MatrixFinder-0.9.1-macos-arm64.dmg", "size": 18000000, "browser_download_url": "https://github.com/quangtiencs/matrix_finder/releases/download/v0.9.1/MatrixFinder-0.9.1-macos-arm64.dmg"},
            {"name": "broken"}
        ]
    }"#;

    #[test]
    fn versions() {
        assert_eq!(parse_version("v0.7.0"), Some((0, 7, 0)));
        assert_eq!(parse_version("Matrix Finder 0.10.2"), Some((0, 10, 2)));
        assert_eq!(parse_version("1.2"), Some((1, 2, 0)));
        assert_eq!(parse_version("2.0.0-beta.1"), Some((2, 0, 0)));
        assert_eq!(parse_version("latest"), None);
        assert_eq!(parse_version("v1"), None);
        assert!((0, 10, 0) > (0, 9, 9), "compared as numbers, not text");
        assert_eq!(current(), parse_version(env!("CARGO_PKG_VERSION")).unwrap());
        assert_eq!(version_text((0, 8, 0)), "0.8.0");
    }

    #[test]
    fn a_release_from_github() {
        let r = parse_release(SAMPLE.as_bytes()).unwrap();
        assert_eq!((r.version, r.tag.as_str(), r.date.as_str()), ((0, 9, 1), "v0.9.1", "2026-12-01"));
        assert_eq!(r.assets.len(), 2, "assets without a URL are left out");
        assert!(r.notes.starts_with("Faster"));
        assert_eq!(asset_for(&r, "macos", "aarch64").unwrap().name, "MatrixFinder-0.9.1-macos-arm64.dmg");
        assert_eq!(asset_for(&r, "macos", "x86_64").unwrap().name, "MatrixFinder-0.9.1-macos-universal.dmg");
        // Apple silicon only, as from 0.8.0: nothing to download on an Intel Mac.
        let arm_only = Release { assets: r.assets.iter().filter(|a| a.name.contains("arm64")).cloned().collect(), ..r.clone() };
        assert_eq!(asset_for(&arm_only, "macos", "aarch64").unwrap().name, "MatrixFinder-0.9.1-macos-arm64.dmg");
        assert_eq!(asset_for(&arm_only, "macos", "x86_64"), None);
        assert_eq!(asset_for(&r, "linux", "x86_64"), None, "Linux installs from source");
        // Only GitHub pages are opened.
        let odd = SAMPLE.replace("https://github.com/quangtiencs/matrix_finder/releases/tag/v0.9.1", "https://example.com/x");
        assert_eq!(parse_release(odd.as_bytes()).unwrap().url, format!("https://github.com/{REPO}/releases/tag/v0.9.1"));
        assert!(parse_release(b"{\"message\": \"Not Found\"}").is_err());
        assert!(parse_release(b"<html>").is_err());
    }

    #[test]
    fn newer_or_not() {
        let r = parse_release(SAMPLE.as_bytes()).unwrap();
        assert!(matches!(outcome(r.clone(), (0, 7, 0)), Outcome::Newer(_)));
        assert_eq!(outcome(r.clone(), (0, 9, 1)), Outcome::UpToDate((0, 9, 1)));
        // A build newer than the latest release (from source) is up to date.
        assert_eq!(outcome(r, (1, 0, 0)), Outcome::UpToDate((1, 0, 0)));
    }

    #[test]
    fn check_uses_the_answer() {
        *FAKE_ANSWER.lock().unwrap() = Some(Ok(SAMPLE.replace("0.9.1", "99.0.0")));
        assert!(matches!(check(), Ok(Outcome::Newer(r)) if r.version == (99, 0, 0)));
        *FAKE_ANSWER.lock().unwrap() = Some(Err("down".into()));
        assert_eq!(check(), Err("down".into()));
        *FAKE_ANSWER.lock().unwrap() = None;
    }
}
