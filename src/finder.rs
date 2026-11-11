//! Finding an app's leftover files, and orphaned files from apps already gone.
//! Port of AppPathsFetch.swift (AppPathFinder) and ReversePathsFetch.swift (ReversePathsSearcher).

use crate::apps::AppInfo;
use crate::locations::*;
use crate::util::*;
use rayon::prelude::*;
use serde::Serialize;
use std::collections::HashSet;
use std::path::Path;
use std::sync::Mutex;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Sensitivity {
    Strict,
    Enhanced,
    Deep,
}
impl Sensitivity {
    pub fn parse(s: &str) -> Sensitivity {
        match s {
            "enhanced" => Sensitivity::Enhanced,
            "deep" => Sensitivity::Deep,
            _ => Sensitivity::Strict,
        }
    }
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Found {
    pub path: String,
    pub size: u64,
    pub dir: bool,
    /// The app bundle itself (always first).
    pub bundle: bool,
    /// Linked to this app by the user from the orphan list.
    pub linked: bool,
    /// Not writable by this user, so removing it needs the admin password.
    pub protected: bool,
}

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult {
    pub files: Vec<Found>,
    /// Folders macOS wouldn't let Deskmart read (Full Disk Access / app-data protection on macOS 27).
    pub denied: Vec<String>,
    pub spotlight: usize,
    pub millis: u64,
}

struct Ids {
    bundle_id: String,
    last_two: String,
    app_name: String,
    app_name_stripped: Option<String>,
    letters_only: String,
    path_component: String,
    use_bundle_id: bool,
    company: Option<String>,
    entitlements: Vec<String>,
    team_id: Option<String>,
    base_bundle_id: Option<String>,
}

fn ids(app: &AppInfo) -> Ids {
    let raw: Vec<&str> = app.bundle_id.split('.').collect();
    let last_two = raw.iter().filter(|c| **c != "-").map(|c| c.to_lowercase()).collect::<Vec<_>>();
    let last_two = last_two[last_two.len().saturating_sub(2)..].join("");
    let app_name = pear_format(&app.name);
    let stripped = pear_format(&strip_trailing_digits(&app.name));
    let suffixes = ["helper", "agent", "daemon", "service", "xpc", "launcher", "updater", "installer", "uninstaller", "login", "extension", "plugin"];
    let base_bundle_id = (raw.len() >= 4 && suffixes.contains(&raw.last().unwrap().to_lowercase().as_str())).then(|| pear_format(&raw[..raw.len() - 1].join(".")));
    let path_component = file_name(&app.path).trim_end_matches(".app").to_string();
    Ids {
        bundle_id: pear_format(&app.bundle_id),
        last_two,
        letters_only: app_name.chars().filter(|c| c.is_alphabetic()).collect(),
        app_name_stripped: (stripped != app_name && !stripped.is_empty()).then_some(stripped),
        app_name,
        path_component: pear_format(&path_component),
        use_bundle_id: raw.len() > 1 || app.bundle_id.len() >= 5,
        company: (raw.len() == 3).then(|| pear_format(raw[1])).filter(|c| !c.is_empty()),
        entitlements: app.entitlements.iter().map(|e| pear_format(e)).filter(|e| !e.is_empty()).collect(),
        team_id: app.team_id.as_deref().map(pear_format).filter(|t| !t.is_empty()),
        base_bundle_id,
    }
}

struct Finder<'a> {
    app: &'a AppInfo,
    ids: Ids,
    level: Sensitivity,
    conds: Vec<Condition>,
    skip_paths: Vec<String>,
    found: Mutex<HashSet<String>>,
    denied: Mutex<Vec<String>>,
    steam_id: Option<String>,
}

impl<'a> Finder<'a> {
    fn should_skip(&self, norm: &str, path: &str) -> bool {
        if self.found.lock().unwrap().contains(path) {
            return true;
        }
        if self.skip_paths.iter().any(|s| path.starts_with(s.as_str())) {
            return true;
        }
        SKIP_PREFIX.iter().any(|p| norm.starts_with(p)) && !ALLOW_PREFIX.iter().any(|p| norm.starts_with(p))
    }

