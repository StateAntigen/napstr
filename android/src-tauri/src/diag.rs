//! A line about what this phone is doing, where a person can read it.
//!
//! The companion is the half of a pairing with no window to look at and no
//! console to read. When it decides a computer is unreachable, its answer is a
//! word on screen - "Offline" - and the reason is nowhere: the Rust side has
//! been silent, so a phone that flickered between "Connecting" and "Offline"
//! for fifteen minutes told us nothing at all. This is that silence fixed.
//!
//! On Android the lines go to logcat under one tag, so
//!
//! ```text
//! adb logcat -s Napstrfy
//! ```
//!
//! is the whole of it. Anywhere else they go to stderr, which is what a dev run
//! and a test already have.
//!
//! Deliberately *not* a logging framework. There are no levels, no filters, no
//! logger to install and no dependency to keep working; the handful of places
//! that matter - opening a tunnel, answering a status question, an exchange that
//! failed, the addresses this phone holds - each say one line, and the line
//! carries the facts. What a reader needs is the timeline, and a timeline is
//! free.

use std::sync::atomic::{AtomicBool, Ordering};

/// Whether anything is listening.
///
/// On by default: a build that can be asked why is worth more than the bytes it
/// costs, and the alternative - a switch nobody finds while their phone is
/// misbehaving - is what left this hole in the first place.
static LISTENING: AtomicBool = AtomicBool::new(true);

/// Every line this phone writes is prefixed with this, so one filter finds all
/// of it and no other app's noise.
pub const TAG: &str = "Napstrfy";

#[cfg(target_os = "android")]
const ANDROID_LOG_INFO: i32 = 4;

#[cfg(target_os = "android")]
extern "C" {
    fn __android_log_write(priority: i32, tag: *const u8, text: *const u8) -> i32;
}

/// Write one line, if anyone is listening.
pub fn write(message: &str) {
    if !LISTENING.load(Ordering::Relaxed) {
        return;
    }
    #[cfg(target_os = "android")]
    {
        // Interior NULs cannot happen in a formatted line, but a silent drop is
        // cheaper than a panic on the one path that exists to explain a panic.
        if let (Ok(tag), Ok(text)) = (
            std::ffi::CString::new(TAG),
            std::ffi::CString::new(message.replace('\0', " ")),
        ) {
            // SAFETY: both pointers are valid NUL-terminated C strings for the
            // duration of the call, which is all `__android_log_write` reads.
            unsafe { __android_log_write(ANDROID_LOG_INFO, tag.as_ptr() as *const u8, text.as_ptr() as *const u8) };
        }
    }
    #[cfg(not(target_os = "android"))]
    {
        eprintln!("[{TAG}] {message}");
    }
}

/// A line, without the ceremony of a level nobody reads.
pub fn note(message: &str) {
    write(message);
}

/// What kind of request this is, for a line about one.
///
/// The wire form is camelCase JSON with a `type` field, so the name is read from
/// that rather than listed a second time by hand: a request that is added and
/// never mentioned here is still named correctly in the log.
pub fn request_kind(request: &napstr_remote_protocol::ClientRequest) -> String {
    serde_json::to_value(request)
        .ok()
        .and_then(|value| value.get("type").and_then(|kind| kind.as_str()).map(str::to_string))
        .unwrap_or_else(|| "a request".to_string())
}

/// How long something took, to one decimal place, in milliseconds.
pub fn millis(started: std::time::Instant) -> String {
    format!("{:.0}ms", started.elapsed().as_secs_f64() * 1000.0)
}
