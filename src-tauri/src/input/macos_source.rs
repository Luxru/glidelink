//! Caps Lock's input-source action. Carbon TIS must run on the main thread;
//! the injector waits for completion so subsequent keys use the new source.
use std::cell::RefCell;
use std::ffi::{c_char, c_void};
use std::ptr::null;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, OnceLock};
use std::time::{Duration, Instant};

type CFRef = *const c_void;
static APP: OnceLock<tauri::AppHandle> = OnceLock::new();

thread_local! {
    static LAST_NON_ASCII: RefCell<Option<OwnedCF>> = const { RefCell::new(None) };
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(value: CFRef);
    fn CFRetain(value: CFRef) -> CFRef;
    fn CFEqual(a: CFRef, b: CFRef) -> u8;
    fn CFStringCreateWithCString(allocator: CFRef, text: *const c_char, encoding: u32) -> CFRef;
    fn CFPreferencesGetAppBooleanValue(key: CFRef, app: CFRef, valid: *mut u8) -> u8;
    fn CFArrayGetCount(array: CFRef) -> isize;
    fn CFArrayGetValueAtIndex(array: CFRef, index: isize) -> CFRef;
    static kCFPreferencesAnyApplication: CFRef;
    static kCFBooleanTrue: CFRef;
}

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn TISCopyCurrentKeyboardInputSource() -> CFRef;
    fn TISCopyCurrentASCIICapableKeyboardInputSource() -> CFRef;
    fn TISCreateInputSourceList(properties: CFRef, include_all: u8) -> CFRef;
    fn TISGetInputSourceProperty(source: CFRef, key: CFRef) -> CFRef;
    fn TISEnableInputSource(source: CFRef) -> i32;
    fn TISSelectInputSource(source: CFRef) -> i32;
    static kTISPropertyInputSourceIsASCIICapable: CFRef;
    static kTISPropertyInputSourceIsEnabled: CFRef;
    static kTISPropertyInputSourceIsSelectCapable: CFRef;
    static kTISPropertyInputSourceCategory: CFRef;
    static kTISCategoryKeyboardInputSource: CFRef;
}

struct OwnedCF(CFRef);

impl Drop for OwnedCF {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CFRelease(self.0) };
        }
    }
}

pub fn initialize(app: &tauri::AppHandle) {
    let _ = APP.set(app.clone());
}

/// Returning true means the press switched the system source, so it must not
/// also toggle AlphaShift or send a second Caps Lock event to the IME.
pub fn switch_for_caps_lock(stop: &Arc<AtomicBool>) -> bool {
    let Some(app) = APP.get() else {
        return false;
    };
    let (tx, rx) = mpsc::sync_channel(1);
    let cancelled = Arc::new(AtomicBool::new(false));
    let task_cancelled = Arc::clone(&cancelled);
    let task_stop = Arc::clone(stop);
    if app
        .run_on_main_thread(move || {
            if !task_cancelled.load(Ordering::Acquire) && !task_stop.load(Ordering::Acquire) {
                let _ = tx.send(switch_on_main_thread());
            }
        })
        .is_err()
    {
        return false;
    }

    wait_for_switch(rx, &cancelled, stop, Duration::from_millis(500))
}

