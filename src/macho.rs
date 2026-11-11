//! Mach-O reading without Apple's developer tools: architectures, code-signature entitlements and
//! Team ID, and "lipo" thinning (keep only the slice this Mac runs).

use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

pub const CPU_ARM64: u32 = 0x0100_000c;
pub const CPU_X86_64: u32 = 0x0100_0007;
const FAT_MAGIC: u32 = 0xcafe_babe;
const FAT_MAGIC_64: u32 = 0xcafe_babf;
const MH_MAGIC_64: u32 = 0xfeed_facf;
const MH_MAGIC: u32 = 0xfeed_face;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arch {
    Arm,
    Intel,
    Universal,
    Unknown,
}
impl Arch {
    pub fn as_str(self) -> &'static str {
        match self {
            Arch::Arm => "arm",
            Arch::Intel => "intel",
            Arch::Universal => "universal",
            Arch::Unknown => "",
        }
    }
}

/// The architecture this build of Pearfin runs as (and so what the Mac runs natively).
pub fn native_cpu() -> u32 {
    if cfg!(target_arch = "x86_64") {
        CPU_X86_64
    } else {
        CPU_ARM64
    }
}

#[derive(Clone, Debug)]
pub struct Slice {
    pub cpu: u32,
    pub offset: u64,
    pub size: u64,
}

