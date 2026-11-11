//! File Search: walk a folder and match filters (port of FileSearchLogic.swift).

use crate::util::*;
use serde::Serialize;
use serde_json::Value;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Hit {
    pub path: String,
    pub name: String,
    pub kind: String,
    pub size: u64,
    pub modified: Option<u64>,
    pub dir: bool,
}

enum F {
    Name(String, String),
    Ext(bool, Vec<String>),
    Size(String, u64, u64),
    Date(String, u64, u64),
    Kind(String),
    Tags(bool, Vec<String>),
    Comment(bool, String),
}

fn parse_filters(v: &Value) -> Result<Vec<F>, String> {
    let mut out = Vec::new();
    for f in v.as_array().cloned().unwrap_or_default() {
        let t = s(&f, "type");
        let op = s(&f, "op");
        let val = s(&f, "value");
        let n = |k: &str| f.get(k).and_then(|x| x.as_f64()).unwrap_or(0.0).max(0.0) as u64;
        out.push(match t.as_str() {
            "name" => {
                if op == "regex" {
                    regex::Regex::new(&val).map_err(|e| format!("That pattern isn't valid: {e}"))?;
                }
                F::Name(op, val)
            }
            "extension" => F::Ext(op != "isNot", val.split(',').map(|x| x.trim().trim_start_matches('.').to_lowercase()).filter(|x| !x.is_empty()).collect()),
            "size" => F::Size(op, n("value"), n("max")),
            "date" => F::Date(op, n("value"), n("max")),
            "kind" => F::Kind(val),
            "tags" => F::Tags(op != "doesntContain", val.split(',').map(|x| x.trim().to_lowercase()).filter(|x| !x.is_empty()).collect()),
            "comment" => F::Comment(op != "doesntContain", val.to_lowercase()),
            _ => continue,
        });
    }
    Ok(out)
}

#[cfg(target_os = "macos")]
fn xattr(path: &str, name: &str) -> Option<Vec<u8>> {
    use std::ffi::CString;
    extern "C" {
        fn getxattr(path: *const std::os::raw::c_char, name: *const std::os::raw::c_char, value: *mut u8, size: usize, position: u32, options: i32) -> isize;
    }
    let p = CString::new(path).ok()?;
    let n = CString::new(name).ok()?;
    // SAFETY: probing size with a null buffer, then reading into a buffer of that size.
    unsafe {
        let len = getxattr(p.as_ptr(), n.as_ptr(), std::ptr::null_mut(), 0, 0, 1);
        if len <= 0 {
            return None;
        }
        let mut buf = vec![0u8; len as usize];
        let got = getxattr(p.as_ptr(), n.as_ptr(), buf.as_mut_ptr(), buf.len(), 0, 1);
        (got > 0).then(|| {
            buf.truncate(got as usize);
            buf
        })
    }
}
#[cfg(not(target_os = "macos"))]
fn xattr(_path: &str, _name: &str) -> Option<Vec<u8>> {
    None
}

fn tags_of(path: &str) -> Vec<String> {
    let Some(b) = xattr(path, "com.apple.metadata:_kMDItemUserTags") else { return vec![] };
    plist::Value::from_reader(std::io::Cursor::new(b))
        .ok()
        .and_then(|v| v.into_array())
        .map(|a| a.into_iter().filter_map(|x| x.into_string()).map(|t| t.split('\n').next().unwrap_or("").to_lowercase()).collect())
        .unwrap_or_default()
}
fn comment_of(path: &str) -> String {
    xattr(path, "com.apple.metadata:kMDItemFinderComment")
        .and_then(|b| plist::Value::from_reader(std::io::Cursor::new(b)).ok())
        .and_then(|v| v.into_string())
        .unwrap_or_default()
        .to_lowercase()
}

fn kind_of(name: &str, dir: bool) -> String {
    if dir {
        if name.ends_with(".app") {
            return "Application".into();
        }
        return "Folder".into();
    }
    Path::new(name).extension().map(|e| e.to_string_lossy().to_uppercase()).unwrap_or_else(|| "Document".into())
}

