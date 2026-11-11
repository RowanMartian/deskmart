//! Shared helpers: API errors, settings, name normalisation, sizes, running system tools.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

// ------------------------------------------------------------------ errors

#[derive(Debug)]
pub struct ApiError {
    pub status: u16,
    pub message: String,
    pub extra: Map<String, Value>,
}
pub type R<T> = Result<T, ApiError>;

impl ApiError {
    pub fn new(status: u16, message: impl Into<String>) -> Self {
        Self { status, message: message.into(), extra: Map::new() }
    }
    pub fn with(mut self, key: &str, value: impl Into<Value>) -> Self {
        self.extra.insert(key.to_string(), value.into());
        self
    }
    pub fn reason(self, r: &str) -> Self {
        self.with("reason", r)
    }
}
impl From<std::io::Error> for ApiError {
    fn from(e: std::io::Error) -> Self {
        ApiError::new(500, e.to_string())
    }
}
impl From<String> for ApiError {
    fn from(e: String) -> Self {
        ApiError::new(500, e)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut body = self.extra;
        body.insert("error".into(), Value::String(self.message));
        let status = StatusCode::from_u16(self.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        (status, [("cache-control", "no-store")], axum::Json(Value::Object(body))).into_response()
    }
}

pub fn ok(v: Value) -> Response {
    ([("cache-control", "no-store")], axum::Json(v)).into_response()
}

// ------------------------------------------------------------------ JSON body helpers

pub fn s(v: &Value, k: &str) -> String {
    match v.get(k) {
        Some(Value::String(x)) => x.clone(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}
pub fn b(v: &Value, k: &str) -> bool {
    v.get(k).map(|x| x.as_bool().unwrap_or(false)).unwrap_or(false)
}
pub fn strs(v: &Value, k: &str) -> Vec<String> {
    v.get(k)
        .and_then(|x| x.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
        .unwrap_or_default()
}

// ------------------------------------------------------------------ paths

pub fn home_dir() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}
pub fn home() -> String {
    home_dir().to_string_lossy().to_string()
}
pub fn data_dir() -> PathBuf {
    if cfg!(target_os = "macos") {
        home_dir().join("Library/Application Support/Deskmart")
    } else {
        std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home_dir().join(".config")).join("deskmart")
    }
}
pub fn trash_dir() -> PathBuf {
    home_dir().join(".Trash")
}
/// "~/x" → "/Users/me/x"
pub fn expand(p: &str) -> String {
    if p == "~" {
        home()
    } else if let Some(rest) = p.strip_prefix("~/") {
        format!("{}/{}", home(), rest)
    } else {
        p.to_string()
    }
}
/// "/Users/me/x" → "~/x" (for display)
pub fn tildify(p: &str) -> String {
    let h = home();
    if p == h {
        "~".into()
    } else if let Some(rest) = p.strip_prefix(&(h + "/")) {
        format!("~/{rest}")
    } else {
        p.to_string()
    }
}
pub fn file_name(p: &str) -> String {
    Path::new(p).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| p.to_string())
}
pub fn parent(p: &str) -> String {
    Path::new(p).parent().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "/".into())
}
pub fn exists(p: &str) -> bool {
    std::fs::symlink_metadata(p).is_ok()
}
pub fn is_dir(p: &str) -> bool {
    std::fs::metadata(p).map(|m| m.is_dir()).unwrap_or(false)
}
/// Directory entries' names (hidden included), or empty on error.
pub fn list_dir(p: &str) -> Vec<String> {
    std::fs::read_dir(p).map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().to_string()).collect()).unwrap_or_default()
}
/// Normalises "a/./b/../c" without touching the disk.
pub fn standardize(p: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for part in p.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            x => out.push(x),
        }
    }
    format!("/{}", out.join("/"))
}

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}
pub fn now_secs() -> u64 {
    now_ms() / 1000
}
pub fn secs(t: std::io::Result<SystemTime>) -> Option<u64> {
    t.ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs())
}

