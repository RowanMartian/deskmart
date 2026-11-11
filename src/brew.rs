//! Homebrew manager (port of the Homebrew view: installed, search & install, outdated, maintenance, taps).

use crate::tasks::{run_logged, Ctx};
use crate::util::*;
use serde_json::{json, Value};
use std::time::Duration;

pub fn env() -> Vec<(&'static str, String)> {
    let mut v = vec![
        ("HOMEBREW_NO_AUTO_UPDATE", "1".to_string()),
        ("HOMEBREW_NO_ENV_HINTS", "1".to_string()),
        ("HOMEBREW_NO_INSTALL_CLEANUP", "1".to_string()),
        ("HOMEBREW_COLOR", "0".to_string()),
        ("NONINTERACTIVE", "1".to_string()),
    ];
    // Casks that install .pkg files need sudo; route its password prompt to a macOS dialog.
    if let Some(askpass) = askpass_script() {
        v.push(("SUDO_ASKPASS", askpass));
    }
    v
}

/// A tiny script that runs `Deskmart --askpass` (sudo needs a program path without arguments).
fn askpass_script() -> Option<String> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let exe = std::env::current_exe().ok()?;
    let p = data_dir().join("askpass.sh");
    let body = format!("#!/bin/sh\nexec {} --askpass \"$@\"\n", sh_quote(&exe.to_string_lossy()));
    if std::fs::read_to_string(&p).ok().as_deref() != Some(body.as_str()) {
        std::fs::write(&p, &body).ok()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o700)).ok()?;
        }
    }
    Some(p.to_string_lossy().to_string())
}

/// `Deskmart --askpass`: show a password dialog and print the answer for sudo.
pub fn askpass_main() {
    let script = r#"display dialog "Homebrew needs your Mac password to finish." with title "Deskmart" default answer "" with hidden answer buttons {"Cancel", "OK"} default button "OK" with icon caution
text returned of result"#;
    let o = cmd("/usr/bin/osascript", &["-e", script], Duration::from_secs(300));
    if o.ok {
        print!("{}", o.stdout.trim_end_matches('\n'));
    } else {
        std::process::exit(1);
    }
}

fn brew() -> Result<String, String> {
    crate::apps::brew_bin().ok_or_else(|| "Homebrew isn't installed. Get it from brew.sh.".to_string())
}

fn run_json(args: &[&str], timeout: u64) -> Result<Value, String> {
    let b = brew()?;
    let mut c = std::process::Command::new(&b);
    c.args(args);
    for (k, v) in env() {
        c.env(k, v);
    }
    let out = run_with_timeout(c, Duration::from_secs(timeout))?;
    serde_json::from_str(&out).map_err(|_| "Homebrew returned something unexpected.".to_string())
}

