use super::{
    keymap::{hid_to_scan, is_caps, is_modifier, scan_to_hid, KeyState, CAPS_LOCK, KEY_DEFS},
    plan_injection_batch, CaptureHandle, CaptureMsg, CaptureShared, InjectCursor, InjectMsg,
    InjectStep, InputEvent, KeyAction, KeyOwnership, KeyRepeater, ReconcileState, RemoteInput,
    WarpInput, BUTTON_LEFT, BUTTON_MIDDLE, BUTTON_RIGHT, BUTTON_X1, BUTTON_X2,
};
use crate::error;
use crate::layout::{
    enter_from_host, host_px_to_world, step_active, world_to_host_px, DeviceId, LayoutStore,
    NormPoint, StepResult, WorldLayout, WorldPoint, REENTRY_COOLDOWN_MS,
};
use crate::status::{ScreenInfo, Status};
use crossbeam_channel::{Receiver, Sender};
use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::mem::{size_of, zeroed, MaybeUninit};
use std::ptr::{null, null_mut};
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    mouse_event, GetAsyncKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT,
    KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE, VK_CAPITAL,
};
use windows_sys::Win32::UI::Input::{
    GetRawInputData, RegisterRawInputDevices, RAWINPUT, RAWINPUTDEVICE, RAWINPUTHEADER,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetCursorPos,
    GetMessageW, GetSystemMetrics, KillTimer, PostQuitMessage, RegisterClassW, SetCursorPos,
    SetTimer, SetWindowsHookExW, ShowCursor, TranslateMessage, UnhookWindowsHookEx,
    KBDLLHOOKSTRUCT, LLKHF_EXTENDED, LLKHF_INJECTED, MSG, MSLLHOOKSTRUCT, WH_KEYBOARD_LL,
    WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP, WNDCLASSW,
};

const WH_MOUSE_LL: i32 = 14;
const WM_INPUT: u32 = 0x00FF;
const WM_TIMER: u32 = 0x0113;
const WM_MOUSEMOVE: u32 = 0x0200;
const WM_LBUTTONDOWN: u32 = 0x0201;
const WM_LBUTTONUP: u32 = 0x0202;
const WM_RBUTTONDOWN: u32 = 0x0204;
const WM_RBUTTONUP: u32 = 0x0205;
const WM_MBUTTONDOWN: u32 = 0x0207;
const WM_MBUTTONUP: u32 = 0x0208;
const WM_MOUSEWHEEL: u32 = 0x020A;
const WM_XBUTTONDOWN: u32 = 0x020B;
const WM_XBUTTONUP: u32 = 0x020C;
const WM_MOUSEHWHEEL: u32 = 0x020E;

const TIMER_ID: usize = 1;
const TIMER_INTERVAL_MS: u32 = 50;
const WHEEL_DELTA: i32 = 120;
const RIDEV_INPUTSINK: u32 = 0x0000_0100;
const RID_INPUT: u32 = 0x1000_0003;
const RIM_TYPEMOUSE: u32 = 0;
const HID_USAGE_PAGE_GENERIC: u16 = 0x01;
const HID_USAGE_GENERIC_MOUSE: u16 = 0x02;
const MOUSE_MOVE_ABSOLUTE: u16 = 0x0001;

const SM_XVIRTUALSCREEN: i32 = 76;
const SM_YVIRTUALSCREEN: i32 = 77;
const SM_CXVIRTUALSCREEN: i32 = 78;
const SM_CYVIRTUALSCREEN: i32 = 79;

const VK_CONTROL: i32 = 0x11;
const VK_MENU: i32 = 0x12;
const VK_LBUTTON: i32 = 0x01;
const VK_RBUTTON: i32 = 0x02;
const VK_MBUTTON: i32 = 0x04;
const VK_XBUTTON1: i32 = 0x05;
const VK_XBUTTON2: i32 = 0x06;

const MOUSEEVENTF_MOVE: u32 = 0x0001;
const MOUSEEVENTF_LEFTUP: u32 = 0x0002;
const MOUSEEVENTF_LEFTDOWN: u32 = 0x0004;
const MOUSEEVENTF_RIGHTUP: u32 = 0x0008;
const MOUSEEVENTF_RIGHTDOWN: u32 = 0x0010;
const MOUSEEVENTF_MIDDLEUP: u32 = 0x0020;
const MOUSEEVENTF_MIDDLEDOWN: u32 = 0x0040;
const MOUSEEVENTF_XDOWN: u32 = 0x0080;
const MOUSEEVENTF_XUP: u32 = 0x0100;
const MOUSEEVENTF_WHEEL: u32 = 0x0800;
const MOUSEEVENTF_ABSOLUTE: u32 = 0x8000;
const MOUSEEVENTF_VIRTUALDESK: u32 = 0x4000;
const XBUTTON1: u32 = 0x0001;
const XBUTTON2: u32 = 0x0002;
const WS_POPUP: u32 = 0x8000_0000;
const WS_EX_TOOLWINDOW: u32 = 0x0000_0080;

