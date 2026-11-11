//! Local HTTP API used by the interface (served only to the app window, on localhost, with a per-launch token).

use crate::apps::{self, AppInfo};
use crate::util::*;
use crate::{brew, finder, icons, macho, sentinel, tasks::Tasks, tools, trash, updater};
use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, Method, Uri};
use axum::response::{IntoResponse, Response};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Baked in by the GitHub build (see .github/workflows); absent in local builds, which disables update checks.
pub const BUILD_REPO: Option<&str> = option_env!("DESKMART_REPO");
pub const BUILD_COMMIT: Option<&str> = option_env!("DESKMART_COMMIT");

const INDEX_HTML: &str = include_str!("../web/index.html");
const ICON_SVG: &str = include_str!("../web/icon.svg");
const ICON_PNG: &[u8] = include_bytes!("../web/icon-180.png");
const FA_CSS: &str = include_str!("../web/fa/fontawesome.css");
const FA_FONT: &[u8] = include_bytes!("../web/fa/fa-solid-900.woff2");

pub struct App {
    pub token: String,
    pub cfg: Mutex<Settings>,
    pub cfg_file: PathBuf,
    pub data_dir: PathBuf,
    pub history_file: PathBuf,
    pub apps: RwLock<Option<Vec<AppInfo>>>,
    pub tasks: Tasks,
    pub quit: Mutex<Option<Box<dyn Fn() + Send>>>,
    /// Native actions the window performs (open URL scheme handlers etc.).
    update: Mutex<(u64, Option<Value>)>,
}

impl App {
    pub fn new() -> Arc<App> {
        let data_dir = data_dir();
        let _ = std::fs::create_dir_all(&data_dir);
        let cfg_file = data_dir.join("settings.json");
        let token: String = {
            use rand::Rng;
            let mut r = rand::rng();
            (0..24).map(|_| format!("{:02x}", r.random::<u8>())).collect()
        };
        Arc::new(App {
            token,
            cfg: Mutex::new(Settings::load(&cfg_file)),
            cfg_file,
            history_file: data_dir.join("history.json"),
            data_dir,
            apps: RwLock::new(None),
            tasks: Tasks::new(),
            quit: Mutex::new(None),
            update: Mutex::new((0, None)),
        })
    }

    pub fn busy(&self) -> bool {
        self.tasks.running() > 0
    }
    pub fn settings(&self) -> Settings {
        self.cfg.lock().unwrap().clone()
    }
    fn save(&self) {
        self.cfg.lock().unwrap().save(&self.cfg_file);
    }
    fn edit(&self, f: impl FnOnce(&mut Settings)) {
        f(&mut self.cfg.lock().unwrap());
        self.save();
    }

    /// Installed apps (cached; `refresh` rescans).
    pub fn apps(&self, refresh: bool) -> Vec<AppInfo> {
        if !refresh {
            if let Some(a) = self.apps.read().unwrap().as_ref() {
                return a.clone();
            }
        }
        let folders = self.settings().app_folders;
        let mut list = apps::load_all(&folders, false);
        apps::fill_sizes(&mut list);
        log("Apps", format!("Found {} apps", list.len()));
        *self.apps.write().unwrap() = Some(list.clone());
        list
    }
    /// One app with full details (signature, entitlements).
    pub fn app(&self, path: &str) -> Option<AppInfo> {
        let cached = self.apps.read().unwrap().as_ref().and_then(|v| v.iter().find(|a| a.visible_path() == path || a.path == path).cloned());
        let mut a = match cached {
            Some(a) => a,
            None => apps::app_info(path, apps::spotlight_metadata(&[path.to_string()]).get(path), &apps::casks(), false)?,
        };
        if a.entitlements.is_empty() && a.team_id.is_none() {
            apps::fill_details(&mut a);
            if let Some(list) = self.apps.write().unwrap().as_mut() {
                if let Some(x) = list.iter_mut().find(|x| x.path == a.path) {
                    *x = a.clone();
                }
            }
        }
        Some(a)
    }
    fn drop_missing_apps(&self) {
        if let Some(list) = self.apps.write().unwrap().as_mut() {
            list.retain(|a| exists(&a.visible_path()));
        }
    }
    fn refresh_app(&self, path: &str) {
        let fresh = apps::app_info(path, None, &apps::casks(), false).map(|mut a| {
            a.size = size_str(&a.visible_path());
            a
        });
        icons::forget(path);
        if let Some(list) = self.apps.write().unwrap().as_mut() {
            if let Some(i) = list.iter().position(|x| x.visible_path() == path) {
                match fresh {
                    Some(a) => list[i] = a,
                    None => {
                        list.remove(i);
                    }
                }
            }
        }
    }
}