    fn matches(&self, norm: &str, path: &str) -> bool {
        let c = &self.ids;
        let strict = self.level == Sensitivity::Strict;
        let ext = Path::new(path).extension().map(|e| e.to_string_lossy().to_string()).unwrap_or_default();
        // Steam: Desktop shortcuts, the game folder, and its appmanifest_<id>.acf
        if path.contains("/Desktop/") && ext == "app" {
            let n = pear_format(&file_name(path).trim_end_matches(".app"));
            if n == c.app_name || n == c.letters_only {
                return true;
            }
        }
        if self.app.steam {
            if path.contains("/Library/Application Support/Steam/steamapps/common/") {
                let n = pear_format(&file_name(path));
                if n == c.app_name || n == c.letters_only {
                    return true;
                }
            }
            let fname = file_name(path);
            if path.contains("/Library/Application Support/Steam/steamapps/") && fname.starts_with("appmanifest_") && ext == "acf" {
                let id = fname.trim_start_matches("appmanifest_").split('.').next().unwrap_or("");
                return self.steam_id.as_deref() == Some(id);
            }
        }
        for e in &c.entitlements {
            if if strict { norm == e } else { norm.contains(e.as_str()) } {
                return true;
            }
        }
        for cond in &self.conds {
            if c.use_bundle_id && c.bundle_id.contains(&cond.bundle_id) {
                if cond.exclude.iter().any(|x| norm.contains(x.as_str())) {
                    return false;
                }
                if cond.include.iter().any(|x| norm.contains(x.as_str())) {
                    return true;
                }
            }
        }
        if self.app.web_app {
            return norm.contains(&c.bundle_id);
        }
        let m = |v: &str| !v.is_empty() && if strict { norm == v } else { norm.contains(v) };
        let full_bundle = c.use_bundle_id && norm.contains(&c.bundle_id);
        let name_match = m(&c.app_name) || m(&c.path_component) || m(&c.letters_only);
        let two = !strict && !c.last_two.is_empty() && norm.contains(&c.last_two);
        let deep = self.level == Sensitivity::Deep;
        let company = deep && c.company.as_ref().map(|x| norm.contains(x.as_str())).unwrap_or(false);
        let team = deep && c.team_id.as_ref().map(|x| norm.contains(x.as_str())).unwrap_or(false);
        let base = c.base_bundle_id.as_ref().map(|x| !x.is_empty() && norm.contains(x.as_str())).unwrap_or(false);
        let stripped = !strict && c.app_name_stripped.as_ref().map(|x| norm.contains(x.as_str())).unwrap_or(false);
        full_bundle || name_match || two || company || team || base || stripped
    }

