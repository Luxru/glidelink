use crate::layout::{DeviceId, LayoutStore};
#[cfg(not(any(windows, target_os = "macos")))]
use crate::status::ScreenInfo;
use crate::status::Status;
#[cfg(not(any(windows, target_os = "macos")))]
use crossbeam_channel::{Receiver, Sender};
use keymap::{adapt_modifier_hid, is_caps, is_modifier, KeyState};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

pub(crate) mod keymap;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod win;

pub const BUTTON_LEFT: u8 = 1 << 0;
pub const BUTTON_RIGHT: u8 = 1 << 1;
pub const BUTTON_MIDDLE: u8 = 1 << 2;
pub const BUTTON_X1: u8 = 1 << 3;
pub const BUTTON_X2: u8 = 1 << 4;

/// Relative pointer movement in peer screen pixels. Multiple moves may be
/// accumulated before injection; buttons and wheel remain discrete events.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RemoteInput {
    pub dx: i32,
    pub dy: i32,
}

/// One absolute pointer position used to enter or re-synchronize a peer screen.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WarpInput {
    pub x: f32,
    pub y: f32,
}

/// A discrete input edge. This is the semantic unit carried by the wire; the
/// receiver owns key repeat and turns the edges into platform events.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputEvent {
    Button { button: u8, down: bool },
    Wheel { delta: i16 },
    Key { hid: u8, down: bool },
    CapsLock { locked: bool },
    ReleaseAll,
}

/// Periodic reconciliation of state that is idempotent: held keys, held mouse
/// buttons, Caps Lock, and whether the pointer is currently on the peer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReconcileState {
    pub active: bool,
    pub caps_lock: bool,
    pub buttons: u8,
    pub keys: KeyState,
}

impl ReconcileState {
    pub const INACTIVE: Self = Self {
        active: false,
        caps_lock: false,
        buttons: 0,
        keys: KeyState::EMPTY,
    };
}

#[derive(Clone, Copy, Debug)]
pub enum CaptureMsg {
    Move {
        device: DeviceId,
        input: RemoteInput,
    },
    Warp {
        device: DeviceId,
        input: WarpInput,
    },
    Event {
        device: DeviceId,
        event: InputEvent,
    },
    Reconcile {
        device: DeviceId,
        state: ReconcileState,
    },
}

#[derive(Clone, Copy, Debug)]
pub enum InjectMsg {
    Move(RemoteInput),
    Warp(WarpInput),
    Event(InputEvent),
    Reconcile(ReconcileState),
    Reset,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyAction {
    Swallow,
    Passthrough,
    Down,
    Up,
    Repeat,
}

/// Tracks which keys the local OS has seen key-down for versus which keys are
/// currently forwarded to the remote. Keys held before activation stay local.
#[derive(Clone, Copy, Debug, Default)]
pub struct KeyOwnership {
    pub remote: KeyState,
    pub local: KeyState,
}

impl KeyOwnership {
    pub fn begin(&mut self, held: KeyState) {
        self.remote = KeyState::default();
        self.local = held;
    }

    pub fn rebind(&mut self, held: KeyState) {
        self.begin(held);
    }

    pub fn deactivate(&mut self) {
        self.remote = KeyState::default();
        self.local = KeyState::default();
    }

    pub fn key_down(&mut self, active: bool, hid: u8) -> KeyAction {
        if !active {
            self.local.insert(hid);
            return KeyAction::Passthrough;
        }
        if self.local.contains(hid) {
            return KeyAction::Swallow;
        }
        if self.remote.contains(hid) {
            return KeyAction::Repeat;
        }
        self.remote.insert(hid);
        KeyAction::Down
    }

