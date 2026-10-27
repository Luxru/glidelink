use super::{
    CaptureHandle, CaptureShared, InjectMsg, RemoteInput, BUTTON_LEFT, BUTTON_MIDDLE, BUTTON_RIGHT,
};
use crate::error;
use crate::layout::{
    advance, entry_point, has_returned, host_point_to_norm, host_return_point, is_outward, Edge,
    NormPoint, REENTRY_COOLDOWN_MS,
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
const CG_EVENT_SCROLL_WHEEL: u32 = 22;
const CG_EVENT_OTHER_MOUSE_DOWN: u32 = 25;
const CG_EVENT_OTHER_MOUSE_UP: u32 = 26;
const CG_EVENT_OTHER_MOUSE_DRAGGED: u32 = 27;
const CG_EVENT_TAP_DISABLED_BY_TIMEOUT: u32 = 0xFFFF_FFFE;
const CG_EVENT_TAP_DISABLED_BY_USER_INPUT: u32 = 0xFFFF_FFFF;

const CG_MOUSE_EVENT_DELTA_X: u32 = 4;
const CG_MOUSE_EVENT_DELTA_Y: u32 = 5;
const CG_MOUSE_EVENT_BUTTON_NUMBER: u32 = 3;
const CG_SCROLL_WHEEL_EVENT_DELTA_AXIS_1: u32 = 11;

const CG_EVENT_SOURCE_STATE_HID_SYSTEM: i32 = 1;
const CG_SCROLL_EVENT_UNIT_LINE: u32 = 1;

const CG_MOUSE_BUTTON_LEFT: u32 = 0;
const CG_MOUSE_BUTTON_RIGHT: u32 = 1;
const CG_MOUSE_BUTTON_CENTER: u32 = 2;

const CG_EVENT_MASK: u64 = (1 << CG_EVENT_LEFT_MOUSE_DOWN)
    | (1 << CG_EVENT_LEFT_MOUSE_UP)
    | (1 << CG_EVENT_RIGHT_MOUSE_DOWN)
    | (1 << CG_EVENT_RIGHT_MOUSE_UP)
    | (1 << CG_EVENT_MOUSE_MOVED)
    | (1 << CG_EVENT_LEFT_MOUSE_DRAGGED)
    | (1 << CG_EVENT_RIGHT_MOUSE_DRAGGED)
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
    fn CGEventGetLocation(event: CGEventRef) -> CGPoint;
    fn CGEventGetIntegerValueField(event: CGEventRef, field: u32) -> i64;
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
    fn CGEventSourceButtonState(state_id: i32, button: u32) -> u8;
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
    static kCFRunLoopDefaultMode: CFStringRef;
}

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> u8;
    fn CGPreflightListenEventAccess() -> u8;
    fn CGPreflightPostEventAccess() -> u8;
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

pub fn permissions_ok() -> bool {
    unsafe {
        AXIsProcessTrusted() != 0
            && CGPreflightListenEventAccess() != 0
            && CGPreflightPostEventAccess() != 0
    }
}

pub fn open_permissions() -> Result<(), String> {
    std::process::Command::new("open")
        .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")
        .spawn()
        .map(|_| ())
        .map_err(|e| error::detail("E_OPEN_SETTINGS", e))
}

struct CaptureState {
    shared: Arc<CaptureShared>,
    tx: Sender<RemoteInput>,
    edge: Edge,
    bounds: CGRect,
    tap: CFMachPortRef,
    active: bool,
    hidden: bool,
    buttons: u8,
    point: NormPoint,
    cooldown_until: Instant,
}

