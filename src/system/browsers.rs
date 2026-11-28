//! Which browser, and which of its profiles, opens a web link (Settings → Terminal →
//! Links): the default one, or the one a rule names for the link's website; and the
//! profiles a browser has, read from its own files on this computer.
//!
//! Chromium browsers (Chrome, Chromium, Brave, Edge, Vivaldi) list their profiles in
//! `Local State` and take `--profile-directory=<folder>`; Firefox and its relatives (Zen,
//! LibreWolf, Waterfox) list them in `profiles.ini` and take `-P <name>`. Other browsers
//! (Safari, Arc, Opera, …) have no profile to choose from the command line.
use crate::config::LinkRule;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// A profile of a browser: `id` is what the browser is started with, `name` what the
/// user sees in the browser.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Profile {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Family {
    Chromium,
    Firefox,
}

/// A browser's release channel: its profiles are kept apart from the stable one's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Channel {
    Stable,
    Beta,
    Dev,
    Canary,
}

/// The kind of browser a program is, by the app's own name (not the folders it is in: a
/// user called "Hedge" or "Zena" has them in every path), with its release channel.
fn identify(program: &str) -> Option<(&'static str, Channel)> {
    let name = crate::system::defaultapps::program_name(program).to_lowercase();
    let words: Vec<&str> = name.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).collect();
    let has = |w: &str| words.contains(&w);
    // On Windows every channel's program is `chrome.exe` (`msedge.exe`, `brave.exe`): its
    // folder tells them apart, by names no user folder is likely to have.
    let path = program.to_lowercase().replace('\\', "/");
    let channel = if has("canary")
        || has("sxs")
        || has("nightly")
        || path.contains("chrome sxs/")
        || path.contains("edge sxs/")
        || path.contains("brave-browser-nightly/")
    {
        Channel::Canary
    } else if has("beta") || path.contains("chrome beta/") || path.contains("edge beta/") || path.contains("brave-browser-beta/") {
        Channel::Beta
    } else if has("dev") || has("unstable") || path.contains("chrome dev/") || path.contains("edge dev/") {
        Channel::Dev
    } else {
        Channel::Stable
    };
    let kind = if has("brave") {
        "brave"
    } else if has("edge") || has("msedge") {
        "edge"
    } else if has("vivaldi") {
        "vivaldi"
    } else if has("chromium") {
        "chromium"
    } else if has("chrome") {
        "chrome"
    } else if has("zen") {
        "zen"
    } else if has("librewolf") {
        "librewolf"
    } else if has("waterfox") {
        "waterfox"
    } else if has("firefox") {
        "firefox"
    } else {
        return None;
    };
    Some((kind, channel))
}

/// The kind of browser a program is, and the folders its profiles are listed in (the
/// first that exists is read).
fn family(program: &str) -> Option<(Family, Vec<PathBuf>)> {
    let (kind, channel) = identify(program)?;
    let home = dirs::home_dir().unwrap_or_default();
    let config = dirs::config_dir().unwrap_or_else(|| home.join(".config"));
    let local = dirs::data_local_dir().unwrap_or_default();
    let roaming = dirs::data_dir().unwrap_or_default();
    let os = |mac: String, linux: Vec<PathBuf>, win: PathBuf| -> Vec<PathBuf> {
        if cfg!(target_os = "macos") {
            vec![home.join("Library/Application Support").join(mac)]
        } else if cfg!(windows) {
            vec![win]
        } else {
            linux
        }
    };
    use Channel::*;
    let chromium = |mac: &str, linux: &str, win: &str| Some((Family::Chromium, os(mac.into(), vec![config.join(linux)], local.join(win))));
    let firefox = |mac: &str, linux: Vec<PathBuf>, win: &str| Some((Family::Firefox, os(mac.into(), linux, roaming.join(win))));
    match (kind, channel) {
        ("chrome", Stable) => chromium("Google/Chrome", "google-chrome", "Google\\Chrome\\User Data"),
        ("chrome", Beta) => chromium("Google/Chrome Beta", "google-chrome-beta", "Google\\Chrome Beta\\User Data"),
        ("chrome", Dev) => chromium("Google/Chrome Dev", "google-chrome-unstable", "Google\\Chrome Dev\\User Data"),
        ("chrome", Canary) => chromium("Google/Chrome Canary", "google-chrome-canary", "Google\\Chrome SxS\\User Data"),
        ("edge", Stable) => chromium("Microsoft Edge", "microsoft-edge", "Microsoft\\Edge\\User Data"),
        ("edge", Beta) => chromium("Microsoft Edge Beta", "microsoft-edge-beta", "Microsoft\\Edge Beta\\User Data"),
        ("edge", Dev) => chromium("Microsoft Edge Dev", "microsoft-edge-dev", "Microsoft\\Edge Dev\\User Data"),
        ("edge", Canary) => chromium("Microsoft Edge Canary", "microsoft-edge-canary", "Microsoft\\Edge SxS\\User Data"),
        ("brave", Beta) => chromium("BraveSoftware/Brave-Browser-Beta", "BraveSoftware/Brave-Browser-Beta", "BraveSoftware\\Brave-Browser-Beta\\User Data"),
        ("brave", Canary | Dev) => {
            chromium("BraveSoftware/Brave-Browser-Nightly", "BraveSoftware/Brave-Browser-Nightly", "BraveSoftware\\Brave-Browser-Nightly\\User Data")
        }
        ("brave", _) => chromium("BraveSoftware/Brave-Browser", "BraveSoftware/Brave-Browser", "BraveSoftware\\Brave-Browser\\User Data"),
        ("vivaldi", _) => chromium("Vivaldi", "vivaldi", "Vivaldi\\User Data"),
        ("chromium", _) => chromium("Chromium", "chromium", "Chromium\\User Data"),
        ("zen", _) => firefox("zen", vec![home.join(".zen")], "zen"),
        ("librewolf", _) => firefox("librewolf", vec![home.join(".librewolf")], "librewolf"),
        ("waterfox", _) => firefox("Waterfox", vec![home.join(".waterfox")], "Waterfox"),
        // Every Firefox channel (Developer Edition, Nightly) shares one list of profiles.
        _ => firefox(
            "Firefox",
            vec![home.join(".mozilla/firefox"), home.join("snap/firefox/common/.mozilla/firefox"), home.join(".var/app/org.mozilla.firefox/.mozilla/firefox")],
            "Mozilla\\Firefox",
        ),
    }
}

