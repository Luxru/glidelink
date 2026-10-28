//! Platform latency hints for the input and network threads.
//!
//! These calls are best-effort: a failure must never fail a session.

#[cfg(target_os = "macos")]
extern "C" {
    // libc 0.2.191 no longer re-exports this Apple extension at the crate
    // root, so declare the stable C ABI directly. QOS_CLASS_USER_INTERACTIVE
    // is 0x21 (sys/qos.h).
    fn pthread_set_qos_class_self_np(qos_class: u32, relative_priority: i32) -> i32;
}

/// Raise the current thread to the platform's interactive/input QoS class.
pub fn boost_current_thread() {
    #[cfg(target_os = "macos")]
    {
        let result = unsafe { pthread_set_qos_class_self_np(0x21, 0) };
        if result != 0 {
            log::debug!("pthread QoS failed: {result}");
        }
    }

    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::Threading::{
            GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_HIGHEST,
        };
        if SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_HIGHEST) == 0 {
            log::debug!("SetThreadPriority failed");
        }
    }
}

/// Keeps the process out of macOS App Nap and restores low-latency timer
/// behavior on Windows for the lifetime of a session.
pub struct SessionGuard {
    #[cfg(target_os = "macos")]
    activity:
        objc2::rc::Retained<objc2::runtime::ProtocolObject<dyn objc2_foundation::NSObjectProtocol>>,
    #[cfg(windows)]
    timer_resolution: bool,
}

impl SessionGuard {
    pub fn begin() -> Self {
        #[cfg(target_os = "macos")]
        {
            use objc2_foundation::{NSActivityOptions, NSProcessInfo, NSString};

            let info = NSProcessInfo::processInfo();
            let reason = NSString::from_str("GlideLink active input session");
            let options = NSActivityOptions::UserInitiatedAllowingIdleSystemSleep
                | NSActivityOptions::LatencyCritical;
            let activity = info.beginActivityWithOptions_reason(options, &reason);
            log::debug!("macOS App Nap prevention active");
            Self { activity }
        }

        #[cfg(windows)]
        {
            use std::ffi::c_void;
            use std::mem::size_of;
            use windows_sys::Win32::Media::timeBeginPeriod;
            use windows_sys::Win32::System::Threading::{
                GetCurrentProcess, ProcessPowerThrottling, SetProcessInformation,
                PROCESS_POWER_THROTTLING_CURRENT_VERSION,
                PROCESS_POWER_THROTTLING_IGNORE_TIMER_RESOLUTION, PROCESS_POWER_THROTTLING_STATE,
            };

            let timer_resolution = unsafe { timeBeginPeriod(1) } == 0;
            if !timer_resolution {
                log::debug!("timeBeginPeriod(1) failed");
            }

            // Windows 11 can ignore a raised timer resolution for occluded or
            // minimized windows unless this throttling bit is cleared. Older
            // Windows versions simply reject the call; that is fine.
            let state = PROCESS_POWER_THROTTLING_STATE {
                Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
                ControlMask: PROCESS_POWER_THROTTLING_IGNORE_TIMER_RESOLUTION,
                StateMask: 0,
            };
            let configured = unsafe {
                SetProcessInformation(
                    GetCurrentProcess(),
                    ProcessPowerThrottling,
                    &state as *const _ as *const c_void,
                    size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
                )
            } != 0;
            log::debug!("timer resolution 1ms: {timer_resolution}, throttle opt-out: {configured}");

            Self { timer_resolution }
        }

        #[cfg(not(any(target_os = "macos", windows)))]
        {
            Self {}
        }
    }
}

impl Drop for SessionGuard {
    fn drop(&mut self) {
        #[cfg(target_os = "macos")]
        {
            use objc2_foundation::NSProcessInfo;

            let info = NSProcessInfo::processInfo();
            unsafe {
                info.endActivity(&self.activity);
            }
            log::debug!("macOS App Nap prevention released");
        }

        #[cfg(windows)]
        {
            if self.timer_resolution {
                unsafe {
                    windows_sys::Win32::Media::timeEndPeriod(1);
                }
            }
        }
    }
}