// ------------------------------------------------------------------ dispatcher

pub fn router(app: Arc<App>) -> axum::Router {
    axum::Router::new().fallback(handle).with_state(app)
}

async fn handle(State(app): State<Arc<App>>, method: Method, uri: Uri, headers: HeaderMap, body: Body) -> Response {
    match dispatch(&app, method, uri, headers, body).await {
        Ok(r) => r,
        Err(e) => e.into_response(),
    }
}

fn query(uri: &Uri) -> HashMap<String, String> {
    axum::extract::Query::<HashMap<String, String>>::try_from_uri(uri).map(|q| q.0).unwrap_or_default()
}

async fn json_body(body: Body) -> R<Value> {
    let bytes = axum::body::to_bytes(body, 8_000_000).await.map_err(|_| ApiError::new(413, "Too large"))?;
    if bytes.is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_slice(&bytes).map_err(|_| ApiError::new(400, "Invalid JSON"))
}

/// Runs blocking work (file system scans) off the async threads.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> R<T> {
    tokio::task::spawn_blocking(f).await.map_err(|_| ApiError::new(500, "Something went wrong."))
}

async fn dispatch(app: &Arc<App>, method: Method, uri: Uri, headers: HeaderMap, body: Body) -> R<Response> {
    // DNS-rebinding guard: only answer to localhost names.
    let host = headers.get("host").and_then(|h| h.to_str().ok()).unwrap_or("");
    let host = host.rsplit_once(':').map(|(h, _)| h).unwrap_or(host);
    if !["localhost", "127.0.0.1", "[::1]"].contains(&host) {
        return Err(ApiError::new(403, "Forbidden"));
    }
    let path = uri.path().to_string();
    if path == "/api/hello" {
        return Ok(ok(json!({ "app": "deskmart", "version": VERSION })));
    }
    if !path.starts_with("/api/") {
        return static_file(app, &path);
    }
    let q = query(&uri);
    let token = headers.get("x-deskmart-token").and_then(|h| h.to_str().ok()).map(String::from).or_else(|| q.get("t").cloned());
    if token.as_deref() != Some(app.token.as_str()) {
        return Err(ApiError::new(403, "Deskmart restarted — reload the page.").reason("token"));
    }
    if method == Method::GET && path == "/api/icon" {
        return icon(&q).await;
    }
    let b = if method == Method::POST { json_body(body).await? } else { json!({}) };
    let qs = |k: &str| q.get(k).cloned().unwrap_or_default();
    let a = app.clone();
    let v: Value = match (method.as_str(), path.as_str()) {
        ("GET", "/api/state") => blocking(move || state(&a)).await?,
        ("POST", "/api/settings") => {
            let old = app.settings();
            app.edit(|c| c.merge(&b));
            let new = app.settings();
            if old.app_folders != new.app_folders {
                *app.apps.write().unwrap() = None;
            }
            if old.sentinel != new.sentinel {
                if let Err(e) = sentinel::set_enabled(new.sentinel) {
                    app.edit(|c| c.sentinel = old.sentinel);
                    return Err(ApiError::new(500, e));
                }
            }
            json!({ "settings": app.settings() })
        }
        ("GET", "/api/apps") => {
            let refresh = qs("refresh") == "1";
            blocking(move || json!({ "apps": a.apps(refresh), "nativeArch": native_arch() })).await?
        }
        ("GET", "/api/app") => {
            let p = qs("path");
            blocking(move || {
                let info = a.app(&p).ok_or_else(|| ApiError::new(404, "That app isn't there anymore."))?;
                let running = !apps::running_pids(&info.visible_path()).is_empty();
                let savings = if info.arch == "universal" { macho::bundle_savings(Path::new(&info.path)) } else { 0 };
                let sens = a.settings().app_sensitivity.get(&info.visible_path()).cloned();
                Ok::<_, ApiError>(json!({ "app": info, "running": running, "lipoSavings": savings, "sensitivity": sens }))
            })
            .await??
        }
        ("POST", "/api/app/files") => {
            let p = s(&b, "path");
            let cfg = app.settings();
            blocking(move || {
                let info = a.app(&p).ok_or_else(|| ApiError::new(404, "That app isn't there anymore."))?;
                Ok::<_, ApiError>(json!(finder::find_leftovers(&info, &cfg)))
            })
            .await??
        }
        ("POST", "/api/app/sensitivity") => {
            let p = s(&b, "path");
            let lvl = s(&b, "level");
            app.edit(|c| {
                if lvl.is_empty() {
                    c.app_sensitivity.remove(&p);
                } else {
                    c.app_sensitivity.insert(p.clone(), lvl.clone());
                }
            });
            json!({ "ok": true })
        }
        ("POST", "/api/app/quit") => {
            let p = s(&b, "path");
            blocking(move || {
                if let Some(info) = a.app(&p) {
                    apps::quit_app(&info);
                }
                json!({ "running": !apps::running_pids(&p).is_empty() })
            })
            .await?
        }
        ("POST", "/api/trash") => trash_paths(app, b).await?,
        ("POST", "/api/reveal") => {
            let p = s(&b, "path");
            if exists(&p) && cfg!(target_os = "macos") {
                let _ = std::process::Command::new("/usr/bin/open").arg("-R").arg(&p).spawn();
            }
            json!({ "ok": true })
        }
        ("POST", "/api/open") => {
            let p = s(&b, "path");
            if exists(&p) && cfg!(target_os = "macos") {
                let _ = std::process::Command::new("/usr/bin/open").arg(&p).spawn();
            }
            json!({ "ok": true })
        }
        ("POST", "/api/openurl") => {
            let u = s(&b, "url");
            if (u.starts_with("https://") || u.starts_with("macappstore://")) && cfg!(target_os = "macos") {
                let _ = std::process::Command::new("/usr/bin/open").arg(&u).spawn();
            }
            json!({ "ok": true })
        }
        ("POST", "/api/exclude") => {
            let paths = strs(&b, "paths");
            let list = s(&b, "list");
            let remove = crate::util::b(&b, "remove");
            app.edit(|c| {
                let l = if list == "orphans" { &mut c.exclude_orphans } else { &mut c.exclude_apps };
                for p in paths {
                    if remove {
                        l.retain(|x| *x != p);
                    } else if !l.contains(&p) {
                        l.push(p);
                    }
                }
            });
            json!({ "settings": app.settings() })
        }
        ("POST", "/api/link") => {
            let appp = s(&b, "app");
            let paths = strs(&b, "paths");
            let unlink = crate::util::b(&b, "unlink");
            app.edit(|c| {
                if unlink {
                    if let Some(v) = c.associations.get_mut(&appp) {
                        v.retain(|x| !paths.contains(x));
                    }
                    c.associations.retain(|_, v| !v.is_empty());
                } else {
                    let v = c.associations.entry(appp.clone()).or_default();
                    for p in paths {
                        if !v.contains(&p) {
                            v.push(p);
                        }
                    }
                }
            });
            json!({ "ok": true })
        }
        ("GET", "/api/history") => {
            let h = trash::load_history(&app.history_file);
            json!({ "records": h.iter().map(|r| json!({ "id": r.id, "name": r.name, "folder": r.folder, "time": r.time, "size": r.size,
                "count": r.pairs.len(), "available": r.pairs.iter().filter(|p| exists(&p.trashed)).count(), "pairs": r.pairs })).collect::<Vec<_>>() })
        }
        ("POST", "/api/history/restore") => {
            let ids = strs(&b, "ids");
            let file = app.history_file.clone();
            let res = blocking(move || {
                let mut h = trash::load_history(&file);
                let mut restored = 0;
                let mut failed = Vec::new();
                for id in &ids {
                    if let Some(r) = h.iter().find(|r| &r.id == id).cloned() {
                        let (n, f) = trash::restore(&r);
                        restored += n;
                        failed.extend(f.into_iter().map(|(p, e)| json!({ "path": p, "error": e })));
                        if n > 0 {
                            h.retain(|x| &x.id != id);
                        }
                    }
                }
                trash::save_history(&file, &h);
                json!({ "restored": restored, "failed": failed })
            })
            .await?;
            *app.apps.write().unwrap() = None;
            res
        }
        ("POST", "/api/history/forget") => {
            let ids = strs(&b, "ids");
            let mut h = trash::load_history(&app.history_file);
            h.retain(|r| !ids.contains(&r.id));
            trash::save_history(&app.history_file, &h);
            json!({ "ok": true })
        }
        ("POST", "/api/trash/empty") => {
            blocking(trash::empty_trash).await?.map_err(|e| ApiError::new(500, e))?;
            json!({ "ok": true })
        }
        ("GET", "/api/languages") => {
            let p = qs("path");
            blocking(move || {
                let langs = tools::languages(&p);
                json!({ "languages": langs, "auto": tools::auto_prune_selection(&langs), "preferred": tools::preferred_languages() })
            })
            .await?
        }
        ("GET", "/api/orphans/start") | ("POST", "/api/orphans/start") => {
            let a2 = a.clone();
            let id = app.tasks.spawn("orphans", "Orphans", move |ctx| {
                ctx.log("Loading installed apps…");
                let mut list = a2.apps(false);
                ctx.progress(json!({ "step": "Reading app signatures", "done": 0, "total": list.len() }));
                use rayon::prelude::*;
                list.par_iter_mut().filter(|x| x.entitlements.is_empty()).for_each(|x| apps::fill_details(x));
                let cfg = a2.settings();
                let (items, denied) = finder::find_orphans(&list, &cfg, &ctx.stop, &|n, loc| ctx.progress(json!({ "step": tildify(loc), "found": n })));
                ctx.log(format!("Found {} orphaned items ({})", items.len(), fmt_size(items.iter().map(|x| x.size).sum())));
                Ok(json!({ "items": items, "denied": denied, "fullDiskAccess": tools::full_disk_access() }))
            });
            json!({ "task": id })
        }
        ("GET", "/api/lipo/scan") => {
            let a2 = a.clone();
            let id = app.tasks.spawn("lipo-scan", "Lipo", move |ctx| {
                let list = a2.apps(false);
                let uni: Vec<AppInfo> = list.into_iter().filter(|x| x.arch == "universal" && !x.wrapped).collect();
                let total = uni.len();
                let done = std::sync::atomic::AtomicUsize::new(0);
                use rayon::prelude::*;
                uni.par_iter().for_each(|x| {
                    if ctx.stopped() {
                        return;
                    }
                    let sav = macho::bundle_savings(Path::new(&x.path));
                    let n = done.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                    ctx.progress(json!({ "done": n, "total": total }));
                    if sav > 0 {
                        ctx.item(json!({ "path": x.visible_path(), "name": x.name, "size": x.size, "savings": sav, "system": x.system, "version": x.version }));
                    }
                });
                Ok(json!({ "ok": true }))
            });
            json!({ "task": id })
        }
        ("POST", "/api/lipo/thin") => {
            let paths = strs(&b, "paths");
            let prune = crate::util::b(&b, "prune");
            let a2 = a.clone();
            let id = app.tasks.spawn("lipo", "Lipo", move |ctx| {
                let mut saved = 0u64;
                let mut failures = Vec::new();
                for (i, p) in paths.iter().enumerate() {
                    if ctx.stopped() {
                        break;
                    }
                    ctx.progress(json!({ "done": i, "total": paths.len(), "step": file_name(p) }));
                    match thin_app(p) {
                        Ok(n) => {
                            saved += n;
                            ctx.log(format!("{}: saved {}", file_name(p), fmt_size(n)));
                        }
                        Err(e) => {
                            ctx.log(format!("{}: {e}", file_name(p)));
                            failures.push(json!({ "path": p, "error": e }));
                        }
                    }
                    if prune {
                        let langs = tools::languages(p);
                        let sel = tools::auto_prune_selection(&langs);
                        if !sel.is_empty() {
                            let out = trash::trash(&sel, &format!("{} - Translations", file_name(p).trim_end_matches(".app")), &a2.settings().app_folders, &a2.history_file);
                            ctx.log(format!("{}: removed {} translation folders", file_name(p), out.moved.len()));
                        }
                    }
                    a2.refresh_app(p);
                }
                Ok(json!({ "saved": saved, "failed": failures }))
            });
            json!({ "task": id })
        }
        ("GET", "/api/dev") => blocking(|| json!({ "environments": tools::dev_scan() })).await?,
        ("GET", "/api/dev/workspaces") => blocking(|| json!({ "items": tools::orphaned_workspaces() })).await?,
        ("POST", "/api/dev/contents") => {
            let paths = strs(&b, "paths");
            json!({ "paths": paths.iter().flat_map(|p| tools::contents_of(p)).collect::<Vec<_>>() })
        }
        ("GET", "/api/packages") => blocking(|| json!({ "packages": tools::packages() })).await?,
        ("GET", "/api/package/files") => {
            let id = qs("id");
            blocking(move || tools::package_files(&id)).await?
        }
        ("POST", "/api/package/forget") => {
            let id = s(&b, "id");
            blocking(move || tools::forget_package(&id)).await?.map_err(|e| ApiError::new(500, e))?;
            json!({ "ok": true })
        }
        ("GET", "/api/plugins") => blocking(|| json!({ "plugins": tools::plugins() })).await?,
        ("GET", "/api/services") => blocking(|| json!({ "services": tools::services() })).await?,
        ("POST", "/api/service") => {
            let (act, label, kind) = (s(&b, "action"), s(&b, "label"), s(&b, "kind"));
            let p = Some(s(&b, "path")).filter(|x| !x.is_empty());
            let out = blocking(move || tools::service_action(&act, &label, p.as_deref(), &kind)).await?.map_err(|e| ApiError::new(500, e))?;
            json!({ "ok": true, "output": out })
        }
        ("POST", "/api/search/start") => {
            let opts = crate::filesearch::Options {
                root: Some(s(&b, "root")).filter(|x| !x.is_empty()).unwrap_or_else(home),
                hidden: crate::util::b(&b, "hidden"),
                packages: crate::util::b(&b, "packages"),
                limit: 20000,
            };
            let filters = b.get("filters").cloned().unwrap_or(json!([]));
            let id = app.tasks.spawn("filesearch", "File Search", move |ctx| {
                let started = std::time::Instant::now();
                let n = crate::filesearch::search(&opts, &filters, &ctx.stop, |h| ctx.item(json!(h)), &|scanned| ctx.progress(json!({ "scanned": scanned, "ms": started.elapsed().as_millis() as u64 })))?;
                Ok(json!({ "found": n, "ms": started.elapsed().as_millis() as u64 }))
            });
            json!({ "task": id })
        }
        ("POST", "/api/sizes") => {
            let paths = strs(&b, "paths");
            blocking(move || {
                use rayon::prelude::*;
                let m: serde_json::Map<String, Value> = paths.par_iter().map(|p| (p.clone(), json!(size_str(p)))).collect::<Vec<_>>().into_iter().collect();
                json!({ "sizes": m })
            })
            .await?
        }
        ("POST", "/api/rename") => {
            let (p, name) = (s(&b, "path"), s(&b, "name"));
            if name.is_empty() || name.contains('/') || name == "." || name == ".." {
                return Err(ApiError::new(400, "That name can't be used."));
            }
            let dest = format!("{}/{}", parent(&p), name);
            if exists(&dest) {
                return Err(ApiError::new(409, "Something with that name is already there."));
            }
            std::fs::rename(&p, &dest)?;
            json!({ "path": dest })
        }
        ("GET", "/api/brew/status") => blocking(brew::status).await?,
        ("GET", "/api/brew/installed") => blocking(brew::installed).await?.map_err(|e| ApiError::new(500, e))?,
        ("GET", "/api/brew/outdated") => blocking(brew::outdated).await?.map_err(|e| ApiError::new(500, e))?,
        ("GET", "/api/brew/search") => {
            let term = qs("q");
            blocking(move || brew::search(&term)).await?.map_err(|e| ApiError::new(500, e))?
        }
        ("GET", "/api/brew/info") => {
            let (n, c) = (qs("name"), qs("cask") == "1");
            blocking(move || brew::info(&n, c)).await?.map_err(|e| ApiError::new(500, e))?
        }
        ("GET", "/api/brew/taps") => blocking(brew::taps).await?.map_err(|e| ApiError::new(500, e))?,
        ("POST", "/api/brew/action") => {
            let (act, names, cask, zap) = (s(&b, "action"), strs(&b, "names"), crate::util::b(&b, "cask"), crate::util::b(&b, "zap"));
            let a2 = a.clone();
            let id = app.tasks.spawn("brew", "Homebrew", move |ctx| {
                let r = brew::action(&ctx, &act, &names, cask, zap);
                if cask && ["install", "uninstall", "upgrade", "reinstall"].contains(&act.as_str()) {
                    *a2.apps.write().unwrap() = None;
                }
                r
            });
            json!({ "task": id })
        }
        ("POST", "/api/updater/check") => {
            let a2 = a.clone();
            let id = app.tasks.spawn("updater", "Updater", move |ctx| {
                let list = a2.apps(true);
                let mut list = list;
                use rayon::prelude::*;
                list.par_iter_mut().filter(|x| x.sparkle && x.team_id.is_none()).for_each(|x| apps::fill_details(x));
                let ign = a2.settings().updater_ignored;
                Ok(updater::check(&ctx, &list, &ign))
            });
            json!({ "task": id })
        }
        ("POST", "/api/updater/install") => {
            let p = s(&b, "path");
            let a2 = a.clone();
            let id = app.tasks.spawn("update-install", "Updater", move |ctx| {
                let info = a2.app(&p).ok_or("That app isn't there anymore.")?;
                let r = updater::install_sparkle(&ctx, &info, &a2.settings(), &a2.history_file);
                a2.refresh_app(&p);
                r
            });
            json!({ "task": id })
        }
        ("GET", "/api/task") => {
            let id: u64 = qs("id").parse().unwrap_or(0);
            let (i, l) = (qs("items").parse().unwrap_or(0), qs("lines").parse().unwrap_or(0));
            app.tasks.view(id, i, l).ok_or_else(|| ApiError::new(404, "Task not found"))?
        }
        ("POST", "/api/task/stop") => {
            app.tasks.stop(s(&b, "id").parse().unwrap_or(0));
            json!({ "ok": true })
        }
        ("GET", "/api/console") => json!({ "lines": CONSOLE.since(qs("after").parse().unwrap_or(0)) }),
        ("GET", "/api/permissions") => blocking(permissions).await?,
        ("POST", "/api/permissions/open") => {
            tools::open_privacy_pane(&s(&b, "which"));
            json!({ "ok": true })
        }
        ("POST", "/api/cli") => {
            let install = crate::util::b(&b, "install");
            blocking(move || cli_link(install)).await?.map_err(|e| ApiError::new(500, e))?;
            json!({ "installed": cli_installed() })
        }
        ("POST", "/api/export") => {
            let paths = strs(&b, "paths");
            let name = s(&b, "name");
            let dir = home_dir().join("Downloads");
            let file = dir.join(format!("{} files.txt", if name.is_empty() { "Deskmart" } else { &name }));
            std::fs::write(&file, paths.join("\n") + "\n")?;
            if cfg!(target_os = "macos") {
                let _ = std::process::Command::new("/usr/bin/open").arg("-R").arg(&file).spawn();
            }
            json!({ "file": file.to_string_lossy() })
        }
        ("GET", "/api/update") => check_update(app, qs("force") == "1").await?,
        ("POST", "/api/update/open") => {
            let url = app.update.lock().unwrap().1.as_ref().and_then(|v| v["url"].as_str().map(String::from));
            match url {
                Some(u) if u.starts_with("https://github.com/") => {
                    if cfg!(target_os = "macos") {
                        let _ = std::process::Command::new("/usr/bin/open").arg(&u).spawn();
                    }
                    json!({ "ok": true, "url": u })
                }
                _ => return Err(ApiError::new(404, "No update to open")),
            }
        }
        ("POST", "/api/update/install") => {
            let (Some(repo), Some(commit)) = (BUILD_REPO.filter(|r| !r.is_empty()), BUILD_COMMIT) else {
                return Err(ApiError::new(400, "This copy wasn't built by GitHub, so it can't update itself."));
            };
            let a2 = a.clone();
            let id = app.tasks.spawn("selfupdate", "Update", move |ctx| {
                let cfg = a2.settings();
                let r = crate::selfupdate::install(&ctx, repo, commit, &cfg.app_folders, &a2.history_file)?;
                // Quit so the relauncher can open the new copy.
                let a3 = a2.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(900));
                    match a3.quit.lock().unwrap().as_ref() {
                        Some(f) => f(),
                        None => std::process::exit(0),
                    }
                });
                Ok(r)
            });
            json!({ "task": id })
        }
        ("POST", "/api/quit") => {
            let app2 = app.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(200)).await;
                match app2.quit.lock().unwrap().as_ref() {
                    Some(f) => f(),
                    None => std::process::exit(0),
                }
            });
            json!({ "ok": true })
        }
        _ => return Err(ApiError::new(404, "Not found")),
    };
    Ok(ok(v))
}