// ------------------------------------------------------------------ name normalisation

/// PearCleaner's `pearFormat()`: keep letters and digits only, lowercased.
/// "Visual Studio Code" → "visualstudiocode", "com.microsoft.VSCode" → "commicrosoftvscode".
pub fn pear_format(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(|c| c.to_lowercase()).collect()
}

/// "Bartender 6" → "Bartender", "Firefox 120.0" → "Firefox"
pub fn strip_trailing_digits(s: &str) -> String {
    static RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| regex::Regex::new(r"\s+\d+(\.\d+)*\s*$").unwrap());
    RE.replace(s, "").trim().to_string()
}

// ------------------------------------------------------------------ sizes

/// Logical size (what Finder shows) of a file, or of everything inside a folder. Symlinks count as themselves.
pub fn size_of(p: &Path) -> u64 {
    let Ok(m) = std::fs::symlink_metadata(p) else { return 0 };
    if !m.is_dir() {
        return m.len();
    }
    let mut total = 0u64;
    let mut stack = vec![p.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() {
                stack.push(e.path());
            } else if let Ok(m) = e.metadata() {
                total += m.len();
            }
        }
    }
    total
}
pub fn size_str(p: &str) -> u64 {
    size_of(Path::new(p))
}

pub fn fmt_size(b: u64) -> String {
    if b < 1000 {
        return format!("{b} bytes");
    }
    let units = ["KB", "MB", "GB", "TB"];
    let mut v = b as f64;
    let mut i = 0;
    v /= 1000.0;
    while v >= 1000.0 && i < units.len() - 1 {
        v /= 1000.0;
        i += 1;
    }
    if v >= 100.0 {
        format!("{:.0} {}", v, units[i])
    } else {
        format!("{:.1} {}", v, units[i])
    }
}

// ------------------------------------------------------------------ running tools

pub struct Out {
    pub ok: bool,
    pub stdout: String,
    pub stderr: String,
}

/// Runs a program and waits for it (with a timeout). Never goes through a shell.
pub fn cmd(program: &str, args: &[&str], timeout: Duration) -> Out {
    use std::process::{Command, Stdio};
    let child = Command::new(program).args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn();
    let mut child = match child {
        Ok(c) => c,
        Err(e) => return Out { ok: false, stdout: String::new(), stderr: e.to_string() },
    };
    // Read output on threads so a chatty program can't block on a full pipe.
    let mut so = child.stdout.take().unwrap();
    let mut se = child.stderr.take().unwrap();
    let t1 = std::thread::spawn(move || {
        let mut v = Vec::new();
        let _ = std::io::Read::read_to_end(&mut so, &mut v);
        v
    });
    let t2 = std::thread::spawn(move || {
        let mut v = Vec::new();
        let _ = std::io::Read::read_to_end(&mut se, &mut v);
        v
    });
    let start = std::time::Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break Some(st),
            Ok(None) if start.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(15)),
            Err(_) => break None,
        }
    };
    let stdout = String::from_utf8_lossy(&t1.join().unwrap_or_default()).to_string();
    let mut stderr = String::from_utf8_lossy(&t2.join().unwrap_or_default()).to_string();
    if status.is_none() && stderr.is_empty() {
        stderr = format!("{program} took too long and was stopped.");
    }
    Out { ok: status.map(|s| s.success()).unwrap_or(false), stdout, stderr }
}
pub fn cmd_quick(program: &str, args: &[&str]) -> Out {
    cmd(program, args, Duration::from_secs(20))
}

