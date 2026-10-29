#[cfg(not(any(windows, target_os = "macos")))]
use crate::layout::Edge;
#[cfg(not(any(windows, target_os = "macos")))]
use crate::status::ScreenInfo;
use crate::status::Status;
#[cfg(not(any(windows, target_os = "macos")))]
use crossbeam_channel::{Receiver, Sender};
use keymap::{adapt_modifier_hid, is_caps, is_modifier, KeyState};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
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

/// Latest absolute pointer position in the peer screen. Buttons and wheel
/// travel as discrete events so clicks and scroll ticks cannot be coalesced
/// away with a move.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RemoteInput {
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
    Move(RemoteInput),
    Event(InputEvent),
    Reconcile(ReconcileState),
}

#[derive(Clone, Copy, Debug)]
pub enum InjectMsg {
    Move(RemoteInput),
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
    /// Buttons in effect for this move (used to pick a drag event).
    Move {
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
    pub position: Option<(u16, u16)>,
    pub keys: KeyState,
    pub caps_lock: bool,
}

fn quantize(value: f32) -> u16 {
    (value.clamp(0.0, 1.0) * u16::MAX as f32).round() as u16
}

fn flush_pending_move(
    cursor: &mut InjectCursor,
    pending_move: &mut Option<(u16, u16)>,
    out: &mut Vec<InjectStep>,
) {
    if let Some(position) = pending_move.take() {
        if cursor.position != Some(position) {
            out.push(InjectStep::Move {
                x: position.0,
                y: position.1,
                buttons: cursor.buttons,
            });
            cursor.position = Some(position);
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

/// Collapse runs of pure pointer moves to the latest position while keeping
/// every button transition, wheel delta, key edge, reconciliation, and Reset in
/// order. A discrete event is applied at its own packet position.
pub(crate) fn plan_injection_batch(
    msgs: &[InjectMsg],
    cursor: &mut InjectCursor,
    out: &mut Vec<InjectStep>,
) {
    let mut pending_move = None;

    for msg in msgs {
        match *msg {
            InjectMsg::Move(input) => {
                pending_move = Some((quantize(input.x), quantize(input.y)));
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
                cursor.position = None;
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
    pub connected: AtomicBool,
    pub remote_width: AtomicU32,
    pub remote_height: AtomicU32,
    pub active: AtomicBool,
    pub status: Status,
}

impl CaptureShared {
    pub fn new(status: Status) -> Arc<Self> {
        Arc::new(Self {
            stop: AtomicBool::new(false),
            connected: AtomicBool::new(false),
            remote_width: AtomicU32::new(0),
            remote_height: AtomicU32::new(0),
            active: AtomicBool::new(false),
            status,
        })
    }

    pub fn set_peer(&self, width: u32, height: u32) {
        self.remote_width.store(width, Ordering::Release);
        self.remote_height.store(height, Ordering::Release);
        self.connected.store(true, Ordering::Release);
    }

    pub fn clear_peer(&self) {
        self.connected.store(false, Ordering::Release);
    }

    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Acquire)
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
        _edge: Edge,
        _offset: f32,
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

    fn mv(x: f32, y: f32) -> InjectMsg {
        InjectMsg::Move(RemoteInput { x, y })
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

    #[test]
    fn pure_moves_collapse_to_the_latest_position() {
        let msgs: Vec<_> = (0..100).map(|i| mv(i as f32 / 100.0, 0.5)).collect();
        let mut cursor = InjectCursor::default();
        let steps = plan(&msgs, &mut cursor);
        assert_eq!(
            steps,
            vec![InjectStep::Move {
                x: px(0.99),
                y: px(0.5),
                buttons: 0,
            }]
        );
    }

    #[test]
    fn button_event_is_applied_at_its_own_position() {
        let msgs = [
            mv(0.25, 0.25),
            InjectMsg::Event(InputEvent::Button {
                button: BUTTON_LEFT,
                down: true,
            }),
            InjectMsg::Event(InputEvent::Button {
                button: BUTTON_LEFT,
                down: false,
            }),
        ];
        let mut cursor = InjectCursor::default();
        let steps = plan(&msgs, &mut cursor);
        assert_eq!(
            steps,
            vec![
                InjectStep::Move {
                    x: px(0.25),
                    y: px(0.25),
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
            mv(0.5, 0.5),
            InjectMsg::Event(InputEvent::Button {
                button: BUTTON_LEFT,
                down: true,
            }),
            InjectMsg::Event(InputEvent::Button {
                button: BUTTON_LEFT,
                down: false,
            }),
            InjectMsg::Event(InputEvent::Button {
                button: BUTTON_LEFT,
                down: true,
            }),
            InjectMsg::Event(InputEvent::Button {
                button: BUTTON_LEFT,
                down: false,
            }),
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
            mv(0.3, 0.3),
            InjectMsg::Event(InputEvent::Button {
                button: BUTTON_LEFT,
                down: true,
            }),
            InjectMsg::Event(InputEvent::Wheel { delta: 1 }),
        ];
        let mut cursor = InjectCursor::default();
        let steps = plan(&msgs, &mut cursor);
        assert_eq!(
            steps,
            vec![
                InjectStep::Move {
                    x: px(0.3),
                    y: px(0.3),
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
        let msgs = [
            mv(0.1, 0.1),
            InjectMsg::Event(InputEvent::Button {
                button: BUTTON_LEFT,
                down: true,
            }),
            mv(0.2, 0.2),
            mv(0.3, 0.3),
        ];
        let mut cursor = InjectCursor::default();
        let steps = plan(&msgs, &mut cursor);
        assert_eq!(
            steps,
            vec![
                InjectStep::Move {
                    x: px(0.1),
                    y: px(0.1),
                    buttons: 0,
                },
                InjectStep::Buttons {
                    from: 0,
                    to: BUTTON_LEFT,
                },
                InjectStep::Move {
                    x: px(0.3),
                    y: px(0.3),
                    buttons: BUTTON_LEFT,
                },
            ]
        );
    }

    #[test]
    fn reset_flushes_the_latest_move_and_clears_state() {
        let msgs = [mv(0.2, 0.2), mv(0.8, 0.8), InjectMsg::Reset];
        let mut cursor = InjectCursor::default();
        let steps = plan(&msgs, &mut cursor);
        assert_eq!(
            steps,
            vec![
                InjectStep::Move {
                    x: px(0.8),
                    y: px(0.8),
                    buttons: 0,
                },
                InjectStep::Reset,
            ]
        );
        assert_eq!(cursor.buttons, 0);
        assert_eq!(cursor.position, None);
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
            mv(0.2, 0.2),
            InjectMsg::Event(InputEvent::Key {
                hid: 0x04,
                down: true,
            }),
            mv(0.6, 0.6),
        ];
        let mut cursor = InjectCursor::default();
        let steps = plan(&msgs, &mut cursor);
        assert_eq!(
            steps,
            vec![
                InjectStep::Move {
                    x: px(0.2),
                    y: px(0.2),
                    buttons: 0,
                },
                InjectStep::KeyPress(0x04),
                InjectStep::Move {
                    x: px(0.6),
                    y: px(0.6),
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
            InjectMsg::Event(InputEvent::Button {
                button: BUTTON_LEFT,
                down: true,
            }),
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
        let msgs = [InjectMsg::Reconcile(state)];
        let mut cursor = InjectCursor::default();
        let steps = plan(&msgs, &mut cursor);
        assert_eq!(
            steps,
            vec![
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