fn native_arch() -> &'static str {
    if macho::native_cpu() == macho::CPU_ARM64 {
        "arm"
    } else {
        "intel"
    }
}

/// Thins every binary in an app. Asks for the admin password if the app isn't writable by this user.
pub fn thin_app(path: &str) -> Result<u64, String> {
    if trash::blocked(path, &[]) || !path.ends_with(".app") {
        return Err("Not an app.".into());
    }
    let bins = macho::bundle_binaries(Path::new(path));
    let writable = bins.iter().all(|b| std::fs::OpenOptions::new().append(true).open(b).is_ok()) && std::fs::OpenOptions::new().append(true).open(Path::new(path).join("Contents/Info.plist")).is_ok();
    if writable {
        let mut saved = 0;
        for b in bins {
            if macho::savings(&b) > 0 {
                saved += macho::thin(&b)?;
            }
        }
        // Touch the bundle so Finder refreshes its size.
        let _ = cmd_quick("/usr/bin/touch", &[path]);
        return Ok(saved);
    }
    let before = size_str(path);
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    run_as_admin(&format!("{} --thin {}", sh_quote(&exe.to_string_lossy()), sh_quote(path)), &format!("Deskmart wants to slim down {}.", file_name(path)))?;
    Ok(before.saturating_sub(size_str(path)))
}