fn matches(fs: &[F], name: &str, path: &str, meta: &std::fs::Metadata, regexes: &[Option<regex::Regex>]) -> bool {
    let lname = name.to_lowercase();
    let dir = meta.is_dir();
    for (i, f) in fs.iter().enumerate() {
        let ok = match f {
            F::Name(op, v) => {
                let v = v.to_lowercase();
                match op.as_str() {
                    "doesntContain" => !lname.contains(&v),
                    "startsWith" => lname.starts_with(&v),
                    "endsWith" => lname.ends_with(&v),
                    "equals" => lname == v,
                    "regex" => regexes[i].as_ref().map(|r| r.is_match(name)).unwrap_or(false),
                    _ => lname.contains(&v),
                }
            }
            F::Ext(is, exts) => {
                let e = Path::new(name).extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
                exts.contains(&e) == *is
            }
            F::Size(op, a, b) => {
                if dir {
                    false
                } else {
                    let s = meta.len();
                    match op.as_str() {
                        "lessThan" => s < *a,
                        "between" => s >= *a && s <= *b,
                        _ => s > *a,
                    }
                }
            }
            F::Date(op, a, b) => {
                let m = secs(meta.modified()).unwrap_or(0);
                let c = secs(meta.created()).unwrap_or(m);
                match op.as_str() {
                    "modifiedBefore" => m < *a,
                    "modifiedBetween" => m >= *a && m <= *b,
                    "createdAfter" => c > *a,
                    "createdBefore" => c < *a,
                    "createdBetween" => c >= *a && c <= *b,
                    _ => m > *a,
                }
            }
            F::Kind(k) => match k.as_str() {
                "folder" => dir && !name.ends_with(".app"),
                "app" => name.ends_with(".app"),
                "file" => !dir,
                "image" => ["jpg", "jpeg", "png", "gif", "heic", "webp", "tiff", "bmp", "raw", "svg"].iter().any(|e| lname.ends_with(&format!(".{e}"))),
                "video" => ["mp4", "mov", "mkv", "avi", "m4v", "webm"].iter().any(|e| lname.ends_with(&format!(".{e}"))),
                "audio" => ["mp3", "m4a", "wav", "flac", "aac", "aiff", "ogg"].iter().any(|e| lname.ends_with(&format!(".{e}"))),
                "archive" => ["zip", "dmg", "tar", "gz", "7z", "rar", "pkg", "xz", "bz2"].iter().any(|e| lname.ends_with(&format!(".{e}"))),
                "document" => ["pdf", "doc", "docx", "pages", "txt", "rtf", "md", "key", "numbers", "xls", "xlsx", "ppt", "pptx"].iter().any(|e| lname.ends_with(&format!(".{e}"))),
                _ => true,
            },
            F::Tags(has, want) => {
                let t = tags_of(path);
                want.iter().any(|w| t.contains(w)) == *has
            }
            F::Comment(has, want) => comment_of(path).contains(want.as_str()) == *has,
        };
        if !ok {
            return false;
        }
    }
    true
}

pub struct Options {
    pub root: String,
    pub hidden: bool,
    pub packages: bool,
    pub limit: usize,
}

/// Walks `root` and reports matches as they're found. Folder sizes are filled in at the end.
pub fn search(opts: &Options, filters: &Value, stop: &AtomicBool, mut on_hit: impl FnMut(Hit), on_progress: &dyn Fn(u64)) -> Result<usize, String> {
    let fs = parse_filters(filters)?;
    if fs.is_empty() {
        return Err("Add at least one filter.".into());
    }
    let regexes: Vec<Option<regex::Regex>> = fs.iter().map(|f| if let F::Name(op, v) = f { (op == "regex").then(|| regex::Regex::new(v).ok()).flatten() } else { None }).collect();
    let root = expand(&opts.root);
    let skip_roots = ["/System/Volumes", "/dev", "/private/var/vm", "/Volumes", "/proc", "/sys", "/System"];
    let mut stack = vec![root.clone()];
    let mut scanned = 0u64;
    let mut found = 0usize;
    while let Some(dir) = stack.pop() {
        if stop.load(Ordering::SeqCst) || found >= opts.limit {
            break;
        }
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if !opts.hidden && name.starts_with('.') {
                continue;
            }
            let path = format!("{}/{}", dir.trim_end_matches('/'), name);
            let Ok(meta) = e.metadata() else { continue };
            scanned += 1;
            if scanned % 2000 == 0 {
                on_progress(scanned);
                if stop.load(Ordering::SeqCst) {
                    break;
                }
            }
            if matches(&fs, &name, &path, &meta, &regexes) {
                found += 1;
                on_hit(Hit { kind: kind_of(&name, meta.is_dir()), size: if meta.is_dir() { 0 } else { meta.len() }, modified: secs(meta.modified()), dir: meta.is_dir(), name: name.clone(), path: path.clone() });
                if found >= opts.limit {
                    break;
                }
            }
            let ft = e.file_type().ok();
            if meta.is_dir() && !ft.map(|f| f.is_symlink()).unwrap_or(true) {
                let bundle = [".app", ".framework", ".bundle", ".photoslibrary", ".musiclibrary", ".xcarchive", ".pkg", ".plugin", ".kext"].iter().any(|x| name.ends_with(x));
                if (bundle && !opts.packages) || skip_roots.contains(&path.as_str()) {
                    continue;
                }
                stack.push(path);
            }
        }
    }
    on_progress(scanned);
    Ok(found)
}
