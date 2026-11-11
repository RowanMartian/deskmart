//! Finder icons for any path, as PNG, via NSWorkspace (drawn on the main thread).

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

type Runner = Box<dyn Fn(Box<dyn FnOnce() + Send>) + Send + Sync>;
static MAIN: OnceLock<Runner> = OnceLock::new();
static CACHE: OnceLock<Mutex<HashMap<(String, u32), Option<Vec<u8>>>>> = OnceLock::new();

/// The window sets this so icon drawing happens on the main thread (AppKit's rule).
pub fn set_main_runner(r: Runner) {
    let _ = MAIN.set(r);
}

pub fn icon(path: &str, px: u32) -> Option<Vec<u8>> {
    let px = px.clamp(16, 512);
    let key = (path.to_string(), px);
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(v) = cache.lock().unwrap().get(&key) {
        return v.clone();
    }
    let v = render(path, px);
    let mut c = cache.lock().unwrap();
    if c.len() > 3000 {
        c.clear();
    }
    c.insert(key, v.clone());
    v
}

pub fn forget(path: &str) {
    if let Some(c) = CACHE.get() {
        c.lock().unwrap().retain(|k, _| k.0 != path);
    }
}

#[cfg(target_os = "macos")]
fn render(path: &str, px: u32) -> Option<Vec<u8>> {
    let path = path.to_string();
    match MAIN.get() {
        Some(run) => {
            let (tx, rx) = std::sync::mpsc::channel();
            run(Box::new(move || {
                let _ = tx.send(draw(&path, px));
            }));
            rx.recv_timeout(std::time::Duration::from_secs(4)).ok().flatten()
        }
        None => draw(&path, px),
    }
}

#[cfg(target_os = "macos")]
fn draw(path: &str, px: u32) -> Option<Vec<u8>> {
    use objc2::AnyThread;
    use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep, NSCompositingOperation, NSDeviceRGBColorSpace, NSGraphicsContext, NSWorkspace};
    use objc2_foundation::{NSDictionary, NSPoint, NSRect, NSSize, NSString};
    if std::fs::symlink_metadata(path).is_err() {
        return None;
    }
    objc2::rc::autoreleasepool(|_| unsafe {
        let ws = NSWorkspace::sharedWorkspace();
        let img = ws.iconForFile(&NSString::from_str(path));
        let side = px as isize;
        let rep = NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
            NSBitmapImageRep::alloc(),
            std::ptr::null_mut(),
            side,
            side,
            8,
            4,
            true,
            false,
            NSDeviceRGBColorSpace,
            0,
            0,
        )?;
        let ctx = NSGraphicsContext::graphicsContextWithBitmapImageRep(&rep)?;
        NSGraphicsContext::saveGraphicsState_class();
        NSGraphicsContext::setCurrentContext(Some(&ctx));
        let rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(px as f64, px as f64));
        img.drawInRect_fromRect_operation_fraction(rect, NSRect::ZERO, NSCompositingOperation::SourceOver, 1.0);
        ctx.flushGraphics();
        NSGraphicsContext::restoreGraphicsState_class();
        let data = rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new())?;
        Some(data.to_vec())
    })
}

#[cfg(not(target_os = "macos"))]
fn render(_path: &str, _px: u32) -> Option<Vec<u8>> {
    None
}
