//! Finding installed apps and reading what PearCleaner needs about each (port of AppInfoFetch.swift).

use crate::macho;
use crate::util::*;
use rayon::prelude::*;
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub path: String,
    pub bundle_id: String,
    pub name: String,
    pub version: String,
    pub build: Option<String>,
    pub size: u64,
    pub web_app: bool,
    /// iPhone/iPad app running on Apple Silicon (bundle inside `Foo.app/Wrapper/`).
    pub wrapped: bool,
    /// Outside the home folder.
    pub system: bool,
    pub arch: String,
    pub cask: Option<String>,
    pub steam: bool,
    pub sparkle: bool,
    pub app_store: bool,
    pub feed_url: Option<String>,
    pub created: Option<u64>,
    pub modified: Option<u64>,
    pub added: Option<u64>,
    pub last_used: Option<u64>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub entitlements: Vec<String>,
    pub team_id: Option<String>,
    /// For a wrapped app: the outer container `.app` (what Finder shows and what gets deleted).
    pub container: Option<String>,
}

impl AppInfo {
    /// The bundle shown in Finder (outer wrapper for iOS apps).
    pub fn visible_path(&self) -> String {
        self.container.clone().unwrap_or_else(|| self.path.clone())
    }
}

/// Apps Pearfin never lists (part of macOS, or Pearfin itself).
fn restricted(p: &str) -> bool {
    p.contains("/Applications/Safari") || p.contains("/Applications/Utilities") || p.ends_with("/Pearfin.app") || p.starts_with("/System/")
}

pub fn collect_app_paths(folders: &[String]) -> Vec<String> {
    fn walk(dir: &Path, depth: usize, out: &mut Vec<String>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_symlink() {
                continue;
            }
            let p = e.path();
            let ps = p.to_string_lossy().to_string();
            if p.extension().map(|x| x == "app").unwrap_or(false) {
                if !restricted(&ps) {
                    out.push(ps);
                }
            } else if ft.is_dir() && depth < 4 {
                walk(&p, depth + 1, out);
            }
        }
    }
    let mut out = Vec::new();
    for f in folders {
        walk(Path::new(&expand(f)), 0, &mut out);
    }
    out.sort();
    out.dedup();
    out
}

pub fn info_plist(app: &Path) -> Option<plist::Dictionary> {
    read_plist(&app.join("Contents/Info.plist")).or_else(|| read_plist(&app.join("Info.plist")))
}

/// Port of `getEntitlements`: app groups, iCloud containers, plus names of the binaries and helper apps inside the bundle.
pub fn entitlement_names(app: &Path, sig: Option<&macho::Signature>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let push = |s: String, out: &mut Vec<String>| {
        if !out.contains(&s) {
            out.push(s)
        }
    };
    if let Some(e) = sig.and_then(|s| s.entitlements.as_ref()) {
        for key in ["com.apple.security.application-groups", "com.apple.developer.icloud-container-identifiers"] {
            if let Some(arr) = e.get(key).and_then(|v| v.as_array()) {
                for v in arr {
                    if let Some(s) = v.as_string() {
                        push(s.to_string(), &mut out);
                    }
                }
            }
        }
    }
    let excluded = ["crashhandler", "crash handler", "electron"];
    for f in list_dir(&app.join("Contents/MacOS").to_string_lossy()) {
        if !f.starts_with('.') && f.chars().count() >= 5 && !excluded.contains(&f.to_lowercase().as_str()) {
            push(f, &mut out);
        }
    }
    // Helper apps one level down, e.g. Contents/SharedSupport/Helper.app
    let contents = app.join("Contents");
    for sub in list_dir(&contents.to_string_lossy()) {
        let subp = contents.join(&sub);
        if !subp.is_dir() {
            continue;
        }
        for item in list_dir(&subp.to_string_lossy()) {
            if let Some(stem) = item.strip_suffix(".app") {
                if stem.chars().count() >= 5 && !excluded.contains(&stem.to_lowercase().as_str()) {
                    push(stem.to_string(), &mut out);
                }
                for bin in list_dir(&subp.join(&item).join("Contents/MacOS").to_string_lossy()) {
                    if !bin.starts_with('.') && bin.chars().count() >= 5 && !excluded.contains(&bin.to_lowercase().as_str()) {
                        push(bin, &mut out);
                    }
                }
            }
        }
    }
    out
}

