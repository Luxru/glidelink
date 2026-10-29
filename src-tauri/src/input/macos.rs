use super::{
    keymap::{
        hid_to_mac, is_caps, is_modifier, mac_to_hid, KeyEvent, KeyPacket, KeyState, CAPS_LOCK,
        KEY_DEFS,
    },
    plan_injection_batch, CaptureHandle, CaptureMsg, CaptureShared, InjectCursor, InjectMsg,
    InjectStep, KeyAction, KeyOwnership, RemoteInput, BUTTON_LEFT, BUTTON_MIDDLE, BUTTON_RIGHT,
    BUTTON_X1, BUTTON_X2,
};
use crate::error;
use crate::layout::{
    advance, clamp_for_edge, entry_point, has_returned, host_point_to_norm, host_return_point,
    is_outward, Edge, NormPoint, REENTRY_COOLDOWN_MS,
};
use crate::status::{ScreenInfo, Status};
use crossbeam_channel::{Receiver, Sender};
use std::ffi::c_void;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

type CGEventRef = *mut c_void;
type CFMachPortRef = *mut c_void;
type CFRunLoopRef = *mut c_void;
type CFRunLoopSourceRef = *mut c_void;
type CFStringRef = *const c_void;
type CFAllocatorRef = *const c_void;
type CFDictionaryRef = *const c_void;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct CGPoint {
    x: f64,
    y: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct CGSize {
    width: f64,
    height: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct CGRect {
    origin: CGPoint,
    size: CGSize,
}

impl CGRect {
    fn min_x(self) -> f64 {
        self.origin.x
    }

    fn min_y(self) -> f64 {
        self.origin.y
    }

    fn max_x(self) -> f64 {
        self.origin.x + self.size.width
    }

    fn max_y(self) -> f64 {
        self.origin.y + self.size.height
    }
}

const CG_TAP_LOCATION_HID: u32 = 0;
const CG_TAP_PLACEMENT_HEAD_INSERT: u32 = 0;
const CG_TAP_OPTION_DEFAULT: u32 = 0;

const CG_EVENT_LEFT_MOUSE_DOWN: u32 = 1;
const CG_EVENT_LEFT_MOUSE_UP: u32 = 2;
const CG_EVENT_RIGHT_MOUSE_DOWN: u32 = 3;
const CG_EVENT_RIGHT_MOUSE_UP: u32 = 4;
const CG_EVENT_MOUSE_MOVED: u32 = 5;
const CG_EVENT_LEFT_MOUSE_DRAGGED: u32 = 6;
const CG_EVENT_RIGHT_MOUSE_DRAGGED: u32 = 7;
const CG_EVENT_KEY_DOWN: u32 = 10;
const CG_EVENT_KEY_UP: u32 = 11;
const CG_EVENT_FLAGS_CHANGED: u32 = 12;
const CG_EVENT_SCROLL_WHEEL: u32 = 22;
const CG_EVENT_OTHER_MOUSE_DOWN: u32 = 25;
const CG_EVENT_OTHER_MOUSE_UP: u32 = 26;
const CG_EVENT_OTHER_MOUSE_DRAGGED: u32 = 27;
const CG_EVENT_TAP_DISABLED_BY_TIMEOUT: u32 = 0xFFFF_FFFE;
const CG_EVENT_TAP_DISABLED_BY_USER_INPUT: u32 = 0xFFFF_FFFF;

const CG_MOUSE_EVENT_DELTA_X: u32 = 4;
const CG_MOUSE_EVENT_DELTA_Y: u32 = 5;
const CG_MOUSE_EVENT_BUTTON_NUMBER: u32 = 3;
const CG_MOUSE_EVENT_CLICK_STATE: u32 = 1;
const CG_SCROLL_WHEEL_EVENT_DELTA_AXIS_1: u32 = 11;
const DOUBLE_CLICK_INTERVAL: Duration = Duration::from_millis(500);
const DOUBLE_CLICK_DISTANCE: f64 = 4.0;
const CG_KEYBOARD_EVENT_AUTOREPEAT: u32 = 8;
const CG_KEYBOARD_EVENT_KEYCODE: u32 = 9;

const CG_FLAG_NON_COALESCED: u64 = 0x0000_0100;
const CG_FLAG_ALPHA_SHIFT: u64 = 1 << 16;
const CG_FLAG_SHIFT: u64 = 1 << 17;
const CG_FLAG_CONTROL: u64 = 1 << 18;
const CG_FLAG_ALTERNATE: u64 = 1 << 19;
const CG_FLAG_COMMAND: u64 = 1 << 20;
const CG_FLAG_NUMERIC_PAD: u64 = 0x0020_0000;
const CG_FLAG_SECONDARY_FN: u64 = 0x0080_0000;

// IOKit NX_DEVICE*KEYMASK bits carried by hardware FlagsChanged events.
const NX_DEVICE_L_CTRL: u64 = 0x0000_0001;
const NX_DEVICE_L_SHIFT: u64 = 0x0000_0002;
const NX_DEVICE_R_SHIFT: u64 = 0x0000_0004;
const NX_DEVICE_L_CMD: u64 = 0x0000_0008;
const NX_DEVICE_R_CMD: u64 = 0x0000_0010;
const NX_DEVICE_L_ALT: u64 = 0x0000_0020;
const NX_DEVICE_R_ALT: u64 = 0x0000_0040;
const NX_DEVICE_R_CTRL: u64 = 0x0000_2000;

const CG_EVENT_SOURCE_STATE_HID_SYSTEM: i32 = 1;
const CG_SCROLL_EVENT_UNIT_LINE: u32 = 1;

const CG_MOUSE_BUTTON_LEFT: u32 = 0;
const CG_MOUSE_BUTTON_RIGHT: u32 = 1;
const CG_MOUSE_BUTTON_CENTER: u32 = 2;
const CG_MOUSE_BUTTON_X1: u32 = 3;
const CG_MOUSE_BUTTON_X2: u32 = 4;

const CG_EVENT_MASK: u64 = (1 << CG_EVENT_LEFT_MOUSE_DOWN)
    | (1 << CG_EVENT_LEFT_MOUSE_UP)
    | (1 << CG_EVENT_RIGHT_MOUSE_DOWN)
    | (1 << CG_EVENT_RIGHT_MOUSE_UP)
    | (1 << CG_EVENT_MOUSE_MOVED)
    | (1 << CG_EVENT_LEFT_MOUSE_DRAGGED)
    | (1 << CG_EVENT_RIGHT_MOUSE_DRAGGED)
    | (1 << CG_EVENT_KEY_DOWN)
    | (1 << CG_EVENT_KEY_UP)
    | (1 << CG_EVENT_FLAGS_CHANGED)
    | (1 << CG_EVENT_SCROLL_WHEEL)
    | (1 << CG_EVENT_OTHER_MOUSE_DOWN)
    | (1 << CG_EVENT_OTHER_MOUSE_UP)
    | (1 << CG_EVENT_OTHER_MOUSE_DRAGGED);

type CGEventTapCallBack =
    Option<unsafe extern "C" fn(*mut c_void, u32, CGEventRef, *mut c_void) -> CGEventRef>;

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventTapCreate(
        tap: u32,
        place: u32,
        options: u32,
        events_of_interest: u64,
        callback: CGEventTapCallBack,
        user_info: *mut c_void,
    ) -> CFMachPortRef;
    fn CGEventTapEnable(tap: CFMachPortRef, enable: u8);
    fn CGEventCreate(source: *mut c_void) -> CGEventRef;
    fn CGEventGetLocation(event: CGEventRef) -> CGPoint;
    fn CGEventGetIntegerValueField(event: CGEventRef, field: u32) -> i64;
    fn CGEventSetIntegerValueField(event: CGEventRef, field: u32, value: i64);
    fn CGEventSetType(event: CGEventRef, event_type: u32);
    fn CGEventPost(tap: u32, event: CGEventRef);
    fn CGEventCreateMouseEvent(
        source: *mut c_void,
        mouse_type: u32,
        mouse_cursor_position: CGPoint,
        mouse_button: u32,
    ) -> CGEventRef;
    fn CGEventCreateScrollWheelEvent(
        source: *mut c_void,
        units: u32,
        wheel_count: u32,
        ...
    ) -> CGEventRef;
    fn CGEventCreateKeyboardEvent(source: *mut c_void, key: u16, key_down: u8) -> CGEventRef;
    fn CGEventSetFlags(event: CGEventRef, flags: u64);
    fn CGEventSourceCreate(state_id: i32) -> *mut c_void;
    fn CGEventSourceButtonState(state_id: i32, button: u32) -> u8;
    fn CGEventSourceFlagsState(state_id: i32) -> u64;
    fn CGEventSourceKeyState(state_id: i32, key: u16) -> u8;
    fn CGMainDisplayID() -> u32;
    fn CGDisplayHideCursor(display: u32) -> i32;
    fn CGDisplayShowCursor(display: u32) -> i32;
    fn CGAssociateMouseAndMouseCursorPosition(connected: i32) -> i32;
    fn CGWarpMouseCursorPosition(point: CGPoint) -> i32;
    fn CGGetActiveDisplayList(
        max_displays: u32,
        active_displays: *mut u32,
        display_count: *mut u32,
    ) -> i32;
    fn CGDisplayBounds(display: u32) -> CGRect;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFMachPortCreateRunLoopSource(
        allocator: CFAllocatorRef,
        port: CFMachPortRef,
        order: isize,
    ) -> CFRunLoopSourceRef;
    fn CFRunLoopGetCurrent() -> CFRunLoopRef;
    fn CFRunLoopAddSource(rl: CFRunLoopRef, source: CFRunLoopSourceRef, mode: CFStringRef);
    fn CFRunLoopRemoveSource(rl: CFRunLoopRef, source: CFRunLoopSourceRef, mode: CFStringRef);
    fn CFRunLoopRunInMode(mode: CFStringRef, seconds: f64, return_after_source_handled: u8) -> i32;
    fn CFRelease(cf: *const c_void);
    fn CFDictionaryCreate(
        allocator: CFAllocatorRef,
        keys: *const *const c_void,
        values: *const *const c_void,
        num_values: isize,
        key_callbacks: *const c_void,
        value_callbacks: *const c_void,
    ) -> CFDictionaryRef;
    static kCFRunLoopDefaultMode: CFStringRef;
    static kCFBooleanTrue: *const c_void;
}

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> u8;
    fn AXIsProcessTrustedWithOptions(options: CFDictionaryRef) -> u8;
    fn CGPreflightListenEventAccess() -> u8;
    fn CGPreflightPostEventAccess() -> u8;
    fn CGRequestListenEventAccess() -> u8;
    fn CGRequestPostEventAccess() -> u8;
    static kAXTrustedCheckOptionPrompt: CFStringRef;
}

