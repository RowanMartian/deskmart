//! Terminal commands (port of CLI.swift). Install with Settings → General → "pearfin command".

use crate::util::*;

const HELP: &str = "Pearfin — uninstall Mac apps and everything they leave behind

Usage:
  pearfin list [folder]               List installed apps
  pearfin search <app> [--level L]    Show the files that belong to an app (L: strict, enhanced, deep)
  pearfin uninstall <app> [--level L] Move an app and its files to the Trash
  pearfin remove <app> [--level L]    Move only the app's leftover files to the Trash (keep the app)
  pearfin orphans                     List files left behind by apps that are gone
  pearfin remove-orphans              Move all orphaned files to the Trash
  pearfin help                        Show this help

<app> is a path (/Applications/Foo.app) or an app name.";

pub fn is_command(a: &str) -> bool {
    ["list", "search", "uninstall", "remove", "orphans", "remove-orphans", "help", "--help", "-h"].contains(&a)
}

fn level(args: &[String]) -> Option<String> {
    args.iter().position(|a| a == "--level").and_then(|i| args.get(i + 1)).cloned()
}

fn resolve(cfg: &Settings, q: &str) -> Option<crate::apps::AppInfo> {
    let casks = crate::apps::casks();
    if q.ends_with(".app") && exists(&expand(q)) {
        let p = standardize(&expand(q));
        return crate::apps::app_info(&p, None, &casks, true);
    }
    let all = crate::apps::load_all(&cfg.app_folders, false);
    let mut a = all.into_iter().find(|a| a.name.eq_ignore_ascii_case(q) || a.bundle_id.eq_ignore_ascii_case(q))?;
    crate::apps::fill_details(&mut a);
    Some(a)
}

pub fn run(args: &[String]) -> i32 {
    let mut cfg = Settings::load(&data_dir().join("settings.json"));
    if let Some(l) = level(args) {
        cfg.sensitivity = l;
        cfg.app_sensitivity.clear();
    }
    let history = data_dir().join("history.json");
    let _ = std::fs::create_dir_all(data_dir());
    match args[0].as_str() {
        "list" => {
            let folders = args.get(1).filter(|a| !a.starts_with("--")).map(|f| vec![f.clone()]).unwrap_or(cfg.app_folders.clone());
            let mut apps = crate::apps::load_all(&folders, false);
            crate::apps::fill_sizes(&mut apps);
            for a in &apps {
                println!("{:<40} {:<12} {:>10}  {}", a.name, a.version.chars().take(12).collect::<String>(), fmt_size(a.size), a.visible_path());
            }
            println!("\n{} apps", apps.len());
            0
        }
        "search" | "uninstall" | "remove" => {
            let Some(q) = args.get(1).filter(|a| !a.starts_with("--")) else {
                eprintln!("Which app? For example: pearfin {} /Applications/Foo.app", args[0]);
                return 2;
            };
            let Some(app) = resolve(&cfg, q) else {
                eprintln!("Couldn't find an app called “{q}”.");
                return 1;
            };
            let res = crate::finder::find_leftovers(&app, &cfg);
            let mut files: Vec<_> = res.files;
            if args[0] == "remove" {
                files.retain(|f| !f.bundle);
            }
            let total: u64 = files.iter().map(|f| f.size).sum();
            for f in &files {
                println!("{:>10}  {}", fmt_size(f.size), f.path);
            }
            println!("\n{} items, {}", files.len(), fmt_size(total));
            for d in &res.denied {
                eprintln!("Couldn't read {d} (give Terminal Full Disk Access to include it)");
            }
            if args[0] == "search" || files.is_empty() {
                return 0;
            }
            if !crate::apps::running_pids(&app.visible_path()).is_empty() && args[0] == "uninstall" {
                eprintln!("{} is open; quitting it…", app.name);
                crate::apps::quit_app(&app);
            }
            let paths: Vec<String> = files.into_iter().map(|f| f.path).collect();
            let out = crate::trash::trash(&paths, &app.name, &cfg.app_folders, &history);
            println!("Moved {} items to the Trash.", out.moved.len());
            for (p, e) in &out.failed {
                eprintln!("Couldn't remove {p}: {e}");
            }
            if out.failed.is_empty() {
                0
            } else {
                1
            }
        }
        "orphans" | "remove-orphans" => {
            let mut apps = crate::apps::load_all(&cfg.app_folders, false);
            for a in apps.iter_mut() {
                crate::apps::fill_details(a);
            }
            let stop = std::sync::atomic::AtomicBool::new(false);
            let (items, denied) = crate::finder::find_orphans(&apps, &cfg, &stop, &|_, _| {});
            for o in &items {
                println!("{:>10}  {}", fmt_size(o.size), o.path);
            }
            println!("\n{} orphaned items, {}", items.len(), fmt_size(items.iter().map(|o| o.size).sum()));
            for d in &denied {
                eprintln!("Couldn't read {d}");
            }
            if args[0] == "remove-orphans" && !items.is_empty() {
                let paths: Vec<String> = items.into_iter().map(|o| o.path).collect();
                let out = crate::trash::trash(&paths, "Orphaned Files", &cfg.app_folders, &history);
                println!("Moved {} items to the Trash.", out.moved.len());
            }
            0
        }
        _ => {
            println!("{HELP}");
            0
        }
    }
}
