//! Smaller PearCleaner tools: translations, developer caches, installer packages, plugins,
//! launch services, and the permission check.

use crate::util::*;
use rayon::prelude::*;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

// ------------------------------------------------------------------ permissions (works on macOS 27)

/// Full Disk Access check. PearCleaner read TCC.db, which macOS 27 Golden Gate no longer allows even
/// with access granted. Instead, try folders that macOS only opens to apps with Full Disk Access.
pub fn full_disk_access() -> Option<bool> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let probes = [
        "~/Library/Safari",
        "~/Library/Mail",
        "~/Library/Messages",
        "~/Library/Containers/com.apple.stocks",
        "~/Library/Containers/com.apple.Safari",
        "~/Library/Application Support/com.apple.TCC",
        "/Library/Application Support/com.apple.TCC",
    ];
    let mut saw = false;
    for p in probes {
        let p = expand(p);
        if std::fs::symlink_metadata(&p).is_err() {
            continue;
        }
        saw = true;
        match std::fs::read_dir(&p) {
            Ok(_) => return Some(true),
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied || e.raw_os_error() == Some(1) => continue,
            Err(_) => continue,
        }
    }
    if saw {
        Some(false)
    } else {
        None
    }
}

pub fn macos_version() -> (String, u32) {
    if !cfg!(target_os = "macos") {
        return (String::new(), 0);
    }
    let v = cmd_quick("/usr/bin/sw_vers", &["-productVersion"]).stdout.trim().to_string();
    let major = v.split('.').next().and_then(|x| x.parse().ok()).unwrap_or(0);
    (v, major)
}
pub fn macos_name(major: u32) -> &'static str {
    match major {
        27 => "Golden Gate",
        26 => "Tahoe",
        15 => "Sequoia",
        14 => "Sonoma",
        13 => "Ventura",
        12 => "Monterey",
        11 => "Big Sur",
        _ => "",
    }
}

pub fn open_privacy_pane(which: &str) {
    if !cfg!(target_os = "macos") {
        return;
    }
    let anchor = match which {
        "appManagement" => "Privacy_AppBundles",
        "automation" => "Privacy_Automation",
        _ => "Privacy_AllFiles",
    };
    let url = format!("x-apple.systempreferences:com.apple.preference.security?{anchor}");
    let _ = std::process::Command::new("/usr/bin/open").arg(url).spawn();
}

// ------------------------------------------------------------------ translations

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Language {
    pub code: String,
    pub name: String,
    pub preferred: bool,
    pub files: usize,
    pub size: u64,
    pub paths: Vec<String>,
}

pub fn preferred_languages() -> Vec<String> {
    if !cfg!(target_os = "macos") {
        return std::env::var("LANG").ok().map(|l| vec![l.split('.').next().unwrap_or("en").replace('_', "-")]).unwrap_or_else(|| vec!["en".into()]);
    }
    let o = cmd_quick("/usr/bin/defaults", &["read", "-g", "AppleLanguages"]);
    let v: Vec<String> = o.stdout.lines().map(|l| l.trim().trim_matches(|c| c == ',' || c == '"' || c == '(' || c == ')').to_string()).filter(|l| !l.is_empty()).collect();
    if v.is_empty() {
        vec!["en".into()]
    } else {
        v
    }
}