fn virtual_bounds() -> Option<CGRect> {
    let mut displays = [0u32; 16];
    let mut count = 0u32;
    let result =
        unsafe { CGGetActiveDisplayList(displays.len() as u32, displays.as_mut_ptr(), &mut count) };
    if result != 0 || count == 0 {
        return None;
    }
    let mut min_x = f64::INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut max_y = f64::NEG_INFINITY;
    for display in displays.iter().take(count as usize) {
        let bounds = unsafe { CGDisplayBounds(*display) };
        if bounds.size.width <= 0.0 || bounds.size.height <= 0.0 {
            continue;
        }
        min_x = min_x.min(bounds.min_x());
        min_y = min_y.min(bounds.min_y());
        max_x = max_x.max(bounds.max_x());
        max_y = max_y.max(bounds.max_y());
    }
    if !min_x.is_finite() || !min_y.is_finite() || max_x <= min_x || max_y <= min_y {
        return None;
    }
    Some(CGRect {
        origin: CGPoint { x: min_x, y: min_y },
        size: CGSize {
            width: max_x - min_x,
            height: max_y - min_y,
        },
    })
}

pub fn screen_info() -> Option<ScreenInfo> {
    let bounds = virtual_bounds()?;
    Some(ScreenInfo::new(
        bounds.size.width.round() as i32,
        bounds.size.height.round() as i32,
    ))
}

fn accessibility_trusted() -> bool {
    unsafe { AXIsProcessTrusted() != 0 }
}

fn listen_event_access() -> bool {
    unsafe { CGPreflightListenEventAccess() != 0 }
}

fn post_event_access() -> bool {
    unsafe { CGPreflightPostEventAccess() != 0 }
}

pub fn permissions_ok() -> bool {
    accessibility_trusted() && listen_event_access() && post_event_access()
}

/// Ask macOS to show its permission prompt. Returns the current accessibility
/// status; the user's answer is asynchronous and usually requires a relaunch.
fn request_accessibility_prompt() -> bool {
    unsafe {
        let key = kAXTrustedCheckOptionPrompt as *const c_void;
        let value = kCFBooleanTrue;
        let options = CFDictionaryCreate(null(), &key, &value, 1, null(), null());
        if options.is_null() {
            return accessibility_trusted();
        }
        let trusted = AXIsProcessTrustedWithOptions(options);
        CFRelease(options as *const c_void);
        trusted != 0
    }
}

pub fn open_permissions() -> Result<(), String> {
    // Trigger the system prompt for whichever permission is missing, then open
    // the matching pane. Input Monitoring is a separate pane from Accessibility.
    let pane = if !accessibility_trusted() {
        let _ = request_accessibility_prompt();
        "Privacy_Accessibility"
    } else if !listen_event_access() {
        unsafe {
            CGRequestListenEventAccess();
        }
        "Privacy_ListenEvent"
    } else {
        if !post_event_access() {
            unsafe {
                CGRequestPostEventAccess();
            }
        }
        "Privacy_Accessibility"
    };

    log::info!("opening macOS privacy pane: {pane}");
    std::process::Command::new("open")
        .arg(format!(
            "x-apple.systempreferences:com.apple.preference.security?{pane}"
        ))
        .spawn()
        .map(|_| ())
        .map_err(|e| error::detail("E_OPEN_SETTINGS", e))
}

fn active_displays() -> ([u32; 16], usize) {
    let mut displays = [0u32; 16];
    let mut count = 0u32;
    let result =
        unsafe { CGGetActiveDisplayList(displays.len() as u32, displays.as_mut_ptr(), &mut count) };
    if result != 0 {
        return (displays, 0);
    }
    (displays, count as usize)
}

fn hide_cursor() {
    let (displays, count) = active_displays();
    if count == 0 {
        unsafe {
            CGDisplayHideCursor(CGMainDisplayID());
        }
        return;
    }
    for display in displays.iter().take(count) {
        unsafe {
            CGDisplayHideCursor(*display);
        }
    }
}

fn show_cursor() {
    let (displays, count) = active_displays();
    if count == 0 {
        unsafe {
            CGDisplayShowCursor(CGMainDisplayID());
        }
        return;
    }
    for display in displays.iter().take(count) {
        unsafe {
            CGDisplayShowCursor(*display);
        }
    }
}

