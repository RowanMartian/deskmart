//! Moving files to the Trash (never deleting outright), with undo history.
//! Port of UndoManager.swift / UndoHistoryManager.swift: everything removed in one go lands in
//! ~/.Trash/<Name>_<timestamp>/ so it can be put back.

use crate::util::*;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Pair {
    pub original: String,
    pub trashed: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Record {
    pub id: String,
    pub name: String,
    pub folder: String,
    pub time: u64,
    pub size: u64,
    pub pairs: Vec<Pair>,
}

pub struct Outcome {
    pub moved: Vec<Pair>,
    pub failed: Vec<(String, String)>,
    pub record: Option<Record>,
}

/// Paths that must never be removed, whatever the search found.
pub fn blocked(path: &str, app_folders: &[String]) -> bool {
    let p = standardize(path);
    if p.trim().is_empty() || p == "/" {
        return true;
    }
    let h = home();
    let mut critical: Vec<String> = ["/", "/Applications", "/Library", "/System", "/Users", "/usr", "/bin", "/sbin", "/etc", "/var", "/private", "/opt", "/cores", "/Volumes"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    critical.push(h.clone());
    for sub in ["Library", "Documents", "Desktop", "Downloads", "Applications", "Movies", "Music", "Pictures", "Public", ".Trash", ".config", "Library/Application Support",
        "Library/Caches", "Library/Containers", "Library/Group Containers", "Library/Preferences", "Library/LaunchAgents", "Library/Logs"]
    {
        critical.push(format!("{h}/{sub}"));
    }
    for sub in ["Application Support", "Caches", "LaunchAgents", "LaunchDaemons", "Logs", "Preferences", "PrivilegedHelperTools", "Extensions"] {
        critical.push(format!("/Library/{sub}"));
    }
    critical.extend(app_folders.iter().map(|f| standardize(&expand(f))));
    if critical.contains(&p) {
        return true;
    }
    // Anything in the sealed system volume
    p.starts_with("/System/") || p.starts_with("/usr/bin/") || p.starts_with("/usr/lib/") || p.starts_with("/bin/") || p.starts_with("/sbin/")
}

fn timestamp() -> String {
    // yyyy-MM-dd_HH-mm-ss in local time, via `date` (avoids a time-zone crate)
    let o = cmd_quick("/bin/date", &["+%Y-%m-%d_%H-%M-%S"]);
    let t = o.stdout.trim().to_string();
    if t.is_empty() {
        now_secs().to_string()
    } else {
        t
    }
}

fn safe_folder_name(name: &str) -> String {
    let n: String = name.chars().map(|c| if c == '/' || c == ':' { '-' } else { c }).collect();
    let n = n.trim().trim_start_matches('.').to_string();
    if n.is_empty() {
        "Pearfin".into()
    } else {
        n.chars().take(80).collect()
    }
}

fn unlock(p: &str) {
    // Clear the "locked" (uchg) flag so the item can be moved.
    if cfg!(target_os = "macos") {
        let _ = cmd_quick("/usr/bin/chflags", &["-R", "nouchg", p]);
    }
}

/// Moves `paths` into ~/.Trash/<name>_<timestamp>/. Items this user can't move are moved in one
/// batch with the admin password prompt.
pub fn trash(paths: &[String], name: &str, app_folders: &[String], history_file: &Path) -> Outcome {
    let mut failed = Vec::new();
    let mut todo = Vec::new();
    for p in paths {
        let p = standardize(p);
        if blocked(&p, app_folders) {
            failed.push((p, "Protected location — never removed.".into()));
        } else if !exists(&p) {
            continue;
        } else if !todo.contains(&p) {
            todo.push(p);
        }
    }
    let mut moved = Vec::new();
    if todo.is_empty() {
        return Outcome { moved, failed, record: None };
    }
    let folder = trash_dir().join(format!("{}_{}", safe_folder_name(name), timestamp()));
    if let Err(e) = std::fs::create_dir_all(&folder) {
        for p in todo {
            failed.push((p, format!("Couldn't create a folder in the Trash: {e}")));
        }
        return Outcome { moved, failed, record: None };
    }
    let mut used = std::collections::HashSet::new();
    let mut admin: Vec<(String, String)> = Vec::new();
    for p in &todo {
        let base = file_name(p);
        let mut dest_name = base.clone();
        let mut i = 1;
        while used.contains(&dest_name) || folder.join(&dest_name).exists() {
            dest_name = format!("{base}-{i}");
            i += 1;
        }
        used.insert(dest_name.clone());
        let dest = folder.join(&dest_name).to_string_lossy().to_string();
        let mut r = std::fs::rename(p, &dest);
        if matches!(&r, Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied) {
            unlock(p);
            r = std::fs::rename(p, &dest);
        }
        match r {
            Ok(()) => moved.push(Pair { original: p.clone(), trashed: dest }),
            Err(e) if e.raw_os_error() == Some(18) => {
                // EXDEV: different volume — copy then remove, via mv
                let o = cmd("/bin/mv", &[p, &dest], std::time::Duration::from_secs(600));
                if o.ok {
                    moved.push(Pair { original: p.clone(), trashed: dest });
                } else if o.stderr.to_lowercase().contains("permission") || o.stderr.contains("not permitted") {
                    admin.push((p.clone(), dest));
                } else {
                    failed.push((p.clone(), o.stderr.trim().to_string()));
                }
            }
            Err(e) if matches!(e.kind(), std::io::ErrorKind::PermissionDenied) || e.raw_os_error() == Some(1) => admin.push((p.clone(), dest)),
            Err(e) => failed.push((p.clone(), e.to_string())),
        }
    }
    if !admin.is_empty() {
        let script = admin
            .iter()
            .map(|(src, dst)| format!("/usr/bin/chflags -R nouchg {s} 2>/dev/null; /bin/mv -f {s} {d}", s = sh_quote(src), d = sh_quote(dst)))
            .collect::<Vec<_>>()
            .join("; ");
        let names = if admin.len() == 1 { file_name(&admin[0].0) } else { format!("{} items", admin.len()) };
        match run_as_admin(&script, &format!("Pearfin wants to move {names} to the Trash.")) {
            Ok(_) => {
                for (src, dst) in admin {
                    if exists(&dst) && !exists(&src) {
                        moved.push(Pair { original: src, trashed: dst });
                    } else {
                        failed.push((src, permission_hint()));
                    }
                }
            }
            Err(e) => {
                let msg = if e == "Cancelled." { e } else { format!("{e} {}", permission_hint()) };
                for (src, _) in admin {
                    failed.push((src, msg.clone()));
                }
            }
        }
    }
    if moved.is_empty() {
        let _ = std::fs::remove_dir(&folder);
        return Outcome { moved, failed, record: None };
    }
    let size = moved.iter().map(|m| size_str(&m.trashed)).sum();
    let rec = Record {
        id: format!("{:x}", now_ms()),
        name: name.to_string(),
        folder: folder.to_string_lossy().to_string(),
        time: now_secs(),
        size,
        pairs: moved.clone(),
    };
    let mut h = load_history(history_file);
    h.insert(0, rec.clone());
    h.truncate(10);
    save_history(history_file, &h);
    play_sound(false);
    log("Trash", format!("Moved {} item(s) to the Trash: {}", moved.len(), rec.folder));
    Outcome { moved, failed, record: Some(rec) }
}

pub fn permission_hint() -> String {
    "macOS blocked this. Give Pearfin Full Disk Access and App Management in System Settings → Privacy & Security.".into()
}

pub fn load_history(file: &Path) -> Vec<Record> {
    std::fs::read_to_string(file).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}
pub fn save_history(file: &Path, h: &[Record]) {
    let _ = std::fs::write(file, serde_json::to_string_pretty(h).unwrap_or_default());
}

/// Puts a history record's files back where they were.
pub fn restore(rec: &Record) -> (usize, Vec<(String, String)>) {
    let mut ok = 0;
    let mut failed = Vec::new();
    let mut admin = Vec::new();
    for pair in &rec.pairs {
        if !exists(&pair.trashed) {
            failed.push((pair.original.clone(), "No longer in the Trash.".into()));
            continue;
        }
        if exists(&pair.original) {
            failed.push((pair.original.clone(), "Something else is already there.".into()));
            continue;
        }
        let _ = std::fs::create_dir_all(parent(&pair.original));
        match std::fs::rename(&pair.trashed, &pair.original) {
            Ok(()) => ok += 1,
            Err(_) => {
                let o = cmd("/bin/mv", &[&pair.trashed, &pair.original], std::time::Duration::from_secs(600));
                if o.ok {
                    ok += 1
                } else {
                    admin.push(pair.clone())
                }
            }
        }
    }
    if !admin.is_empty() {
        let script = admin.iter().map(|p| format!("/bin/mv -f {} {}", sh_quote(&p.trashed), sh_quote(&p.original))).collect::<Vec<_>>().join("; ");
        match run_as_admin(&script, "Pearfin wants to put files back where they were.") {
            Ok(_) => {
                for p in admin {
                    if exists(&p.original) {
                        ok += 1
                    } else {
                        failed.push((p.original, "Couldn't move it back.".into()))
                    }
                }
            }
            Err(e) => {
                for p in admin {
                    failed.push((p.original, e.clone()))
                }
            }
        }
    }
    let _ = std::fs::remove_dir(&rec.folder);
    if ok > 0 {
        play_sound(true);
    }
    (ok, failed)
}

pub fn play_sound(undo: bool) {
    if !cfg!(target_os = "macos") {
        return;
    }
    let f = if undo { "/System/Library/Components/CoreAudio.component/Contents/SharedSupport/SystemSounds/dock/drag to trash.aif" } else { "/System/Library/Components/CoreAudio.component/Contents/SharedSupport/SystemSounds/finder/move to trash.aif" };
    if Path::new(f).exists() {
        let _ = std::process::Command::new("/usr/bin/afplay").arg(f).spawn();
    }
}

pub fn empty_trash() -> Result<(), String> {
    if !cfg!(target_os = "macos") {
        return Err("Only on macOS".into());
    }
    let o = cmd("/usr/bin/osascript", &["-e", "tell application \"Finder\" to empty trash"], std::time::Duration::from_secs(120));
    if o.ok {
        Ok(())
    } else {
        Err(o.stderr.trim().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn blocks_critical() {
        assert!(blocked("/", &[]));
        assert!(blocked("/Applications/", &[]));
        assert!(blocked(&home(), &[]));
        assert!(blocked(&format!("{}/Library", home()), &[]));
        assert!(blocked("/System/Library/Foo", &[]));
        assert!(!blocked("/Applications/Foo.app", &[]));
        assert!(!blocked(&format!("{}/Library/Caches/com.foo", home()), &[]));
    }
}