/// The profiles in a Chromium browser's `Local State`, sorted by name.
pub fn parse_local_state(text: &str) -> Vec<Profile> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else { return Vec::new() };
    let Some(cache) = v.pointer("/profile/info_cache").and_then(|c| c.as_object()) else { return Vec::new() };
    let mut out: Vec<Profile> =
        cache.iter().map(|(id, info)| Profile { id: id.clone(), name: info.get("name").and_then(|n| n.as_str()).unwrap_or(id).to_string() }).collect();
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}

/// The profiles in Firefox's `profiles.ini`, in its order.
pub fn parse_profiles_ini(text: &str) -> Vec<Profile> {
    let mut out = Vec::new();
    let mut in_profile = false;
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            in_profile = line.to_ascii_lowercase().starts_with("[profile");
        } else if let Some(name) = line.split_once('=').filter(|(k, _)| in_profile && k.trim() == "Name").map(|(_, v)| v) {
            let name = name.trim();
            if !name.is_empty() && !out.iter().any(|p: &Profile| p.id == name) {
                out.push(Profile { id: name.to_string(), name: name.to_string() });
            }
        }
    }
    out
}

fn read_profiles(program: &str) -> Vec<Profile> {
    let Some((fam, dirs)) = family(program) else { return Vec::new() };
    for d in dirs {
        let (file, parse): (_, fn(&str) -> Vec<Profile>) = match fam {
            Family::Chromium => (d.join("Local State"), parse_local_state),
            Family::Firefox => (d.join("profiles.ini"), parse_profiles_ini),
        };
        // Small files; a larger one is not a browser's list.
        if std::fs::metadata(&file).is_ok_and(|m| m.len() < 4 << 20) {
            if let Ok(text) = std::fs::read_to_string(&file) {
                return parse(&text);
            }
        }
    }
    Vec::new()
}

/// The profiles of the browser `program` (as Settings stores it), read again at most
/// every few seconds (Settings asks every frame).
pub fn profiles(program: &str) -> Vec<Profile> {
    const FRESH: Duration = Duration::from_secs(10);
    static CACHE: OnceLock<Mutex<HashMap<String, (Instant, Vec<Profile>)>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some((at, list)) = cache.lock().ok().and_then(|c| c.get(program).cloned()) {
        if at.elapsed() < FRESH {
            return list;
        }
    }
    let list = read_profiles(program);
    if let Ok(mut c) = cache.lock() {
        c.insert(program.to_string(), (Instant::now(), list.clone()));
    }
    list
}

/// Whether the browser `program` has profiles that can be chosen.
pub fn has_profiles(program: &str) -> bool {
    !program.trim().is_empty() && family(program).is_some()
}