// ------------------------------------------------------------------ Homebrew casks

#[derive(Clone, Debug)]
pub struct Cask {
    pub token: String,
    pub apps: Vec<String>,
    #[allow(dead_code)]
    pub auto_updates: bool,
}

pub fn brew_prefixes() -> Vec<&'static str> {
    vec!["/opt/homebrew", "/usr/local", "/home/linuxbrew/.linuxbrew"]
}
pub fn brew_bin() -> Option<String> {
    brew_prefixes().into_iter().map(|p| format!("{p}/bin/brew")).find(|p| Path::new(p).exists())
}

/// Installed casks and the `.app` names each one installed, read from Homebrew's Caskroom metadata.
pub fn casks() -> Vec<Cask> {
    let mut out = Vec::new();
    for pre in brew_prefixes() {
        let room = format!("{pre}/Caskroom");
        for token in list_dir(&room) {
            if token.starts_with('.') {
                continue;
            }
            let meta = format!("{room}/{token}/.metadata");
            let mut apps = Vec::new();
            let mut auto = false;
            // .metadata/<version>/<timestamp>/Casks/<token>.json (newer) or .rb (older)
            let mut files = Vec::new();
            for ver in list_dir(&meta) {
                for ts in list_dir(&format!("{meta}/{ver}")) {
                    let d = format!("{meta}/{ver}/{ts}/Casks");
                    for f in list_dir(&d) {
                        files.push(format!("{d}/{f}"));
                    }
                }
            }
            files.sort();
            if let Some(f) = files.last() {
                let text = std::fs::read_to_string(f).unwrap_or_default();
                if f.ends_with(".json") {
                    if let Ok(v) = serde_json::from_str::<Value>(&text) {
                        auto = v["auto_updates"].as_bool().unwrap_or(false);
                        if let Some(arts) = v["artifacts"].as_array() {
                            for a in arts {
                                if let Some(list) = a["app"].as_array() {
                                    for x in list {
                                        if let Some(s) = x.as_str() {
                                            apps.push(s.to_string());
                                        } else if let Some(t) = x["target"].as_str() {
                                            apps.push(t.to_string());
                                        }
                                    }
                                }
                            }
                        }
                    }
                } else {
                    auto = text.contains("auto_updates true");
                    let re = regex::Regex::new(r#"(?m)^\s*app\s+"([^"]+)""#).unwrap();
                    for c in re.captures_iter(&text) {
                        apps.push(c[1].to_string());
                    }
                }
            }
            out.push(Cask { token, apps: apps.iter().map(|a| file_name(a)).collect(), auto_updates: auto });
        }
    }
    out
}

// ------------------------------------------------------------------ Spotlight metadata (dates, sizes)

#[derive(Default, Clone)]
pub struct Md {
    pub size: Option<u64>,
    pub added: Option<u64>,
    pub last_used: Option<u64>,
}

fn parse_md_date(s: &str) -> Option<u64> {
    // "2024-03-01 18:22:05 +0000"
    static RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| regex::Regex::new(r"^(\d{4})-(\d{2})-(\d{2}) (\d{2}):(\d{2}):(\d{2})").unwrap());
    let c = RE.captures(s.trim())?;
    let n = |i: usize| c[i].parse::<i64>().unwrap_or(0);
    let (y, m, d) = (n(1), n(2), n(3));
    // days from civil (Howard Hinnant)
    let y2 = if m <= 2 { y - 1 } else { y };
    let era = if y2 >= 0 { y2 } else { y2 - 399 } / 400;
    let yoe = y2 - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    Some((days * 86400 + n(4) * 3600 + n(5) * 60 + n(6)) as u64)
}