#[derive(Clone, Copy)]
enum HookAction {
    Pass,
    Swallow,
}

struct CaptureState {
    shared: Arc<CaptureShared>,
    tx: Sender<CaptureMsg>,
    vx: i32,
    vy: i32,
    vw: i32,
    vh: i32,
    cursor: POINT,
    layout: LayoutStore,
    layout_generation: u64,
    world_layout: Arc<WorldLayout>,
    target: Option<DeviceId>,
    armed: bool,
    active: bool,
    hidden: bool,
    buttons: u8,
    point: WorldPoint,
    host_anchor: NormPoint,
    cooldown_until: Instant,
    keys: KeyOwnership,
    caps_lock: bool,
    caps_down: bool,
}

thread_local! {
    static CAPTURE: RefCell<Option<CaptureState>> = RefCell::new(None);
    static HOOK: Cell<*mut c_void> = Cell::new(null_mut());
    static HOOK_KEYBOARD: Cell<*mut c_void> = Cell::new(null_mut());
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

fn with_state<R>(f: impl FnOnce(&mut CaptureState) -> R) -> Option<R> {
    CAPTURE.with(|cell| match cell.try_borrow_mut() {
        Ok(mut slot) => slot.as_mut().map(f),
        Err(_) => None,
    })
}

impl CaptureState {
    fn refresh_layout(&mut self) -> bool {
        let (generation, layout) = self.layout.snapshot();
        if generation == self.layout_generation {
            return false;
        }
        self.layout_generation = generation;
        self.world_layout = layout;
        true
    }

    fn target_online(&self) -> bool {
        self.target
            .map(|device| {
                self.world_layout
                    .is_online(crate::layout::ScreenId::Device(device))
            })
            .unwrap_or(false)
    }

    fn local_point(&self) -> Option<NormPoint> {
        let device = self.target?;
        self.world_layout
            .world_to_local(crate::layout::ScreenId::Device(device), self.point)
    }

    fn send_relative(&self, dx: i32, dy: i32) {
        if dx == 0 && dy == 0 {
            return;
        }
        let Some(device) = self.target else {
            return;
        };
        let _ = self.tx.try_send(CaptureMsg::Move {
            device,
            input: RemoteInput {
                dx,
                dy,
                buttons: self.buttons,
            },
        });
    }

    fn send_warp(&self) {
        let Some(device) = self.target else {
            return;
        };
        let Some(local) = self.local_point() else {
            return;
        };
        let _ = self.tx.try_send(CaptureMsg::Warp {
            device,
            input: WarpInput {
                x: local.x,
                y: local.y,
            },
        });
    }

    fn send_event(&self, event: InputEvent) {
        let Some(device) = self.target else {
            return;
        };
        let _ = self.tx.try_send(CaptureMsg::Event { device, event });
    }

    fn send_reconcile(&self, active: bool) {
        let Some(device) = self.target else {
            return;
        };
        let state = ReconcileState {
            active,
            caps_lock: self.caps_lock,
            buttons: if active { self.buttons } else { 0 },
            keys: if active {
                self.keys.remote
            } else {
                KeyState::EMPTY
            },
        };
        let _ = self.tx.try_send(CaptureMsg::Reconcile { device, state });
    }

    fn at_host_edge(&self) -> bool {
        self.cursor.x >= self.vx + self.vw - 1
            || self.cursor.x <= self.vx
            || self.cursor.y <= self.vy
            || self.cursor.y >= self.vy + self.vh - 1
    }

    fn handle_hook(&mut self, code: i32, wparam: WPARAM, lparam: LPARAM) -> HookAction {
        if code < 0 {
            return HookAction::Pass;
        }
        let message = wparam as u32;

        if self.active {
            match message {
                WM_LBUTTONDOWN => {
                    self.buttons |= BUTTON_LEFT;
                    self.send_event(InputEvent::Button {
                        button: BUTTON_LEFT,
                        down: true,
                    });
                }
                WM_LBUTTONUP => {
                    self.buttons &= !BUTTON_LEFT;
                    self.send_event(InputEvent::Button {
                        button: BUTTON_LEFT,
                        down: false,
                    });
                }
                WM_RBUTTONDOWN => {
                    self.buttons |= BUTTON_RIGHT;
                    self.send_event(InputEvent::Button {
                        button: BUTTON_RIGHT,
                        down: true,
                    });
                }
                WM_RBUTTONUP => {
                    self.buttons &= !BUTTON_RIGHT;
                    self.send_event(InputEvent::Button {
                        button: BUTTON_RIGHT,
                        down: false,
                    });
                }
                WM_MBUTTONDOWN => {
                    self.buttons |= BUTTON_MIDDLE;
                    self.send_event(InputEvent::Button {
                        button: BUTTON_MIDDLE,
                        down: true,
                    });
                }
                WM_MBUTTONUP => {
                    self.buttons &= !BUTTON_MIDDLE;
                    self.send_event(InputEvent::Button {
                        button: BUTTON_MIDDLE,
                        down: false,
                    });
                }
                WM_XBUTTONDOWN | WM_XBUTTONUP => {
                    let info = unsafe { &*(lparam as *const MSLLHOOKSTRUCT) };
                    let xbutton = ((info.mouseData >> 16) & 0xffff) as u16;
                    let bit = if xbutton == XBUTTON1 as u16 {
                        BUTTON_X1
                    } else {
                        BUTTON_X2
                    };
                    let down = message == WM_XBUTTONDOWN;
                    if down {
                        self.buttons |= bit;
                    } else {
                        self.buttons &= !bit;
                    }
                    self.send_event(InputEvent::Button { button: bit, down });
                }
                WM_MOUSEWHEEL => {
                    let info = unsafe { &*(lparam as *const MSLLHOOKSTRUCT) };
                    let raw_delta = ((info.mouseData >> 16) & 0xffff) as u16 as i16;
                    let notches = if raw_delta > 0 {
                        (raw_delta as i32 / WHEEL_DELTA).max(1) as i16
                    } else if raw_delta < 0 {
                        (raw_delta as i32 / WHEEL_DELTA).min(-1) as i16
                    } else {
                        0
                    };
                    if notches != 0 {
                        self.send_event(InputEvent::Wheel { delta: notches });
                    }
                }
                WM_MOUSEHWHEEL => {}
                WM_MOUSEMOVE => {}
                _ => {}
            }
            HookAction::Swallow
        } else {
            if message == WM_MOUSEMOVE {
                let info = unsafe { &*(lparam as *const MSLLHOOKSTRUCT) };
                self.cursor = info.pt;
                let connected = self.world_layout.screens.iter().any(|screen| screen.online);
                self.armed =
                    connected && self.at_host_edge() && Instant::now() >= self.cooldown_until;
            }
            HookAction::Pass
        }
    }

    fn handle_key_hook(&mut self, code: i32, wparam: WPARAM, lparam: LPARAM) -> HookAction {
        if code < 0 || !self.active {
            return HookAction::Pass;
        }
        let message = wparam as u32;
        let down = matches!(message, WM_KEYDOWN | WM_SYSKEYDOWN);
        let up = matches!(message, WM_KEYUP | WM_SYSKEYUP);
        if !down && !up {
            return HookAction::Pass;
        }

        let info = unsafe { &*(lparam as *const KBDLLHOOKSTRUCT) };
        if info.flags & LLKHF_INJECTED != 0 {
            return HookAction::Pass;
        }
        let extended = info.flags & LLKHF_EXTENDED != 0;
        let Some(hid) = scan_to_hid(info.scanCode as u16, extended) else {
            return if up {
                HookAction::Pass
            } else {
                HookAction::Swallow
            };
        };

        if down && hid == 0x2A && ctrl_alt_pressed() {
            self.deactivate_to_host(true, None);
            return HookAction::Swallow;
        }

        if is_caps(hid) {
            if down {
                if !self.caps_down {
                    self.caps_down = true;
                    self.caps_lock = !self.caps_lock;
                    self.send_event(InputEvent::CapsLock {
                        locked: self.caps_lock,
                    });
                }
            } else {
                self.caps_down = false;
            }
            return HookAction::Swallow;
        }

        if down {
            match self.keys.key_down(true, hid) {
                KeyAction::Down => {
                    self.send_event(InputEvent::Key { hid, down: true });
                    HookAction::Swallow
                }
                // The receiver owns repeat timing.
                KeyAction::Repeat => HookAction::Swallow,
                _ => HookAction::Swallow,
            }
        } else {
            match self.keys.key_up(true, hid) {
                KeyAction::Up => {
                    self.send_event(InputEvent::Key { hid, down: false });
                    HookAction::Swallow
                }
                KeyAction::Passthrough => HookAction::Pass,
                _ => HookAction::Swallow,
            }
        }
    }

    fn handle_raw(&mut self, dx: f64, dy: f64) {
        self.refresh_layout();
        if self.active && !self.target_online() {
            self.deactivate_to_host(true, None);
        }

        if self.active {
            let Some(device) = self.target else {
                return;
            };
            let old = self.point;
            let host_w = self.vw as f32;
            match step_active(&self.world_layout, device, self.point, dx, dy, self.vw) {
                StepResult::Stay { world, .. } | StepResult::Wall { world, .. } => {
                    self.point = world;
                    self.send_relative(
                        ((world.x - old.x) * host_w).round() as i32,
                        ((world.y - old.y) * host_w).round() as i32,
                    );
                }
                StepResult::ReturnHost {
                    world, consumed, ..
                } => {
                    self.send_relative(
                        ((consumed.x - old.x) * host_w).round() as i32,
                        ((consumed.y - old.y) * host_w).round() as i32,
                    );
                    self.point = world;
                    self.deactivate_to_host(true, Some(world));
                }
                StepResult::Switch {
                    target,
                    world,
                    consumed,
                    ..
                } => {
                    self.send_relative(
                        ((consumed.x - old.x) * host_w).round() as i32,
                        ((consumed.y - old.y) * host_w).round() as i32,
                    );
                    self.switch_target(target, world);
                }
            }
            return;
        }

        if !self.armed {
            let connected = self.world_layout.screens.iter().any(|screen| screen.online);
            if !connected || Instant::now() < self.cooldown_until || !self.at_host_edge() {
                return;
            }
            self.armed = true;
        }
        if Instant::now() < self.cooldown_until {
            self.armed = false;
            return;
        }
        let host_world = host_px_to_world(
            self.cursor.x as f64,
            self.cursor.y as f64,
            self.vx,
            self.vy,
            self.vw,
        );
        if let Some((device, _, world)) =
            enter_from_host(&self.world_layout, host_world, dx, dy, self.vw)
        {
            self.activate_target(
                device,
                world,
                self.world_layout.world_to_host_norm(host_world).inset(),
            );
        }
    }

    fn activate_target(&mut self, device: DeviceId, world: WorldPoint, host_anchor: NormPoint) {
        if self.active {
            return;
        }
        self.active = true;
        self.target = Some(device);
        self.point = world;
        self.host_anchor = host_anchor;
        log::info!(
            "input target activate {}",
            crate::layout::device_id_to_hex(&device)
        );
        self.buttons = unsafe { query_buttons() };
        self.keys.begin(unsafe { query_held_keys() });
        self.caps_lock = unsafe { query_caps_lock() };
        self.caps_down = false;
        self.armed = false;
        self.shared.set_active(true);
        if !self.hidden {
            unsafe {
                ShowCursor(0);
            }
            self.hidden = true;
        }
        self.send_warp();
        self.send_reconcile(true);
    }

    fn switch_target(&mut self, device: DeviceId, world: WorldPoint) {
        if self.target == Some(device) {
            return;
        }
        let old = self.target;
        if let Some(old) = old {
            let _ = self.tx.try_send(CaptureMsg::Event {
                device: old,
                event: InputEvent::ReleaseAll,
            });
            let _ = self.tx.try_send(CaptureMsg::Reconcile {
                device: old,
                state: ReconcileState {
                    active: false,
                    caps_lock: self.caps_lock,
                    buttons: 0,
                    keys: KeyState::EMPTY,
                },
            });
        }
        log::info!(
            "input target switch {} -> {}",
            old.map(|value| crate::layout::device_id_to_hex(&value))
                .unwrap_or_else(|| "host".to_string()),
            crate::layout::device_id_to_hex(&device)
        );
        self.target = Some(device);
        self.point = world;
        self.buttons = unsafe { query_buttons() };
        self.keys.deactivate();
        self.keys.begin(unsafe { query_held_keys() });
        self.caps_lock = unsafe { query_caps_lock() };
        self.caps_down = false;
        self.shared.set_active(true);
        self.send_warp();
        self.send_reconcile(true);
    }

    fn deactivate_to_host(&mut self, warp_cursor: bool, world: Option<WorldPoint>) {
        let old = self.target.take();
        if let Some(device) = old {
            let _ = self.tx.try_send(CaptureMsg::Event {
                device,
                event: InputEvent::ReleaseAll,
            });
            let _ = self.tx.try_send(CaptureMsg::Reconcile {
                device,
                state: ReconcileState {
                    active: false,
                    caps_lock: self.caps_lock,
                    buttons: 0,
                    keys: KeyState::EMPTY,
                },
            });
        }
        if let Some(device) = old {
            log::info!(
                "input target leave {}",
                crate::layout::device_id_to_hex(&device)
            );
        }
        self.active = false;
        self.buttons = 0;
        self.keys.deactivate();
        self.caps_down = false;
        self.shared.set_active(false);
        self.armed = false;

        if warp_cursor {
            let host_world = world.unwrap_or_else(|| {
                self.world_layout
                    .local_to_world(crate::layout::ScreenId::Host, self.host_anchor)
                    .unwrap_or(self.world_layout.host.center())
            });
            let (x, y) = world_to_host_px(host_world, self.vx, self.vy, self.vw, self.vh);
            unsafe {
                let _ = SetCursorPos(x.round() as i32, y.round() as i32);
            }
        }
        if self.hidden {
            unsafe {
                ShowCursor(1);
            }
            self.hidden = false;
        }
        self.cooldown_until = Instant::now() + Duration::from_millis(REENTRY_COOLDOWN_MS);
    }

    fn on_timer(&mut self) {
        if self.shared.stop.load(Ordering::Acquire) {
            if self.active {
                self.deactivate_to_host(true, None);
            }
            if self.hidden {
                unsafe {
                    ShowCursor(1);
                }
                self.hidden = false;
            }
            unsafe {
                PostQuitMessage(0);
            }
            return;
        }
        self.refresh_layout();
        if self.active && !self.target_online() {
            self.deactivate_to_host(true, None);
        }
    }
}

unsafe fn query_held_keys() -> KeyState {
    let mut held = KeyState::default();
    for def in KEY_DEFS {
        if is_caps(def.hid) {
            continue;
        }
        if GetAsyncKeyState(def.vk as i32) as u16 & 0x8000 != 0 {
            held.insert(def.hid);
        }
    }
    held
}

unsafe fn query_caps_lock() -> bool {
    GetAsyncKeyState(VK_CAPITAL as i32) as u16 & 1 != 0
}

fn ctrl_alt_pressed() -> bool {
    unsafe {
        (GetAsyncKeyState(VK_CONTROL) as u16 & 0x8000 != 0)
            && (GetAsyncKeyState(VK_MENU) as u16 & 0x8000 != 0)
    }
}

unsafe fn query_buttons() -> u8 {
    let mut buttons = 0u8;
    if GetAsyncKeyState(VK_LBUTTON) as u16 & 0x8000 != 0 {
        buttons |= BUTTON_LEFT;
    }
    if GetAsyncKeyState(VK_RBUTTON) as u16 & 0x8000 != 0 {
        buttons |= BUTTON_RIGHT;
    }
    if GetAsyncKeyState(VK_MBUTTON) as u16 & 0x8000 != 0 {
        buttons |= BUTTON_MIDDLE;
    }
    if GetAsyncKeyState(VK_XBUTTON1) as u16 & 0x8000 != 0 {
        buttons |= BUTTON_X1;
    }
    if GetAsyncKeyState(VK_XBUTTON2) as u16 & 0x8000 != 0 {
        buttons |= BUTTON_X2;
    }
    buttons
}

unsafe extern "system" fn low_level_mouse_proc(
    code: i32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let action =
        with_state(|state| state.handle_hook(code, wparam, lparam)).unwrap_or(HookAction::Pass);
    match action {
        HookAction::Swallow => 1,
        HookAction::Pass => CallNextHookEx(HOOK.with(|h| h.get()), code, wparam, lparam),
    }
}

unsafe extern "system" fn low_level_keyboard_proc(
    code: i32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let action =
        with_state(|state| state.handle_key_hook(code, wparam, lparam)).unwrap_or(HookAction::Pass);
    match action {
        HookAction::Swallow => 1,
        HookAction::Pass => CallNextHookEx(HOOK_KEYBOARD.with(|h| h.get()), code, wparam, lparam),
    }
}

unsafe extern "system" fn hidden_wnd_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_INPUT => {
            let mut raw = MaybeUninit::<RAWINPUT>::zeroed();
            let mut size = size_of::<RAWINPUT>() as u32;
            let got = GetRawInputData(
                lparam as *mut c_void,
                RID_INPUT,
                raw.as_mut_ptr() as *mut c_void,
                &mut size,
                size_of::<RAWINPUTHEADER>() as u32,
            );
            if got != u32::MAX && got != 0 {
                let raw = raw.assume_init();
                if raw.header.dwType == RIM_TYPEMOUSE {
                    let mouse = raw.data.mouse;
                    if mouse.usFlags & MOUSE_MOVE_ABSOLUTE == 0 {
                        let dx = mouse.lLastX as f64;
                        let dy = mouse.lLastY as f64;
                        if dx != 0.0 || dy != 0.0 {
                            with_state(|state| state.handle_raw(dx, dy));
                        }
                    }
                }
            }
            0
        }
        WM_TIMER => {
            with_state(|state| state.on_timer());
            0
        }
        _ => DefWindowProcW(hwnd, message, wparam, lparam),
    }
}