/// The arguments that start `program` in `profile` (none for no profile, or a browser
/// without profiles).
pub fn profile_args(program: &str, profile: &str) -> Vec<String> {
    let profile = profile.trim();
    if profile.is_empty() {
        return Vec::new();
    }
    match family(program) {
        Some((Family::Chromium, _)) => vec![format!("--profile-directory={profile}")],
        Some((Family::Firefox, _)) => vec!["-P".into(), profile.into()],
        None => Vec::new(),
    }
}

/// A website as a rule names it: lower case, without a scheme, path, port, `*.` or dots
/// around it (`https://*.GitHub.com/x` → `github.com`).
pub fn normalize_domain(s: &str) -> String {
    let s = s.trim();
    let s = s.split_once("://").map_or(s, |(_, rest)| rest);
    let s = s.split(['/', '?', '#']).next().unwrap_or_default();
    let s = s.rsplit_once('@').map_or(s, |(_, h)| h);
    let s = if s.starts_with('[') { s.split_inclusive(']').next().unwrap_or(s) } else { s.split(':').next().unwrap_or(s) };
    let s = s.trim_start_matches("*.").trim_matches('.').to_lowercase();
    // A website typed with accents (`bücher.de`) matches its links, which use the
    // ASCII form (`xn--bcher-kva.de`), and the other way round.
    if s.is_ascii() || s.starts_with('[') {
        return s;
    }
    idna::domain_to_ascii(&s).unwrap_or(s)
}

/// The host of a web link, without `user@` and port (`None` for other links).
pub fn link_host(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once(':')?;
    if !(scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https")) {
        return None;
    }
    rest.strip_prefix("//")?;
    let host = normalize_domain(url);
    (!host.is_empty()).then_some(host)
}

/// Whether a rule's website covers `host`: the site itself and its subdomains
/// (`github.com` covers `gist.github.com`, not `notgithub.com`).
pub fn domain_matches(domain: &str, host: &str) -> bool {
    let d = normalize_domain(domain);
    !d.is_empty() && (host == d || host.strip_suffix(d.as_str()).is_some_and(|front| front.ends_with('.')))
}