async fn trash_paths(app: &Arc<App>, b: Value) -> R<Value> {
    let paths = strs(&b, "paths");
    let name = Some(s(&b, "name")).filter(|x| !x.is_empty()).unwrap_or_else(|| paths.first().map(|p| file_name(p)).unwrap_or_default());
    let app_path = Some(s(&b, "app")).filter(|x| !x.is_empty());
    let quit = crate::util::b(&b, "quit");
    let a = app.clone();
    blocking(move || {
        let cfg = a.settings();
        let info = app_path.as_ref().and_then(|p| a.app(p));
        if let Some(i) = &info {
            if paths.contains(&i.visible_path()) && !apps::running_pids(&i.visible_path()).is_empty() {
                if quit {
                    apps::quit_app(i);
                } else {
                    return Err(ApiError::new(409, format!("{} is open. Quit it first.", i.name)).reason("running"));
                }
            }
        }
        let out = trash::trash(&paths, &name, &cfg.app_folders, &a.history_file);
        let mut brew_log = None;
        if let Some(i) = &info {
            if !exists(&i.visible_path()) {
                a.edit(|c| {
                    c.associations.remove(&i.visible_path());
                    c.app_sensitivity.remove(&i.visible_path());
                });
                if cfg.brew_cleanup {
                    if let (Some(cask), Some(b)) = (&i.cask, apps::brew_bin()) {
                        let o = cmd(&b, &["uninstall", "--cask", "--force", cask], Duration::from_secs(300));
                        brew_log = Some(if o.ok { format!("Removed the Homebrew record for {cask}.") } else { o.stderr.trim().to_string() });
                    }
                }
            }
        }
        a.drop_missing_apps();
        Ok(json!({
            "moved": out.moved.len(), "size": out.record.as_ref().map(|r| r.size).unwrap_or(0), "record": out.record.as_ref().map(|r| r.id.clone()),
            "failed": out.failed.iter().map(|(p, e)| json!({ "path": p, "error": e })).collect::<Vec<_>>(), "brew": brew_log,
        }))
    })
    .await?
}