pub fn screen_info() -> Option<ScreenInfo> {
    let vx = unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) };
    let vy = unsafe { GetSystemMetrics(SM_YVIRTUALSCREEN) };
    let width = unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN) };
    let height = unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN) };
    let _ = (vx, vy);
    if width <= 0 || height <= 0 {
        None
    } else {
        Some(ScreenInfo::new(width, height))
    }
}

pub fn permissions_ok() -> bool {
    true
}

pub fn open_permissions() -> Result<(), String> {
    Ok(())
}

pub fn start_capture(
    shared: Arc<CaptureShared>,
    tx: Sender<CaptureMsg>,
) -> Result<CaptureHandle, String> {
    let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();
    let thread_shared = Arc::clone(&shared);
    let join = thread::Builder::new()
        .name("glidelink-input-capture".into())
        .spawn(move || run_capture_thread(thread_shared, tx, ready_tx))
        .map_err(|e| error::detail("E_CAPTURE_THREAD", e))?;

    match ready_rx.recv_timeout(Duration::from_secs(3)) {
        Ok(Ok(())) => Ok(CaptureHandle::new(shared, join)),
        Ok(Err(e)) => {
            shared.stop.store(true, Ordering::Release);
            let _ = join.join();
            Err(e)
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
    ready_tx: mpsc::Sender<Result<(), String>>,
) {
    crate::sched::boost_current_thread();
    let vx = unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) };
    let vy = unsafe { GetSystemMetrics(SM_YVIRTUALSCREEN) };
    let vw = unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN) };
    let vh = unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN) };
    if vw <= 0 || vh <= 0 {
        let _ = ready_tx.send(Err(error::code("E_VIRTUAL_SCREEN")));
        return;
    }

    let class_name = wide("GlideLinkHiddenInputWindow");
    let hinstance = unsafe { GetModuleHandleW(null()) };
    if hinstance.is_null() {
        let _ = ready_tx.send(Err(error::code("E_MODULE_HANDLE")));
        return;
    }

    let mut wc: WNDCLASSW = unsafe { zeroed() };
    wc.lpfnWndProc = Some(hidden_wnd_proc);
    wc.hInstance = hinstance;
    wc.lpszClassName = class_name.as_ptr();
    unsafe {
        RegisterClassW(&wc);
    }

    // A hidden top-level popup is used instead of HWND_MESSAGE. Raw Input
    // background delivery is best supported for desktop windows.
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_TOOLWINDOW,
            class_name.as_ptr(),
            class_name.as_ptr(),
            WS_POPUP,
            0,
            0,
            0,
            0,
            null_mut(),
            null_mut(),
            hinstance,
            null(),
        )
    };
    if hwnd.is_null() {
        let _ = ready_tx.send(Err(error::detail(
            "E_CAPTURE_WINDOW",
            std::io::Error::last_os_error(),
        )));
        return;
    }

    let rid = RAWINPUTDEVICE {
        usUsagePage: HID_USAGE_PAGE_GENERIC,
        usUsage: HID_USAGE_GENERIC_MOUSE,
        dwFlags: RIDEV_INPUTSINK,
        hwndTarget: hwnd,
    };
    if unsafe { RegisterRawInputDevices(&rid, 1, size_of::<RAWINPUTDEVICE>() as u32) } == 0 {
        unsafe {
            DestroyWindow(hwnd);
        }
        let _ = ready_tx.send(Err(error::detail(
            "E_RAW_INPUT",
            std::io::Error::last_os_error(),
        )));
        return;
    }

    let hook = unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(low_level_mouse_proc), hinstance, 0) };
    if hook.is_null() {
        unsafe {
            DestroyWindow(hwnd);
        }
        let _ = ready_tx.send(Err(error::detail(
            "E_MOUSE_HOOK",
            std::io::Error::last_os_error(),
        )));
        return;
    }
    HOOK.with(|h| h.set(hook));

    let keyboard_hook =
        unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(low_level_keyboard_proc), hinstance, 0) };
    if keyboard_hook.is_null() {
        HOOK.with(|h| h.set(null_mut()));
        unsafe {
            let _ = UnhookWindowsHookEx(hook);
            DestroyWindow(hwnd);
        }
        let _ = ready_tx.send(Err(error::detail(
            "E_KEYBOARD_HOOK",
            std::io::Error::last_os_error(),
        )));
        return;
    }
    HOOK_KEYBOARD.with(|h| h.set(keyboard_hook));

    let timer = unsafe { SetTimer(hwnd, TIMER_ID, TIMER_INTERVAL_MS, None) };
    if timer == 0 {
        HOOK.with(|h| h.set(null_mut()));
        HOOK_KEYBOARD.with(|h| h.set(null_mut()));
        unsafe {
            let _ = UnhookWindowsHookEx(hook);
            let _ = UnhookWindowsHookEx(keyboard_hook);
            DestroyWindow(hwnd);
        }
        let _ = ready_tx.send(Err(error::code("E_CAPTURE_TIMER")));
        return;
    }

    let mut cursor = POINT { x: 0, y: 0 };
    unsafe {
        let _ = GetCursorPos(&mut cursor);
    }
    let layout = shared.layout();
    let (layout_generation, world_layout) = layout.snapshot();
    let state = CaptureState {
        shared,
        tx,
        vx,
        vy,
        vw,
        vh,
        cursor,
        layout,
        layout_generation,
        world_layout,
        target: None,
        armed: false,
        active: false,
        hidden: false,
        buttons: 0,
        point: WorldPoint::default(),
        host_anchor: NormPoint::new(0.5, 0.5),
        cooldown_until: Instant::now(),
        keys: KeyOwnership::default(),
        caps_lock: false,
        caps_down: false,
    };
    CAPTURE.with(|slot| {
        *slot.borrow_mut() = Some(state);
    });

    let _ = ready_tx.send(Ok(()));

    let mut msg: MSG = unsafe { zeroed() };
    loop {
        let result = unsafe { GetMessageW(&mut msg, null_mut(), 0, 0) };
        if result <= 0 {
            break;
        }
        unsafe {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }

    with_state(|state| {
        if state.active {
            state.deactivate_to_host(true, None);
        }
        if state.hidden {
            unsafe {
                ShowCursor(1);
            }
            state.hidden = false;
        }
    });
    CAPTURE.with(|slot| {
        slot.borrow_mut().take();
    });
    HOOK.with(|h| h.set(null_mut()));
    HOOK_KEYBOARD.with(|h| h.set(null_mut()));
    unsafe {
        let _ = KillTimer(hwnd, TIMER_ID);
        let _ = UnhookWindowsHookEx(hook);
        let _ = UnhookWindowsHookEx(keyboard_hook);
        let _ = DestroyWindow(hwnd);
    }
}

