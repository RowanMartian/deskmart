//! Sentinel: a small background agent that cleans up an app's leftovers when you drag the app to the Trash.
//! Installed as a LaunchAgent that runs this same binary with `--sentinel`.

use crate::util::*;
use std::collections::HashSet;
use std::time::Duration;

pub const LABEL: &str = "app.pearfin.sentinel";

fn plist_path() -> String {
    expand(&format!("~/Library/LaunchAgents/{LABEL}.plist"))
}

pub fn installed() -> bool {
    exists(&plist_path())
}

pub fn set_enabled(on: bool) -> Result<(), String> {
    if !cfg!(target_os = "macos") {
        return Err("Sentinel only runs on macOS.".into());
    }
    let uid = crate::tools::uid();
    let p = plist_path();
    let _ = cmd_quick("/bin/launchctl", &["bootout", &format!("gui/{uid}/{LABEL}")]);
    if !on {
        let _ = std::fs::remove_file(&p);
        return Ok(());
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut d = plist::Dictionary::new();
    d.insert("Label".into(), LABEL.into());
    d.insert("ProgramArguments".into(), plist::Value::Array(vec![exe.to_string_lossy().to_string().into(), "--sentinel".into()]));
    d.insert("RunAtLoad".into(), true.into());
    d.insert("KeepAlive".into(), true.into());
    d.insert("ProcessType".into(), "Background".into());
    d.insert("LowPriorityIO".into(), true.into());
    let _ = std::fs::create_dir_all(parent(&p));
    plist::Value::Dictionary(d).to_file_xml(&p).map_err(|e| e.to_string())?;
    let o = cmd_quick("/bin/launchctl", &["bootstrap", &format!("gui/{uid}"), &p]);
    if o.ok {
        Ok(())
    } else {
        Err(format!("launchctl: {}", o.stderr.trim()))
    }
}

/// The agent loop: watch ~/.Trash for newly arrived apps.
pub fn run() {
    let trash = trash_dir();
    let logf = data_dir().join("sentinel.log");
    let note = |m: String| {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&logf) {
            let _ = writeln!(f, "{} {m}", now_secs());
        }
    };
    let list = || -> HashSet<String> { list_dir(&trash.to_string_lossy()).into_iter().filter(|n| n.ends_with(".app")).collect() };
    let mut seen = list();
    note(format!("Sentinel started, watching {}", trash.display()));
    loop {
        std::thread::sleep(Duration::from_secs(3));
        let now = list();
        for n in now.difference(&seen) {
            let path = trash.join(n).to_string_lossy().to_string();
            std::thread::sleep(Duration::from_secs(2));
            let cfg = Settings::load(&data_dir().join("settings.json"));
            let Some(mut app) = crate::apps::app_info(&path, None, &[], true) else { continue };
            app.system = false;
            let mut strict = cfg.clone();
            strict.sensitivity = "strict".into();
            strict.app_sensitivity.clear();
            let res = crate::finder::find_leftovers(&app, &strict);
            let files: Vec<String> = res.files.into_iter().filter(|f| !f.bundle && !f.path.starts_with(&trash.to_string_lossy().to_string()) && !f.protected).map(|f| f.path).collect();
            if files.is_empty() {
                note(format!("{}: nothing left behind", app.name));
                continue;
            }
            let out = crate::trash::trash(&files, &format!("{} (Sentinel)", app.name), &cfg.app_folders, &data_dir().join("history.json"));
            note(format!("{}: moved {} leftover item(s) to the Trash", app.name, out.moved.len()));
        }
        seen = now;
    }
}