    pub fn key_up(&mut self, active: bool, hid: u8) -> KeyAction {
        if !active {
            self.local.remove(hid);
            return KeyAction::Passthrough;
        }
        if self.local.contains(hid) {
            self.local.remove(hid);
            return KeyAction::Passthrough;
        }
        if self.remote.contains(hid) {
            self.remote.remove(hid);
            return KeyAction::Up;
        }
        KeyAction::Swallow
    }
}

/// Cross-system adaptation keeps the primary shortcut modifier natural:
/// Mac Command <-> Windows Control. Shift, Alt/Option, and Caps are unchanged.
pub fn adapt_event(event: &mut InputEvent) {
    if let InputEvent::Key { hid, .. } = event {
        *hid = adapt_modifier_hid(*hid);
    }
}

pub fn adapt_reconcile(state: &mut ReconcileState) {
    let mut keys = KeyState::default();
    for hid in state.keys.iter() {
        keys.insert(adapt_modifier_hid(hid));
    }
    state.keys = keys;
}

/// A reduced, ordered sequence of platform injection actions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InjectStep {
    /// Relative movement; buttons select the platform drag event.
    Move {
        dx: i32,
        dy: i32,
        buttons: u8,
    },
    /// Absolute entry/re-synchronization position.
    Warp {
        x: u16,
        y: u16,
        buttons: u8,
    },
    Buttons {
        from: u8,
        to: u8,
    },
    Wheel(i16),
    KeyPress(u8),
    KeyRelease(u8),
    CapsLock(bool),
    Reset,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct InjectCursor {
    pub buttons: u8,
    pub keys: KeyState,
    pub caps_lock: bool,
}

fn quantize(value: f32) -> u16 {
    (value.clamp(0.0, 1.0) * u16::MAX as f32).round() as u16
}

fn flush_pending_move(
    cursor: &mut InjectCursor,
    pending_move: &mut Option<RemoteInput>,
    out: &mut Vec<InjectStep>,
) {
    if let Some(input) = pending_move.take() {
        if input.dx != 0 || input.dy != 0 {
            out.push(InjectStep::Move {
                dx: input.dx,
                dy: input.dy,
                buttons: cursor.buttons,
            });
        }
    }
}

fn release_all_keys(cursor: &mut InjectCursor, out: &mut Vec<InjectStep>) {
    let mut keys: Vec<u8> = cursor.keys.iter().collect();
    keys.sort_unstable();
    for hid in keys.iter().copied().filter(|hid| !is_modifier(*hid)) {
        out.push(InjectStep::KeyRelease(hid));
    }
    for hid in keys.iter().copied().filter(|hid| is_modifier(*hid)) {
        out.push(InjectStep::KeyRelease(hid));
    }
    cursor.keys = KeyState::default();
}

fn release_all_buttons(cursor: &mut InjectCursor, out: &mut Vec<InjectStep>) {
    if cursor.buttons != 0 {
        out.push(InjectStep::Buttons {
            from: cursor.buttons,
            to: 0,
        });
        cursor.buttons = 0;
    }
}

fn press_key(cursor: &mut InjectCursor, hid: u8, out: &mut Vec<InjectStep>) {
    if !cursor.keys.contains(hid) {
        cursor.keys.insert(hid);
        out.push(InjectStep::KeyPress(hid));
    }
}

fn release_key(cursor: &mut InjectCursor, hid: u8, out: &mut Vec<InjectStep>) {
    if cursor.keys.contains(hid) {
        cursor.keys.remove(hid);
        out.push(InjectStep::KeyRelease(hid));
    }
}

fn apply_event(cursor: &mut InjectCursor, event: InputEvent, out: &mut Vec<InjectStep>) {
    match event {
        InputEvent::Button { button, down } => {
            let next = if down {
                cursor.buttons | button
            } else {
                cursor.buttons & !button
            };
            if next != cursor.buttons {
                out.push(InjectStep::Buttons {
                    from: cursor.buttons,
                    to: next,
                });
                cursor.buttons = next;
            }
        }
        InputEvent::Wheel { delta } => {
            if delta != 0 {
                out.push(InjectStep::Wheel(delta));
            }
        }
        InputEvent::Key { hid, down } => {
            if down {
                press_key(cursor, hid, out);
            } else {
                release_key(cursor, hid, out);
            }
        }
        InputEvent::CapsLock { locked } => {
            if locked != cursor.caps_lock {
                out.push(InjectStep::CapsLock(locked));
                cursor.caps_lock = locked;
            }
        }
        InputEvent::ReleaseAll => {
            release_all_keys(cursor, out);
            release_all_buttons(cursor, out);
        }
    }
}

fn apply_reconcile(cursor: &mut InjectCursor, state: ReconcileState, out: &mut Vec<InjectStep>) {
    if !state.active {
        release_all_keys(cursor, out);
        release_all_buttons(cursor, out);
        return;
    }

    if state.buttons != cursor.buttons {
        out.push(InjectStep::Buttons {
            from: cursor.buttons,
            to: state.buttons,
        });
        cursor.buttons = state.buttons;
    }

    let mut releases: Vec<u8> = cursor
        .keys
        .iter()
        .filter(|hid| !state.keys.contains(*hid))
        .collect();
    releases.sort_unstable();
    for hid in releases.iter().copied().filter(|hid| !is_modifier(*hid)) {
        out.push(InjectStep::KeyRelease(hid));
        cursor.keys.remove(hid);
    }
    for hid in releases.iter().copied().filter(|hid| is_modifier(*hid)) {
        out.push(InjectStep::KeyRelease(hid));
        cursor.keys.remove(hid);
    }

    if state.caps_lock != cursor.caps_lock {
        out.push(InjectStep::CapsLock(state.caps_lock));
        cursor.caps_lock = state.caps_lock;
    }

    let mut presses: Vec<u8> = state
        .keys
        .iter()
        .filter(|hid| !cursor.keys.contains(*hid))
        .collect();
    presses.sort_unstable();
    for hid in presses.iter().copied().filter(|hid| is_modifier(*hid)) {
        out.push(InjectStep::KeyPress(hid));
        cursor.keys.insert(hid);
    }
    for hid in presses.iter().copied().filter(|hid| !is_modifier(*hid)) {
        out.push(InjectStep::KeyPress(hid));
        cursor.keys.insert(hid);
    }
}

/// Accumulate relative pointer movement while keeping every button transition,
/// wheel delta, key edge, warp, reconciliation, and Reset in order. A discrete
/// event is applied at its own packet position.
pub(crate) fn plan_injection_batch(
    msgs: &[InjectMsg],
    cursor: &mut InjectCursor,
    out: &mut Vec<InjectStep>,
) {
    let mut pending_move = None;

    for msg in msgs {
        match *msg {
            InjectMsg::Move(input) => {
                let pending = pending_move.get_or_insert_with(RemoteInput::default);
                pending.dx = pending.dx.saturating_add(input.dx);
                pending.dy = pending.dy.saturating_add(input.dy);
            }
            InjectMsg::Warp(input) => {
                flush_pending_move(cursor, &mut pending_move, out);
                out.push(InjectStep::Warp {
                    x: quantize(input.x),
                    y: quantize(input.y),
                    buttons: cursor.buttons,
                });
            }
            InjectMsg::Event(event) => {
                flush_pending_move(cursor, &mut pending_move, out);
                apply_event(cursor, event, out);
            }
            InjectMsg::Reconcile(state) => {
                flush_pending_move(cursor, &mut pending_move, out);
                apply_reconcile(cursor, state, out);
            }
            InjectMsg::Reset => {
                flush_pending_move(cursor, &mut pending_move, out);
                release_all_keys(cursor, out);
                release_all_buttons(cursor, out);
                out.push(InjectStep::Reset);
                cursor.caps_lock = false;
            }
        }
    }

    flush_pending_move(cursor, &mut pending_move, out);
}

pub(crate) const REPEAT_DELAY: Duration = Duration::from_millis(500);
pub(crate) const REPEAT_INTERVAL: Duration = Duration::from_millis(32);

/// Receiver-side key repeat. Synthetic key events do not reliably trigger the
/// platform's autorepeat, so the injector owns the timer like lan-mouse does:
/// one repeating key, 500 ms initial delay, 32 ms interval, and no repeat for
/// modifiers or Caps Lock.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct KeyRepeater {
    key: Option<u8>,
    next: Option<Instant>,
}