fn wait_for_switch(
    rx: mpsc::Receiver<bool>,
    cancelled: &AtomicBool,
    stop: &AtomicBool,
    timeout: Duration,
) -> bool {
    let deadline = Instant::now() + timeout;
    // Engine shutdown can join this thread from the main thread. A cancellable
    // wait avoids deadlocking with a queued main-thread source switch.
    loop {
        if stop.load(Ordering::Acquire) || Instant::now() >= deadline {
            cancelled.store(true, Ordering::Release);
            return false;
        }
        match rx.recv_timeout(Duration::from_millis(10)) {
            Ok(switched) => return switched,
            Err(mpsc::RecvTimeoutError::Disconnected) => return false,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
}

unsafe fn property(source: CFRef, key: CFRef) -> bool {
    !source.is_null() && TISGetInputSourceProperty(source, key) == kCFBooleanTrue
}

unsafe fn selectable_non_ascii(source: CFRef) -> bool {
    !source.is_null()
        && property(source, kTISPropertyInputSourceIsEnabled)
        && property(source, kTISPropertyInputSourceIsSelectCapable)
        && !property(source, kTISPropertyInputSourceIsASCIICapable)
        && {
            let category = TISGetInputSourceProperty(source, kTISPropertyInputSourceCategory);
            !category.is_null() && CFEqual(category, kTISCategoryKeyboardInputSource) != 0
        }
}

fn switch_on_main_thread() -> bool {
    unsafe {
        let key = OwnedCF(CFStringCreateWithCString(
            null(),
            c"TISRomanSwitchState".as_ptr(),
            0x0800_0100,
        ));
        if key.0.is_null() {
            return false;
        }
        let mut valid = 0;
        let enabled =
            CFPreferencesGetAppBooleanValue(key.0, kCFPreferencesAnyApplication, &mut valid);
        // macOS can leave this key unset when using its default CJK/Latin
        // switching behavior. An explicit zero disables switching; with no
        // override we still require both an ASCII and non-ASCII source below.
        if valid != 0 && enabled == 0 {
            return false;
        }
        let current = OwnedCF(TISCopyCurrentKeyboardInputSource());
        if current.0.is_null() {
            return false;
        }
        let target = if !property(current.0, kTISPropertyInputSourceIsASCIICapable) {
            let ascii = OwnedCF(TISCopyCurrentASCIICapableKeyboardInputSource());
            LAST_NON_ASCII.with(|last| *last.borrow_mut() = Some(OwnedCF(CFRetain(current.0))));
            ascii
        } else {
            let remembered = LAST_NON_ASCII.with(|last| {
                last.borrow()
                    .as_ref()
                    .filter(|source| selectable_non_ascii(source.0))
                    .map(|source| OwnedCF(CFRetain(source.0)))
            });
            if let Some(remembered) = remembered {
                remembered
            } else {
                let list = OwnedCF(TISCreateInputSourceList(null(), 0));
                if list.0.is_null() {
                    return false;
                }
                let Some(source) = (0..CFArrayGetCount(list.0))
                    .map(|index| CFArrayGetValueAtIndex(list.0, index))
                    .find(|source| selectable_non_ascii(*source))
                else {
                    return false;
                };
                OwnedCF(CFRetain(source))
            }
        };
        if target.0.is_null() || CFEqual(current.0, target.0) != 0 {
            return false;
        }
        // Enabling the source before selecting it also activates its IME
        // session in the focused application, rather than just changing its icon.
        let enabled = TISEnableInputSource(target.0);
        let selected = if enabled == 0 {
            TISSelectInputSource(target.0)
        } else {
            enabled
        };
        if selected != 0 {
            log::warn!("Caps Lock input-source switch failed: {selected}");
            return false;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shutdown_cancels_a_switch_waiting_for_the_main_thread() {
        let (_tx, rx) = mpsc::sync_channel(1);
        let cancelled = AtomicBool::new(false);
        let stop = AtomicBool::new(true);
        assert!(!wait_for_switch(
            rx,
            &cancelled,
            &stop,
            Duration::from_secs(60)
        ));
        assert!(cancelled.load(Ordering::Acquire));
    }

    #[test]
    fn timed_out_main_thread_task_is_cancelled_before_it_can_switch_sources() {
        let (_tx, rx) = mpsc::sync_channel(1);
        let cancelled = AtomicBool::new(false);
        assert!(!wait_for_switch(
            rx,
            &cancelled,
            &AtomicBool::new(false),
            Duration::ZERO
        ));
        assert!(cancelled.load(Ordering::Acquire));
    }

    #[test]
    fn acknowledged_switch_does_not_fall_through_to_caps_lock() {
        let (tx, rx) = mpsc::sync_channel(1);
        tx.send(true).unwrap();
        assert!(wait_for_switch(
            rx,
            &AtomicBool::new(false),
            &AtomicBool::new(false),
            Duration::from_secs(1)
        ));
    }
}