fn language_name(code: &str) -> String {
    let names = [
        ("ar", "Arabic"), ("ca", "Catalan"), ("cs", "Czech"), ("da", "Danish"), ("de", "German"), ("el", "Greek"), ("en", "English"), ("es", "Spanish"), ("fi", "Finnish"),
        ("fr", "French"), ("he", "Hebrew"), ("hi", "Hindi"), ("hr", "Croatian"), ("hu", "Hungarian"), ("id", "Indonesian"), ("it", "Italian"), ("ja", "Japanese"),
        ("ko", "Korean"), ("ms", "Malay"), ("nb", "Norwegian Bokmål"), ("nl", "Dutch"), ("no", "Norwegian"), ("pl", "Polish"), ("pt", "Portuguese"), ("ro", "Romanian"),
        ("ru", "Russian"), ("sk", "Slovak"), ("sl", "Slovenian"), ("sr", "Serbian"), ("sv", "Swedish"), ("th", "Thai"), ("tr", "Turkish"), ("uk", "Ukrainian"),
        ("vi", "Vietnamese"), ("zh", "Chinese"), ("bg", "Bulgarian"), ("et", "Estonian"), ("lt", "Lithuanian"), ("lv", "Latvian"), ("fa", "Persian"), ("bn", "Bengali"),
        ("ta", "Tamil"), ("te", "Telugu"), ("ur", "Urdu"), ("is", "Icelandic"), ("ga", "Irish"), ("cy", "Welsh"), ("eu", "Basque"), ("gl", "Galician"),
        ("English", "English"), ("French", "French"), ("German", "German"), ("Spanish", "Spanish"), ("Japanese", "Japanese"), ("Italian", "Italian"), ("Dutch", "Dutch"),
    ];
    let base = code.split(['-', '_']).next().unwrap_or(code);
    let n = names.iter().find(|(c, _)| *c == base).map(|x| x.1).unwrap_or(base);
    if base.len() < code.len() {
        format!("{n} ({})", &code[base.len() + 1..])
    } else {
        n.to_string()
    }
}