/// Single-quotes a string for /bin/sh.
pub fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Runs a /bin/sh script as root via the standard macOS admin password prompt.
/// (PearCleaner used a privileged helper, which needs a paid Apple signing identity; this needs nothing.)
pub fn run_as_admin(script: &str, prompt: &str) -> Result<String, String> {
    if !cfg!(target_os = "macos") {
        return Err("Administrator actions are only available on macOS.".into());
    }
    let esc = |x: &str| x.replace('\\', "\\\\").replace('"', "\\\"");
    let apple = format!("do shell script \"{}\" with prompt \"{}\" with administrator privileges", esc(script), esc(prompt));
    let o = cmd("/usr/bin/osascript", &["-e", &apple], Duration::from_secs(600));
    if o.ok {
        Ok(o.stdout)
    } else if o.stderr.contains("-128") {
        Err("Cancelled.".into())
    } else {
        Err(o.stderr.trim().to_string())
    }
}

// ------------------------------------------------------------------ settings

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// "strict" | "enhanced" | "deep"
    pub sensitivity: String,
    /// Deep level: also match Spotlight text content.
    pub search_text_content: bool,
    /// Ask before moving files to the Trash.
    pub confirm_delete: bool,
    /// Also run `brew uninstall --cask` for Homebrew-installed apps.
    pub brew_cleanup: bool,
    /// Folders scanned for apps.
    pub app_folders: Vec<String>,
    /// Paths never offered for removal when uninstalling apps.
    pub exclude_apps: Vec<String>,
    /// Paths never shown as orphans.
    pub exclude_orphans: Vec<String>,
    /// Per-app sensitivity override, keyed by bundle path.
    pub app_sensitivity: BTreeMap<String, String>,
    /// Orphaned files the user linked to an app: app path → files.
    pub associations: BTreeMap<String, Vec<String>>,
    pub hidden_pages: Vec<String>,
    pub startup_page: String,
    pub show_badges: bool,
    pub updater_on_launch: bool,
    pub updater_ignored: Vec<String>,
    pub sentinel: bool,
    /// Has the orphan-search warning been shown.
    pub orphan_warning_seen: bool,
    pub prefs: Map<String, Value>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            sensitivity: "strict".into(),
            search_text_content: false,
            confirm_delete: true,
            brew_cleanup: false,
            app_folders: vec!["/Applications".into(), format!("{}/Applications", home())],
            exclude_apps: vec![],
            exclude_orphans: vec![],
            app_sensitivity: BTreeMap::new(),
            associations: BTreeMap::new(),
            hidden_pages: vec![],
            startup_page: "apps".into(),
            show_badges: true,
            updater_on_launch: false,
            updater_ignored: vec![],
            sentinel: false,
            orphan_warning_seen: false,
            prefs: Map::new(),
        }
    }
}

impl Settings {
    pub fn load(file: &Path) -> Settings {
        std::fs::read_to_string(file).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }
    pub fn save(&self, file: &Path) {
        if let Some(dir) = file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let tmp = file.with_extension("json.tmp");
        if std::fs::write(&tmp, serde_json::to_string_pretty(self).unwrap_or_default()).is_ok() {
            let _ = std::fs::rename(&tmp, file);
        }
    }
    /// Applies a partial update from the UI (only known keys).
    pub fn merge(&mut self, patch: &Value) {
        let mut cur = serde_json::to_value(&*self).unwrap_or(json!({}));
        if let (Some(c), Some(p)) = (cur.as_object_mut(), patch.as_object()) {
            for (k, v) in p {
                if c.contains_key(k) {
                    c.insert(k.clone(), v.clone());
                }
            }
        }
        if let Ok(s) = serde_json::from_value::<Settings>(cur) {
            *self = s;
        }
        if !["strict", "enhanced", "deep"].contains(&self.sensitivity.as_str()) {
            self.sensitivity = "strict".into();
        }
    }
}

// ------------------------------------------------------------------ console log (shown in the app's Console panel)