pub fn start_injector(
    rx: Receiver<InjectMsg>,
    stop: Arc<std::sync::atomic::AtomicBool>,
    status: Status,
) -> Result<JoinHandle<()>, String> {
    let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();
    let join = thread::Builder::new()
        .name("glidelink-input-injector".into())
        .spawn(move || injector_loop(rx, stop, status, ready_tx))
        .map_err(|e| error::detail("E_INJECTOR_THREAD", e))?;
    match ready_rx.recv_timeout(Duration::from_secs(2)) {
        Ok(Ok(())) => Ok(join),
        Ok(Err(e)) => Err(e),
        Err(_) => Err(error::code("E_INJECTOR_TIMEOUT")),
    }
}

fn injector_loop(
    rx: Receiver<InjectMsg>,
    stop: Arc<std::sync::atomic::AtomicBool>,
    status: Status,
    ready_tx: mpsc::Sender<Result<(), String>>,
) {
    crate::sched::boost_current_thread();
    let _ = ready_tx.send(Ok(()));
    let mut applied_buttons = 0u8;
    let mut applied_keys = KeyState::default();
    let caps_actual = unsafe { GetAsyncKeyState(VK_CAPITAL as i32) as u16 & 1 != 0 };
    let mut repeater = KeyRepeater::default();
    let mut cursor = InjectCursor {
        caps_lock: caps_actual,
        ..InjectCursor::default()
    };
    let mut batch = Vec::with_capacity(16);
    let mut steps = Vec::with_capacity(16);

    while !stop.load(Ordering::Acquire) {
        let now = Instant::now();
        if let Some(hid) = repeater.poll(now) {
            if applied_keys.contains(hid) && !is_modifier(hid) {
                send_key_hid(hid, true);
            }
        }

        let timeout = repeater.timeout(now, Duration::from_millis(100));
        let first = match rx.recv_timeout(timeout) {
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
                InjectStep::Move { dx, dy, .. } => unsafe {
                    mouse_event(MOUSEEVENTF_MOVE, dx, dy, 0, 0);
                },
                InjectStep::Warp { x, y, .. } => unsafe {
                    mouse_event(
                        MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
                        x as i32,
                        y as i32,
                        0,
                        0,
                    );
                },
                InjectStep::Buttons { from, to } => {
                    apply_button_transitions(from, to);
                    applied_buttons = to;
                }
                InjectStep::Wheel(delta) => unsafe {
                    mouse_event(MOUSEEVENTF_WHEEL, 0, 0, delta as i32 * WHEEL_DELTA, 0);
                },
                InjectStep::KeyPress(hid) => {
                    if !applied_keys.contains(hid) {
                        applied_keys.insert(hid);
                        repeater.press(hid, Instant::now());
                        send_key_hid(hid, true);
                    }
                }
                InjectStep::KeyRelease(hid) => {
                    repeater.release(hid);
                    if applied_keys.contains(hid) {
                        applied_keys.remove(hid);
                        send_key_hid(hid, false);
                    }
                }
                InjectStep::CapsLock(desired) => {
                    if desired != unsafe { query_caps_lock() } {
                        send_key_hid(CAPS_LOCK, true);
                        send_key_hid(CAPS_LOCK, false);
                    }
                }
                InjectStep::Reset => {
                    release_keys(&mut applied_keys);
                    repeater.release_all();
                    apply_button_transitions(applied_buttons, 0);
                    applied_buttons = 0;
                }
            }
        }
    }

    release_keys(&mut applied_keys);
    if applied_buttons != 0 {
        apply_button_transitions(applied_buttons, 0);
    }
    status.update(|s| {
        s.last_error_code = None;
        s.last_error_detail = None;
    });
}