fn disk() -> (u64, u64) {
    let target = if Path::new("/System/Volumes/Data").exists() { "/System/Volumes/Data" } else { "/" };
    let o = cmd_quick("/bin/df", &["-k", target]);
    let line = o.stdout.lines().nth(1).unwrap_or("");
    let cols: Vec<&str> = line.split_whitespace().collect();
    let total = cols.get(1).and_then(|x| x.parse::<u64>().ok()).unwrap_or(0) * 1024;
    let free = cols.get(3).and_then(|x| x.parse::<u64>().ok()).unwrap_or(0) * 1024;
    (free, total)
}

fn state(app: &App) -> Value {
    let (ver, major) = tools::macos_version();
    let (free, total) = disk();
    json!({
        "version": VERSION, "commit": BUILD_COMMIT.map(|c| &c[..c.len().min(7)]), "mac": cfg!(target_os = "macos"),
        "macos": ver, "macosMajor": major, "macosName": tools::macos_name(major), "home": home(),
        "free": free, "total": total, "settings": app.settings(), "nativeArch": native_arch(),
        "fullDiskAccess": tools::full_disk_access(), "sentinel": sentinel::installed(), "cli": cli_installed(),
        "brew": apps::brew_bin().is_some(), "trash": trash_dir().to_string_lossy(),
        "selfUpdate": cfg!(target_os = "macos") && BUILD_REPO.is_some() && crate::selfupdate::current_bundle().is_some(),
    })
}

