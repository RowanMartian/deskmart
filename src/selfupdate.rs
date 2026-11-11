//! Updating Deskmart itself in place: download the latest GitHub build, check it, swap it in, relaunch.

use crate::tasks::Ctx;
use crate::util::*;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const BUNDLE_ID: &str = "app.deskmart.mac";
const ASSET: &str = "Deskmart-mac.zip";

/// The .app this process runs from (…/Deskmart.app/Contents/MacOS/Deskmart).
pub fn current_bundle() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?.canonicalize().ok()?;
    let b = exe.parent()?.parent()?.parent()?.to_path_buf();
    b.extension().map(|e| e == "app").unwrap_or(false).then_some(b)
}

fn client() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .user_agent(format!("Deskmart/{}", crate::api::VERSION))
        .timeout(Duration::from_secs(900))
        .build()
        .map_err(|e| e.to_string())
}

/// The requirement macOS uses to recognise this app (`codesign -d -r-`).
fn designated_requirement(app: &Path) -> Option<String> {
    let o = cmd_quick("/usr/bin/codesign", &["-d", "-r-", &app.to_string_lossy()]);
    let text = format!("{}\n{}", o.stdout, o.stderr);
    text.lines().find_map(|l| l.trim().strip_prefix("designated => ").map(|r| r.to_string()))
}

fn sha256_hex(file: &Path) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    let mut f = std::fs::File::open(file).map_err(|e| e.to_string())?;
    let mut h = Sha256::new();
    std::io::copy(&mut f, &mut h).map_err(|e| e.to_string())?;
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