fn cursor_location() -> CGPoint {
    unsafe {
        let event = CGEventCreate(null_mut());
        if event.is_null() {
            return CGPoint::default();
        }
        let location = CGEventGetLocation(event);
        CFRelease(event as *const c_void);
        location
    }
}

fn warp_cursor(point: CGPoint) {
    unsafe {
        CGWarpMouseCursorPosition(point);
    }
}

#[derive(Clone, Copy)]
struct ButtonClick {
    last_down: Option<Instant>,
    last_point: CGPoint,
    count: i64,
    down_count: i64,
}

impl Default for ButtonClick {
    fn default() -> Self {
        Self {
            last_down: None,
            last_point: CGPoint::default(),
            count: 1,
            down_count: 1,
        }
    }
}

struct ClickTracker {
    interval: Duration,
    buttons: [ButtonClick; 5],
}

impl ClickTracker {
    fn new() -> Self {
        Self {
            interval: DOUBLE_CLICK_INTERVAL,
            buttons: [ButtonClick::default(); 5],
        }
    }

    fn slot(bit: u8) -> usize {
        match bit {
            BUTTON_LEFT => 0,
            BUTTON_RIGHT => 1,
            BUTTON_MIDDLE => 2,
            BUTTON_X1 => 3,
            _ => 4,
        }
    }

    fn down(&mut self, bit: u8, point: CGPoint) -> i64 {
        let now = Instant::now();
        let slot = &mut self.buttons[Self::slot(bit)];
        let repeated = slot
            .last_down
            .map(|last| {
                now.saturating_duration_since(last) <= self.interval
                    && (slot.last_point.x - point.x).abs() <= DOUBLE_CLICK_DISTANCE
                    && (slot.last_point.y - point.y).abs() <= DOUBLE_CLICK_DISTANCE
            })
            .unwrap_or(false);
        slot.count = if repeated { (slot.count + 1).min(3) } else { 1 };
        slot.last_down = Some(now);
        slot.last_point = point;
        slot.down_count = slot.count;
        slot.count
    }

    fn up(&mut self, bit: u8) -> i64 {
        self.buttons[Self::slot(bit)].down_count.max(1)
    }

    fn reset(&mut self) {
        self.buttons = [ButtonClick::default(); 5];
    }
}

struct CaptureState {
    shared: Arc<CaptureShared>,
    tx: Sender<CaptureMsg>,
    edge: Edge,
    offset: f32,
    bounds: CGRect,
    tap: CFMachPortRef,
    active: bool,
    hidden: bool,
    buttons: u8,
    point: NormPoint,
    cooldown_until: Instant,
    keys: KeyOwnership,
    caps_lock: bool,
    caps_down: bool,
}

impl CaptureState {
    fn send(&self, wheel: i16) {
        let input = RemoteInput {
            x: self.point.x,
            y: self.point.y,
            buttons: self.buttons,
            wheel,
        };
        let _ = self.tx.try_send(CaptureMsg::Mouse(input));
    }

    fn send_keys(&self, event: KeyEvent) {
        let packet = KeyPacket {
            active: true,
            caps_lock: self.caps_lock,
            state: self.keys.remote,
            event,
        };
        let _ = self.tx.try_send(CaptureMsg::Keys(packet));
    }

    fn send_inactive_keys(&self) {
        let packet = KeyPacket {
            active: false,
            caps_lock: self.caps_lock,
            state: KeyState::default(),
            event: KeyEvent::None,
        };
        let _ = self.tx.try_send(CaptureMsg::Keys(packet));
    }

    fn apply_key_action(&mut self, action: KeyAction, hid: u8, event: CGEventRef) -> CGEventRef {
        match action {
            KeyAction::Down => {
                self.send_keys(KeyEvent::None);
                std::ptr::null_mut()
            }
            KeyAction::Up => {
                self.send_keys(KeyEvent::None);
                std::ptr::null_mut()
            }
            KeyAction::Repeat => {
                self.send_keys(KeyEvent::Repeat(hid));
                std::ptr::null_mut()
            }
            KeyAction::Passthrough => event,
            KeyAction::Swallow => std::ptr::null_mut(),
        }
    }

    fn handle_key_event(&mut self, event_type: u32, event: CGEventRef) -> CGEventRef {
        if !self.active {
            return event;
        }

        let keycode =
            unsafe { CGEventGetIntegerValueField(event, CG_KEYBOARD_EVENT_KEYCODE) } as u16;
        let Some(hid) = mac_to_hid(keycode) else {
            // Unsupported keys are swallowed while remote, except their release
            // so a key held before activation cannot stick locally.
            return if event_type == CG_EVENT_KEY_UP {
                event
            } else {
                std::ptr::null_mut()
            };
        };

        if is_caps(hid) {
            let physical_down =
                unsafe { CGEventSourceKeyState(CG_EVENT_SOURCE_STATE_HID_SYSTEM, keycode) } != 0;
            let pressed = match event_type {
                CG_EVENT_KEY_DOWN => true,
                CG_EVENT_KEY_UP => false,
                CG_EVENT_FLAGS_CHANGED => physical_down,
                _ => false,
            };
            if pressed {
                if !self.caps_down {
                    self.caps_down = true;
                    self.caps_lock = !self.caps_lock;
                    self.send_keys(KeyEvent::None);
                }
            } else {
                self.caps_down = false;
            }
            return std::ptr::null_mut();
        }

        if event_type == CG_EVENT_FLAGS_CHANGED {
            if is_modifier(hid) {
                let was_down = self.keys.local.contains(hid) || self.keys.remote.contains(hid);
                let action = if was_down {
                    self.keys.key_up(true, hid)
                } else {
                    self.keys.key_down(true, hid)
                };
                return self.apply_key_action(action, hid, event);
            }
            return std::ptr::null_mut();
        }

        if is_modifier(hid) {
            // Modifier transitions arrive as flagsChanged; ignore duplicates.
            return std::ptr::null_mut();
        }

        if event_type == CG_EVENT_KEY_DOWN {
            let repeat =
                unsafe { CGEventGetIntegerValueField(event, CG_KEYBOARD_EVENT_AUTOREPEAT) } != 0;
            let action = self.keys.key_down(true, hid);
            if repeat {
                match action {
                    KeyAction::Repeat => self.apply_key_action(KeyAction::Repeat, hid, event),
                    KeyAction::Down => {
                        self.send_keys(KeyEvent::None);
                        std::ptr::null_mut()
                    }
                    _ => std::ptr::null_mut(),
                }
            } else {
                match action {
                    KeyAction::Down | KeyAction::Repeat => {
                        self.send_keys(KeyEvent::None);
                        std::ptr::null_mut()
                    }
                    _ => std::ptr::null_mut(),
                }
            }
        } else if event_type == CG_EVENT_KEY_UP {
            let action = self.keys.key_up(true, hid);
            self.apply_key_action(action, hid, event)
        } else {
            std::ptr::null_mut()
        }
    }

    fn at_edge(&self, location: CGPoint) -> bool {
        match self.edge {
            Edge::Right => location.x >= self.bounds.max_x() - 1.5,
            Edge::Left => location.x <= self.bounds.min_x() + 1.5,
            Edge::Top => location.y <= self.bounds.min_y() + 1.5,
            Edge::Bottom => location.y >= self.bounds.max_y() - 1.5,
        }
    }

