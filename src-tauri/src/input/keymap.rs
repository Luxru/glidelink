//! Neutral keyboard identity and platform key mappings.
//!
//! The wire format uses USB HID Keyboard/Keypad usage IDs (page 0x07), which
//! describe physical key positions. Platform keycodes/scancodes are derived
//! from this table at the edges.

use std::fmt;

/// HID usage IDs for the keys supported in v1.
pub const CAPS_LOCK: u8 = 0x39;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyDef {
    pub hid: u8,
    pub mac: u16,
    pub scan: u16,
    pub extended: bool,
    pub vk: u16,
}

const fn key(hid: u8, mac: u16, scan: u16, extended: bool, vk: u16) -> KeyDef {
    KeyDef {
        hid,
        mac,
        scan,
        extended,
        vk,
    }
}

/// Standard keyboard only: letters, digits, punctuation, controls, F1-F12,
/// navigation/arrows, Caps Lock, and left/right modifiers. No numpad or media
/// keys.
pub const KEY_DEFS: &[KeyDef] = &[
    key(0x04, 0x00, 0x1E, false, 0x41),      // A
    key(0x05, 0x0B, 0x30, false, 0x42),      // B
    key(0x06, 0x08, 0x2E, false, 0x43),      // C
    key(0x07, 0x02, 0x20, false, 0x44),      // D
    key(0x08, 0x0E, 0x12, false, 0x45),      // E
    key(0x09, 0x03, 0x21, false, 0x46),      // F
    key(0x0A, 0x05, 0x22, false, 0x47),      // G
    key(0x0B, 0x04, 0x23, false, 0x48),      // H
    key(0x0C, 0x22, 0x17, false, 0x49),      // I
    key(0x0D, 0x26, 0x24, false, 0x4A),      // J
    key(0x0E, 0x28, 0x25, false, 0x4B),      // K
    key(0x0F, 0x25, 0x26, false, 0x4C),      // L
    key(0x10, 0x2E, 0x32, false, 0x4D),      // M
    key(0x11, 0x2D, 0x31, false, 0x4E),      // N
    key(0x12, 0x1F, 0x18, false, 0x4F),      // O
    key(0x13, 0x23, 0x19, false, 0x50),      // P
    key(0x14, 0x0C, 0x10, false, 0x51),      // Q
    key(0x15, 0x0F, 0x13, false, 0x52),      // R
    key(0x16, 0x01, 0x1F, false, 0x53),      // S
    key(0x17, 0x11, 0x14, false, 0x54),      // T
    key(0x18, 0x20, 0x16, false, 0x55),      // U
    key(0x19, 0x09, 0x2F, false, 0x56),      // V
    key(0x1A, 0x0D, 0x11, false, 0x57),      // W
    key(0x1B, 0x07, 0x2D, false, 0x58),      // X
    key(0x1C, 0x10, 0x15, false, 0x59),      // Y
    key(0x1D, 0x06, 0x2C, false, 0x5A),      // Z
    key(0x1E, 0x12, 0x02, false, 0x31),      // 1
    key(0x1F, 0x13, 0x03, false, 0x32),      // 2
    key(0x20, 0x14, 0x04, false, 0x33),      // 3
    key(0x21, 0x15, 0x05, false, 0x34),      // 4
    key(0x22, 0x17, 0x06, false, 0x35),      // 5
    key(0x23, 0x16, 0x07, false, 0x36),      // 6
    key(0x24, 0x1A, 0x08, false, 0x37),      // 7
    key(0x25, 0x1C, 0x09, false, 0x38),      // 8
    key(0x26, 0x19, 0x0A, false, 0x39),      // 9
    key(0x27, 0x1D, 0x0B, false, 0x30),      // 0
    key(0x28, 0x24, 0x1C, false, 0x0D),      // Enter
    key(0x29, 0x35, 0x01, false, 0x1B),      // Escape
    key(0x2A, 0x33, 0x0E, false, 0x08),      // Backspace
    key(0x2B, 0x30, 0x0F, false, 0x09),      // Tab
    key(0x2C, 0x31, 0x39, false, 0x20),      // Space
    key(0x2D, 0x1B, 0x0C, false, 0xBD),      // Minus
    key(0x2E, 0x18, 0x0D, false, 0xBB),      // Equal
    key(0x2F, 0x21, 0x1A, false, 0xDB),      // Left bracket
    key(0x30, 0x1E, 0x1B, false, 0xDD),      // Right bracket
    key(0x31, 0x2A, 0x2B, false, 0xDC),      // Backslash
    key(0x33, 0x29, 0x27, false, 0xBA),      // Semicolon
    key(0x34, 0x27, 0x28, false, 0xDE),      // Quote
    key(0x35, 0x32, 0x29, false, 0xC0),      // Grave
    key(0x36, 0x2B, 0x33, false, 0xBC),      // Comma
    key(0x37, 0x2F, 0x34, false, 0xBE),      // Period
    key(0x38, 0x2C, 0x35, false, 0xBF),      // Slash
    key(CAPS_LOCK, 0x39, 0x3A, false, 0x14), // Caps Lock
    key(0x3A, 0x7A, 0x3B, false, 0x70),      // F1
    key(0x3B, 0x78, 0x3C, false, 0x71),      // F2
    key(0x3C, 0x63, 0x3D, false, 0x72),      // F3
    key(0x3D, 0x76, 0x3E, false, 0x73),      // F4
    key(0x3E, 0x60, 0x3F, false, 0x74),      // F5
    key(0x3F, 0x61, 0x40, false, 0x75),      // F6
    key(0x40, 0x62, 0x41, false, 0x76),      // F7
    key(0x41, 0x64, 0x42, false, 0x77),      // F8
    key(0x42, 0x65, 0x43, false, 0x78),      // F9
    key(0x43, 0x6D, 0x44, false, 0x79),      // F10
    key(0x44, 0x67, 0x57, false, 0x7A),      // F11
    key(0x45, 0x6F, 0x58, false, 0x7B),      // F12
    key(0x4A, 0x73, 0x47, true, 0x24),       // Home
    key(0x4B, 0x74, 0x49, true, 0x21),       // Page Up
    key(0x4C, 0x75, 0x53, true, 0x2E),       // Delete
    key(0x4D, 0x77, 0x4F, true, 0x23),       // End
    key(0x4E, 0x79, 0x51, true, 0x22),       // Page Down
    key(0x4F, 0x7C, 0x4D, true, 0x27),       // Right
    key(0x50, 0x7B, 0x4B, true, 0x25),       // Left
    key(0x51, 0x7D, 0x50, true, 0x28),       // Down
    key(0x52, 0x7E, 0x48, true, 0x26),       // Up
    key(0xE0, 0x3B, 0x1D, false, 0xA2),      // Left Control
    key(0xE1, 0x38, 0x2A, false, 0xA0),      // Left Shift
    key(0xE2, 0x3A, 0x38, false, 0xA4),      // Left Option/Alt
    key(0xE3, 0x37, 0x5B, true, 0x5B),       // Left GUI/Command
    key(0xE4, 0x3E, 0x1D, true, 0xA3),       // Right Control
    key(0xE5, 0x3C, 0x36, false, 0xA1),      // Right Shift
    key(0xE6, 0x3D, 0x38, true, 0xA5),       // Right Option/Alt
    key(0xE7, 0x36, 0x5C, true, 0x5C),       // Right GUI/Command
];