fn be32(b: &[u8], o: usize) -> u32 {
    u32::from_be_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
fn be64(b: &[u8], o: usize) -> u64 {
    u64::from_be_bytes(b[o..o + 8].try_into().unwrap())
}
fn le32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

/// Slices of a fat (universal) binary, `Some(vec![])` for a thin Mach-O, `None` if not Mach-O at all.
pub fn slices(path: &Path) -> Option<(bool, Vec<Slice>)> {
    let mut f = File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let mut head = [0u8; 8];
    f.read_exact(&mut head).ok()?;
    let magic = be32(&head, 0);
    if magic == FAT_MAGIC || magic == FAT_MAGIC_64 {
        let n = be32(&head, 4) as usize;
        // Java class files share 0xcafebabe; their "count" is a version number in the 40s+.
        if n == 0 || n > 20 {
            return None;
        }
        let entry = if magic == FAT_MAGIC_64 { 32 } else { 20 };
        let mut buf = vec![0u8; n * entry];
        f.read_exact(&mut buf).ok()?;
        let mut out = Vec::new();
        for i in 0..n {
            let o = i * entry;
            let (offset, size) = if magic == FAT_MAGIC_64 { (be64(&buf, o + 8), be64(&buf, o + 16)) } else { (be32(&buf, o + 8) as u64, be32(&buf, o + 12) as u64) };
            if offset + size > len {
                return None;
            }
            out.push(Slice { cpu: be32(&buf, o), offset, size });
        }
        return Some((true, out));
    }
    let lm = le32(&head, 0);
    if lm == MH_MAGIC_64 || lm == MH_MAGIC {
        return Some((false, vec![Slice { cpu: le32(&head, 4), offset: 0, size: len }]));
    }
    None
}

pub fn arch_of_binary(path: &Path) -> Arch {
    match slices(path) {
        Some((_, s)) => {
            let arm = s.iter().any(|x| x.cpu == CPU_ARM64);
            let intel = s.iter().any(|x| x.cpu == CPU_X86_64);
            match (arm, intel) {
                (true, true) => Arch::Universal,
                (true, false) => Arch::Arm,
                (false, true) => Arch::Intel,
                _ => Arch::Unknown,
            }
        }
        None => Arch::Unknown,
    }
}

/// Main executable of an app bundle.
pub fn bundle_executable(app: &Path) -> Option<std::path::PathBuf> {
    let info = crate::util::read_plist(&app.join("Contents/Info.plist"))?;
    let exe = crate::util::pstr(&info, "CFBundleExecutable")?;
    let p = app.join("Contents/MacOS").join(exe);
    p.exists().then_some(p)
}

pub fn app_arch(app: &Path) -> Arch {
    bundle_executable(app).map(|e| arch_of_binary(&e)).unwrap_or(Arch::Unknown)
}

// ------------------------------------------------------------------ code signature

pub struct Signature {
    pub team_id: Option<String>,
    pub entitlements: Option<plist::Dictionary>,
}

/// Reads the embedded code signature of a binary (first native slice, else the first slice).
pub fn signature(path: &Path) -> Option<Signature> {
    let (_, sl) = slices(path)?;
    let slice = sl.iter().find(|s| s.cpu == native_cpu()).or_else(|| sl.first())?.clone();
    let mut f = File::open(path).ok()?;
    // Mach-O header + load commands
    f.seek(SeekFrom::Start(slice.offset)).ok()?;
    let mut hdr = [0u8; 32];
    f.read_exact(&mut hdr).ok()?;
    let magic = le32(&hdr, 0);
    let (hsize, ncmds, sizeofcmds) = match magic {
        MH_MAGIC_64 => (32u64, le32(&hdr, 16), le32(&hdr, 20)),
        MH_MAGIC => (28u64, le32(&hdr, 16), le32(&hdr, 20)),
        _ => return None,
    };
    if sizeofcmds > 4 << 20 {
        return None;
    }
    let mut cmds = vec![0u8; sizeofcmds as usize];
    f.seek(SeekFrom::Start(slice.offset + hsize)).ok()?;
    f.read_exact(&mut cmds).ok()?;
    let mut o = 0usize;
    let mut sig: Option<(u32, u32)> = None;
    for _ in 0..ncmds {
        if o + 8 > cmds.len() {
            break;
        }
        let cmd = le32(&cmds, o);
        let size = le32(&cmds, o + 4) as usize;
        if cmd == 0x1d && o + 16 <= cmds.len() {
            sig = Some((le32(&cmds, o + 8), le32(&cmds, o + 12)));
        }
        if size == 0 {
            break;
        }
        o += size;
    }
    let (dataoff, datasize) = sig?;
    if datasize > 16 << 20 || dataoff as u64 + datasize as u64 > slice.size {
        return None;
    }
    let mut blob = vec![0u8; datasize as usize];
    f.seek(SeekFrom::Start(slice.offset + dataoff as u64)).ok()?;
    f.read_exact(&mut blob).ok()?;
    parse_superblob(&blob)
}

/// Code signing SuperBlob (all big-endian): magic 0xfade0cc0, length, count, then (type, offset) pairs.
pub fn parse_superblob(blob: &[u8]) -> Option<Signature> {
    if blob.len() < 12 || be32(blob, 0) != 0xfade_0cc0 {
        return None;
    }
    let count = be32(blob, 8) as usize;
    let mut out = Signature { team_id: None, entitlements: None };
    for i in 0..count.min(64) {
        let idx = 12 + i * 8;
        if idx + 8 > blob.len() {
            break;
        }
        let off = be32(blob, idx + 4) as usize;
        if off + 8 > blob.len() {
            continue;
        }
        let magic = be32(blob, off);
        let len = (be32(blob, off + 4) as usize).min(blob.len() - off);
        let b = &blob[off..off + len];
        match magic {
            // CodeDirectory: teamOffset at byte 48 when version >= 0x20200
            0xfade_0c02 if out.team_id.is_none() && len >= 52 => {
                let version = be32(b, 8);
                if version >= 0x20200 {
                    let t = be32(b, 48) as usize;
                    if t > 0 && t < b.len() {
                        let end = b[t..].iter().position(|c| *c == 0).map(|p| t + p).unwrap_or(b.len());
                        let s = String::from_utf8_lossy(&b[t..end]).to_string();
                        if !s.is_empty() {
                            out.team_id = Some(s);
                        }
                    }
                }
            }
            // Entitlements (XML plist)
            0xfade_7171 if len > 8 => {
                out.entitlements = plist::Value::from_reader_xml(&b[8..]).ok().and_then(|v| v.into_dictionary());
            }
            _ => {}
        }
    }
    Some(out)
}

// ------------------------------------------------------------------ lipo

pub fn is_machine_code(path: &Path) -> bool {
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    if ["appex", "xpc", "framework", "app", "png", "jpg", "nib", "car", "strings", "plist", "json", "js", "css", "html", "txt"].contains(&ext.as_str()) {
        return false;
    }
    let Ok(mut f) = File::open(path) else { return false };
    let mut m = [0u8; 4];
    if f.read_exact(&mut m).is_err() {
        return false;
    }
    let be = u32::from_be_bytes(m);
    let le = u32::from_le_bytes(m);
    be == FAT_MAGIC || be == FAT_MAGIC_64 || le == MH_MAGIC_64 || le == MH_MAGIC
}

/// Bytes that thinning would remove from one binary (0 if it isn't universal or lacks the native slice).
pub fn savings(path: &Path) -> u64 {
    match slices(path) {
        Some((true, sl)) if sl.iter().any(|s| s.cpu == native_cpu()) && sl.len() > 1 => {
            let len = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
            let keep = sl.iter().find(|s| s.cpu == native_cpu()).map(|s| s.size).unwrap_or(len);
            len.saturating_sub(keep)
        }
        _ => 0,
    }
}

/// Replaces a universal binary with just its native slice. Each slice carries its own code signature,
/// so the result stays validly signed. Writes to a temp file first, then renames over the original.
pub fn thin(path: &Path) -> Result<u64, String> {
    let (fat, sl) = slices(path).ok_or("not a Mach-O file")?;
    if !fat || sl.len() < 2 {
        return Ok(0);
    }
    let s = sl.iter().find(|s| s.cpu == native_cpu()).ok_or("no slice for this Mac")?.clone();
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    let before = meta.len();
    let mut f = File::open(path).map_err(|e| e.to_string())?;
    f.seek(SeekFrom::Start(s.offset)).map_err(|e| e.to_string())?;
    let mut data = vec![0u8; s.size as usize];
    f.read_exact(&mut data).map_err(|e| e.to_string())?;
    drop(f);
    let tmp = path.with_file_name(format!(".{}.pearfin-thin", path.file_name().unwrap().to_string_lossy()));
    {
        let mut out = File::create(&tmp).map_err(|e| e.to_string())?;
        out.write_all(&data).map_err(|e| e.to_string())?;
        out.sync_all().ok();
    }
    std::fs::set_permissions(&tmp, meta.permissions()).ok();
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        e.to_string()
    })?;
    Ok(before.saturating_sub(s.size))
}