/// One `mdls` call per batch of apps. macOS only; returns empty elsewhere.
pub fn spotlight_metadata(paths: &[String]) -> HashMap<String, Md> {
    let mut out = HashMap::new();
    if !cfg!(target_os = "macos") || paths.is_empty() {
        return out;
    }
    let attrs = ["kMDItemLogicalSize", "kMDItemDateAdded", "kMDItemLastUsedDate"];
    let chunks: Vec<&[String]> = paths.chunks(40).collect();
    let results: Vec<HashMap<String, Md>> = chunks
        .par_iter()
        .map(|chunk| {
            let mut args: Vec<&str> = vec!["-raw", "-nullMarker", "(null)"];
            for a in attrs {
                args.push("-name");
                args.push(a);
            }
            for p in chunk.iter() {
                args.push(p.as_str());
            }
            let o = cmd("/usr/bin/mdls", &args, std::time::Duration::from_secs(20));
            let vals: Vec<&str> = o.stdout.split('\0').collect();
            let mut m = HashMap::new();
            if vals.len() >= chunk.len() * attrs.len() {
                for (i, p) in chunk.iter().enumerate() {
                    let v = |k: usize| vals[i * attrs.len() + k].trim();
                    let size = v(0).parse::<u64>().ok();
                    m.insert(p.clone(), Md { size, added: parse_md_date(v(1)), last_used: parse_md_date(v(2)) });
                }
            }
            m
        })
        .collect();
    for r in results {
        out.extend(r);
    }
    out
}

// ------------------------------------------------------------------ building AppInfo

pub fn app_info(path: &str, md: Option<&Md>, casks: &[Cask], full: bool) -> Option<AppInfo> {
    let p = Path::new(path);
    // iPhone/iPad apps: Foo.app/Wrapper/Foo.app
    let wrapper = p.join("Wrapper");
    if wrapper.is_dir() {
        let inner = list_dir(&wrapper.to_string_lossy()).into_iter().find(|n| n.ends_with(".app"))?;
        let mut a = bundle_info(&wrapper.join(inner), md, casks, full, true)?;
        a.name = p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or(a.name);
        a.container = Some(path.to_string());
        if a.arch.is_empty() {
            a.arch = "arm".into(); // iPhone/iPad apps only run on Apple silicon
        }
        a.app_store = p.join("Wrapper/iTunesMetadata.plist").exists() || a.app_store;
        a.system = !path.starts_with(&home());
        return Some(a);
    }
    bundle_info(p, md, casks, full, false).or_else(|| steam_game(p))
}

fn bundle_info(p: &Path, md: Option<&Md>, casks: &[Cask], full: bool, wrapped: bool) -> Option<AppInfo> {
    let info = info_plist(p)?;
    let bundle_id = pstr(&info, "CFBundleIdentifier")?;
    let path = p.to_string_lossy().to_string();
    let stem = p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    // Finder shows the folder name, which can differ from CFBundleName ("Visual Studio Code" vs "Code").
    let name = if stem.is_empty() { pstr(&info, "CFBundleDisplayName").or_else(|| pstr(&info, "CFBundleName")).unwrap_or_default() } else { stem };
    let short = pstr(&info, "CFBundleShortVersionString").unwrap_or_default();
    let build = pstr(&info, "CFBundleVersion");
    let version = if short.is_empty() { build.clone().unwrap_or_default() } else { short.clone() };
    let web_app = info.get("LSTemplateApplication").and_then(|v| v.as_boolean()).unwrap_or(false) || pstr(&info, "CFBundleExecutable").as_deref() == Some("app_mode_loader");
    let setapp = bundle_id.ends_with("-setapp");
    let feed = pstr(&info, "SUFeedURL").or_else(|| pstr(&info, "SUFeedUrl"));
    let sparkle = !setapp && (feed.is_some() || info.contains_key("SUPublicEDKey") || info.contains_key("SUPublicDSAKeyFile") || info.contains_key("SUEnableAutomaticChecks"));
    let app_store = p.join("Contents/_MASReceipt/receipt").exists();
    let file = format!("{name}.app");
    let cask = casks.iter().find(|c| c.apps.iter().any(|a| a.eq_ignore_ascii_case(&file))).map(|c| c.token.clone());
    let meta = std::fs::metadata(p).ok();
    let mut a = AppInfo {
        path: path.clone(),
        bundle_id,
        name,
        version,
        build: if short.is_empty() { None } else { build },
        web_app,
        wrapped,
        system: !path.starts_with(&home()),
        arch: macho::app_arch(p).as_str().into(),
        cask,
        steam: false,
        sparkle,
        app_store,
        feed_url: feed,
        created: meta.as_ref().and_then(|m| secs(m.created())),
        modified: meta.as_ref().and_then(|m| secs(m.modified())),
        added: md.and_then(|m| m.added),
        last_used: md.and_then(|m| m.last_used),
        size: md.and_then(|m| m.size).unwrap_or(0),
        ..Default::default()
    };
    if full {
        fill_details(&mut a);
    }
    Some(a)
}