pub fn is_modifier(hid: u8) -> bool {
    (0xE0..=0xE7).contains(&hid)
}

pub fn is_caps(hid: u8) -> bool {
    hid == CAPS_LOCK
}

#[allow(dead_code)]
pub fn hid_to_mac(hid: u8) -> Option<u16> {
    KEY_DEFS
        .iter()
        .find(|def| def.hid == hid)
        .map(|def| def.mac)
}

#[allow(dead_code)]
pub fn mac_to_hid(mac: u16) -> Option<u8> {
    KEY_DEFS
        .iter()
        .find(|def| def.mac == mac)
        .map(|def| def.hid)
}

#[allow(dead_code)]
pub fn hid_to_scan(hid: u8) -> Option<(u16, bool)> {
    KEY_DEFS
        .iter()
        .find(|def| def.hid == hid)
        .map(|def| (def.scan, def.extended))
}

#[allow(dead_code)]
pub fn scan_to_hid(scan: u16, extended: bool) -> Option<u8> {
    KEY_DEFS
        .iter()
        .find(|def| def.scan == scan && def.extended == extended)
        .map(|def| def.hid)
}

/// Cross-system adaptation keeps the primary shortcut modifier natural:
/// Mac Command <-> Windows Control. Shift, Alt/Option, and Caps are unchanged.
pub fn adapt_modifier_hid(hid: u8) -> u8 {
    match hid {
        0xE0 => 0xE3,
        0xE3 => 0xE0,
        0xE4 => 0xE7,
        0xE7 => 0xE4,
        other => other,
    }
}