    fn update_buttons(&mut self, event_type: u32, event: CGEventRef) {
        match event_type {
            CG_EVENT_LEFT_MOUSE_DOWN => self.buttons |= BUTTON_LEFT,
            CG_EVENT_LEFT_MOUSE_UP => self.buttons &= !BUTTON_LEFT,
            CG_EVENT_RIGHT_MOUSE_DOWN => self.buttons |= BUTTON_RIGHT,
            CG_EVENT_RIGHT_MOUSE_UP => self.buttons &= !BUTTON_RIGHT,
            CG_EVENT_OTHER_MOUSE_DOWN => {
                let number =
                    unsafe { CGEventGetIntegerValueField(event, CG_MOUSE_EVENT_BUTTON_NUMBER) };
                self.buttons |= other_button_bit(number);
            }
            CG_EVENT_OTHER_MOUSE_UP => {
                let number =
                    unsafe { CGEventGetIntegerValueField(event, CG_MOUSE_EVENT_BUTTON_NUMBER) };
                self.buttons &= !other_button_bit(number);
            }
            _ => {}
        }
    }

    fn handle_event(&mut self, event_type: u32, event: CGEventRef) -> CGEventRef {
        if event_type == CG_EVENT_TAP_DISABLED_BY_TIMEOUT
            || event_type == CG_EVENT_TAP_DISABLED_BY_USER_INPUT
        {
            if !self.tap.is_null() {
                unsafe {
                    CGEventTapEnable(self.tap, 1);
                }
            }
            if self.active {
                let held = current_held_keys();
                self.keys.rebind(held);
                self.caps_down = false;
                self.send_keys(KeyEvent::None);
            }
            return event;
        }

        if matches!(
            event_type,
            CG_EVENT_KEY_DOWN | CG_EVENT_KEY_UP | CG_EVENT_FLAGS_CHANGED
        ) {
            return self.handle_key_event(event_type, event);
        }

        let location = unsafe { CGEventGetLocation(event) };
        let dx = unsafe { CGEventGetIntegerValueField(event, CG_MOUSE_EVENT_DELTA_X) } as f64;
        let dy = unsafe { CGEventGetIntegerValueField(event, CG_MOUSE_EVENT_DELTA_Y) } as f64;

        if self.active {
            match event_type {
                CG_EVENT_MOUSE_MOVED
                | CG_EVENT_LEFT_MOUSE_DRAGGED
                | CG_EVENT_RIGHT_MOUSE_DRAGGED
                | CG_EVENT_OTHER_MOUSE_DRAGGED => {
                    self.point = advance(
                        self.point,
                        dx,
                        dy,
                        self.bounds.size.width.round() as i32,
                        self.bounds.size.height.round() as i32,
                    );
                    if has_returned(self.edge, self.point) {
                        self.deactivate(true);
                    } else {
                        self.point = clamp_for_edge(self.edge, self.point);
                        self.send(0);
                    }
                }
                CG_EVENT_LEFT_MOUSE_DOWN
                | CG_EVENT_LEFT_MOUSE_UP
                | CG_EVENT_RIGHT_MOUSE_DOWN
                | CG_EVENT_RIGHT_MOUSE_UP
                | CG_EVENT_OTHER_MOUSE_DOWN
                | CG_EVENT_OTHER_MOUSE_UP => {
                    self.update_buttons(event_type, event);
                    self.send(0);
                }
                CG_EVENT_SCROLL_WHEEL => {
                    let wheel = unsafe {
                        CGEventGetIntegerValueField(event, CG_SCROLL_WHEEL_EVENT_DELTA_AXIS_1)
                    };
                    let wheel = wheel.clamp(i16::MIN as i64, i16::MAX as i64) as i16;
                    if wheel != 0 {
                        self.send(wheel);
                    }
                }
                _ => {}
            }
            return null_mut();
        }

        match event_type {
            CG_EVENT_MOUSE_MOVED
            | CG_EVENT_LEFT_MOUSE_DRAGGED
            | CG_EVENT_RIGHT_MOUSE_DRAGGED
            | CG_EVENT_OTHER_MOUSE_DRAGGED => {
                if self.shared.is_connected()
                    && Instant::now() >= self.cooldown_until
                    && self.at_edge(location)
                    && is_outward(self.edge, dx, dy)
                {
                    self.activate(location);
                    return null_mut();
                }
            }
            CG_EVENT_LEFT_MOUSE_DOWN
            | CG_EVENT_LEFT_MOUSE_UP
            | CG_EVENT_RIGHT_MOUSE_DOWN
            | CG_EVENT_RIGHT_MOUSE_UP
            | CG_EVENT_OTHER_MOUSE_DOWN
            | CG_EVENT_OTHER_MOUSE_UP => {
                self.update_buttons(event_type, event);
            }
            _ => {}
        }

        event
    }

    fn activate(&mut self, host_location: CGPoint) {
        if self.active {
            return;
        }
        let host = host_point_to_norm(
            host_location.x,
            host_location.y,
            self.bounds.min_x().round() as i32,
            self.bounds.min_y().round() as i32,
            self.bounds.size.width.round() as i32,
            self.bounds.size.height.round() as i32,
        );
        self.point = entry_point(self.edge, host, self.offset);
        self.buttons = unsafe {
            let mut buttons = 0u8;
            for (button, bit) in [
                (CG_MOUSE_BUTTON_LEFT, BUTTON_LEFT),
                (CG_MOUSE_BUTTON_RIGHT, BUTTON_RIGHT),
                (CG_MOUSE_BUTTON_CENTER, BUTTON_MIDDLE),
                (CG_MOUSE_BUTTON_X1, BUTTON_X1),
                (CG_MOUSE_BUTTON_X2, BUTTON_X2),
            ] {
                if CGEventSourceButtonState(CG_EVENT_SOURCE_STATE_HID_SYSTEM, button) != 0 {
                    buttons |= bit;
                }
            }
            buttons
        };
        self.active = true;
        self.shared.set_active(true);
        self.keys.begin(current_held_keys());
        self.caps_lock = current_caps_lock();
        self.caps_down = false;
        if !self.hidden {
            hide_cursor();
            unsafe {
                CGAssociateMouseAndMouseCursorPosition(0);
            }
            self.hidden = true;
        }
        self.send_keys(KeyEvent::None);
        self.send(0);
    }

    fn deactivate(&mut self, warp_back: bool) {
        if !self.active {
            if self.hidden {
                unsafe {
                    CGAssociateMouseAndMouseCursorPosition(1);
                }
                show_cursor();
                self.hidden = false;
            }
            return;
        }
        self.active = false;
        self.buttons = 0;
        self.point = self.point.clamped();
        self.send_inactive_keys();
        self.keys.deactivate();
        self.caps_down = false;
        self.send(0);
        self.shared.set_active(false);

        if self.hidden {
            unsafe {
                CGAssociateMouseAndMouseCursorPosition(1);
            }
            self.hidden = false;
        }

        if warp_back {
            let host = host_return_point(self.edge, self.point, self.offset);
            warp_cursor(CGPoint {
                x: self.bounds.min_x() + host.x as f64 * (self.bounds.size.width - 1.0).max(1.0),
                y: self.bounds.min_y() + host.y as f64 * (self.bounds.size.height - 1.0).max(1.0),
            });
        }
        show_cursor();
        self.cooldown_until = Instant::now() + Duration::from_millis(REENTRY_COOLDOWN_MS);
    }
}

fn current_held_keys() -> KeyState {
    let mut held = KeyState::default();
    for def in KEY_DEFS {
        if is_caps(def.hid) {
            continue;
        }
        if unsafe { CGEventSourceKeyState(CG_EVENT_SOURCE_STATE_HID_SYSTEM, def.mac) } != 0 {
            held.insert(def.hid);
        }
    }
    held
}