/// The rule for a web link: of those whose website covers its host, the most particular
/// one (the longest website), the first of equals.
pub fn rule_for<'a>(rules: &'a [LinkRule], url: &str) -> Option<&'a LinkRule> {
    let host = link_host(url)?;
    let mut best: Option<(&LinkRule, usize)> = None;
    for r in rules {
        if domain_matches(&r.domain, &host) {
            let len = normalize_domain(&r.domain).len();
            if best.is_none_or(|(_, l)| len > l) {
                best = Some((r, len));
            }
        }
    }
    best.map(|(r, _)| r)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(domain: &str, browser: &str) -> LinkRule {
        LinkRule { domain: domain.into(), browser: browser.into(), profile: String::new() }
    }

    #[test]
    fn websites_match_themselves_and_their_subdomains() {
        assert_eq!(normalize_domain(" https://*.GitHub.com:443/x?y "), "github.com");
        assert_eq!(normalize_domain(".example.org."), "example.org");
        assert_eq!(normalize_domain("user@host.dev"), "host.dev");
        assert_eq!(normalize_domain("[::1]:8080"), "[::1]");
        assert_eq!(link_host("https://u@Gist.GitHub.com:8443/a").as_deref(), Some("gist.github.com"));
        assert_eq!(link_host("mailto:a@b.c"), None);
        assert_eq!(link_host("file:///tmp/x"), None);
        assert!(domain_matches("github.com", "github.com"));
        assert!(domain_matches("*.github.com", "gist.github.com"));
        assert!(!domain_matches("github.com", "notgithub.com"), "a whole label, not the end of one");
        assert!(!domain_matches("gist.github.com", "github.com"));
        assert!(!domain_matches("", "github.com"));
    }

    #[test]
    fn the_most_particular_rule_wins() {
        let rules = [rule("google.com", "chrome"), rule("docs.google.com", "firefox"), rule("google.com", "edge"), rule("", "brave")];
        let b = |url: &str| rule_for(&rules, url).map(|r| r.browser.as_str());
        assert_eq!(b("https://mail.google.com/"), Some("chrome"), "the first of equals");
        assert_eq!(b("https://docs.google.com/d/1"), Some("firefox"));
        assert_eq!(b("https://www.docs.google.com"), Some("firefox"));
        assert_eq!(b("https://example.com"), None);
        assert_eq!(b("mailto:x@google.com"), None, "web links only");
        // `user@` cannot make a link look like a covered site.
        assert_eq!(b("https://google.com@evil.example/"), None);
    }

    #[test]
    fn profiles_are_read_from_the_browsers_files() {
        let local_state = r#"{"profile": {"info_cache": {
            "Default": {"name": "Personal"}, "Profile 3": {"name": "Work"}, "Profile 9": {}
        }}, "other": 1}"#;
        let got = parse_local_state(local_state);
        assert_eq!(
            got,
            [
                Profile { id: "Default".into(), name: "Personal".into() },
                Profile { id: "Profile 9".into(), name: "Profile 9".into() },
                Profile { id: "Profile 3".into(), name: "Work".into() },
            ]
        );
        assert!(parse_local_state("not json").is_empty());
        assert!(parse_local_state("{}").is_empty());
        let ini = "[General]\nStartWithLastProfile=1\nName=not a profile\n\n[Profile1]\nName=work\nIsRelative=1\nPath=abc.work\n\n[Install4F96]\nDefault=x\n[Profile0]\nName = default-release \nPath=x\n";
        assert_eq!(parse_profiles_ini(ini).iter().map(|p| p.id.as_str()).collect::<Vec<_>>(), ["work", "default-release"]);
    }

    #[test]
    fn browsers_are_known_by_their_own_name_and_channel() {
        let id = |p: &str| identify(p);
        // A folder or user name is not the browser.
        assert_eq!(id("/Users/hedgehog/Applications/Firefox.app"), Some(("firefox", Channel::Stable)));
        assert_eq!(id("\"C:\\Users\\Zena\\AppData\\Local\\Mozilla Firefox\\firefox.exe\""), Some(("firefox", Channel::Stable)));
        assert_eq!(id("/home/zena/.local/bin/firefox"), Some(("firefox", Channel::Stable)));
        assert_eq!(id("/home/chromeuser/bin/vivaldi"), Some(("vivaldi", Channel::Stable)));
        assert_eq!(id("/Applications/Zen.app"), Some(("zen", Channel::Stable)));
        assert_eq!(id("/Applications/Safari.app"), None);
        // Channels keep their own profiles.
        assert_eq!(id("/Applications/Google Chrome Canary.app"), Some(("chrome", Channel::Canary)));
        assert_eq!(id("google-chrome-beta"), Some(("chrome", Channel::Beta)));
        assert_eq!(id("google-chrome-unstable"), Some(("chrome", Channel::Dev)));
        assert_eq!(id("\"C:\\Program Files\\Google\\Chrome Beta\\Application\\chrome.exe\""), Some(("chrome", Channel::Beta)));
        assert_eq!(id("\"C:\\Users\\x\\AppData\\Local\\Google\\Chrome SxS\\Application\\chrome.exe\""), Some(("chrome", Channel::Canary)));
        assert_eq!(id("/Applications/Microsoft Edge Dev.app"), Some(("edge", Channel::Dev)));
        assert_eq!(id("\"C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe\""), Some(("edge", Channel::Stable)));
        assert_eq!(id("brave-browser-nightly"), Some(("brave", Channel::Canary)));
        assert_eq!(id("/Applications/Firefox Developer Edition.app").map(|(k, _)| k), Some("firefox"), "one list for every Firefox");
        let dir = |p: &str| family(p).unwrap().1[0].to_string_lossy().to_lowercase();
        assert!(["chrome canary", "canary", "sxs"].iter().any(|w| dir("/Applications/Google Chrome Canary.app").contains(w)));
        assert!(!dir("google-chrome").contains("canary") && !dir("google-chrome").contains("beta"));
    }

    #[test]
    fn websites_with_accents_match_their_links() {
        assert_eq!(normalize_domain("Bücher.de"), "xn--bcher-kva.de");
        let rules = [rule("bücher.de", "firefox")];
        assert_eq!(rule_for(&rules, "https://www.xn--bcher-kva.de/x").map(|r| r.browser.as_str()), Some("firefox"));
        assert_eq!(rule_for(&[rule("xn--bcher-kva.de", "edge")], "https://shop.bücher.de/").map(|r| r.browser.as_str()), Some("edge"));
    }

    #[test]
    fn a_profile_starts_the_browser_the_way_it_takes_it() {
        assert_eq!(profile_args("/Applications/Google Chrome.app", "Profile 3"), ["--profile-directory=Profile 3"]);
        assert_eq!(profile_args("brave-browser", "Default"), ["--profile-directory=Default"]);
        assert_eq!(profile_args("\"C:\\Program Files\\Mozilla Firefox\\firefox.exe\"", "work"), ["-P", "work"]);
        assert_eq!(profile_args("/Applications/Zen.app", "dev"), ["-P", "dev"]);
        assert!(profile_args("/Applications/Safari.app", "x").is_empty(), "no profiles");
        assert!(profile_args("firefox", "  ").is_empty(), "no profile chosen");
        assert!(has_profiles("google-chrome") && has_profiles("firefox") && !has_profiles("epiphany") && !has_profiles(""));
    }
}