impl KeyRepeater {
    pub fn press(&mut self, hid: u8, now: Instant) {
        if is_modifier(hid) || is_caps(hid) {
            return;
        }
        self.key = Some(hid);
        self.next = Some(now + REPEAT_DELAY);
    }

    pub fn release(&mut self, hid: u8) {
        if self.key == Some(hid) {
            self.key = None;
            self.next = None;
        }
    }

    pub fn release_all(&mut self) {
        self.key = None;
        self.next = None;
    }

    pub fn poll(&mut self, now: Instant) -> Option<u8> {
        let key = self.key?;
        let next = self.next?;
        if now >= next {
            self.next = Some(now + REPEAT_INTERVAL);
            Some(key)
        } else {
            None
        }
    }

    pub fn timeout(&self, now: Instant, cap: Duration) -> Duration {
        self.next
            .map(|next| next.saturating_duration_since(now).min(cap))
            .unwrap_or(cap)
    }
}

/// Cross-thread state shared by the capture thread and networking.
pub struct CaptureShared {
    pub stop: AtomicBool,
    pub active: AtomicBool,
    pub layout: LayoutStore,
    pub status: Status,
}

impl CaptureShared {
    pub fn new(status: Status, layout: LayoutStore) -> Arc<Self> {
        Arc::new(Self {
            stop: AtomicBool::new(false),
            active: AtomicBool::new(false),
            layout,
            status,
        })
    }