fn current_caps_lock() -> bool {
    unsafe { CGEventSourceFlagsState(CG_EVENT_SOURCE_STATE_HID_SYSTEM) & CG_FLAG_ALPHA_SHIFT != 0 }
}

unsafe extern "C" fn event_tap_callback(
    _proxy: *mut c_void,
    event_type: u32,
    event: CGEventRef,
    user_info: *mut c_void,
) -> CGEventRef {
    if user_info.is_null() {
        return event;
    }
    let state = &mut *(user_info as *mut CaptureState);
    state.handle_event(event_type, event)
}

pub fn start_capture(
    shared: Arc<CaptureShared>,
    tx: Sender<CaptureMsg>,
    edge: Edge,
    offset: f32,
) -> Result<CaptureHandle, String> {
    unsafe {
        if AXIsProcessTrusted() == 0 || CGPreflightListenEventAccess() == 0 {
            return Err(error::code("E_MAC_PERMISSIONS_CAPTURE"));
        }
    }
    let bounds = virtual_bounds().ok_or_else(|| error::code("E_MAC_DISPLAYS"))?;

    let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();
    let thread_shared = Arc::clone(&shared);
    let join = thread::Builder::new()
        .name("glidelink-input-capture".into())
        .spawn(move || run_capture_thread(thread_shared, tx, edge, offset, bounds, ready_tx))
        .map_err(|e| error::detail("E_CAPTURE_THREAD", e))?;

    match ready_rx.recv_timeout(Duration::from_secs(3)) {
        Ok(Ok(())) => Ok(CaptureHandle::new(shared, join)),
        Ok(Err(error)) => {
            shared.stop.store(true, Ordering::Release);
            let _ = join.join();
            Err(error)
        }
        Err(_) => {
            shared.stop.store(true, Ordering::Release);
            let _ = join.join();
            Err(error::code("E_CAPTURE_TIMEOUT"))
        }
    }
}

fn run_capture_thread(
    shared: Arc<CaptureShared>,
    tx: Sender<CaptureMsg>,
    edge: Edge,
    offset: f32,
    bounds: CGRect,
    ready_tx: mpsc::Sender<Result<(), String>>,
) {
    crate::sched::boost_current_thread();
    let mut state = Box::new(CaptureState {
        shared,
        tx,
        edge,
        offset,
        bounds,
        tap: null_mut(),
        active: false,
        hidden: false,
        buttons: 0,
        point: NormPoint::default(),
        cooldown_until: Instant::now(),
        keys: KeyOwnership::default(),
        caps_lock: false,
        caps_down: false,
    });
    let state_ptr = (&mut *state) as *mut CaptureState;

    let tap = unsafe {
        CGEventTapCreate(
            CG_TAP_LOCATION_HID,
            CG_TAP_PLACEMENT_HEAD_INSERT,
            CG_TAP_OPTION_DEFAULT,
            CG_EVENT_MASK,
            Some(event_tap_callback),
            state_ptr as *mut c_void,
        )
    };
    if tap.is_null() {
        let _ = ready_tx.send(Err(error::code("E_MAC_EVENT_TAP")));
        return;
    }
    state.tap = tap;

    let source = unsafe { CFMachPortCreateRunLoopSource(null(), tap, 0) };
    if source.is_null() {
        unsafe {
            CFRelease(tap as *const c_void);
        }
        let _ = ready_tx.send(Err(error::code("E_MAC_RUNLOOP")));
        return;
    }

    let run_loop = unsafe { CFRunLoopGetCurrent() };
    unsafe {
        CFRunLoopAddSource(run_loop, source, kCFRunLoopDefaultMode);
        CGEventTapEnable(tap, 1);
    }

    let _ = ready_tx.send(Ok(()));

    loop {
        let stop = state.shared.stop.load(Ordering::Acquire);
        if stop {
            break;
        }
        unsafe {
            CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.1, 0);
        }

        let state = unsafe { &mut *state_ptr };
        if state.active && !state.shared.is_connected() {
            state.deactivate(true);
        }
    }

    let state = unsafe { &mut *state_ptr };
    if state.active {
        state.deactivate(true);
    }
    if state.hidden {
        unsafe {
            CGAssociateMouseAndMouseCursorPosition(1);
        }
        show_cursor();
        state.hidden = false;
    }

    unsafe {
        CGEventTapEnable(tap, 0);
        CFRunLoopRemoveSource(run_loop, source, kCFRunLoopDefaultMode);
        CFRelease(source as *const c_void);
        CFRelease(tap as *const c_void);
    }
    // `state` is dropped here. The tap has been disabled and released above,
    // so no callback can reach its raw pointer afterwards.
}

pub fn start_injector(
    rx: Receiver<InjectMsg>,
    stop: Arc<AtomicBool>,
    _status: Status,
) -> Result<JoinHandle<()>, String> {
    unsafe {
        if AXIsProcessTrusted() == 0 || CGPreflightPostEventAccess() == 0 {
            return Err(error::code("E_MAC_PERMISSIONS_INJECT"));
        }
    }
    let bounds = virtual_bounds().ok_or_else(|| error::code("E_MAC_DISPLAYS"))?;

    let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();
    let thread_stop = Arc::clone(&stop);
    let join = thread::Builder::new()
        .name("glidelink-input-injector".into())
        .spawn(move || injector_loop(rx, thread_stop, bounds, ready_tx))
        .map_err(|e| error::detail("E_INJECTOR_THREAD", e))?;

    match ready_rx.recv_timeout(Duration::from_secs(2)) {
        Ok(Ok(())) => Ok(join),
        Ok(Err(error)) => {
            let _ = join.join();
            Err(error)
        }
        Err(_) => {
            stop.store(true, Ordering::Release);
            let _ = join.join();
            Err(error::code("E_INJECTOR_TIMEOUT"))
        }
    }
}