impl CaptureState {
    fn send(&self, wheel: i16) {
        let input = RemoteInput {
            x: self.point.x,
            y: self.point.y,
            buttons: self.buttons,
            wheel,
        };
        let _ = self.tx.try_send(input);
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
                if number == CG_MOUSE_BUTTON_CENTER as i64 {
                    self.buttons |= BUTTON_MIDDLE;
                }
            }
            CG_EVENT_OTHER_MOUSE_UP => {
                let number =
                    unsafe { CGEventGetIntegerValueField(event, CG_MOUSE_EVENT_BUTTON_NUMBER) };
                if number == CG_MOUSE_BUTTON_CENTER as i64 {
                    self.buttons &= !BUTTON_MIDDLE;
                }
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
            return event;
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
        self.point = entry_point(self.edge, host);
        self.buttons = unsafe {
            let mut buttons = 0u8;
            if CGEventSourceButtonState(CG_EVENT_SOURCE_STATE_HID_SYSTEM, CG_MOUSE_BUTTON_LEFT) != 0
            {
                buttons |= BUTTON_LEFT;
            }
            if CGEventSourceButtonState(CG_EVENT_SOURCE_STATE_HID_SYSTEM, CG_MOUSE_BUTTON_RIGHT)
                != 0
            {
                buttons |= BUTTON_RIGHT;
            }
            if CGEventSourceButtonState(CG_EVENT_SOURCE_STATE_HID_SYSTEM, CG_MOUSE_BUTTON_CENTER)
                != 0
            {
                buttons |= BUTTON_MIDDLE;
            }
            buttons
        };
        self.active = true;
        self.shared.set_active(true);
        if !self.hidden {
            unsafe {
                CGDisplayHideCursor(CGMainDisplayID());
                CGAssociateMouseAndMouseCursorPosition(0);
            }
            self.hidden = true;
        }
        self.send(0);
    }

    fn deactivate(&mut self, warp_cursor: bool) {
        if !self.active {
            if self.hidden {
                unsafe {
                    CGAssociateMouseAndMouseCursorPosition(1);
                    CGDisplayShowCursor(CGMainDisplayID());
                }
                self.hidden = false;
            }
            return;
        }
        self.active = false;
        self.buttons = 0;
        self.point = self.point.clamped();
        self.send(0);
        self.shared.set_active(false);

        if self.hidden {
            unsafe {
                CGAssociateMouseAndMouseCursorPosition(1);
            }
            self.hidden = false;
        }

        if warp_cursor {
            let host = host_return_point(self.edge, self.point);
            let point = CGPoint {
                x: self.bounds.min_x() + host.x as f64 * (self.bounds.size.width - 1.0).max(1.0),
                y: self.bounds.min_y() + host.y as f64 * (self.bounds.size.height - 1.0).max(1.0),
            };
            unsafe {
                CGWarpMouseCursorPosition(point);
            }
        }
        unsafe {
            CGDisplayShowCursor(CGMainDisplayID());
        }
        self.cooldown_until = Instant::now() + Duration::from_millis(REENTRY_COOLDOWN_MS);
    }
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
    tx: Sender<RemoteInput>,
    edge: Edge,
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
        .spawn(move || run_capture_thread(thread_shared, tx, edge, bounds, ready_tx))
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
            Err(error::code("E_CAPTURE_TIMEOUT"))
        }
    }
}

fn run_capture_thread(
    shared: Arc<CaptureShared>,
    tx: Sender<RemoteInput>,
    edge: Edge,
    bounds: CGRect,
    ready_tx: mpsc::Sender<Result<(), String>>,
) {
    let mut state = Box::new(CaptureState {
        shared,
        tx,
        edge,
        bounds,
        tap: null_mut(),
        active: false,
        hidden: false,
        buttons: 0,
        point: NormPoint::default(),
        cooldown_until: Instant::now(),
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
            CGDisplayShowCursor(CGMainDisplayID());
        }
        state.hidden = false;
    }

    unsafe {
        CGEventTapEnable(tap, 0);
        CFRunLoopRemoveSource(run_loop, source, kCFRunLoopDefaultMode);
        CFRelease(source as *const c_void);
        CFRelease(tap as *const c_void);
    }
    drop(unsafe { Box::from_raw(state_ptr) });
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
    let join = thread::Builder::new()
        .name("glidelink-input-injector".into())
        .spawn(move || injector_loop(rx, stop, bounds, ready_tx))
        .map_err(|e| error::detail("E_INJECTOR_THREAD", e))?;

    match ready_rx.recv_timeout(Duration::from_secs(2)) {
        Ok(Ok(())) => Ok(join),
        Ok(Err(error)) => Err(error),
        Err(_) => Err(error::code("E_INJECTOR_TIMEOUT")),
    }
}