    fn process(&self, location: &str, depth: usize, max: usize, lib_root: bool) {
        let rd = match std::fs::read_dir(location) {
            Ok(rd) => rd,
            Err(e) => {
                if e.kind() == std::io::ErrorKind::PermissionDenied {
                    self.denied.lock().unwrap().push(location.to_string());
                }
                return;
            }
        };
        let mut local = Vec::new();
        let mut subdirs = Vec::new();
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let path = format!("{}/{}", location.trim_end_matches('/'), name);
            let Ok(meta) = std::fs::metadata(&path).or_else(|_| std::fs::symlink_metadata(&path)) else { continue };
            let is_dir = meta.is_dir();
            // Strip the extension from files (not folders/bundles) before comparing.
            let norm = if is_dir || !name.contains('.') { pear_format(&name) } else { pear_format(name.rsplit_once('.').map(|x| x.0).unwrap_or(&name)) };
            if self.should_skip(&norm, &path) {
                continue;
            }
            if self.matches(&norm, &path) {
                let add = if lib_root && depth == 2 {
                    let par = parent(&path);
                    if STANDARD_LIBRARY_SUBDIRS.contains(&file_name(&par).as_str()) {
                        path.clone()
                    } else {
                        par
                    }
                } else {
                    path.clone()
                };
                local.push(add);
            }
            if is_dir && depth < max && !meta.file_type().is_symlink() {
                if lib_root && depth == 0 {
                    if !SKIP_DEEP_SEARCH.contains(&name.as_str()) {
                        subdirs.push(path);
                    }
                } else {
                    subdirs.push(path);
                }
            }
        }
        self.found.lock().unwrap().extend(local);
        for d in subdirs {
            self.process(&d, depth + 1, max, lib_root);
        }
    }

    /// App containers live in ~/Library/Containers/<UUID> for some apps; the bundle id is in a metadata plist.
    fn containers(&self) -> Vec<String> {
        let base = expand("~/Library/Containers");
        let mut out = Vec::new();
        let rd = match std::fs::read_dir(&base) {
            Ok(r) => r,
            Err(e) => {
                if e.kind() == std::io::ErrorKind::PermissionDenied {
                    self.denied.lock().unwrap().push(base.clone());
                }
                return out;
            }
        };
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            if n == self.app.bundle_id {
                out.push(format!("{base}/{n}"));
            } else if uuid_like(&n) {
                let meta = format!("{base}/{n}/.com.apple.containermanagerd.metadata.plist");
                if let Some(d) = read_plist(Path::new(&meta)) {
                    if pstr(&d, "MCMMetadataIdentifier").as_deref() == Some(self.app.bundle_id.as_str()) {
                        out.push(format!("{base}/{n}"));
                    }
                }
            }
        }
        // Group container named exactly after the bundle id
        let g = expand(&format!("~/Library/Group Containers/{}", self.app.bundle_id));
        if exists(&g) {
            out.push(g);
        }
        out
    }

    /// Spotlight pass for files the folder walk missed (mdfind, 5 s limit).
    fn spotlight(&self, text_content: bool) -> Vec<String> {
        if !cfg!(target_os = "macos") {
            return vec![];
        }
        let q = |s: &str| s.replace('\\', "").replace('"', "\\\"");
        let (n, b) = (q(&self.app.name), q(&self.app.bundle_id));
        let query = match self.level {
            Sensitivity::Strict => format!("kMDItemDisplayName == \"{n}\"cd || kMDItemDisplayName == \"{b}\"cd"),
            Sensitivity::Enhanced => format!("kMDItemDisplayName == \"*{n}*\"cd || kMDItemDisplayName == \"*{b}*\"cd"),
            Sensitivity::Deep => {
                let mut parts = vec![
                    format!("kMDItemDisplayName == \"*{n}*\"cd"),
                    format!("kMDItemDisplayName == \"*{b}*\"cd"),
                    format!("kMDItemFinderComment == \"*{n}*\"cd"),
                    format!("kMDItemCreator == \"{n}\"cd"),
                    format!("kMDItemCopyright == \"*{n}*\"cd"),
                    format!("kMDItemEncodingApplications == \"*{n}*\"cd"),
                ];
                if text_content {
                    parts.push(format!("kMDItemTextContent == \"*{n}*\"cd"));
                }
                let words: Vec<&str> = self.app.name.split_whitespace().collect();
                if words.len() > 1 {
                    parts.push(format!("({})", words.iter().map(|w| format!("kMDItemDisplayName == \"*{}*\"cd", q(w))).collect::<Vec<_>>().join(" && ")));
                }
                parts.join(" || ")
            }
        };
        let o = cmd("/usr/bin/mdfind", &["-onlyin", &home(), &query], std::time::Duration::from_secs(5));
        let mut v: Vec<String> = o.stdout.lines().map(|l| l.to_string()).filter(|l| !l.is_empty()).take(500).collect();
        if self.level == Sensitivity::Strict {
            let (nf, bf) = (pear_format(&self.app.name), pear_format(&self.app.bundle_id));
            v.retain(|p| {
                let f = pear_format(&file_name(p));
                f == nf || f == bf
            });
        }
        // Never offer things inside other apps' bundles or the app's own bundle twice.
        v.retain(|p| !p.contains(".app/") && !p.contains("/.Trash/"));
        v
    }
}

fn steam_game_id(app: &AppInfo) -> Option<String> {
    let text = std::fs::read_to_string(Path::new(&app.path).join("Contents/MacOS/run.sh")).ok()?;
    let i = text.find("steam://run/")?;
    let id: String = text[i + 12..].chars().take_while(|c| c.is_ascii_digit()).collect();
    (!id.is_empty()).then_some(id)
}

fn is_protected(p: &str) -> bool {
    // Moving an item needs write access to the folder it's in.
    let par = parent(p);
    #[cfg(unix)]
    {
        use std::ffi::CString;
        if let Ok(c) = CString::new(par.as_bytes()) {
            // SAFETY: access(2) with a valid C string.
            return unsafe { libc_access(c.as_ptr(), 2) } != 0;
        }
    }
    false
}
#[cfg(unix)]
extern "C" {
    #[link_name = "access"]
    fn libc_access(path: *const std::os::raw::c_char, mode: i32) -> i32;
}

/// Removes children of paths already in the list ("/a" covers "/a/b"). Input must be sorted.
pub fn collapse(sorted: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for p in sorted {
        if let Some(last) = out.last() {
            if p == *last || p.starts_with(&format!("{last}/")) {
                continue;
            }
        }
        out.push(p);
    }
    out
}