pub fn install(ctx: &Ctx, repo: &str, current_commit: &str, app_folders: &[String], history: &Path) -> Result<Value, String> {
    if !cfg!(target_os = "macos") {
        return Err("Updating in place only works in the Mac app.".into());
    }
    let bundle = current_bundle().ok_or("Couldn't tell where Deskmart is installed.")?;
    let bundle_s = bundle.to_string_lossy().to_string();
    if bundle_s.contains("/AppTranslocation/") || bundle_s.starts_with("/Volumes/") {
        return Err("Deskmart is running from the disk image or a temporary location. Drag it into Applications, open it from there, then update.".into());
    }

    // 1. Find the build
    ctx.progress(json!({ "step": "Checking the latest build" }));
    let http = client()?;
    let rel: Value = http
        .get(format!("https://api.github.com/repos/{repo}/releases/tags/latest"))
        .header("Accept", "application/vnd.github+json")
        .send()
        .and_then(|r| r.error_for_status())
        .map_err(|e| format!("Couldn't reach GitHub: {e}"))?
        .json()
        .map_err(|e| e.to_string())?;
    let latest = rel["target_commitish"].as_str().unwrap_or("").to_string();
    if !latest.is_empty() && current_commit.len() >= 7 && latest.starts_with(&current_commit[..7]) {
        return Err("You already have the latest build.".into());
    }
    let asset = rel["assets"].as_array().and_then(|a| a.iter().find(|x| x["name"] == ASSET)).cloned().ok_or("The latest release has no Deskmart-mac.zip.")?;
    let url = asset["browser_download_url"].as_str().unwrap_or("").to_string();
    if !url.starts_with("https://github.com/") {
        return Err("Unexpected download address.".into());
    }
    let total = asset["size"].as_u64().unwrap_or(0);

    // 2. Download
    let work = std::env::temp_dir().join(format!("deskmart-update-{}", now_ms()));
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let cleanup = scopeguard(work.clone());
    let zip = work.join(ASSET);
    ctx.log(format!("Downloading {url}"));
    {
        let mut resp = http.get(&url).send().and_then(|r| r.error_for_status()).map_err(|e| format!("Download failed: {e}"))?;
        let mut out = std::fs::File::create(&zip).map_err(|e| e.to_string())?;
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

    // 3. Check it
    ctx.progress(json!({ "step": "Checking the download" }));
    if let Some(want) = asset["digest"].as_str().and_then(|d| d.strip_prefix("sha256:")) {
        let got = sha256_hex(&zip)?;
        if !got.eq_ignore_ascii_case(want) {
            return Err("The download doesn't match what GitHub published. Nothing was changed.".into());
        }
        ctx.log("SHA-256 matches GitHub's record.");
    }
    let unpack = work.join("unpacked");
    let o = cmd("/usr/bin/ditto", &["-x", "-k", &zip.to_string_lossy(), &unpack.to_string_lossy()], Duration::from_secs(300));
    if !o.ok {
        return Err(format!("Couldn't unpack the update: {}", o.stderr.trim()));
    }
    let new_app = unpack.join("Deskmart.app");
    let info = read_plist(&new_app.join("Contents/Info.plist")).ok_or("The download doesn't contain Deskmart.")?;
    if pstr(&info, "CFBundleIdentifier").as_deref() != Some(BUNDLE_ID) {
        return Err("The download isn't Deskmart. Nothing was changed.".into());
    }
    if let (Some(built), false) = (pstr(&info, "DeskmartCommit"), latest.is_empty()) {
        if built.len() >= 7 && !latest.starts_with(&built[..7]) {
            return Err("The download is from a different build than the release says. Try again in a few minutes.".into());
        }
    }
    let v = cmd("/usr/bin/codesign", &["--verify", "--deep", "--strict", &new_app.to_string_lossy()], Duration::from_secs(120));
    if !v.ok {
        return Err(format!("The download's code signature is broken: {}", v.stderr.trim()));
    }
    // If this copy is signed with a certificate (not ad hoc), the new one must be signed by the same one.
    if let Some(req) = designated_requirement(&bundle).filter(|r| r.contains("certificate")) {
        let r = cmd("/usr/bin/codesign", &["--verify", &format!("-R={req}"), &new_app.to_string_lossy()], Duration::from_secs(60));
        if !r.ok {
            return Err("The new build isn't signed with the same certificate as this one, so it wasn't installed. Download it from the release page instead.".into());
        }
        ctx.log("Signed by the same certificate as this copy.");
    }

    // 4. Swap: stage next to the current copy, move the current copy to the Trash, move the new one in.
    ctx.progress(json!({ "step": "Installing" }));
    let parent_dir = bundle.parent().ok_or("Odd install location.")?;
    let staged = parent_dir.join(format!(".Deskmart-update-{}.app", now_ms()));
    let staged_s = staged.to_string_lossy().to_string();
    let copied = cmd("/usr/bin/ditto", &[&new_app.to_string_lossy(), &staged_s], Duration::from_secs(300)).ok;
    if copied {
        let out = crate::trash::trash(&[bundle_s.clone()], "Deskmart (previous version)", app_folders, history);
        if out.moved.is_empty() {
            let _ = std::fs::remove_dir_all(&staged);
            return Err(out.failed.first().map(|f| f.1.clone()).unwrap_or_else(|| "Couldn't move the current copy aside.".into()));
        }
        if let Err(e) = std::fs::rename(&staged, &bundle) {
            // Put the old copy back so Deskmart isn't left missing.
            let _ = std::fs::rename(&out.moved[0].trashed, &bundle);
            let _ = std::fs::remove_dir_all(&staged);
            return Err(format!("Couldn't install the update: {e}"));
        }
    } else {
        // Applications isn't writable by this user: do the swap with the admin password.
        let trash = trash_dir().join(format!("Deskmart (previous version)_{}", now_secs()));
        let script = format!(
            "/usr/bin/ditto {new} {staged} && /bin/mkdir -p {tr} && /bin/mv {cur} {tr}/ && /bin/mv {staged} {cur}",
            new = sh_quote(&new_app.to_string_lossy()),
            staged = sh_quote(&staged_s),
            tr = sh_quote(&trash.to_string_lossy()),
            cur = sh_quote(&bundle_s)
        );
        run_as_admin(&script, "Deskmart wants to install its update.")?;
    }
    drop(cleanup);
    ctx.log("Installed. Relaunching…");
    relaunch_after_exit(&bundle_s);
    Ok(json!({ "ok": true, "relaunch": true, "commit": latest.chars().take(7).collect::<String>() }))
}

/// Starts a tiny detached shell that waits for this process to quit, then opens the app again.
fn relaunch_after_exit(bundle: &str) {
    use std::os::unix::process::CommandExt;
    let pid = std::process::id().to_string();
    let _ = std::process::Command::new("/bin/sh")
        .args(["-c", "while /bin/kill -0 \"$1\" 2>/dev/null; do /bin/sleep 0.2; done; /bin/sleep 0.4; /usr/bin/open \"$2\"", "sh", &pid, bundle])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .process_group(0)
        .spawn();
}

/// Removes the temp folder when the update finishes or fails.
struct Guard(PathBuf);
impl Drop for Guard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn scopeguard(p: PathBuf) -> Guard {
    Guard(p)
}