fn injector_loop(
    rx: Receiver<InjectMsg>,
    stop: Arc<AtomicBool>,
    bounds: CGRect,
    ready_tx: mpsc::Sender<Result<(), String>>,
) {
    let _ = ready_tx.send(Ok(()));
    let mut previous_buttons = 0u8;
    let mut previous_position: Option<(u16, u16)> = None;

    while !stop.load(Ordering::Acquire) {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(InjectMsg::State(input)) => {
                let x = (input.x.clamp(0.0, 1.0) * 65535.0).round() as u16;
                let y = (input.y.clamp(0.0, 1.0) * 65535.0).round() as u16;
                let point = CGPoint {
                    x: bounds.min_x() + x as f64 / 65535.0 * (bounds.size.width - 1.0).max(1.0),
                    y: bounds.min_y() + y as f64 / 65535.0 * (bounds.size.height - 1.0).max(1.0),
                };

                if previous_position != Some((x, y)) {
                    let (event_type, button) = if previous_buttons & BUTTON_LEFT != 0 {
                        (CG_EVENT_LEFT_MOUSE_DRAGGED, CG_MOUSE_BUTTON_LEFT)
                    } else if previous_buttons & BUTTON_RIGHT != 0 {
                        (CG_EVENT_RIGHT_MOUSE_DRAGGED, CG_MOUSE_BUTTON_RIGHT)
                    } else if previous_buttons & BUTTON_MIDDLE != 0 {
                        (CG_EVENT_OTHER_MOUSE_DRAGGED, CG_MOUSE_BUTTON_CENTER)
                    } else {
                        (CG_EVENT_MOUSE_MOVED, CG_MOUSE_BUTTON_LEFT)
                    };
                    post_mouse_event(event_type, point, button);
                    previous_position = Some((x, y));
                }

                apply_button_transitions(previous_buttons, input.buttons, point);
                previous_buttons = input.buttons;

                if input.wheel != 0 {
                    unsafe {
                        let event = CGEventCreateScrollWheelEvent(
                            null_mut(),
                            CG_SCROLL_EVENT_UNIT_LINE,
                            1,
                            input.wheel as i32,
                        );
                        if !event.is_null() {
                            CGEventPost(CG_TAP_LOCATION_HID, event);
                            CFRelease(event as *const c_void);
                        }
                    }
                }
            }
            Ok(InjectMsg::Reset) => {
                let point = previous_position
                    .map(|(x, y)| CGPoint {
                        x: bounds.min_x() + x as f64 / 65535.0 * (bounds.size.width - 1.0).max(1.0),
                        y: bounds.min_y()
                            + y as f64 / 65535.0 * (bounds.size.height - 1.0).max(1.0),
                    })
                    .unwrap_or(CGPoint {
                        x: bounds.origin.x,
                        y: bounds.origin.y,
                    });
                apply_button_transitions(previous_buttons, 0, point);
                previous_buttons = 0;
                previous_position = None;
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        }
    }

    if previous_buttons != 0 {
        let point = previous_position
            .map(|(x, y)| CGPoint {
                x: bounds.min_x() + x as f64 / 65535.0 * (bounds.size.width - 1.0).max(1.0),
                y: bounds.min_y() + y as f64 / 65535.0 * (bounds.size.height - 1.0).max(1.0),
            })
            .unwrap_or(CGPoint {
                x: bounds.origin.x,
                y: bounds.origin.y,
            });
        apply_button_transitions(previous_buttons, 0, point);
    }
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

fn apply_button_transitions(previous: u8, current: u8, point: CGPoint) {
    let changed = previous ^ current;
    if changed & BUTTON_LEFT != 0 {
        let event_type = if current & BUTTON_LEFT != 0 {
            CG_EVENT_LEFT_MOUSE_DOWN
        } else {
            CG_EVENT_LEFT_MOUSE_UP
        };
        post_mouse_event(event_type, point, CG_MOUSE_BUTTON_LEFT);
    }
    if changed & BUTTON_RIGHT != 0 {
        let event_type = if current & BUTTON_RIGHT != 0 {
            CG_EVENT_RIGHT_MOUSE_DOWN
        } else {
            CG_EVENT_RIGHT_MOUSE_UP
        };
        post_mouse_event(event_type, point, CG_MOUSE_BUTTON_RIGHT);
    }
    if changed & BUTTON_MIDDLE != 0 {
        let event_type = if current & BUTTON_MIDDLE != 0 {
            CG_EVENT_OTHER_MOUSE_DOWN
        } else {
            CG_EVENT_OTHER_MOUSE_UP
        };
        post_mouse_event(event_type, point, CG_MOUSE_BUTTON_CENTER);
    }
}