/// Everything belonging to `app`: the bundle, then leftovers, sized and sorted.
pub fn find_leftovers(app: &AppInfo, cfg: &Settings) -> SearchResult {
    let started = std::time::Instant::now();
    let level = Sensitivity::parse(cfg.app_sensitivity.get(&app.visible_path()).unwrap_or(&cfg.sensitivity));
    let f = Finder {
        app,
        ids: ids(app),
        level,
        conds: conditions(),
        skip_paths: skip_paths(),
        found: Mutex::new(HashSet::new()),
        denied: Mutex::new(Vec::new()),
        steam_id: app.steam.then(|| steam_game_id(app)).flatten(),
    };
    let bundle = app.visible_path();
    log("Apps", format!("Searching for files related to {}…", app.name));
    let containers = f.containers();
    if !app.web_app {
        let locs = app_locations();
        log("Apps", format!("Scanning {} locations", locs.len()));
        let lib = [expand("~/Library"), "/Library".to_string()];
        locs.par_iter().filter(|l| is_dir(l)).for_each(|l| {
            let root = lib.contains(l);
            f.process(l, 0, if root { 2 } else { 1 }, root);
        });
    }
    let mut all: Vec<String> = f.found.lock().unwrap().iter().cloned().collect();
    all.extend(containers);
    // Forced includes / excludes from per-app conditions
    let bid = pear_format(&app.bundle_id);
    for c in f.conds.iter().filter(|c| bid.contains(&c.bundle_id)) {
        all.extend(c.include_force.iter().cloned());
    }
    let spot: Vec<String> = f.spotlight(cfg.search_text_content).into_iter().filter(|p| !all.contains(p)).collect();
    let spotlight = spot.len();
    if spotlight > 0 {
        log("Apps", format!("Spotlight found {spotlight} more"));
    }
    all.extend(spot);
    // User exclusions (Settings → Folders)
    let excl: Vec<String> = cfg.exclude_apps.iter().map(|p| pear_format(&expand(p))).filter(|p| !p.is_empty()).collect();
    all.retain(|p| {
        let n = pear_format(&standardize(p));
        !excl.iter().any(|e| n == *e || n.contains(e.as_str()))
    });
    // Never offer another app's bundle or the search roots themselves.
    let roots: HashSet<String> = fixed_roots().into_iter().chain(cfg.app_folders.iter().map(|x| expand(x))).collect();
    all.retain(|p| !roots.contains(p) && (p == &bundle || !p.ends_with(".app") || p.contains("/Desktop/") || p.contains("/steamapps/")));
    all.retain(|p| *p != app.path || app.container.is_none());
    let mut all: Vec<String> = all.into_iter().map(|p| standardize(&p)).collect();
    all.push(bundle.clone());
    all.sort();
    let mut all = collapse(all);
    // Files the user linked to this app from the orphan list
    let linked: Vec<String> = cfg.associations.get(&bundle).cloned().unwrap_or_default().into_iter().filter(|p| exists(p)).collect();
    for l in &linked {
        if !all.contains(l) {
            all.push(l.clone());
        }
    }
    if bundle.contains("/.Trash/") {
        all.retain(|p| *p != bundle);
    }
    let mut files: Vec<Found> = all
        .par_iter()
        .map(|p| Found {
            size: size_str(p),
            dir: is_dir(p),
            bundle: *p == bundle,
            linked: linked.contains(p),
            protected: is_protected(p),
            path: p.clone(),
        })
        .collect();
    files.sort_by(|a, b| b.bundle.cmp(&a.bundle).then(b.size.cmp(&a.size)));
    let mut denied = f.denied.into_inner().unwrap();
    denied.sort();
    denied.dedup();
    let total: u64 = files.iter().map(|x| x.size).sum();
    log("Apps", format!("Found {} items ({})", files.len(), fmt_size(total)));
    SearchResult { files, denied, spotlight, millis: started.elapsed().as_millis() as u64 }
}

// ------------------------------------------------------------------ orphans

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Orphan {
    pub path: String,
    pub size: u64,
    pub dir: bool,
    pub protected: bool,
}