fn run_with_timeout(mut c: std::process::Command, t: Duration) -> Result<String, String> {
    use std::process::Stdio;
    let mut child = c.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().map_err(|e| e.to_string())?;
    let mut so = child.stdout.take().unwrap();
    let h = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = std::io::Read::read_to_string(&mut so, &mut s);
        s
    });
    let start = std::time::Instant::now();
    loop {
        if let Ok(Some(_)) = child.try_wait() {
            break;
        }
        if start.elapsed() > t {
            let _ = child.kill();
            return Err("Homebrew took too long.".into());
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    Ok(h.join().unwrap_or_default())
}

pub fn status() -> Value {
    match crate::apps::brew_bin() {
        Some(b) => {
            let v = cmd_quick(&b, &["--version"]).stdout.lines().next().unwrap_or("").to_string();
            json!({ "installed": true, "path": b, "version": v })
        }
        None => json!({ "installed": false }),
    }
}

pub fn installed() -> Result<Value, String> {
    let v = run_json(&["info", "--json=v2", "--installed"], 180)?;
    let formulae: Vec<Value> = v["formulae"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|f| {
            let inst = f["installed"].as_array().and_then(|a| a.last().cloned()).unwrap_or(Value::Null);
            json!({
                "name": f["name"], "kind": "formula", "desc": f["desc"], "homepage": f["homepage"], "version": inst["version"],
                "latest": f["versions"]["stable"], "outdated": f["outdated"], "pinned": f["pinned"], "onRequest": inst["installed_on_request"],
                "dependency": inst["installed_as_dependency"], "tap": f["tap"],
            })
        })
        .collect();
    let casks: Vec<Value> = v["casks"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|c| {
            json!({
                "name": c["token"], "kind": "cask", "title": c["name"].get(0), "desc": c["desc"], "homepage": c["homepage"], "version": c["installed"],
                "latest": c["version"], "outdated": c["outdated"], "autoUpdates": c["auto_updates"], "tap": c["tap"],
            })
        })
        .collect();
    Ok(json!({ "formulae": formulae, "casks": casks }))
}

pub fn outdated() -> Result<Value, String> {
    let v = run_json(&["outdated", "--json=v2", "--greedy"], 180)?;
    let mut out = Vec::new();
    for f in v["formulae"].as_array().cloned().unwrap_or_default() {
        out.push(json!({ "name": f["name"], "kind": "formula", "version": f["installed_versions"].get(0), "latest": f["current_version"], "pinned": f["pinned"] }));
    }
    for c in v["casks"].as_array().cloned().unwrap_or_default() {
        out.push(json!({ "name": c["name"], "kind": "cask", "version": c["installed_versions"].get(0), "latest": c["current_version"] }));
    }
    Ok(json!({ "items": out }))
}

pub fn search(q: &str) -> Result<Value, String> {
    let q = q.trim();
    if q.is_empty() || q.starts_with('-') {
        return Ok(json!({ "formulae": [], "casks": [] }));
    }
    let b = brew()?;
    let mut c = std::process::Command::new(&b);
    c.args(["search", q]);
    for (k, v) in env() {
        c.env(k, v);
    }
    let text = run_with_timeout(c, Duration::from_secs(60))?;
    let (mut f, mut k) = (Vec::new(), Vec::new());
    let mut section = "formulae";
    for l in text.lines() {
        let l = l.trim();
        if l.starts_with("==> Formulae") {
            section = "formulae";
        } else if l.starts_with("==> Casks") {
            section = "casks";
        } else if !l.is_empty() && !l.starts_with("==>") && !l.contains(' ') {
            let name = l.trim_end_matches(" ✔").to_string();
            if section == "formulae" {
                f.push(name)
            } else {
                k.push(name)
            }
        }
    }
    Ok(json!({ "formulae": f, "casks": k }))
}

pub fn info(name: &str, cask: bool) -> Result<Value, String> {
    let v = run_json(&["info", "--json=v2", if cask { "--cask" } else { "--formula" }, name], 60)?;
    Ok(if cask { v["casks"][0].clone() } else { v["formulae"][0].clone() })
}

pub fn taps() -> Result<Value, String> {
    let b = brew()?;
    let o = cmd(&b, &["tap"], Duration::from_secs(30));
    Ok(json!({ "taps": o.stdout.lines().map(|l| l.trim()).filter(|l| !l.is_empty()).collect::<Vec<_>>() }))
}

fn valid_name(n: &str) -> bool {
    !n.is_empty() && !n.starts_with('-') && n.chars().all(|c| c.is_ascii_alphanumeric() || "@._+-/".contains(c))
}

/// Runs a Homebrew action in a task, streaming output.
pub fn action(ctx: &Ctx, action: &str, names: &[String], cask: bool, zap: bool) -> Result<Value, String> {
    let b = brew()?;
    for n in names {
        if !valid_name(n) {
            return Err(format!("“{n}” isn't a valid Homebrew name."));
        }
    }
    let mut args: Vec<String> = match action {
        "install" => vec!["install".into()],
        "uninstall" => vec!["uninstall".into()],
        "upgrade" => vec!["upgrade".into()],
        "reinstall" => vec!["reinstall".into()],
        "pin" => vec!["pin".into()],
        "unpin" => vec!["unpin".into()],
        "update" => vec!["update".into()],
        "cleanup" => vec!["cleanup".into(), "--prune=all".into(), "-s".into()],
        "autoremove" => vec!["autoremove".into()],
        "doctor" => vec!["doctor".into()],
        "tap" => vec!["tap".into()],
        "untap" => vec!["untap".into()],
        _ => return Err("Unknown Homebrew action".into()),
    };
    if ["install", "uninstall", "upgrade", "reinstall"].contains(&action) && !names.is_empty() {
        args.push(if cask { "--cask".into() } else { "--formula".into() });
    }
    if action == "uninstall" && cask && zap {
        args.push("--zap".into());
    }
    if action == "upgrade" && names.is_empty() {
        args.push("--greedy".into());
    }
    args.extend(names.iter().cloned());
    let env = env();
    let envs: Vec<(&str, &str)> = env.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let ok = run_logged(ctx, &b, &refs, &envs);
    // doctor exits non-zero when it has warnings; that's still a finished run
    if ok || action == "doctor" {
        Ok(json!({ "ok": ok }))
    } else {
        Err(format!("brew {action} didn't finish. See the output for details."))
    }
}