pub fn adapt_key_packet(packet: &mut KeyPacket) {
    let mut state = KeyState::default();
    for hid in packet.state.iter() {
        state.insert(adapt_modifier_hid(hid));
    }
    packet.state = state;
    if let KeyEvent::Repeat(hid) = packet.event {
        packet.event = KeyEvent::Repeat(adapt_modifier_hid(hid));
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct KeyState([u8; 32]);

impl KeyState {
    pub const BITS: usize = 32;

    pub fn contains(&self, hid: u8) -> bool {
        self.0[hid as usize / 8] & (1 << (hid % 8)) != 0
    }

    pub fn insert(&mut self, hid: u8) {
        self.0[hid as usize / 8] |= 1 << (hid % 8);
    }

    pub fn remove(&mut self, hid: u8) {
        self.0[hid as usize / 8] &= !(1 << (hid % 8));
    }

    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.0.iter().all(|byte| *byte == 0)
    }

    pub fn iter(&self) -> impl Iterator<Item = u8> + '_ {
        (0u16..=255)
            .map(|value| value as u8)
            .filter(|hid| self.contains(*hid))
    }

    pub fn bits(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn from_bits(bits: [u8; 32]) -> Self {
        Self(bits)
    }
}

impl fmt::Debug for KeyState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyEvent {
    None,
    Repeat(u8),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyPacket {
    pub active: bool,
    pub caps_lock: bool,
    pub state: KeyState,
    pub event: KeyEvent,
}

impl KeyPacket {
    pub const INACTIVE: Self = Self {
        active: false,
        caps_lock: false,
        state: KeyState([0u8; 32]),
        event: KeyEvent::None,
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_round_trips_and_has_unique_identities() {
        let mut macs = Vec::new();
        let mut scans = Vec::new();
        let mut hids = Vec::new();
        let mut vks = Vec::new();
        for def in KEY_DEFS {
            assert_eq!(hid_to_mac(def.hid), Some(def.mac));
            assert_eq!(mac_to_hid(def.mac), Some(def.hid));
            assert_eq!(hid_to_scan(def.hid), Some((def.scan, def.extended)));
            assert_eq!(scan_to_hid(def.scan, def.extended), Some(def.hid));
            assert!(!hids.contains(&def.hid), "duplicate HID {:#x}", def.hid);
            assert!(!macs.contains(&def.mac), "duplicate mac {:#x}", def.mac);
            assert!(
                !scans.contains(&(def.scan, def.extended)),
                "duplicate scan {:#x}/{}",
                def.scan,
                def.extended
            );
            assert!(!vks.contains(&def.vk), "duplicate VK {:#x}", def.vk);
            hids.push(def.hid);
            macs.push(def.mac);
            scans.push((def.scan, def.extended));
            vks.push(def.vk);
        }
    }

    #[test]
    fn modifier_adaptation_swaps_command_and_control_only() {
        assert_eq!(adapt_modifier_hid(0xE0), 0xE3);
        assert_eq!(adapt_modifier_hid(0xE3), 0xE0);
        assert_eq!(adapt_modifier_hid(0xE4), 0xE7);
        assert_eq!(adapt_modifier_hid(0xE7), 0xE4);
        for unchanged in [0xE1, 0xE2, 0xE5, 0xE6, 0x04, 0x39] {
            assert_eq!(adapt_modifier_hid(unchanged), unchanged);
        }

        let mut state = KeyState::default();
        for hid in [0xE0, 0xE3, 0xE4, 0xE7, 0xE1, 0x04] {
            state.insert(hid);
        }
        let mut packet = KeyPacket {
            active: true,
            caps_lock: true,
            state,
            event: KeyEvent::Repeat(0xE3),
        };
        adapt_key_packet(&mut packet);
        for hid in [0xE3, 0xE0, 0xE7, 0xE4, 0xE1, 0x04] {
            assert!(packet.state.contains(hid), "missing {hid:#x}");
        }
        assert_eq!(packet.event, KeyEvent::Repeat(0xE0));
        assert!(packet.active);
        assert!(packet.caps_lock);
    }

    #[test]
    fn key_state_insert_remove_and_iterate() {
        let mut state = KeyState::default();
        assert!(state.is_empty());
        state.insert(0x04);
        state.insert(0xE3);
        assert!(state.contains(0x04));
        assert!(state.contains(0xE3));
        assert_eq!(state.iter().collect::<Vec<_>>(), vec![0x04, 0xE3]);
        state.remove(0x04);
        assert!(!state.contains(0x04));
        assert_eq!(state.iter().collect::<Vec<_>>(), vec![0xE3]);
    }

    #[test]
    fn known_modifier_mappings_match_platform_tables() {
        assert_eq!(hid_to_mac(0xE3), Some(0x37)); // Command
        assert_eq!(hid_to_mac(0xE0), Some(0x3B)); // Control
        assert_eq!(hid_to_scan(0xE7), Some((0x5C, true))); // Right GUI
        assert_eq!(hid_to_scan(0xE0), Some((0x1D, false))); // Left Control
        assert_eq!(mac_to_hid(0x36), Some(0xE7)); // Right Command
        assert_eq!(scan_to_hid(0x1C, false), Some(0x28)); // Enter
        assert_eq!(hid_to_mac(CAPS_LOCK), Some(0x39));
    }
}
