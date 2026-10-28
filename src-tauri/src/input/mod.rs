#[cfg(not(any(windows, target_os = "macos")))]
use crate::layout::Edge;
#[cfg(not(any(windows, target_os = "macos")))]
use crate::status::ScreenInfo;
use crate::status::Status;
#[cfg(not(any(windows, target_os = "macos")))]
use crossbeam_channel::{Receiver, Sender};
use keymap::{is_modifier, KeyEvent, KeyPacket, KeyState};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;

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

/// Absolute position in the peer screen plus the current button state.
/// The packet is sent for movement and every button/wheel change.
#[derive(Clone, Copy, Debug, Default)]
pub struct RemoteInput {
    pub x: f32,
    pub y: f32,
    pub buttons: u8,
    pub wheel: i16,
}

#[derive(Clone, Copy, Debug)]
pub enum CaptureMsg {
    Mouse(RemoteInput),
    Keys(KeyPacket),
}

#[derive(Clone, Copy, Debug)]
pub enum InjectMsg {
    State(RemoteInput),
    Keys(KeyPacket),
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
    KeyRepeat(u8),
    CapsLock(bool),
    Reset,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct InjectCursor {
    pub buttons: u8,
    pub position: Option<(u16, u16)>,
    pub keys: KeyState,
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

/// Collapse runs of pure pointer moves to the latest position while keeping
/// every button transition, wheel delta, key update, and Reset in order. A
/// button/wheel/key change is applied at its own packet position.
pub(crate) fn plan_injection_batch(
    msgs: &[InjectMsg],
    cursor: &mut InjectCursor,
    out: &mut Vec<InjectStep>,
) {
    let mut pending_move = None;

    for msg in msgs {
        match *msg {
            InjectMsg::State(input) => {
                let position = (quantize(input.x), quantize(input.y));
                if input.buttons == cursor.buttons && input.wheel == 0 {
                    pending_move = Some(position);
                    continue;
                }

                if cursor.position != Some(position) {
                    out.push(InjectStep::Move {
                        x: position.0,
                        y: position.1,
                        buttons: cursor.buttons,
                    });
                    cursor.position = Some(position);
                }
                pending_move = None;
                if input.buttons != cursor.buttons {
                    out.push(InjectStep::Buttons {
                        from: cursor.buttons,
                        to: input.buttons,
                    });
                    cursor.buttons = input.buttons;
                }
                if input.wheel != 0 {
                    out.push(InjectStep::Wheel(input.wheel));
                }
            }
            InjectMsg::Keys(packet) => {
                flush_pending_move(cursor, &mut pending_move, out);
                if !packet.active {
                    release_all_keys(cursor, out);
                    continue;
                }

                let repeat = match packet.event {
                    KeyEvent::Repeat(hid) => Some(hid),
                    KeyEvent::None => None,
                };
                let repeat_was_down = repeat.map(|hid| cursor.keys.contains(hid)).unwrap_or(false);

                let mut releases: Vec<u8> = cursor
                    .keys
                    .iter()
                    .filter(|hid| !packet.state.contains(*hid))
                    .collect();
                releases.sort_unstable();
                for hid in releases.iter().copied().filter(|hid| !is_modifier(*hid)) {
                    out.push(InjectStep::KeyRelease(hid));
                }
                for hid in releases.iter().copied().filter(|hid| is_modifier(*hid)) {
                    out.push(InjectStep::KeyRelease(hid));
                }
                for hid in releases {
                    cursor.keys.remove(hid);
                }

                out.push(InjectStep::CapsLock(packet.caps_lock));

                let mut presses: Vec<u8> = packet
                    .state
                    .iter()
                    .filter(|hid| !cursor.keys.contains(*hid))
                    .collect();
                presses.sort_unstable();
                for hid in presses.iter().copied().filter(|hid| is_modifier(*hid)) {
                    out.push(InjectStep::KeyPress(hid));
                }
                for hid in presses.iter().copied().filter(|hid| !is_modifier(*hid)) {
                    out.push(InjectStep::KeyPress(hid));
                }
                for hid in presses {
                    cursor.keys.insert(hid);
                }

                if let Some(hid) = repeat {
                    if repeat_was_down && cursor.keys.contains(hid) {
                        out.push(InjectStep::KeyRepeat(hid));
                    }
                }
            }
            InjectMsg::Reset => {
                flush_pending_move(cursor, &mut pending_move, out);
                release_all_keys(cursor, out);
                out.push(InjectStep::Reset);
                cursor.buttons = 0;
                cursor.position = None;
            }
        }
    }

    flush_pending_move(cursor, &mut pending_move, out);
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

    fn state(x: f32, y: f32, buttons: u8, wheel: i16) -> InjectMsg {
        InjectMsg::State(RemoteInput {
            x,
            y,
            buttons,
            wheel,
        })
    }

    fn px(value: f32) -> u16 {
        (value.clamp(0.0, 1.0) * u16::MAX as f32).round() as u16
    }

    fn plan(msgs: &[InjectMsg], cursor: &mut InjectCursor) -> Vec<InjectStep> {
        let mut out = Vec::new();
        plan_injection_batch(msgs, cursor, &mut out);
        out
    }

    #[test]
    fn pure_moves_collapse_to_the_latest_position() {
        let msgs: Vec<_> = (0..100)
            .map(|i| state(i as f32 / 100.0, 0.5, 0, 0))
            .collect();
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
    fn click_is_applied_at_its_own_position() {
        let msgs = [
            state(0.25, 0.25, 0, 0),
            state(0.25, 0.25, BUTTON_LEFT, 0),
            state(0.25, 0.25, 0, 0),
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
            state(0.5, 0.5, 0, 0),
            state(0.5, 0.5, BUTTON_LEFT, 0),
            state(0.5, 0.5, 0, 0),
            state(0.5, 0.5, BUTTON_LEFT, 0),
            state(0.5, 0.5, 0, 0),
        ];
        let mut cursor = InjectCursor::default();
        let steps = plan(&msgs, &mut cursor);
        let buttons = steps
            .iter()
            .filter(|step| matches!(step, InjectStep::Buttons { .. }))
            .count();
        assert_eq!(buttons, 4);
        assert_eq!(
            steps.first(),
            Some(&InjectStep::Move {
                x: px(0.5),
                y: px(0.5),
                buttons: 0,
            })
        );
    }

    #[test]
    fn click_position_is_not_moved_by_a_later_move() {
        let msgs = [
            state(0.2, 0.2, 0, 0),
            state(0.2, 0.2, BUTTON_LEFT, 0),
            state(0.2, 0.2, 0, 0),
            state(0.6, 0.6, 0, 0),
        ];
        let mut cursor = InjectCursor::default();
        let steps = plan(&msgs, &mut cursor);
        assert_eq!(
            &steps[0..3],
            &[
                InjectStep::Move {
                    x: px(0.2),
                    y: px(0.2),
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
        assert_eq!(
            steps.last(),
            Some(&InjectStep::Move {
                x: px(0.6),
                y: px(0.6),
                buttons: 0,
            })
        );
    }

    #[test]
    fn wheel_follows_move_and_button_transition() {
        let msgs = [state(0.3, 0.3, 0, 0), state(0.4, 0.4, BUTTON_LEFT, 1)];
        let mut cursor = InjectCursor::default();
        let steps = plan(&msgs, &mut cursor);
        assert_eq!(
            steps,
            vec![
                InjectStep::Move {
                    x: px(0.4),
                    y: px(0.4),
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
            state(0.1, 0.1, 0, 0),
            state(0.1, 0.1, BUTTON_LEFT, 0),
            state(0.2, 0.2, BUTTON_LEFT, 0),
            state(0.3, 0.3, BUTTON_LEFT, 0),
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
        let msgs = [
            state(0.2, 0.2, 0, 0),
            state(0.8, 0.8, 0, 0),
            InjectMsg::Reset,
        ];
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
    fn reset_then_move_starts_without_held_buttons() {
        let msgs = [
            state(0.2, 0.2, BUTTON_LEFT, 0),
            InjectMsg::Reset,
            state(0.6, 0.6, 0, 0),
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
                InjectStep::Buttons {
                    from: 0,
                    to: BUTTON_LEFT,
                },
                InjectStep::Reset,
                InjectStep::Move {
                    x: px(0.6),
                    y: px(0.6),
                    buttons: 0,
                },
            ]
        );
    }

    #[test]
    fn same_position_button_change_does_not_emit_a_move() {
        let msgs = [state(0.5, 0.5, 0, 0), state(0.5, 0.5, BUTTON_LEFT, 0)];
        let mut cursor = InjectCursor::default();
        let steps = plan(&msgs, &mut cursor);
        assert_eq!(
            steps,
            vec![
                InjectStep::Move {
                    x: px(0.5),
                    y: px(0.5),
                    buttons: 0,
                },
                InjectStep::Buttons {
                    from: 0,
                    to: BUTTON_LEFT,
                },
            ]
        );
    }

    fn key_packet(active: bool, caps: bool, hids: &[u8], event: KeyEvent) -> InjectMsg {
        let mut key_state = KeyState::default();
        for hid in hids {
            key_state.insert(*hid);
        }
        InjectMsg::Keys(KeyPacket {
            active,
            caps_lock: caps,
            state: key_state,
            event,
        })
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
    fn key_updates_flush_pending_mouse_move() {
        let msgs = [
            state(0.2, 0.2, 0, 0),
            key_packet(true, false, &[0x04], KeyEvent::None),
            state(0.6, 0.6, 0, 0),
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
                InjectStep::CapsLock(false),
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
    fn modifiers_press_first_and_release_last() {
        let msgs = [
            key_packet(true, false, &[0x04, 0xE0, 0xE1], KeyEvent::None),
            key_packet(true, false, &[], KeyEvent::None),
        ];
        let mut cursor = InjectCursor::default();
        let steps = plan(&msgs, &mut cursor);
        assert_eq!(
            steps,
            vec![
                InjectStep::CapsLock(false),
                InjectStep::KeyPress(0xE0),
                InjectStep::KeyPress(0xE1),
                InjectStep::KeyPress(0x04),
                InjectStep::KeyRelease(0x04),
                InjectStep::KeyRelease(0xE0),
                InjectStep::KeyRelease(0xE1),
                InjectStep::CapsLock(false),
            ]
        );
    }

    #[test]
    fn repeat_is_skipped_when_the_key_is_new_in_the_same_packet() {
        let msgs = [
            key_packet(true, false, &[0x04], KeyEvent::Repeat(0x04)),
            key_packet(true, false, &[0x04], KeyEvent::Repeat(0x04)),
        ];
        let mut cursor = InjectCursor::default();
        let steps = plan(&msgs, &mut cursor);
        assert_eq!(
            steps,
            vec![
                InjectStep::CapsLock(false),
                InjectStep::KeyPress(0x04),
                InjectStep::CapsLock(false),
                InjectStep::KeyRepeat(0x04),
            ]
        );
    }

    #[test]
    fn inactive_and_reset_packets_release_keys() {
        let msgs = [
            key_packet(true, false, &[0xE0], KeyEvent::None),
            key_packet(false, false, &[], KeyEvent::None),
        ];
        let mut cursor = InjectCursor::default();
        let steps = plan(&msgs, &mut cursor);
        assert_eq!(
            steps,
            vec![
                InjectStep::CapsLock(false),
                InjectStep::KeyPress(0xE0),
                InjectStep::KeyRelease(0xE0),
            ]
        );
        assert!(cursor.keys.is_empty());

        let mut cursor = InjectCursor::default();
        let msgs = [
            key_packet(true, false, &[0xE0], KeyEvent::None),
            InjectMsg::Reset,
        ];
        let steps = plan(&msgs, &mut cursor);
        assert_eq!(
            steps,
            vec![
                InjectStep::CapsLock(false),
                InjectStep::KeyPress(0xE0),
                InjectStep::KeyRelease(0xE0),
                InjectStep::Reset,
            ]
        );
        assert!(cursor.keys.is_empty());
        assert_eq!(cursor.buttons, 0);
        assert_eq!(cursor.position, None);
    }

    #[test]
    fn caps_sync_is_emitted_for_every_active_packet() {
        let msgs = [
            key_packet(true, true, &[], KeyEvent::None),
            key_packet(true, false, &[], KeyEvent::None),
        ];
        let mut cursor = InjectCursor::default();
        let steps = plan(&msgs, &mut cursor);
        assert_eq!(
            steps,
            vec![InjectStep::CapsLock(true), InjectStep::CapsLock(false)]
        );
    }
}