/// Files in Library folders that don't belong to any installed app (best guess, like PearCleaner).
pub fn find_orphans(apps: &[AppInfo], cfg: &Settings, stop: &std::sync::atomic::AtomicBool, progress: &dyn Fn(usize, &str)) -> (Vec<Orphan>, Vec<String>) {
    struct C {
        bundle: String,
        name: String,
        stripped: String,
        ents: Vec<String>,
    }
    let cached: Vec<C> = apps
        .iter()
        .map(|a| C { bundle: pear_format(&a.bundle_id), name: pear_format(&a.name), stripped: pear_format(&strip_trailing_digits(&a.name)), ents: a.entitlements.iter().map(|e| pear_format(e)).filter(|e| e.len() >= 5).collect() })
        .collect();
    let conds = conditions();
    let active: Vec<&Condition> = conds.iter().filter(|c| cached.iter().any(|a| a.bundle == c.bundle_id || a.bundle.contains(&c.bundle_id))).collect();
    let excl: Vec<String> = cfg.exclude_orphans.iter().map(|p| pear_format(&expand(p))).filter(|p| !p.is_empty()).collect();
    let linked: HashSet<String> = cfg.associations.values().flatten().cloned().collect();
    let hex32 = regex::Regex::new("^[0-9a-f]{32}$").unwrap();
    let mut items: Vec<String> = Vec::new();
    let mut denied = Vec::new();
    for loc in reverse_locations() {
        if stop.load(std::sync::atomic::Ordering::SeqCst) {
            break;
        }
        progress(items.len(), &loc);
        let rd = match std::fs::read_dir(&loc) {
            Ok(r) => r,
            Err(e) => {
                if e.kind() == std::io::ErrorKind::PermissionDenied {
                    denied.push(loc.clone());
                }
                continue;
            }
        };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let path = format!("{loc}/{name}");
            let npath = pear_format(&standardize(&path));
            if npath.contains("dsstore") || npath.contains("daemonnameoridentifierhere") || excl.iter().any(|x| npath == *x || npath.contains(x.as_str())) {
                continue;
            }
            if linked.contains(&path) {
                continue;
            }
            let nname = pear_format(&name);
            if hex32.is_match(&nname) || SKIP_REVERSE.iter().any(|s| nname.contains(s)) {
                continue;
            }
            // Only real files/folders (not symlinks, sockets, …)
            let Ok(ft) = e.file_type() else { continue };
            if !(ft.is_dir() || ft.is_file()) {
                continue;
            }
            // Belongs to an installed app?
            let mut related = false;
            for a in &cached {
                // ("Bartender 6" also owns ".../Bartender")
                if (a.bundle.len() >= 5 && npath.contains(&a.bundle)) || (a.name.len() >= 5 && npath.contains(&a.name)) || (a.stripped.len() >= 5 && nname == a.stripped) || a.ents.iter().any(|x| npath.contains(x.as_str())) {
                    related = true;
                    break;
                }
                if path.contains("/Containers/") && uuid_like(&name) {
                    let meta = format!("{path}/.com.apple.containermanagerd.metadata.plist");
                    if let Some(id) = read_plist(Path::new(&meta)).and_then(|d| pstr(&d, "MCMMetadataIdentifier")) {
                        let id = pear_format(&id);
                        if a.bundle.len() >= 5 && id.contains(&a.bundle) {
                            related = true;
                            break;
                        }
                    }
                }
            }
            if related {
                continue;
            }
            if active.iter().any(|c| c.include.iter().any(|i| npath.contains(i.as_str())) || c.include_force.iter().any(|f| npath.contains(&pear_format(f)))) {
                continue;
            }
            // UUID-named containers whose owner is an Apple service
            if path.contains("/Containers/") && uuid_like(&name) {
                let meta = format!("{path}/.com.apple.containermanagerd.metadata.plist");
                if let Some(id) = read_plist(Path::new(&meta)).and_then(|d| pstr(&d, "MCMMetadataIdentifier")) {
                    if id.starts_with("com.apple.") {
                        continue;
                    }
                }
            }
            items.push(path);
        }
    }
    let out: Vec<Orphan> = items.par_iter().map(|p| Orphan { size: size_str(p), dir: is_dir(p), protected: is_protected(p), path: p.clone() }).collect();
    (out, denied)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn matcher_strict_and_enhanced() {
        let app = AppInfo { path: "/Applications/Bartender 6.app".into(), bundle_id: "com.surteesstudios.Bartender".into(), name: "Bartender 6".into(), ..Default::default() };
        let mk = |level| Finder {
            app: &app,
            ids: ids(&app),
            level,
            conds: vec![],
            skip_paths: vec![],
            found: Mutex::new(HashSet::new()),
            denied: Mutex::new(vec![]),
            steam_id: None,
        };
        let strict = mk(Sensitivity::Strict);
        assert!(strict.matches("comsurteesstudiosbartender", "/x/com.surteesstudios.Bartender.plist"));
        assert!(strict.matches("bartender6", "/x/Bartender 6"));
        assert!(strict.matches("bartender", "/x/Bartender")); // letters-only name, as in PearCleaner
        assert!(!strict.matches("bartenderhelper", "/x/BartenderHelper"));
        let enh = mk(Sensitivity::Enhanced);
        assert!(enh.matches("bartender", "/x/Bartender"));
        assert!(!enh.matches("dropbox", "/x/Dropbox"));
    }
    #[test]
    fn collapse_children() {
        let v = collapse(vec!["/a".into(), "/a/b".into(), "/ab".into(), "/c".into()]);
        assert_eq!(v, vec!["/a", "/ab", "/c"]);
    }
}