/// All Mach-O files inside a bundle (skipping symlinks, so framework "Current" links aren't counted twice).
pub fn bundle_binaries(bundle: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![bundle.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_symlink() {
                continue;
            }
            let p = e.path();
            if ft.is_dir() {
                stack.push(p);
            } else if is_machine_code(&p) {
                out.push(p);
            }
        }
    }
    out
}

pub fn bundle_savings(bundle: &Path) -> u64 {
    bundle_binaries(bundle).iter().map(|p| savings(p)).sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fat_roundtrip() {
        let dir = std::env::temp_dir().join(format!("pearfin-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("bin");
        // Build a fake universal binary: x86_64 slice at 4096 (size 100), arm64 slice at 8192 (size 50).
        let mut v = vec![0u8; 8192 + 50];
        v[0..4].copy_from_slice(&FAT_MAGIC.to_be_bytes());
        v[4..8].copy_from_slice(&2u32.to_be_bytes());
        let arch = |v: &mut Vec<u8>, at: usize, cpu: u32, off: u32, size: u32| {
            v[at..at + 4].copy_from_slice(&cpu.to_be_bytes());
            v[at + 8..at + 12].copy_from_slice(&off.to_be_bytes());
            v[at + 12..at + 16].copy_from_slice(&size.to_be_bytes());
        };
        arch(&mut v, 8, CPU_X86_64, 4096, 100);
        arch(&mut v, 28, CPU_ARM64, 8192, 50);
        v[4096..4100].copy_from_slice(&MH_MAGIC_64.to_le_bytes());
        v[8192..8196].copy_from_slice(&MH_MAGIC_64.to_le_bytes());
        std::fs::write(&p, &v).unwrap();
        assert_eq!(arch_of_binary(&p), Arch::Universal);
        assert!(is_machine_code(&p));
        assert!(savings(&p) > 0);
        thin(&p).unwrap();
        let expect = if native_cpu() == CPU_ARM64 { 50 } else { 100 };
        assert_eq!(std::fs::metadata(&p).unwrap().len(), expect);
        assert_ne!(arch_of_binary(&p), Arch::Universal);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn superblob() {
        let xml = br#"<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict><key>com.apple.security.application-groups</key><array><string>group.com.example.app</string></array></dict></plist>"#;
        let mut cd = vec![0u8; 64];
        cd[0..4].copy_from_slice(&0xfade0c02u32.to_be_bytes());
        cd[4..8].copy_from_slice(&76u32.to_be_bytes());
        cd[8..12].copy_from_slice(&0x20400u32.to_be_bytes());
        cd[48..52].copy_from_slice(&64u32.to_be_bytes());
        cd.extend_from_slice(b"ABCDE12345\0\0");
        let mut ent = Vec::new();
        ent.extend_from_slice(&0xfade7171u32.to_be_bytes());
        ent.extend_from_slice(&((xml.len() + 8) as u32).to_be_bytes());
        ent.extend_from_slice(xml);
        let mut sb = Vec::new();
        sb.extend_from_slice(&0xfade0cc0u32.to_be_bytes());
        let total = 12 + 16 + cd.len() + ent.len();
        sb.extend_from_slice(&(total as u32).to_be_bytes());
        sb.extend_from_slice(&2u32.to_be_bytes());
        sb.extend_from_slice(&0u32.to_be_bytes());
        sb.extend_from_slice(&28u32.to_be_bytes());
        sb.extend_from_slice(&5u32.to_be_bytes());
        sb.extend_from_slice(&((28 + cd.len()) as u32).to_be_bytes());
        sb.extend_from_slice(&cd);
        sb.extend_from_slice(&ent);
        let s = parse_superblob(&sb).unwrap();
        assert_eq!(s.team_id.as_deref(), Some("ABCDE12345"));
        assert!(s.entitlements.unwrap().contains_key("com.apple.security.application-groups"));
    }
}