fn injector_loop(
    rx: Receiver<InjectMsg>,
    stop: Arc<AtomicBool>,
    bounds: CGRect,
    ready_tx: mpsc::Sender<Result<(), String>>,
) {
    crate::sched::boost_current_thread();
    let _ = ready_tx.send(Ok(()));
    let mut applied_buttons = 0u8;
    let mut applied_position: Option<(u16, u16)> = None;
    let mut clicks = ClickTracker::new();
    let mut applied_keys = KeyState::default();
    let mut caps_actual = unsafe {
        CGEventSourceFlagsState(CG_EVENT_SOURCE_STATE_HID_SYSTEM) & CG_FLAG_ALPHA_SHIFT != 0
    };
    let key_source = unsafe { CGEventSourceCreate(CG_EVENT_SOURCE_STATE_HID_SYSTEM) };
    let mut cursor = InjectCursor::default();
    let mut batch = Vec::with_capacity(16);
    let mut steps = Vec::with_capacity(16);

    while !stop.load(Ordering::Acquire) {
        let first = match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(msg) => msg,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        };

        batch.clear();
        batch.push(first);
        while batch.len() < 64 {
            match rx.try_recv() {
                Ok(msg) => batch.push(msg),
                Err(_) => break,
            }
        }

        steps.clear();
        plan_injection_batch(&batch, &mut cursor, &mut steps);
        for step in steps.iter().copied() {
            match step {
                InjectStep::Move { x, y, buttons } => {
                    if applied_position != Some((x, y)) {
                        let point = point_from_normalized(x, y, bounds);
                        let (event_type, button) = drag_event(buttons)
                            .unwrap_or((CG_EVENT_MOUSE_MOVED, CG_MOUSE_BUTTON_LEFT));
                        post_mouse_event(event_type, point, button);
                        applied_position = Some((x, y));
                    }
                }
                InjectStep::Buttons { from, to } => {
                    let point = applied_point_or_origin(applied_position, bounds);
                    apply_button_transitions(from, to, point, &mut clicks);
                    applied_buttons = to;
                }
                InjectStep::Wheel(delta) => unsafe {
                    let event = CGEventCreateScrollWheelEvent(
                        null_mut(),
                        CG_SCROLL_EVENT_UNIT_LINE,
                        1,
                        delta as i32,
                    );
                    if !event.is_null() {
                        CGEventPost(CG_TAP_LOCATION_HID, event);
                        CFRelease(event as *const c_void);
                    }
                },
                InjectStep::KeyPress(hid) => {
                    if !applied_keys.contains(hid) {
                        applied_keys.insert(hid);
                        if is_modifier(hid) {
                            post_modifier_event(
                                key_source,
                                hid,
                                &applied_keys,
                                caps_actual,
                            );
                        } else {
                            post_key_event(
                                key_source,
                                hid,
                                true,
                                false,
                                &applied_keys,
                                caps_actual,
                            );
                        }
                    }
                }
                InjectStep::KeyRelease(hid) => {
                    if applied_keys.contains(hid) {
                        applied_keys.remove(hid);
                        if is_modifier(hid) {
                            post_modifier_event(
                                key_source,
                                hid,
                                &applied_keys,
                                caps_actual,
                            );
                        } else {
                            post_key_event(
                                key_source,
                                hid,
                                false,
                                false,
                                &applied_keys,
                                caps_actual,
                            );
                        }
                    }
                }
                InjectStep::KeyRepeat(hid) => {
                    if applied_keys.contains(hid) && !is_modifier(hid) {
                        post_key_event(key_source, hid, true, true, &applied_keys, caps_actual);
                    }
                }
                InjectStep::CapsLock(desired) => {
                    if desired != caps_actual {
                        inject_caps_tap(key_source, desired, &applied_keys);
                        caps_actual = desired;
                    }
                }
                InjectStep::Reset => {
                    release_keys(key_source, &mut applied_keys, caps_actual);
                    let point = applied_point_or_origin(applied_position, bounds);
                    apply_button_transitions(applied_buttons, 0, point, &mut clicks);
                    applied_buttons = 0;
                    applied_position = None;
                    caps_actual = current_caps_lock();
                    clicks.reset();
                }
            }
        }
    }

    release_keys(key_source, &mut applied_keys, caps_actual);
    if applied_buttons != 0 {
        let point = applied_point_or_origin(applied_position, bounds);
        apply_button_transitions(applied_buttons, 0, point, &mut clicks);
    }
    if !key_source.is_null() {
        unsafe {
            CFRelease(key_source);
        }
    }
}

fn mac_flags(keys: &KeyState, caps_lock: bool) -> u64 {
    let mut flags = 0;
    if keys.contains(0xE1) || keys.contains(0xE5) {
        flags |= CG_FLAG_SHIFT;
    }
    if keys.contains(0xE0) || keys.contains(0xE4) {
        flags |= CG_FLAG_CONTROL;
    }
    if keys.contains(0xE2) || keys.contains(0xE6) {
        flags |= CG_FLAG_ALTERNATE;
    }
    if keys.contains(0xE3) || keys.contains(0xE7) {
        flags |= CG_FLAG_COMMAND;
    }
    if caps_lock {
        flags |= CG_FLAG_ALPHA_SHIFT;
    }
    flags
}

fn post_key_event(
    source: *mut c_void,
    hid: u8,
    down: bool,
    repeat: bool,
    keys: &KeyState,
    caps_lock: bool,
) {
    let Some(keycode) = hid_to_mac(hid) else {
        return;
    };
    unsafe {
        let event = CGEventCreateKeyboardEvent(source, keycode, if down { 1 } else { 0 });
        if event.is_null() {
            return;
        }
        let mut flags = mac_flags(keys, caps_lock);
        if matches!(keycode, 0x7B | 0x7C | 0x7D | 0x7E) {
            flags |= CG_FLAG_NUMERIC_PAD | CG_FLAG_SECONDARY_FN;
        }
        CGEventSetFlags(event, flags);
        if repeat {
            CGEventSetIntegerValueField(event, CG_KEYBOARD_EVENT_AUTOREPEAT, 1);
        }
        CGEventPost(CG_TAP_LOCATION_HID, event);
        CFRelease(event as *const c_void);
    }
}

fn mac_device_flags(keys: &KeyState) -> u64 {
    let mut flags = 0;
    if keys.contains(0xE0) {
        flags |= NX_DEVICE_L_CTRL;
    }
    if keys.contains(0xE4) {
        flags |= NX_DEVICE_R_CTRL;
    }
    if keys.contains(0xE1) {
        flags |= NX_DEVICE_L_SHIFT;
    }
    if keys.contains(0xE5) {
        flags |= NX_DEVICE_R_SHIFT;
    }
    if keys.contains(0xE2) {
        flags |= NX_DEVICE_L_ALT;
    }
    if keys.contains(0xE6) {
        flags |= NX_DEVICE_R_ALT;
    }
    if keys.contains(0xE3) {
        flags |= NX_DEVICE_L_CMD;
    }
    if keys.contains(0xE7) {
        flags |= NX_DEVICE_R_CMD;
    }
    flags
}

fn post_modifier_event(
    source: *mut c_void,
    hid: u8,
    keys: &KeyState,
    caps_lock: bool,
) {
    let Some(keycode) = hid_to_mac(hid) else {
        return;
    };
    unsafe {
        let event = CGEventCreateKeyboardEvent(source, keycode, 1);
        if event.is_null() {
            return;
        }
        CGEventSetType(event, CG_EVENT_FLAGS_CHANGED);
        CGEventSetFlags(
            event,
            mac_flags(keys, caps_lock) | mac_device_flags(keys) | CG_FLAG_NON_COALESCED,
        );
        CGEventPost(CG_TAP_LOCATION_HID, event);
        CFRelease(event as *const c_void);
    }
}

fn inject_caps_tap(source: *mut c_void, caps_after: bool, keys: &KeyState) {
    let Some(keycode) = hid_to_mac(CAPS_LOCK) else {
        return;
    };
    unsafe {
        for down in [1u8, 0u8] {
            let event = CGEventCreateKeyboardEvent(source, keycode, down);
            if !event.is_null() {
                CGEventSetFlags(event, mac_flags(keys, caps_after));
                CGEventPost(CG_TAP_LOCATION_HID, event);
                CFRelease(event as *const c_void);
            }
        }
    }
}

fn release_keys(source: *mut c_void, keys: &mut KeyState, caps_lock: bool) {
    let mut hids: Vec<u8> = keys.iter().collect();
    hids.sort_unstable();
    for hid in hids.iter().copied().filter(|hid| !is_modifier(*hid)) {
        keys.remove(hid);
        post_key_event(source, hid, false, false, keys, caps_lock);
    }
    for hid in hids.iter().copied().filter(|hid| is_modifier(*hid)) {
        keys.remove(hid);
        post_modifier_event(source, hid, keys, caps_lock);
    }
}

fn point_from_normalized(x: u16, y: u16, bounds: CGRect) -> CGPoint {
    CGPoint {
        x: bounds.min_x() + x as f64 / 65535.0 * (bounds.size.width - 1.0).max(1.0),
        y: bounds.min_y() + y as f64 / 65535.0 * (bounds.size.height - 1.0).max(1.0),
    }
}

