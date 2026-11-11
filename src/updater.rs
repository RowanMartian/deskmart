//! Apps Updater: finds updates from Sparkle feeds, the Mac App Store and Homebrew, and installs
//! Sparkle updates after checking their EdDSA signature (or, without a key, the code signature and Team ID).

use crate::apps::AppInfo;
use crate::tasks::Ctx;
use crate::util::*;
use quick_xml::events::Event;
use rayon::prelude::*;
use serde_json::{json, Value};
use std::path::Path;
use std::time::Duration;

fn client() -> reqwest::blocking::Client {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(20))
        .user_agent(format!("Pearfin/{} Sparkle/2", crate::api::VERSION))
        .build()
        .expect("http client")
}

#[derive(Default, Debug, Clone)]
pub struct Item {
    pub title: String,
    pub version: String,
    pub short: String,
    pub url: String,
    pub length: u64,
    pub ed_signature: Option<String>,
    pub notes: String,
    pub notes_link: Option<String>,
    pub min_os: Option<String>,
    pub date: String,
    pub channel: Option<String>,
}

/// Parses a Sparkle appcast.
pub fn parse_appcast(xml: &str) -> Vec<Item> {
    let mut r = quick_xml::Reader::from_str(xml);
    r.config_mut().trim_text(true);
    let mut items = Vec::new();
    let mut cur: Option<Item> = None;
    let mut tag = String::new();
    let attr = |e: &quick_xml::events::BytesStart, name: &str| -> Option<String> {
        e.attributes().flatten().find(|a| a.key.as_ref() == name.as_bytes()).and_then(|a| a.unescape_value().ok()).map(|v| v.to_string())
    };
    loop {
        match r.read_event() {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                if name == "item" {
                    cur = Some(Item::default());
                } else if let Some(it) = cur.as_mut() {
                    if name == "enclosure" {
                        it.url = attr(&e, "url").unwrap_or_default();
                        it.length = attr(&e, "length").and_then(|x| x.parse().ok()).unwrap_or(0);
                        if let Some(v) = attr(&e, "sparkle:version") {
                            it.version = v;
                        }
                        if let Some(v) = attr(&e, "sparkle:shortVersionString") {
                            it.short = v;
                        }
                        it.ed_signature = attr(&e, "sparkle:edSignature");
                    }
                }
                tag = name;
            }
            Ok(Event::Text(t)) => {
                if let Some(it) = cur.as_mut() {
                    let v = t.decode().map(|x| x.to_string()).unwrap_or_default();
                    match tag.as_str() {
                        "title" => it.title = v,
                        "sparkle:version" => it.version = v,
                        "sparkle:shortVersionString" => it.short = v,
                        "sparkle:minimumSystemVersion" => it.min_os = Some(v),
                        "sparkle:releaseNotesLink" => it.notes_link = Some(v),
                        "sparkle:channel" => it.channel = Some(v),
                        "pubDate" => it.date = v,
                        "description" => it.notes = v,
                        _ => {}
                    }
                }
            }
            Ok(Event::CData(t)) => {
                if let Some(it) = cur.as_mut() {
                    if tag == "description" {
                        it.notes = String::from_utf8_lossy(&t).to_string();
                    }
                }
            }
            Ok(Event::End(e)) => {
                if e.name().as_ref() == b"item" {
                    if let Some(it) = cur.take() {
                        if !it.url.is_empty() && !it.version.is_empty() {
                            items.push(it);
                        }
                    }
                }
                tag.clear();
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    items
}

fn os_ok(min: &Option<String>) -> bool {
    let (v, _) = crate::tools::macos_version();
    match min {
        Some(m) if !v.is_empty() => cmp_versions(&v, m) != std::cmp::Ordering::Less,
        _ => true,
    }
}

fn sparkle_check(a: &AppInfo) -> Option<Value> {
    let feed = a.feed_url.as_ref()?;
    if !feed.starts_with("https://") && !feed.starts_with("http://") {
        return None;
    }
    let text = client().get(feed).send().ok()?.text().ok()?;
    let mut items: Vec<Item> = parse_appcast(&text).into_iter().filter(|i| i.channel.is_none() && os_ok(&i.min_os)).collect();
    items.sort_by(|x, y| cmp_versions(&y.version, &x.version));
    let best = items.into_iter().next()?;
    let current = a.build.clone().unwrap_or_else(|| a.version.clone());
    if cmp_versions(&best.version, &current) != std::cmp::Ordering::Greater {
        return None;
    }
    let shown = if best.short.is_empty() { best.version.clone() } else { best.short.clone() };
    if !best.short.is_empty() && cmp_versions(&best.short, &a.version) == std::cmp::Ordering::Less {
        return None;
    }
    Some(json!({
        "source": "sparkle", "path": a.visible_path(), "name": a.name, "bundleId": a.bundle_id, "current": a.version, "latest": shown,
        "url": best.url, "size": best.length, "notes": best.notes.chars().take(20000).collect::<String>(), "notesLink": best.notes_link, "date": best.date,
        "signed": best.ed_signature.is_some(),
    }))
}

fn store_country() -> String {
    let langs = crate::tools::preferred_languages();
    langs
        .first()
        .and_then(|l| l.split('-').nth(1))
        .filter(|c| c.len() == 2 && c.chars().all(|x| x.is_ascii_alphabetic()))
        .map(|c| c.to_lowercase())
        .unwrap_or_else(|| "us".into())
}

fn app_store_check(apps: &[&AppInfo]) -> Vec<Value> {
    let country = store_country();
    apps.par_chunks(40)
        .flat_map(|chunk| {
            let ids = chunk.iter().map(|a| a.bundle_id.as_str()).collect::<Vec<_>>().join(",");
            let url = format!("https://itunes.apple.com/lookup?bundleId={ids}&country={country}&entity=macSoftware");
            let res: Option<Value> = client().get(&url).send().ok().and_then(|r| r.json().ok());
            let mut out = Vec::new();
            for r in res.map(|v| v["results"].as_array().cloned().unwrap_or_default()).unwrap_or_default() {
                let bid = r["bundleId"].as_str().unwrap_or("");
                let Some(a) = chunk.iter().find(|a| a.bundle_id == bid) else { continue };
                let latest = r["version"].as_str().unwrap_or("");
                if !latest.is_empty() && cmp_versions(latest, &a.version) == std::cmp::Ordering::Greater {
                    out.push(json!({
                        "source": "appstore", "path": a.visible_path(), "name": a.name, "bundleId": a.bundle_id, "current": a.version, "latest": latest,
                        "url": format!("macappstore://apps.apple.com/app/id{}", r["trackId"]), "notes": r["releaseNotes"], "size": r["fileSizeBytes"].as_str().and_then(|x| x.parse::<u64>().ok()),
                        "date": r["currentVersionReleaseDate"],
                    }));
                }
            }
            out
        })
        .collect()
}

pub fn check(ctx: &Ctx, apps: &[AppInfo], ignored: &[String]) -> Value {
    let apps: Vec<&AppInfo> = apps.iter().filter(|a| !ignored.contains(&a.bundle_id) && !a.web_app && !a.steam).collect();
    let sparkle: Vec<&AppInfo> = apps.iter().copied().filter(|a| a.sparkle && a.cask.is_none() && !a.app_store).collect();
    ctx.log(format!("Checking {} Sparkle feeds…", sparkle.len()));
    ctx.progress(json!({ "step": "Sparkle feeds" }));
    let mut found: Vec<Value> = sparkle.par_iter().filter_map(|a| sparkle_check(a)).collect();
    let store: Vec<&AppInfo> = apps.iter().copied().filter(|a| a.app_store).collect();
    if !store.is_empty() && !ctx.stopped() {
        ctx.log(format!("Checking {} App Store apps…", store.len()));
        ctx.progress(json!({ "step": "App Store" }));
        found.extend(app_store_check(&store));
    }
    if crate::apps::brew_bin().is_some() && !ctx.stopped() {
        ctx.log("Checking Homebrew…");
        ctx.progress(json!({ "step": "Homebrew" }));
        if let Ok(v) = crate::brew::outdated() {
            for it in v["items"].as_array().cloned().unwrap_or_default() {
                let name = it["name"].as_str().unwrap_or("").to_string();
                let app = apps.iter().find(|a| a.cask.as_deref() == Some(name.as_str()));
                found.push(json!({
                    "source": "homebrew", "kind": it["kind"], "token": name, "name": app.map(|a| a.name.clone()).unwrap_or(name.clone()),
                    "path": app.map(|a| a.visible_path()), "bundleId": app.map(|a| a.bundle_id.clone()), "current": it["version"], "latest": it["latest"],
                }));
            }
        }
    }
    found.sort_by_key(|v| v["name"].as_str().unwrap_or("").to_lowercase());
    ctx.log(format!("{} update(s) available", found.len()));
    json!({ "updates": found, "checked": now_secs() })
}

// ------------------------------------------------------------------ installing a Sparkle update

fn verify_ed25519(file: &Path, sig_b64: &str, key_b64: &str) -> Result<(), String> {
    use base64::Engine;
    use ed25519_dalek::{Signature, Verifier, VerifyingKey};
    let b = base64::engine::general_purpose::STANDARD;
    let key: [u8; 32] = b.decode(key_b64.trim()).map_err(|_| "bad public key")?.try_into().map_err(|_| "bad public key length")?;
    let sig: [u8; 64] = b.decode(sig_b64.trim()).map_err(|_| "bad signature")?.try_into().map_err(|_| "bad signature length")?;
    let data = std::fs::read(file).map_err(|e| e.to_string())?;
    VerifyingKey::from_bytes(&key).map_err(|_| "bad public key")?.verify(&data, &Signature::from_bytes(&sig)).map_err(|_| "The download's signature doesn't match. It was not installed.".to_string())
}

fn find_app(dir: &Path, depth: usize) -> Option<std::path::PathBuf> {
    for n in list_dir(&dir.to_string_lossy()) {
        let p = dir.join(&n);
        if n.ends_with(".app") && p.is_dir() {
            return Some(p);
        }
    }
    if depth > 0 {
        for n in list_dir(&dir.to_string_lossy()) {
            let p = dir.join(&n);
            if p.is_dir() && !n.starts_with('.') && !n.starts_with("__MACOSX") {
                if let Some(x) = find_app(&p, depth - 1) {
                    return Some(x);
                }
            }
        }
    }
    None
}

pub fn install_sparkle(ctx: &Ctx, app: &AppInfo, cfg: &Settings, history: &Path) -> Result<Value, String> {
    if !cfg!(target_os = "macos") {
        return Err("Installing updates only works on macOS.".into());
    }
    let feed = app.feed_url.clone().ok_or("This app has no update feed.")?;
    ctx.log(format!("Reading {feed}"));
    let text = client().get(&feed).send().map_err(|e| e.to_string())?.text().map_err(|e| e.to_string())?;
    let mut items: Vec<Item> = parse_appcast(&text).into_iter().filter(|i| i.channel.is_none() && os_ok(&i.min_os)).collect();
    items.sort_by(|x, y| cmp_versions(&y.version, &x.version));
    let item = items.into_iter().next().ok_or("The feed lists no update.")?;
    if !item.url.starts_with("https://") {
        return Err("The update isn't offered over a secure connection, so Pearfin won't install it. Use the app's own updater.".into());
    }
    let work = std::env::temp_dir().join(format!("pearfin-update-{}", now_ms()));
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let file = work.join(file_name(item.url.split('?').next().unwrap_or("update")));
    ctx.log(format!("Downloading {}", item.url));
    {
        let mut resp = client().get(&item.url).timeout(Duration::from_secs(1800)).send().map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            return Err(format!("Download failed ({})", resp.status()));
        }
        let total = resp.content_length().unwrap_or(item.length);
        let mut out = std::fs::File::create(&file).map_err(|e| e.to_string())?;
        let mut buf = vec![0u8; 256 * 1024];
        let mut done = 0u64;
        loop {
            if ctx.stopped() {
                return Err("Cancelled.".into());
            }
            let n = std::io::Read::read(&mut resp, &mut buf).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            std::io::Write::write_all(&mut out, &buf[..n]).map_err(|e| e.to_string())?;
            done += n as u64;
            ctx.progress(json!({ "step": "Downloading", "done": done, "total": total }));
        }
    }
    // Signature
    let info = crate::apps::info_plist(Path::new(&app.path)).unwrap_or_default();
    let key = pstr(&info, "SUPublicEDKey");
    match (&key, &item.ed_signature) {
        (Some(k), Some(sg)) => {
            verify_ed25519(&file, sg, k)?;
            ctx.log("EdDSA signature verified.");
        }
        (Some(_), None) => return Err("The update isn't signed even though the app expects it. It was not installed.".into()),
        _ => ctx.log("No EdDSA key in the app; will check the code signature instead."),
    }
    // Unpack
    ctx.progress(json!({ "step": "Unpacking" }));
    let unpack = work.join("unpacked");
    std::fs::create_dir_all(&unpack).map_err(|e| e.to_string())?;
    let name = file.to_string_lossy().to_lowercase();
    let mut mount: Option<String> = None;
    let staged = if name.ends_with(".zip") {
        let o = cmd("/usr/bin/ditto", &["-x", "-k", &file.to_string_lossy(), &unpack.to_string_lossy()], Duration::from_secs(600));
        if !o.ok {
            return Err(o.stderr);
        }
        find_app(&unpack, 2)
    } else if name.ends_with(".dmg") {
        let mp = work.join("mnt").to_string_lossy().to_string();
        let o = cmd("/usr/bin/hdiutil", &["attach", "-nobrowse", "-readonly", "-noautoopen", "-mountpoint", &mp, &file.to_string_lossy()], Duration::from_secs(300));
        if !o.ok {
            return Err(format!("Couldn't open the disk image: {}", o.stderr.trim()));
        }
        mount = Some(mp.clone());
        find_app(Path::new(&mp), 1).and_then(|a| {
            let dst = unpack.join(file_name(&a.to_string_lossy()));
            cmd("/usr/bin/ditto", &[&a.to_string_lossy(), &dst.to_string_lossy()], Duration::from_secs(600)).ok.then_some(dst)
        })
    } else if name.ends_with(".tar.gz") || name.ends_with(".tgz") || name.ends_with(".tar.xz") || name.ends_with(".tar.bz2") || name.ends_with(".tar") {
        let o = cmd("/usr/bin/tar", &["-xf", &file.to_string_lossy(), "-C", &unpack.to_string_lossy()], Duration::from_secs(600));
        if !o.ok {
            return Err(o.stderr);
        }
        find_app(&unpack, 2)
    } else if name.ends_with(".pkg") {
        let _ = std::process::Command::new("/usr/bin/open").arg(&file).spawn();
        return Ok(json!({ "ok": true, "opened": true, "message": "The update is an installer package; it opened in Installer." }));
    } else {
        None
    };
    if let Some(mp) = &mount {
        let _ = cmd("/usr/bin/hdiutil", &["detach", mp, "-quiet"], Duration::from_secs(60));
    }
    let staged = staged.ok_or("Couldn't find the app inside the download.")?;
    let new_info = crate::apps::info_plist(&staged).ok_or("The downloaded app looks damaged.")?;
    if pstr(&new_info, "CFBundleIdentifier").as_deref() != Some(app.bundle_id.as_str()) {
        return Err("The download is a different app than the one installed. It was not installed.".into());
    }
    let v = cmd("/usr/bin/codesign", &["--verify", "--deep", "--strict", &staged.to_string_lossy()], Duration::from_secs(300));
    if !v.ok && key.is_none() {
        return Err(format!("The downloaded app's code signature isn't valid: {}", v.stderr.trim()));
    }
    if key.is_none() {
        let new_team = crate::macho::bundle_executable(&staged).and_then(|e| crate::macho::signature(&e)).and_then(|s| s.team_id);
        if app.team_id.is_some() && new_team != app.team_id {
            return Err("The download is signed by a different developer. It was not installed.".into());
        }
    }
    // Swap
    ctx.progress(json!({ "step": "Installing" }));
    if !crate::apps::running_pids(&app.visible_path()).is_empty() {
        ctx.log(format!("Quitting {}…", app.name));
        crate::apps::quit_app(app);
    }
    let target = app.visible_path();
    let out = crate::trash::trash(&[target.clone()], &format!("{} (old version)", app.name), &cfg.app_folders, history);
    if out.moved.is_empty() {
        return Err(out.failed.first().map(|f| f.1.clone()).unwrap_or_else(|| "Couldn't move the old version to the Trash.".into()));
    }
    let o = cmd("/usr/bin/ditto", &[&staged.to_string_lossy(), &target], Duration::from_secs(600));
    if !o.ok {
        let script = format!("/usr/bin/ditto {} {}", sh_quote(&staged.to_string_lossy()), sh_quote(&target));
        run_as_admin(&script, &format!("Pearfin wants to install the update for {}.", app.name))?;
    }
    let _ = std::fs::remove_dir_all(&work);
    let shown = if item.short.is_empty() { item.version } else { item.short };
    ctx.log(format!("Installed {} {}", app.name, shown));
    Ok(json!({ "ok": true, "version": shown }))
}

#[cfg(test)]
mod tests {
    #[test]
    fn appcast() {
        let xml = r#"<?xml version="1.0"?><rss xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle"><channel>
        <item><title>2.1</title><sparkle:version>210</sparkle:version><sparkle:shortVersionString>2.1</sparkle:shortVersionString>
        <description><![CDATA[<p>Fixes</p>]]></description><enclosure url="https://x/App-2.1.zip" length="10" type="application/octet-stream" sparkle:edSignature="abc"/></item>
        <item><title>2.0</title><enclosure url="https://x/App-2.0.zip" sparkle:version="200" sparkle:shortVersionString="2.0" length="9"/></item>
        </channel></rss>"#;
        let v = super::parse_appcast(xml);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].version, "210");
        assert_eq!(v[0].short, "2.1");
        assert_eq!(v[0].ed_signature.as_deref(), Some("abc"));
        assert!(v[0].notes.contains("Fixes"));
        assert_eq!(v[1].short, "2.0");
    }
}
