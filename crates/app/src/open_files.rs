//! Files macOS asks us to open: a double-click in Finder, "Open With",
//! dragging onto the Dock icon or `open -a Rustsheet file.xlsx`. They arrive
//! as an Apple Event that AppKit hands to the application delegate's
//! `application:openURLs:`.

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

static QUEUE: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
static WAKE: OnceLock<egui::Context> = OnceLock::new();

/// Lets queued files wake the UI once it exists.
pub fn set_context(ctx: &egui::Context) {
    let _ = WAKE.set(ctx.clone());
}

/// Files requested since the last call.
pub fn take() -> Vec<PathBuf> {
    QUEUE.lock().map(|mut q| std::mem::take(&mut *q)).unwrap_or_default()
}

fn push(paths: Vec<PathBuf>) {
    if paths.is_empty() {
        return;
    }
    if let Ok(mut q) = QUEUE.lock() {
        q.extend(paths);
    }
    if let Some(ctx) = WAKE.get() {
        ctx.request_repaint();
    }
}

#[cfg(target_os = "macos")]
mod delegate {
    use std::path::PathBuf;

    use objc2::ffi;
    use objc2::runtime::{AnyClass, AnyObject, Imp, Sel};
    use objc2::sel;
    use objc2_foundation::{NSArray, NSURL};

    /// `-[WinitApplicationDelegate application:openURLs:]`, added at runtime.
    unsafe extern "C-unwind" fn open_urls(
        _this: *mut AnyObject,
        _cmd: Sel,
        _application: *mut AnyObject,
        urls: *mut NSArray<NSURL>,
    ) {
        // Never let a panic cross into AppKit.
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            // SAFETY: AppKit passes a valid NSArray<NSURL> (or nil).
            let Some(urls) = (unsafe { urls.as_ref() }) else { return };
            let paths = urls
                .iter()
                .filter(|url| url.isFileURL())
                .filter_map(|url| url.path())
                .map(|path| PathBuf::from(path.to_string()))
                .collect();
            super::push(paths);
        }));
    }

    /// Teaches winit's application delegate `application:openURLs:`.
    ///
    /// winit 0.30 owns the NSApplication delegate and panics if it is
    /// replaced, but its class does not implement this method; AppKit asks
    /// the delegate whether it responds at the moment the Apple Event
    /// arrives, so adding the method is enough. Must run after the event loop
    /// exists (which registers the class) and before it runs, so the files a
    /// launch was asked to open are not missed.
    pub fn install() {
        let Some(class) = AnyClass::get(c"WinitApplicationDelegate") else {
            eprintln!("waarschuwing: winit-delegate niet gevonden, bestanden openen via Finder werkt niet");
            return;
        };
        let selector = sel!(application:openURLs:);
        if class.instance_method(selector).is_some() {
            return;
        }
        let function: unsafe extern "C-unwind" fn(*mut AnyObject, Sel, *mut AnyObject, *mut NSArray<NSURL>) =
            open_urls;
        // SAFETY: the function matches the method's ABI (`v@:@@`: returns
        // void, takes self, _cmd and two objects); Imp is the type-erased
        // form the runtime expects.
        let added = unsafe {
            let imp: Imp = std::mem::transmute(function);
            ffi::class_addMethod(std::ptr::from_ref(class).cast_mut(), selector, imp, c"v@:@@".as_ptr())
        };
        if !added.as_bool() {
            eprintln!("waarschuwing: kon application:openURLs: niet toevoegen");
        }
    }
}

#[cfg(target_os = "macos")]
pub use delegate::install;

#[cfg(not(target_os = "macos"))]
pub fn install() {}