/// The slower bits: signature (entitlements, Team ID) and exact size.
pub fn fill_details(a: &mut AppInfo) {
    let p = PathBuf::from(&a.path);
    let sig = macho::bundle_executable(&p).and_then(|e| macho::signature(&e));
    a.entitlements = entitlement_names(&p, sig.as_ref());
    a.team_id = sig.and_then(|s| s.team_id);
    if a.size == 0 {
        a.size = size_str(&a.visible_path());
    }
}

/// Steam puts a small launcher app on the Desktop / in ~/Applications; the real game lives in steamapps/common.
fn steam_game(launcher: &Path) -> Option<AppInfo> {
    let name = launcher.file_stem()?.to_string_lossy().to_string();
    let common = format!("{}/Library/Application Support/Steam/steamapps/common/{name}", home());
    let real = list_dir(&common).into_iter().find(|n| n.ends_with(".app") && Path::new(&common).join(n).join("Contents/Info.plist").exists())?;
    let real = Path::new(&common).join(real);
    let info = info_plist(&real)?;
    let mut chars = name.chars();
    let pretty = chars.next().map(|c| c.to_uppercase().collect::<String>() + chars.as_str()).unwrap_or_default();
    Some(AppInfo {
        path: launcher.to_string_lossy().to_string(),
        bundle_id: pstr(&info, "CFBundleIdentifier").filter(|s| !s.is_empty()).unwrap_or_else(|| "com.no.bundleid".into()),
        name: pretty,
        version: pstr(&info, "CFBundleShortVersionString").unwrap_or_default(),
        arch: macho::app_arch(&real).as_str().into(),
        steam: true,
        ..Default::default()
    })
}

/// Everything in the app folders, sorted by name. `full` also reads signatures (slower).
pub fn load_all(folders: &[String], full: bool) -> Vec<AppInfo> {
    let mut paths = collect_app_paths(folders);
    // Steam launchers on the Desktop
    for n in list_dir(&format!("{}/Desktop", home())) {
        if n.ends_with(".app") {
            paths.push(format!("{}/Desktop/{n}", home()));
        }
    }
    let md = spotlight_metadata(&paths);
    let casks = casks();
    let mut apps: Vec<AppInfo> = paths
        .par_iter()
        .filter_map(|p| {
            let a = app_info(p, md.get(p), &casks, full)?;
            // Desktop entries only count when they're Steam launchers.
            if p.starts_with(&format!("{}/Desktop/", home())) && !a.steam {
                return None;
            }
            Some(a)
        })
        .collect();
    apps.sort_by_key(|a| a.name.to_lowercase());
    apps
}

/// Fills in sizes for apps Spotlight didn't report.
pub fn fill_sizes(apps: &mut [AppInfo]) {
    apps.par_iter_mut().filter(|a| a.size == 0).for_each(|a| a.size = size_str(&a.visible_path()));
}

/// Processes whose executable lives inside the given bundle.
pub fn running_pids(bundle: &str) -> Vec<u32> {
    let o = cmd_quick("/bin/ps", &["-axo", "pid=,comm="]);
    let prefix = format!("{bundle}/");
    o.stdout
        .lines()
        .filter_map(|l| {
            let l = l.trim();
            let (pid, comm) = l.split_once(' ')?;
            comm.trim().starts_with(&prefix).then(|| pid.parse().ok()).flatten()
        })
        .collect()
}

pub fn quit_app(a: &AppInfo) {
    if cfg!(target_os = "macos") {
        let script = format!("tell application id \"{}\" to quit", a.bundle_id.replace('"', ""));
        let _ = cmd("/usr/bin/osascript", &["-e", &script], std::time::Duration::from_secs(6));
    }
    for _ in 0..20 {
        if running_pids(&a.visible_path()).is_empty() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(150));
    }
    for pid in running_pids(&a.visible_path()) {
        let _ = cmd_quick("/bin/kill", &["-TERM", &pid.to_string()]);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn md_date() {
        assert_eq!(super::parse_md_date("2024-01-01 00:00:00 +0000"), Some(1704067200));
        assert_eq!(super::parse_md_date("(null)"), None);
    }
}