fn permissions() -> Value {
    let (ver, major) = tools::macos_version();
    json!({ "fullDiskAccess": tools::full_disk_access(), "macos": ver, "macosMajor": major, "macosName": tools::macos_name(major) })
}

fn cli_installed() -> bool {
    std::fs::read_link("/usr/local/bin/deskmart").is_ok()
}
fn cli_link(install: bool) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let script = if install {
        format!("/bin/mkdir -p /usr/local/bin && /bin/ln -sf {} /usr/local/bin/deskmart", sh_quote(&exe.to_string_lossy()))
    } else {
        "/bin/rm -f /usr/local/bin/deskmart".to_string()
    };
    run_as_admin(&script, "Deskmart wants to set up the deskmart command for Terminal.").map(|_| ())
}

async fn icon(q: &HashMap<String, String>) -> R<Response> {
    let p = q.get("path").cloned().unwrap_or_default();
    let px: u32 = q.get("size").and_then(|x| x.parse().ok()).unwrap_or(64);
    let png = blocking(move || icons::icon(&p, px)).await?;
    match png {
        Some(b) => Ok(([("content-type", "image/png"), ("cache-control", "private, max-age=600")], Body::from(b)).into_response()),
        None => Err(ApiError::new(404, "No icon")),
    }
}

fn static_file(app: &App, path: &str) -> R<Response> {
    let (ctype, body): (&str, Body) = match path {
        "/" | "/index.html" => {
            let html = INDEX_HTML.replace("__DESKMART_TOKEN__", &app.token);
            return Ok(([("content-type", "text/html; charset=utf-8"), ("cache-control", "no-store"), ("x-frame-options", "DENY"), ("referrer-policy", "no-referrer")], html).into_response());
        }
        "/icon.svg" => ("image/svg+xml", Body::from(ICON_SVG)),
        "/icon-180.png" => ("image/png", Body::from(ICON_PNG)),
        "/fa/fontawesome.css" => ("text/css; charset=utf-8", Body::from(FA_CSS)),
        "/fa/fa-solid-900.woff2" => {
            return Ok(([("content-type", "font/woff2"), ("cache-control", "max-age=31536000, immutable")], Body::from(FA_FONT)).into_response());
        }
        _ => return Err(ApiError::new(404, "Not found")),
    };
    Ok(([("content-type", ctype), ("cache-control", "no-cache")], body).into_response())
}