fn applied_point_or_origin(position: Option<(u16, u16)>, bounds: CGRect) -> CGPoint {
    position
        .map(|(x, y)| point_from_normalized(x, y, bounds))
        .unwrap_or(CGPoint {
            x: bounds.origin.x,
            y: bounds.origin.y,
        })
}

fn post_mouse_event(event_type: u32, point: CGPoint, button: u32) {
    unsafe {
        let event = CGEventCreateMouseEvent(null_mut(), event_type, point, button);
        if !event.is_null() {
            CGEventPost(CG_TAP_LOCATION_HID, event);
            CFRelease(event as *const c_void);
        }
    }
}

fn post_mouse_event_with_click(event_type: u32, point: CGPoint, button: u32, click_state: i64) {
    unsafe {
        let event = CGEventCreateMouseEvent(null_mut(), event_type, point, button);
        if !event.is_null() {
            CGEventSetIntegerValueField(event, CG_MOUSE_EVENT_CLICK_STATE, click_state);
            CGEventPost(CG_TAP_LOCATION_HID, event);
            CFRelease(event as *const c_void);
        }
    }
}

/// Map a CoreGraphics "other mouse button" number to the button bit used by
/// the wire protocol. macOS reports the side buttons as 3 and 4.
fn other_button_bit(number: i64) -> u8 {
    match number as u32 {
        CG_MOUSE_BUTTON_CENTER => BUTTON_MIDDLE,
        CG_MOUSE_BUTTON_X1 => BUTTON_X1,
        CG_MOUSE_BUTTON_X2 => BUTTON_X2,
        _ => 0,
    }
}

fn drag_event(buttons: u8) -> Option<(u32, u32)> {
    if buttons & BUTTON_LEFT != 0 {
        Some((CG_EVENT_LEFT_MOUSE_DRAGGED, CG_MOUSE_BUTTON_LEFT))
    } else if buttons & BUTTON_RIGHT != 0 {
        Some((CG_EVENT_RIGHT_MOUSE_DRAGGED, CG_MOUSE_BUTTON_RIGHT))
    } else if buttons & BUTTON_MIDDLE != 0 {
        Some((CG_EVENT_OTHER_MOUSE_DRAGGED, CG_MOUSE_BUTTON_CENTER))
    } else if buttons & BUTTON_X1 != 0 {
        Some((CG_EVENT_OTHER_MOUSE_DRAGGED, CG_MOUSE_BUTTON_X1))
    } else if buttons & BUTTON_X2 != 0 {
        Some((CG_EVENT_OTHER_MOUSE_DRAGGED, CG_MOUSE_BUTTON_X2))
    } else {
        None
    }
}

fn apply_button_transitions(previous: u8, current: u8, point: CGPoint, clicks: &mut ClickTracker) {
    let changed = previous ^ current;
    post_transition(
        changed,
        current,
        BUTTON_LEFT,
        CG_EVENT_LEFT_MOUSE_DOWN,
        CG_EVENT_LEFT_MOUSE_UP,
        CG_MOUSE_BUTTON_LEFT,
        point,
        clicks,
    );
    post_transition(
        changed,
        current,
        BUTTON_RIGHT,
        CG_EVENT_RIGHT_MOUSE_DOWN,
        CG_EVENT_RIGHT_MOUSE_UP,
        CG_MOUSE_BUTTON_RIGHT,
        point,
        clicks,
    );
    post_transition(
        changed,
        current,
        BUTTON_MIDDLE,
        CG_EVENT_OTHER_MOUSE_DOWN,
        CG_EVENT_OTHER_MOUSE_UP,
        CG_MOUSE_BUTTON_CENTER,
        point,
        clicks,
    );
    post_transition(
        changed,
        current,
        BUTTON_X1,
        CG_EVENT_OTHER_MOUSE_DOWN,
        CG_EVENT_OTHER_MOUSE_UP,
        CG_MOUSE_BUTTON_X1,
        point,
        clicks,
    );
    post_transition(
        changed,
        current,
        BUTTON_X2,
        CG_EVENT_OTHER_MOUSE_DOWN,
        CG_EVENT_OTHER_MOUSE_UP,
        CG_MOUSE_BUTTON_X2,
        point,
        clicks,
    );
}

#[allow(clippy::too_many_arguments)]
fn post_transition(
    changed: u8,
    current: u8,
    bit: u8,
    down: u32,
    up: u32,
    button: u32,
    point: CGPoint,
    clicks: &mut ClickTracker,
) {
    if changed & bit == 0 {
        return;
    }
    let is_down = current & bit != 0;
    let click_state = if is_down {
        clicks.down(bit, point)
    } else {
        clicks.up(bit)
    };
    let event_type = if is_down { down } else { up };
    post_mouse_event_with_click(event_type, point, button, click_state);
}

fn yes_no(value: bool) -> &'static str {
    if value {
        "granted"
    } else {
        "missing"
    }
}

/// Local smoke test for the macOS input backend.
///
/// Capture is exercised with a synthetic edge crossing; injection is checked by
/// moving the cursor to a known point and reading its position back. Both tests
/// require the calling process to hold the relevant TCC permissions, so run this
/// from the terminal/app that will actually be granted access. The user's cursor
/// is restored afterwards.
pub fn self_test() -> bool {
    println!("GlideLink macOS input self-test");
    println!("Permissions are granted per executable. Grant them to the");
    println!("application you run this from, then rerun with --self-test.");
    println!();

    let ax = accessibility_trusted();
    let listen = listen_event_access();
    let post = post_event_access();
    println!("accessibility:          {}", yes_no(ax));
    println!("input monitoring:       {}", yes_no(listen));
    println!("event posting:          {}", yes_no(post));

    let Some(bounds) = virtual_bounds() else {
        println!("displays:               FAILED to enumerate");
        println!();
        println!("result: FAIL");
        return false;
    };
    println!(
        "virtual desktop:        {:.0}x{:.0} at ({:.0}, {:.0})",
        bounds.size.width, bounds.size.height, bounds.origin.x, bounds.origin.y
    );
    println!();

    let mut ok = true;
    if ax && listen {
        match test_capture(bounds) {
            Ok(()) => println!("capture:                PASS"),
            Err(e) => {
                println!("capture:                FAIL ({e})");
                ok = false;
            }
        }
    } else {
        println!("capture:                SKIP (needs Accessibility + Input Monitoring)");
        ok = false;
    }

    if ax && post {
        match test_injection(bounds) {
            Ok(()) => println!("injection:              PASS"),
            Err(e) => {
                println!("injection:              FAIL ({e})");
                ok = false;
            }
        }
        match test_caps_injection() {
            Ok(()) => println!("caps lock:              PASS"),
            Err(e) => {
                println!("caps lock:              FAIL ({e})");
                ok = false;
            }
        }
        match test_modifier_injection() {
            Ok(()) => println!("modifiers:              PASS"),
            Err(e) => {
                println!("modifiers:              FAIL ({e})");
                ok = false;
            }
        }
    } else {
        println!("injection:              SKIP (needs Accessibility + event posting)");
        println!("caps lock:              SKIP (needs Accessibility + event posting)");
        println!("modifiers:              SKIP (needs Accessibility + event posting)");
        ok = false;
    }

    println!();
    if ok {
        println!("result: PASS");
    } else {
        println!("result: FAIL");
        println!();
        println!("Open System Settings -> Privacy & Security and grant GlideLink the");
        println!("permissions reported above, then relaunch and rerun this test.");
    }
    ok
}