    pub fn layout(&self) -> LayoutStore {
        self.layout.clone()
    }

    pub fn set_active(&self, active: bool) {
        self.active.store(active, Ordering::Release);
        self.status.update(|status| status.remote_active = active);
    }
}

pub struct CaptureHandle {
    shared: Arc<CaptureShared>,
    join: JoinHandle<()>,
}

impl CaptureHandle {
    pub fn new(shared: Arc<CaptureShared>, join: JoinHandle<()>) -> Self {
        Self { shared, join }
    }

    pub fn stop_and_join(self) {
        self.shared.stop.store(true, Ordering::Release);
        if self.join.join().is_err() {
            log::error!("input capture thread panicked");
        }
    }
}

#[cfg(windows)]
pub use win::{open_permissions, permissions_ok, screen_info, start_capture, start_injector};

#[cfg(target_os = "macos")]
pub use macos::{
    open_permissions, permissions_ok, screen_info, self_test, start_capture, start_injector,
};

#[cfg(not(any(windows, target_os = "macos")))]
mod unsupported {
    use super::*;

    pub fn screen_info() -> Option<ScreenInfo> {
        None
    }

    pub fn permissions_ok() -> bool {
        false
    }

    pub fn open_permissions() -> Result<(), String> {
        Err(crate::error::code("E_UNSUPPORTED"))
    }

    pub fn start_capture(
        _shared: Arc<CaptureShared>,
        _tx: Sender<CaptureMsg>,
    ) -> Result<CaptureHandle, String> {
        Err(crate::error::code("E_UNSUPPORTED"))
    }