fn send_key_hid(hid: u8, down: bool) -> bool {
    let Some((scan, extended)) = hid_to_scan(hid) else {
        return false;
    };
    let mut flags = KEYEVENTF_SCANCODE;
    if extended {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    if !down {
        flags |= KEYEVENTF_KEYUP;
    }
    let input = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: 0,
                wScan: scan,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    unsafe { SendInput(1, &input, size_of::<INPUT>() as i32) == 1 }
}

fn release_keys(keys: &mut KeyState) {
    let mut hids: Vec<u8> = keys.iter().collect();
    hids.sort_unstable();
    for hid in hids.iter().copied().filter(|hid| !is_modifier(*hid)) {
        keys.remove(hid);
        send_key_hid(hid, false);
    }
    for hid in hids.iter().copied().filter(|hid| is_modifier(*hid)) {
        keys.remove(hid);
        send_key_hid(hid, false);
    }
}

fn apply_button_transitions(previous: u8, current: u8) {
    let changed = previous ^ current;
    if changed != 0 {
        log::debug!("agent buttons {previous:#04x} -> {current:#04x}");
    }
    if changed & BUTTON_LEFT != 0 {
        unsafe {
            mouse_event(
                if current & BUTTON_LEFT != 0 {
                    MOUSEEVENTF_LEFTDOWN
                } else {
                    MOUSEEVENTF_LEFTUP
                },
                0,
                0,
                0,
                0,
            );
        }
    }
    if changed & BUTTON_RIGHT != 0 {
        unsafe {
            mouse_event(
                if current & BUTTON_RIGHT != 0 {
                    MOUSEEVENTF_RIGHTDOWN
                } else {
                    MOUSEEVENTF_RIGHTUP
                },
                0,
                0,
                0,
                0,
            );
        }
    }
    if changed & BUTTON_MIDDLE != 0 {
        unsafe {
            mouse_event(
                if current & BUTTON_MIDDLE != 0 {
                    MOUSEEVENTF_MIDDLEDOWN
                } else {
                    MOUSEEVENTF_MIDDLEUP
                },
                0,
                0,
                0,
                0,
            );
        }
    }
    if changed & BUTTON_X1 != 0 {
        unsafe {
            mouse_event(
                if current & BUTTON_X1 != 0 {
                    MOUSEEVENTF_XDOWN
                } else {
                    MOUSEEVENTF_XUP
                },
                0,
                0,
                XBUTTON1 as i32,
                0,
            );
        }
    }
    if changed & BUTTON_X2 != 0 {
        unsafe {
            mouse_event(
                if current & BUTTON_X2 != 0 {
                    MOUSEEVENTF_XDOWN
                } else {
                    MOUSEEVENTF_XUP
                },
                0,
                0,
                XBUTTON2 as i32,
                0,
            );
        }
    }
}