fn test_capture(bounds: CGRect) -> Result<(), String> {
    let status = Status::new(screen_info());
    let shared = CaptureShared::new(status);
    // Pretend a peer is connected so edge crossing activates immediately.
    shared.set_peer(1920, 1080);
    let (tx, rx) = crossbeam_channel::unbounded::<CaptureMsg>();
    let handle = start_capture(Arc::clone(&shared), tx, Edge::Right, 0.0)?;
    let saved = cursor_location();

    let point = CGPoint {
        x: bounds.max_x() - 1.0,
        y: bounds.min_y() + bounds.size.height / 2.0,
    };
    warp_cursor(point);
    unsafe {
        let event = CGEventCreateMouseEvent(
            null_mut(),
            CG_EVENT_MOUSE_MOVED,
            point,
            CG_MOUSE_BUTTON_LEFT,
        );
        if event.is_null() {
            handle.stop_and_join();
            warp_cursor(saved);
            return Err("could not create a synthetic mouse event".into());
        }
        CGEventSetIntegerValueField(event, CG_MOUSE_EVENT_DELTA_X, 8);
        CGEventSetIntegerValueField(event, CG_MOUSE_EVENT_DELTA_Y, 0);
        CGEventPost(CG_TAP_LOCATION_HID, event);
        CFRelease(event as *const c_void);
    }

    let result = (|| {
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(CaptureMsg::Mouse(_)) => return Ok(()),
                Ok(CaptureMsg::Keys(_)) => continue,
                Err(_) => {
                    return Err("no input was captured after a synthetic edge crossing".to_string())
                }
            }
        }
    })();

    handle.stop_and_join();
    unsafe {
        CGAssociateMouseAndMouseCursorPosition(1);
    }
    warp_cursor(saved);
    result
}

fn test_caps_injection() -> Result<(), String> {
    let status = Status::new(screen_info());
    let stop = Arc::new(AtomicBool::new(false));
    let (tx, rx) = crossbeam_channel::unbounded::<InjectMsg>();
    let handle = start_injector(rx, Arc::clone(&stop), status)?;
    let before = current_caps_lock();

    let send_caps = |desired: bool| {
        tx.send(InjectMsg::Keys(KeyPacket {
            active: true,
            caps_lock: desired,
            state: KeyState::default(),
            event: KeyEvent::None,
        }))
        .map_err(|_| "injector thread stopped".to_string())
    };
    let wait_caps = |desired: bool| {
        for _ in 0..50 {
            if current_caps_lock() == desired {
                return true;
            }
            thread::sleep(Duration::from_millis(10));
        }
        false
    };

    let flipped = send_caps(!before).is_ok() && wait_caps(!before);
    let restored = send_caps(before).is_ok() && wait_caps(before);
    let _ = tx.send(InjectMsg::Reset);
    stop.store(true, Ordering::Release);
    let _ = handle.join();

    let actual = current_caps_lock();
    if actual != before {
        let source = unsafe { CGEventSourceCreate(CG_EVENT_SOURCE_STATE_HID_SYSTEM) };
        inject_caps_tap(source, before, &KeyState::default());
        if !source.is_null() {
            unsafe {
                CFRelease(source);
            }
        }
        thread::sleep(Duration::from_millis(50));
    }

    if flipped && restored && current_caps_lock() == before {
        Ok(())
    } else {
        Err(format!(
            "caps did not toggle/restore (before={before}, flipped={flipped}, restored={restored})"
        ))
    }
}

fn test_modifier_injection() -> Result<(), String> {
    let status = Status::new(screen_info());
    let stop = Arc::new(AtomicBool::new(false));
    let (tx, rx) = crossbeam_channel::unbounded::<InjectMsg>();
    let handle = start_injector(rx, Arc::clone(&stop), status)?;
    let caps = current_caps_lock();

    let send_state = |state: KeyState| {
        tx.send(InjectMsg::Keys(KeyPacket {
            active: true,
            caps_lock: caps,
            state,
            event: KeyEvent::None,
        }))
        .map_err(|_| "injector thread stopped".to_string())
    };
    let wait_shift = |desired: bool| {
        for _ in 0..50 {
            let shift = unsafe {
                CGEventSourceFlagsState(CG_EVENT_SOURCE_STATE_HID_SYSTEM) & CG_FLAG_SHIFT != 0
            };
            if shift == desired {
                return true;
            }
            thread::sleep(Duration::from_millis(10));
        }
        false
    };

    let mut shift = KeyState::default();
    shift.insert(0xE1); // left Shift
    let pressed = send_state(shift).is_ok() && wait_shift(true);
    let released = send_state(KeyState::default()).is_ok() && wait_shift(false);

    let _ = tx.send(InjectMsg::Reset);
    stop.store(true, Ordering::Release);
    let _ = handle.join();

    if pressed && released {
        Ok(())
    } else {
        Err(format!(
            "modifier did not apply/release (pressed={pressed}, released={released})"
        ))
    }
}

fn test_injection(bounds: CGRect) -> Result<(), String> {
    let status = Status::new(screen_info());
    let stop = Arc::new(AtomicBool::new(false));
    let (tx, rx) = crossbeam_channel::unbounded::<InjectMsg>();
    let handle = start_injector(rx, Arc::clone(&stop), status)?;
    let saved = cursor_location();

    // Target the center of the main display rather than the center of the
    // virtual desktop, which can fall in a gap between displays.
    let main = unsafe { CGDisplayBounds(CGMainDisplayID()) };
    let target = CGPoint {
        x: main.min_x() + main.size.width * 0.5,
        y: main.min_y() + main.size.height * 0.5,
    };
    let norm_x = (target.x - bounds.min_x()) / (bounds.size.width - 1.0).max(1.0);
    let norm_y = (target.y - bounds.min_y()) / (bounds.size.height - 1.0).max(1.0);
    tx.send(InjectMsg::State(RemoteInput {
        x: norm_x as f32,
        y: norm_y as f32,
        buttons: 0,
        wheel: 0,
    }))
    .map_err(|_| "injector thread stopped".to_string())?;

    let mut actual = cursor_location();
    for _ in 0..40 {
        if (actual.x - target.x).abs() < 8.0 && (actual.y - target.y).abs() < 8.0 {
            break;
        }
        thread::sleep(Duration::from_millis(25));
        actual = cursor_location();
    }

    let _ = tx.send(InjectMsg::Reset);
    stop.store(true, Ordering::Release);
    let _ = handle.join();
    warp_cursor(saved);

    if (actual.x - target.x).abs() < 8.0 && (actual.y - target.y).abs() < 8.0 {
        Ok(())
    } else {
        Err(format!(
            "cursor is at ({:.0}, {:.0}), expected ({:.0}, {:.0})",
            actual.x, actual.y, target.x, target.y
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_other_mouse_buttons() {
        assert_eq!(
            other_button_bit(CG_MOUSE_BUTTON_CENTER as i64),
            BUTTON_MIDDLE
        );
        assert_eq!(other_button_bit(CG_MOUSE_BUTTON_X1 as i64), BUTTON_X1);
        assert_eq!(other_button_bit(CG_MOUSE_BUTTON_X2 as i64), BUTTON_X2);
        assert_eq!(other_button_bit(9), 0);
    }

    #[test]
    fn drag_event_prefers_primary_buttons() {
        assert_eq!(drag_event(0), None);
        assert_eq!(
            drag_event(BUTTON_X1).map(|(_, button)| button),
            Some(CG_MOUSE_BUTTON_X1)
        );
        assert_eq!(
            drag_event(BUTTON_LEFT | BUTTON_X1).map(|(_, button)| button),
            Some(CG_MOUSE_BUTTON_LEFT)
        );
    }
}
