//! Auto-update through Sparkle 2, as the Swift app did: the same feed (releases/latest/download/appcast.xml), the same
//! EdDSA key and the same user defaults (SUEnableAutomaticChecks, SUAutomaticallyUpdate), so the schedule a user set
//! in 0.9 carries over. scripts/bundle.sh puts Sparkle.framework in the app and the feed and key in its Info.plist.
//! A `cargo run` binary has no framework beside it, so it has no updater, and the menu item and Settings
//! section stay hidden instead of Sparkle failing to start.
use std::cell::OnceCell;
use std::ffi::CString;
use std::ptr;

use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyClass, AnyObject, Bool};
use objc2::msg_send;

thread_local! {
    // Sparkle is main-thread only, like the rest of the UI.
    static CONTROLLER: OnceCell<Option<Retained<AnyObject>>> = const { OnceCell::new() };
}

/// Loads Sparkle and starts its scheduled checks. Call once, at launch; later calls do nothing.
pub fn start() { with(|_| ()); }

pub fn available() -> bool { with(|c| c.is_some()) }

/// The app menu's "Check for Updates…": Sparkle's own window shows the result.
pub fn check_for_updates() {
    with(|c| if let Some(c) = c { unsafe { let _: () = msg_send![c, checkForUpdates: ptr::null::<AnyObject>()]; } });
}

pub fn automatically_checks() -> bool {
    updater().is_some_and(|u| unsafe { let b: Bool = msg_send![&*u, automaticallyChecksForUpdates]; b.as_bool() })
}
pub fn set_automatically_checks(on: bool) {
    if let Some(u) = updater() { unsafe { let _: () = msg_send![&*u, setAutomaticallyChecksForUpdates: Bool::new(on)]; } }
}
pub fn automatically_downloads() -> bool {
    updater().is_some_and(|u| unsafe { let b: Bool = msg_send![&*u, automaticallyDownloadsUpdates]; b.as_bool() })
}
pub fn set_automatically_downloads(on: bool) {
    if let Some(u) = updater() { unsafe { let _: () = msg_send![&*u, setAutomaticallyDownloadsUpdates: Bool::new(on)]; } }
}

fn with<R>(f: impl FnOnce(Option<&AnyObject>) -> R) -> R {
    CONTROLLER.with(|cell| f(cell.get_or_init(load).as_deref()))
}

fn load() -> Option<Retained<AnyObject>> {
    let exe = std::env::current_exe().ok()?;
    let contents = exe.parent()?.parent()?;
    if contents.file_name()? != "Contents" { return None }
    let lib = contents.join("Frameworks/Sparkle.framework/Sparkle");
    if !lib.exists() { return None }
    let path = CString::new(lib.to_str()?).ok()?;
    // Loaded at run time rather than linked, so builds without the framework (cargo run, tests) still start.
    if unsafe { libc::dlopen(path.as_ptr(), libc::RTLD_NOW) }.is_null() { return None }
    let cls = AnyClass::get(c"SPUStandardUpdaterController")?;
    unsafe {
        let alloc: Allocated<AnyObject> = msg_send![cls, alloc];
        msg_send![alloc, initWithStartingUpdater: Bool::YES, updaterDelegate: ptr::null::<AnyObject>(),
                  userDriverDelegate: ptr::null::<AnyObject>()]
    }
}

fn updater() -> Option<Retained<AnyObject>> {
    with(|c| c.map(|c| unsafe { msg_send![c, updater] }))
}