// ------------------------------------------------------------------ update check (same approach as Dropfin)

/// Compares the commit this app was built from with the commit behind the repo's "latest" release.
async fn check_update(app: &App, force: bool) -> R<Value> {
    let (Some(repo), Some(commit)) = (BUILD_REPO.filter(|r| !r.is_empty()), BUILD_COMMIT.filter(|c| c.len() >= 7)) else {
        return Ok(json!({ "enabled": false }));
    };
    {
        let cached = app.update.lock().unwrap();
        if !force && now_ms().saturating_sub(cached.0) < 6 * 3600 * 1000 {
            if let Some(v) = &cached.1 {
                return Ok(v.clone());
            }
        }
    }
    let url = format!("https://api.github.com/repos/{repo}/releases/tags/latest");
    let current = &commit[..7];
    let result = async {
        let client = reqwest::Client::builder().timeout(Duration::from_secs(8)).user_agent(format!("Deskmart/{VERSION}")).build().ok()?;
        let r = client.get(&url).header("Accept", "application/vnd.github+json").send().await.ok()?;
        if !r.status().is_success() {
            return None;
        }
        r.json::<Value>().await.ok()
    }
    .await;
    let v = match result {
        Some(rel) => {
            let latest = rel["target_commitish"].as_str().unwrap_or("").to_string();
            let is_sha = latest.len() >= 7 && latest.chars().all(|c| c.is_ascii_hexdigit());
            let available = is_sha && !latest.starts_with(current) && !commit.starts_with(&latest);
            json!({ "enabled": true, "available": available, "reachable": true, "current": current, "latest": if is_sha { &latest[..7] } else { "" },
                    "url": rel["html_url"], "published": rel["published_at"] })
        }
        None => json!({ "enabled": true, "available": false, "reachable": false, "current": current }),
    };
    *app.update.lock().unwrap() = (now_ms(), Some(v.clone()));
    Ok(v)
}

pub async fn serve(app: Arc<App>, preferred: u16, on_ready: impl FnOnce(u16)) -> anyhow::Result<()> {
    let mut port = preferred;
    let listener = loop {
        match tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
            Ok(l) => break l,
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse && port < preferred + 20 => port += 1,
            Err(e) => return Err(e.into()),
        }
    };
    println!("Deskmart {VERSION} running at http://localhost:{port}/");
    on_ready(port);
    axum::serve(listener, router(app)).await?;
    Ok(())
}