pub fn languages(app: &str) -> Vec<Language> {
    let mut by: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for sub in ["Contents/Resources", "Contents/PlugIns", "Contents/Frameworks"] {
        let mut stack = vec![Path::new(app).join(sub)];
        while let Some(d) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else { continue };
            for e in rd.flatten() {
                let Ok(ft) = e.file_type() else { continue };
                if ft.is_symlink() || !ft.is_dir() {
                    continue;
                }
                let n = e.file_name().to_string_lossy().to_string();
                if let Some(code) = n.strip_suffix(".lproj") {
                    if code != "Base" {
                        by.entry(code.to_string()).or_default().push(e.path().to_string_lossy().to_string());
                    }
                } else if !n.starts_with('.') {
                    stack.push(e.path());
                }
            }
        }
    }
    let pref = preferred_languages();
    let full: Vec<String> = pref.iter().map(|x| x.replace('_', "-")).collect();
    let base: Vec<String> = full.iter().map(|x| x.split('-').next().unwrap_or("").to_string()).collect();
    let regional = full.iter().any(|x| x.contains('-'));
    let mut out: Vec<Language> = by
        .into_iter()
        .map(|(code, paths)| {
            let norm = code.replace('_', "-");
            let b = norm.split('-').next().unwrap_or("").to_string();
            let long = ["English", "French", "German", "Spanish", "Japanese", "Italian", "Dutch"];
            let long_match = long.iter().any(|l| *l == code && base.iter().any(|x| language_name(x) == *l));
            let preferred = full.contains(&norm) || (!norm.contains('-') && base.contains(&norm)) || (!regional && base.contains(&b)) || long_match;
            let files = list_dir(&paths[0]).len();
            let size = paths.iter().map(|p| size_str(p)).sum();
            Language { name: language_name(&code), code, preferred, files, size, paths }
        })
        .collect();
    out.sort_by(|a, b| b.preferred.cmp(&a.preferred).then(a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    out
}

/// What "Auto Prune" removes: everything but preferred languages, keeping English if nothing else would remain.
pub fn auto_prune_selection(langs: &[Language]) -> Vec<String> {
    let mut remove: Vec<&Language> = langs.iter().filter(|l| !l.preferred).collect();
    if langs.iter().all(|l| !l.preferred) {
        remove.retain(|l| !l.code.starts_with("en") && l.code != "English");
    }
    if remove.len() == langs.len() {
        if let Some(en) = langs.iter().find(|l| l.code.starts_with("en")) {
            remove.retain(|l| l.code != en.code);
        }
    }
    remove.into_iter().flat_map(|l| l.paths.clone()).collect()
}

// ------------------------------------------------------------------ developer environments

pub fn dev_environments() -> Vec<(&'static str, Vec<&'static str>)> {
    let mut v = vec![
        ("Android Studio", vec!["~/.android/", "~/Library/Application Support/Google/AndroidStudio*/", "~/Library/Logs/AndroidStudio/", "~/Library/Caches/Google/AndroidStudio*/"]),
        ("Cargo", vec!["~/.cargo/", "~/.cargo/git/", "~/.cargo/registry/"]),
        ("Carthage", vec!["~/Carthage/", "~/Library/Caches/org.carthage.CarthageKit/"]),
        ("CocoaPods", vec!["~/Library/Caches/CocoaPods/", "~/.cocoapods/repos/"]),
        ("Composer", vec!["~/.composer/cache/"]),
        ("Conda", vec!["~/.conda/", "~/anaconda3/", "~/miniconda3/"]),
        ("Cursor", vec![
            "~/Library/Application Support/Cursor/", "~/Library/Application Support/Cursor/Cache", "~/Library/Application Support/Cursor/GPUCache",
            "~/Library/Application Support/Cursor/CachedConfigurations", "~/Library/Application Support/Cursor/CachedData",
            "~/Library/Application Support/Cursor/CachedExtensionVSIXs", "~/Library/Application Support/Cursor/CachedExtensions",
            "~/Library/Application Support/Cursor/CachedProfilesData", "~/Library/Application Support/Cursor/Code Cache", "~/Library/Application Support/Cursor/User",
            "~/.cursor/", "~/.cursor/extensions/",
        ]),
        ("Deno", vec!["~/Library/Caches/deno"]),
        ("Go Modules", vec!["~/go/bin/", "~/go/pkg/mod/"]),
        ("Gradle", vec!["~/.gradle/caches/", "~/.gradle/wrapper/"]),
        ("Haskell Stack", vec!["~/.stack/", "~/.stack/global-project/", "~/.stack/snapshots/"]),
        ("IntelliJ IDEA", vec!["~/Library/Application Support/JetBrains/", "~/Library/Caches/JetBrains/", "~/Library/Logs/JetBrains/"]),
        ("Maven", vec!["~/.m2/"]),
        ("Nix", vec!["/nix/store/", "~/.cache/nix/"]),
        ("Npm", vec!["/usr/local/lib/node_modules/", "~/.nvm/versions/node/*/", "~/.npm/", "~/.nvm/", "~/Library/pnpm/store", "~/.bun/install/cache"]),
        ("Pip", vec!["~/Library/Caches/pip/"]),
        ("Poetry", vec!["~/Library/Caches/pypoetry/", "~/Library/Application Support/pypoetry/"]),
        ("Pub", vec!["~/.pub-cache/", "~/Library/Caches/flutter_engine/"]),
        ("Pyenv", vec!["~/.pyenv/", "~/.pyenv/cache/"]),
        ("Ruby Gems", vec!["~/.gem/", "~/.gem/ruby/*/"]),
        ("Swift", vec!["~/.swiftpm/"]),
        ("Uv", vec!["~/.cache/uv/", "~/.config/uv/", "~/.local/share/uv/"]),
        ("VS Code", vec![
            "~/Library/Application Support/Code/", "~/Library/Application Support/Code/Cache", "~/Library/Application Support/Code/GPUCache",
            "~/Library/Application Support/Code/CachedConfigurations", "~/Library/Application Support/Code/CachedData",
            "~/Library/Application Support/Code/CachedExtensionVSIXs", "~/Library/Application Support/Code/CachedExtensions",
            "~/Library/Application Support/Code/CachedProfilesData", "~/Library/Application Support/Code/Code Cache", "~/Library/Application Support/Code/User",
            "~/.vscode/", "~/.vscode/extensions/", "~/.vscode/cli/",
        ]),
        ("Xcode", vec![
            "~/Library/Caches/com.apple.dt.xcodebuild/", "~/Library/Caches/com.apple.dt.Xcode.sourcecontrol.Git/", "~/Library/Developer/CoreSimulator/Devices/",
            "~/Library/Developer/DeveloperDiskImages/", "~/Library/Developer/Xcode/Archives/", "~/Library/Developer/Xcode/DerivedData/",
            "~/Library/Developer/Xcode/DocumentationCache/", "~/Library/Developer/Xcode/iOS DeviceSupport/", "~/Library/Developer/Xcode/tvOS DeviceSupport/",
            "~/Library/Developer/Xcode/watchOS DeviceSupport/", "~/Library/Developer/Xcode/macOS DeviceSupport/", "~/Library/Developer/Xcode/UserData/",
        ]),
        ("Yarn", vec!["~/.cache/yarn/", "~/.yarn-cache/", "~/.yarn/global/"]),
        ("Zed", vec!["~/.config/zed/", "~/Library/Caches/Zed/", "~/Library/Application Support/Zed/", "~/Library/Application Support/Zed/node/cache/"]),
    ];
    for e in v.iter_mut() {
        e.1.sort();
    }
    v
}

fn glob_expand(pattern: &str) -> Vec<String> {
    let p = expand(pattern.trim_end_matches('/'));
    if !p.contains('*') {
        return vec![p];
    }
    // Only "dir/prefix*suffix" patterns in one component are used.
    let parts: Vec<&str> = p.split('/').collect();
    let idx = parts.iter().position(|x| x.contains('*')).unwrap();
    let base = parts[..idx].join("/");
    let pat = regex::Regex::new(&format!("^{}$", regex::escape(parts[idx]).replace(r"\*", ".*"))).unwrap();
    let rest = parts[idx + 1..].join("/");
    list_dir(&base)
        .into_iter()
        .filter(|n| pat.is_match(n))
        .map(|n| if rest.is_empty() { format!("{base}/{n}") } else { format!("{base}/{n}/{rest}") })
        .collect()
}

pub fn dev_scan() -> Vec<Value> {
    dev_environments()
        .into_par_iter()
        .map(|(name, pats)| {
            let mut paths: Vec<Value> = Vec::new();
            for pat in pats {
                for p in glob_expand(pat) {
                    let present = if is_dir(&p) { list_dir(&p).iter().any(|n| n != ".DS_Store") } else { exists(&p) };
                    if present {
                        paths.push(json!({ "path": p, "display": tildify(&p), "size": size_str(&p), "dir": is_dir(&p) }));
                    }
                }
            }
            json!({ "name": name, "paths": paths })
        })
        .collect()
}

/// VS Code / Cursor keep per-workspace storage forever; find entries whose project folder is gone.
pub fn orphaned_workspaces() -> Vec<Value> {
    let mut out = Vec::new();
    for (editor, base) in [("VS Code", "~/Library/Application Support/Code/User/workspaceStorage"), ("Cursor", "~/Library/Application Support/Cursor/User/workspaceStorage")] {
        let base = expand(base);
        for id in list_dir(&base) {
            let dir = format!("{base}/{id}");
            let Ok(t) = std::fs::read_to_string(format!("{dir}/workspace.json")) else { continue };
            let Ok(v) = serde_json::from_str::<Value>(&t) else { continue };
            let uri = v["folder"].as_str().or_else(|| v["workspace"].as_str()).unwrap_or("");
            let Some(path) = uri.strip_prefix("file://") else { continue };
            let path = percent_decode(path);
            if !exists(&path) {
                out.push(json!({ "editor": editor, "path": dir, "project": path, "size": size_str(&dir) }));
            }
        }
    }
    out
}

pub fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

/// Paths inside a folder (for "Delete Contents").
pub fn contents_of(dir: &str) -> Vec<String> {
    list_dir(dir).into_iter().filter(|n| n != ".DS_Store").map(|n| format!("{}/{n}", dir.trim_end_matches('/'))).collect()
}

// ------------------------------------------------------------------ installer packages (pkgutil)

pub fn packages() -> Vec<Value> {
    let ids: Vec<String> = cmd_quick("/usr/sbin/pkgutil", &["--pkgs"]).stdout.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect();
    let mut v: Vec<Value> = ids
        .par_iter()
        .filter_map(|id| {
            let o = cmd_quick("/usr/sbin/pkgutil", &["--pkg-info-plist", id]);
            let d = plist::Value::from_reader_xml(o.stdout.as_bytes()).ok()?.into_dictionary()?;
            let receipt = format!("/private/var/db/receipts/{id}.plist");
            let bom = format!("/private/var/db/receipts/{id}.bom");
            let receipts: Vec<&String> = [&receipt, &bom].into_iter().filter(|p| exists(p)).collect();
            let secure = !cmd_quick("/usr/sbin/pkgutil", &["--check-signature", &receipt]).stdout.contains("no signature");
            Some(json!({
                "id": id,
                "version": pstr(&d, "pkg-version").unwrap_or_default(),
                "volume": pstr(&d, "volume").unwrap_or_else(|| "/".into()),
                "location": pstr(&d, "install-location").unwrap_or_default(),
                "installed": d.get("install-time").and_then(|t| t.as_unsigned_integer()),
                "apple": id.starts_with("com.apple."),
                "receipts": receipts,
                "secure": secure,
            }))
        })
        .collect();
    v.sort_by_key(|x| x["id"].as_str().unwrap_or("").to_lowercase());
    v
}

/// Files a package installed that still exist (from its bill of materials).
pub fn package_files(id: &str) -> Value {
    let info = cmd_quick("/usr/sbin/pkgutil", &["--pkg-info-plist", id]);
    let d = plist::Value::from_reader_xml(info.stdout.as_bytes()).ok().and_then(|v| v.into_dictionary());
    let vol = d.as_ref().and_then(|d| pstr(d, "volume")).unwrap_or_else(|| "/".into());
    let loc = d.as_ref().and_then(|d| pstr(d, "install-location")).unwrap_or_default();
    let root = standardize(&format!("{}/{}", vol, loc));
    let o = cmd("/usr/sbin/pkgutil", &["--files", id], Duration::from_secs(60));
    let all: Vec<String> = o.stdout.lines().filter(|l| !l.trim().is_empty()).map(|l| standardize(&format!("{root}/{l}"))).collect();
    let total = all.len();
    let mut present: Vec<String> = all.into_iter().filter(|p| exists(p)).collect();
    present.sort();
    // Keep only top-most entries that aren't shared system folders.
    let top = crate::finder::collapse(present.clone());
    let shared = |p: &str| ["/Applications", "/Library", "/usr", "/usr/local", "/usr/local/bin", "/private", "/opt", "/etc", "/Users", "/System"].contains(&p) || crate::trash::blocked(p, &[]);
    let removable: Vec<Value> = top
        .into_iter()
        .flat_map(|p| if shared(&p) { present.iter().filter(|x| parent(x) == p && !shared(x)).cloned().collect::<Vec<_>>() } else { vec![p] })
        .map(|p| json!({ "path": p, "size": size_str(&p), "dir": is_dir(&p) }))
        .collect();
    json!({ "root": root, "total": total, "present": present.len(), "files": removable })
}

pub fn forget_package(id: &str) -> Result<(), String> {
    run_as_admin(&format!("/usr/sbin/pkgutil --forget {}", sh_quote(id)), "Deskmart wants to remove this package's install record.").map(|_| ())
}

// ------------------------------------------------------------------ plugins

pub fn plugins() -> Vec<Value> {
    let mut out = Vec::new();
    for (cat, dirs) in crate::locations::plugin_locations() {
        for d in dirs {
            for n in list_dir(&d) {
                if n.starts_with('.') || n == "Fonts Disabled" {
                    continue;
                }
                let p = format!("{d}/{n}");
                let bundle_id = crate::util::read_plist(&Path::new(&p).join("Contents/Info.plist")).and_then(|i| pstr(&i, "CFBundleIdentifier"));
                out.push(json!({ "category": cat, "name": n, "path": p, "bundleId": bundle_id, "system": !p.starts_with(&home()) }));
            }
        }
    }
    out.par_iter_mut().for_each(|v| {
        let p = v["path"].as_str().unwrap_or("").to_string();
        v["size"] = json!(size_str(&p));
    });
    out
}

// ------------------------------------------------------------------ launch services

pub fn uid() -> String {
    cmd_quick("/usr/bin/id", &["-u"]).stdout.trim().to_string()
}

fn launchctl_list() -> BTreeMap<String, (Option<u64>, String)> {
    let mut m = BTreeMap::new();
    let o = cmd_quick("/bin/launchctl", &["list"]);
    for l in o.stdout.lines().skip(1) {
        let cols: Vec<&str> = l.split('\t').collect();
        if cols.len() >= 3 {
            m.insert(cols[2].to_string(), (cols[0].parse().ok(), cols[1].to_string()));
        }
    }
    m
}

pub fn services() -> Vec<Value> {
    let running = launchctl_list();
    let dirs = [("~/Library/LaunchAgents", "User Agent"), ("/Library/LaunchAgents", "System Agent"), ("/Library/LaunchDaemons", "System Daemon")];
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for (d, kind) in dirs {
        let d = expand(d);
        for n in list_dir(&d) {
            if !n.ends_with(".plist") {
                continue;
            }
            let p = format!("{d}/{n}");
            let info = read_plist(Path::new(&p));
            let label = info.as_ref().and_then(|i| pstr(i, "Label")).unwrap_or_else(|| n.trim_end_matches(".plist").to_string());
            let program = info.as_ref().and_then(|i| {
                pstr(i, "Program").or_else(|| i.get("ProgramArguments").and_then(|a| a.as_array()).and_then(|a| a.first()).and_then(|x| x.as_string()).map(String::from))
            });
            let st = running.get(&label);
            seen.insert(label.clone());
            out.push(json!({
                "label": label, "path": p, "kind": kind, "program": program,
                "loaded": st.is_some(), "pid": st.and_then(|s| s.0), "status": st.map(|s| s.1.clone()),
                "disabled": info.as_ref().and_then(|i| i.get("Disabled")).and_then(|v| v.as_boolean()).unwrap_or(false),
                "apple": label.starts_with("com.apple."), "system": kind != "User Agent",
            }));
        }
    }
    // Agents bundled inside apps (SMAppService), visible in launchctl but with no plist in the Library folders
    for (label, (pid, status)) in running {
        if seen.contains(&label) || label.starts_with("com.apple.") || label.contains(".anonymous.") || label.starts_with("application.") || label.starts_with("0x") {
            continue;
        }
        out.push(json!({ "label": label, "path": null, "kind": "App Service", "program": null, "loaded": true, "pid": pid, "status": status,
            "disabled": false, "apple": false, "system": false }));
    }
    out.sort_by_key(|v| v["label"].as_str().unwrap_or("").to_lowercase());
    out
}

pub fn service_action(action: &str, label: &str, path: Option<&str>, kind: &str) -> Result<String, String> {
    let system = kind == "System Daemon";
    let domain = if system { "system".to_string() } else { format!("gui/{}", uid()) };
    let target = format!("{domain}/{label}");
    let args: Vec<String> = match (action, path) {
        ("start", Some(p)) => vec!["bootstrap".into(), domain.clone(), p.into()],
        ("start", None) => vec!["kickstart".into(), target.clone()],
        ("stop", _) => vec!["bootout".into(), target.clone()],
        ("restart", _) => vec!["kickstart".into(), "-k".into(), target.clone()],
        ("enable", _) => vec!["enable".into(), target.clone()],
        ("disable", _) => vec!["disable".into(), target.clone()],
        _ => return Err("Unknown action".into()),
    };
    if system {
        let script = format!("/bin/launchctl {}", args.iter().map(|a| sh_quote(a)).collect::<Vec<_>>().join(" "));
        return run_as_admin(&script, "Deskmart wants to change a system service.");
    }
    let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let o = cmd_quick("/bin/launchctl", &refs);
    if o.ok {
        Ok(o.stdout)
    } else {
        Err(if o.stderr.trim().is_empty() { "launchctl failed".into() } else { o.stderr.trim().to_string() })
    }
}