pub struct Console {
    lines: std::sync::Mutex<std::collections::VecDeque<(u64, String, String)>>,
    seq: std::sync::atomic::AtomicU64,
}
impl Console {
    pub const fn new() -> Self {
        Console { lines: std::sync::Mutex::new(std::collections::VecDeque::new()), seq: std::sync::atomic::AtomicU64::new(0) }
    }
    pub fn log(&self, source: &str, msg: impl Into<String>) {
        let msg = msg.into();
        if std::env::var_os("DESKMART_VERBOSE").is_some() {
            eprintln!("[{source}] {msg}");
        }
        let id = self.seq.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        let mut l = self.lines.lock().unwrap();
        l.push_back((id, source.to_string(), msg));
        while l.len() > 800 {
            l.pop_front();
        }
    }
    pub fn since(&self, after: u64) -> Vec<Value> {
        self.lines.lock().unwrap().iter().filter(|x| x.0 > after).map(|(id, src, m)| json!({ "id": id, "source": src, "text": m })).collect()
    }
}
pub static CONSOLE: Console = Console::new();
pub fn log(source: &str, msg: impl Into<String>) {
    CONSOLE.log(source, msg)
}

// ------------------------------------------------------------------ misc

pub fn uuid_like(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 36
        && b.iter().enumerate().all(|(i, c)| if [8, 13, 18, 23].contains(&i) { *c == b'-' } else { c.is_ascii_hexdigit() })
}

/// Reads a plist (XML or binary) as a JSON-ish dictionary.
pub fn read_plist(p: &Path) -> Option<plist::Dictionary> {
    plist::Value::from_file(p).ok().and_then(|v| v.into_dictionary())
}
pub fn pstr(d: &plist::Dictionary, k: &str) -> Option<String> {
    d.get(k).and_then(|v| v.as_string()).map(|s| s.to_string())
}

/// Version-aware comparison: "1.10" > "1.9", "2.0b3" < "2.0".
pub fn cmp_versions(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering::*;
    let parts = |s: &str| -> Vec<(u64, String)> {
        static RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| regex::Regex::new(r"(\d+)|([A-Za-z]+)").unwrap());
        RE.captures_iter(s)
            .map(|c| match (c.get(1), c.get(2)) {
                (Some(n), _) => (n.as_str().parse().unwrap_or(0), String::new()),
                (_, Some(t)) => (0, t.as_str().to_lowercase()),
                _ => (0, String::new()),
            })
            .collect()
    };
    let (pa, pb) = (parts(a), parts(b));
    for i in 0..pa.len().max(pb.len()) {
        let x = pa.get(i);
        let y = pb.get(i);
        match (x, y) {
            (Some(x), Some(y)) => {
                // a text part (beta/rc) sorts below a number part
                let o = match (x.1.is_empty(), y.1.is_empty()) {
                    (true, true) => x.0.cmp(&y.0),
                    (false, false) => x.1.cmp(&y.1),
                    (true, false) => Greater,
                    (false, true) => Less,
                };
                if o != Equal {
                    return o;
                }
            }
            (Some(x), None) => return if x.1.is_empty() { if x.0 == 0 { Equal } else { Greater } } else { Less },
            (None, Some(y)) => return if y.1.is_empty() { if y.0 == 0 { Equal } else { Less } } else { Greater },
            (None, None) => return Equal,
        }
    }
    Equal
}



#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn formats() {
        assert_eq!(pear_format("com.microsoft.VSCode"), "commicrosoftvscode");
        assert_eq!(pear_format("Visual Studio Code"), "visualstudiocode");
        assert_eq!(strip_trailing_digits("Bartender 6"), "Bartender");
        assert_eq!(strip_trailing_digits("Firefox 120.0"), "Firefox");
        assert_eq!(standardize("/a/./b/../c/"), "/a/c");
        assert!(uuid_like("0A1B2C3D-0000-1111-2222-333344445555"));
    }
    #[test]
    fn versions() {
        use std::cmp::Ordering::*;
        assert_eq!(cmp_versions("1.10", "1.9"), Greater);
        assert_eq!(cmp_versions("2.0", "2.0.0"), Equal);
        assert_eq!(cmp_versions("2.0b3", "2.0"), Less);
        assert_eq!(cmp_versions("5.4.3", "5.4.10"), Less);
    }
}