    pub fn start_injector(
        _rx: Receiver<InjectMsg>,
        _stop: Arc<AtomicBool>,
        _status: Status,
    ) -> Result<JoinHandle<()>, String> {
        Err(crate::error::code("E_UNSUPPORTED"))
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
pub use unsupported::{
    open_permissions, permissions_ok, screen_info, start_capture, start_injector,
};

pub fn platform_name() -> &'static str {
    if cfg!(windows) {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "unsupported"
    }
}

/// Wire value used in HELLO/ACCEPT for cross-system modifier adaptation.
pub fn platform_id() -> u8 {
    if cfg!(windows) {
        2
    } else if cfg!(target_os = "macos") {
        1
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mv(dx: i32, dy: i32) -> InjectMsg {
        InjectMsg::Move(RemoteInput { dx, dy })
    }

    fn px(value: f32) -> u16 {
        (value.clamp(0.0, 1.0) * u16::MAX as f32).round() as u16
    }

    fn plan(msgs: &[InjectMsg], cursor: &mut InjectCursor) -> Vec<InjectStep> {
        let mut out = Vec::new();
        plan_injection_batch(msgs, cursor, &mut out);
        out
    }

    fn key_state(hids: &[u8]) -> KeyState {
        let mut keys = KeyState::default();
        for hid in hids {
            keys.insert(*hid);
        }
        keys
    }

    fn down(button: u8) -> InjectMsg {
        InjectMsg::Event(InputEvent::Button { button, down: true })
    }

    fn up(button: u8) -> InjectMsg {
        InjectMsg::Event(InputEvent::Button {
            button,
            down: false,
        })
    }

    #[test]
    fn relative_moves_accumulate() {
        let msgs = [mv(1, 2), mv(3, -4), mv(-1, 0)];
        let mut cursor = InjectCursor::default();
        let steps = plan(&msgs, &mut cursor);
        assert_eq!(
            steps,
            vec![InjectStep::Move {
                dx: 3,
                dy: -2,
                buttons: 0,
            }]
        );
    }

    #[test]
    fn button_event_flushes_accumulated_motion() {
        let msgs = [mv(10, 5), down(BUTTON_LEFT), up(BUTTON_LEFT)];
        let mut cursor = InjectCursor::default();
        let steps = plan(&msgs, &mut cursor);
        assert_eq!(
            steps,
            vec![
                InjectStep::Move {
                    dx: 10,
                    dy: 5,
                    buttons: 0,
                },
                InjectStep::Buttons {
                    from: 0,
                    to: BUTTON_LEFT,
                },
                InjectStep::Buttons {
                    from: BUTTON_LEFT,
                    to: 0,
                },
            ]
        );
    }

    #[test]
    fn double_click_preserves_four_button_steps() {
        let msgs = [
            mv(5, 5),
            down(BUTTON_LEFT),
            up(BUTTON_LEFT),
            down(BUTTON_LEFT),
            up(BUTTON_LEFT),
        ];
        let mut cursor = InjectCursor::default();
        let steps = plan(&msgs, &mut cursor);
        let buttons = steps
            .iter()
            .filter(|step| matches!(step, InjectStep::Buttons { .. }))
            .count();
        assert_eq!(buttons, 4);
    }

    #[test]
    fn wheel_follows_move_and_button_transition() {
        let msgs = [
            mv(3, 4),
            down(BUTTON_LEFT),
            InjectMsg::Event(InputEvent::Wheel { delta: 1 }),
        ];
        let mut cursor = InjectCursor::default();
        let steps = plan(&msgs, &mut cursor);
        assert_eq!(
            steps,
            vec![
                InjectStep::Move {
                    dx: 3,
                    dy: 4,
                    buttons: 0,
                },
                InjectStep::Buttons {
                    from: 0,
                    to: BUTTON_LEFT,
                },
                InjectStep::Wheel(1),
            ]
        );
    }

    #[test]
    fn held_drag_moves_carry_the_button_state() {
        let msgs = [mv(1, 1), down(BUTTON_LEFT), mv(2, 2), mv(3, 3)];
        let mut cursor = InjectCursor::default();
        let steps = plan(&msgs, &mut cursor);
        assert_eq!(
            steps,
            vec![
                InjectStep::Move {
                    dx: 1,
                    dy: 1,
                    buttons: 0,
                },
                InjectStep::Buttons {
                    from: 0,
                    to: BUTTON_LEFT,
                },
                InjectStep::Move {
                    dx: 5,
                    dy: 5,
                    buttons: BUTTON_LEFT,
                },
            ]
        );
    }

    #[test]
    fn warp_flushes_motion_and_emits_absolute_position() {
        let msgs = [mv(10, 10), InjectMsg::Warp(WarpInput { x: 0.25, y: 0.75 })];
        let mut cursor = InjectCursor::default();
        let steps = plan(&msgs, &mut cursor);
        assert_eq!(
            steps,
            vec![
                InjectStep::Move {
                    dx: 10,
                    dy: 10,
                    buttons: 0,
                },
                InjectStep::Warp {
                    x: px(0.25),
                    y: px(0.75),
                    buttons: 0,
                },
            ]
        );
    }

    #[test]
    fn reset_flushes_motion_and_clears_state() {
        let msgs = [mv(2, 3), InjectMsg::Reset];
        let mut cursor = InjectCursor::default();
        let steps = plan(&msgs, &mut cursor);
        assert_eq!(
            steps,
            vec![
                InjectStep::Move {
                    dx: 2,
                    dy: 3,
                    buttons: 0,
                },
                InjectStep::Reset,
            ]
        );
        assert_eq!(cursor.buttons, 0);
    }

    #[test]
    fn ownership_keeps_preheld_keys_local() {
        let mut ownership = KeyOwnership::default();
        let mut held = KeyState::default();
        held.insert(0xE1); // left Shift
        ownership.begin(held);

        assert_eq!(ownership.key_down(true, 0xE1), KeyAction::Swallow);
        assert_eq!(ownership.key_up(true, 0xE1), KeyAction::Passthrough);
        assert!(!ownership.local.contains(0xE1));
        assert!(!ownership.remote.contains(0xE1));

        assert_eq!(ownership.key_down(true, 0x04), KeyAction::Down);
        assert!(ownership.remote.contains(0x04));
        assert_eq!(ownership.key_down(true, 0x04), KeyAction::Repeat);
        assert_eq!(ownership.key_up(true, 0x04), KeyAction::Up);
        assert!(!ownership.remote.contains(0x04));
        assert_eq!(ownership.key_up(true, 0x05), KeyAction::Swallow);

        assert_eq!(ownership.key_down(false, 0x06), KeyAction::Passthrough);
        assert!(ownership.local.contains(0x06));
        assert_eq!(ownership.key_up(false, 0x06), KeyAction::Passthrough);
        assert!(!ownership.local.contains(0x06));

        let mut rebound = KeyState::default();
        rebound.insert(0x04);
        ownership.rebind(rebound);
        assert!(ownership.local.contains(0x04));
        assert!(ownership.remote.is_empty());
        ownership.deactivate();
        assert!(ownership.local.is_empty());
        assert!(ownership.remote.is_empty());
    }

    #[test]
    fn key_edges_flush_pending_mouse_move() {
        let msgs = [
            mv(2, 3),
            InjectMsg::Event(InputEvent::Key {
                hid: 0x04,
                down: true,
            }),
            mv(4, 5),
        ];
        let mut cursor = InjectCursor::default();
        let steps = plan(&msgs, &mut cursor);
        assert_eq!(
            steps,
            vec![
                InjectStep::Move {
                    dx: 2,
                    dy: 3,
                    buttons: 0,
                },
                InjectStep::KeyPress(0x04),
                InjectStep::Move {
                    dx: 4,
                    dy: 5,
                    buttons: 0,
                },
            ]
        );
    }

    #[test]
    fn duplicate_key_edges_are_idempotent() {
        let msgs = [
            InjectMsg::Event(InputEvent::Key {
                hid: 0x04,
                down: true,
            }),
            InjectMsg::Event(InputEvent::Key {
                hid: 0x04,
                down: true,
            }),
            InjectMsg::Event(InputEvent::Key {
                hid: 0x04,
                down: false,
            }),
            InjectMsg::Event(InputEvent::Key {
                hid: 0x04,
                down: false,
            }),
        ];
        let mut cursor = InjectCursor::default();
        let steps = plan(&msgs, &mut cursor);
        assert_eq!(
            steps,
            vec![InjectStep::KeyPress(0x04), InjectStep::KeyRelease(0x04),]
        );
    }

    #[test]
    fn release_all_clears_keys_and_buttons_without_touching_caps() {
        let msgs = [
            InjectMsg::Event(InputEvent::CapsLock { locked: true }),
            InjectMsg::Event(InputEvent::Key {
                hid: 0x04,
                down: true,
            }),
            down(BUTTON_LEFT),
            InjectMsg::Event(InputEvent::ReleaseAll),
        ];
        let mut cursor = InjectCursor::default();
        let steps = plan(&msgs, &mut cursor);
        assert_eq!(
            steps,
            vec![
                InjectStep::CapsLock(true),
                InjectStep::KeyPress(0x04),
                InjectStep::Buttons {
                    from: 0,
                    to: BUTTON_LEFT,
                },
                InjectStep::KeyRelease(0x04),
                InjectStep::Buttons {
                    from: BUTTON_LEFT,
                    to: 0,
                },
            ]
        );
        assert!(cursor.keys.is_empty());
        assert_eq!(cursor.buttons, 0);
        assert!(cursor.caps_lock);
    }

    #[test]
    fn reconcile_presses_modifiers_first_and_releases_them_last() {
        let state = ReconcileState {
            active: true,
            caps_lock: false,
            buttons: 0,
            keys: key_state(&[0x04, 0xE0, 0xE1]),
        };
        let msgs = [mv(1, 1), InjectMsg::Reconcile(state)];
        let mut cursor = InjectCursor::default();
        let steps = plan(&msgs, &mut cursor);
        assert_eq!(
            steps,
            vec![
                InjectStep::Move {
                    dx: 1,
                    dy: 1,
                    buttons: 0,
                },
                InjectStep::KeyPress(0xE0),
                InjectStep::KeyPress(0xE1),
                InjectStep::KeyPress(0x04),
            ]
        );

        let state = ReconcileState {
            active: true,
            caps_lock: false,
            buttons: 0,
            keys: KeyState::default(),
        };
        let steps = plan(&[InjectMsg::Reconcile(state)], &mut cursor);
        assert_eq!(
            steps,
            vec![
                InjectStep::KeyRelease(0x04),
                InjectStep::KeyRelease(0xE0),
                InjectStep::KeyRelease(0xE1),
            ]
        );
    }

    #[test]
    fn inactive_reconcile_releases_everything() {
        let active = ReconcileState {
            active: true,
            caps_lock: true,
            buttons: BUTTON_LEFT,
            keys: key_state(&[0x04, 0xE0]),
        };
        let mut cursor = InjectCursor::default();
        let _ = plan(&[InjectMsg::Reconcile(active)], &mut cursor);
        let steps = plan(
            &[InjectMsg::Reconcile(ReconcileState::INACTIVE)],
            &mut cursor,
        );
        assert_eq!(
            steps,
            vec![
                InjectStep::KeyRelease(0x04),
                InjectStep::KeyRelease(0xE0),
                InjectStep::Buttons {
                    from: BUTTON_LEFT,
                    to: 0,
                },
            ]
        );
        assert!(cursor.keys.is_empty());
        assert_eq!(cursor.buttons, 0);
        assert!(cursor.caps_lock);
    }

    #[test]
    fn reconcile_caps_changes_before_new_keys() {
        let mut cursor = InjectCursor::default();
        let state = ReconcileState {
            active: true,
            caps_lock: true,
            buttons: 0,
            keys: KeyState::default(),
        };
        let _ = plan(&[InjectMsg::Reconcile(state)], &mut cursor);
        let state = ReconcileState {
            active: true,
            caps_lock: false,
            buttons: 0,
            keys: key_state(&[0x04]),
        };
        let steps = plan(&[InjectMsg::Reconcile(state)], &mut cursor);
        assert_eq!(
            steps,
            vec![InjectStep::CapsLock(false), InjectStep::KeyPress(0x04)]
        );
    }

    #[test]
    fn repeater_uses_delay_interval_and_one_key() {
        let start = Instant::now();
        let mut repeater = KeyRepeater::default();
        repeater.press(0x04, start);
        assert_eq!(repeater.poll(start + Duration::from_millis(499)), None);
        assert_eq!(repeater.poll(start + REPEAT_DELAY), Some(0x04));
        assert_eq!(
            repeater.poll(start + REPEAT_DELAY + Duration::from_millis(31)),
            None
        );
        assert_eq!(
            repeater.poll(start + REPEAT_DELAY + REPEAT_INTERVAL),
            Some(0x04)
        );

        repeater.press(0xE0, start);
        assert_eq!(repeater.poll(start + Duration::from_secs(2)), Some(0x04));
        repeater.release(0x04);
        assert_eq!(repeater.poll(start + Duration::from_secs(3)), None);
    }
}
