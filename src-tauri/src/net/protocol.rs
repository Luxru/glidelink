use crate::input::keymap::{KeyEvent, KeyPacket, KeyState};
use crate::input::RemoteInput;
use crate::status::ScreenInfo;
use chacha20poly1305::aead::AeadInPlace;
use chacha20poly1305::{ChaCha20Poly1305, Key, KeyInit, Nonce, Tag};
use hmac::{Hmac, Mac};
use rand::RngCore;
use sha2::{Digest, Sha256};
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;

pub const MAGIC: &[u8; 4] = b"GLNK";
pub const VERSION: u8 = 2;

pub const TYPE_HELLO: u8 = 1;
pub const TYPE_ACCEPT: u8 = 2;
pub const TYPE_INPUT: u8 = 3;
pub const TYPE_PING: u8 = 4;
pub const TYPE_PONG: u8 = 5;
pub const TYPE_BYE: u8 = 6;
pub const TYPE_CONFIRM: u8 = 7;
pub const TYPE_CONFIRM_ACK: u8 = 8;
pub const TYPE_KEY: u8 = 9;

pub const SID_LEN: usize = 8;
pub const TAG_LEN: usize = 16;
pub const HEADER_LEN: usize = 4 + 1 + 1 + SID_LEN + 8;
pub const PAKE_MSG_LEN: usize = 33;
pub const HELLO_LEN: usize = HEADER_LEN + 4 + 4 + 8 + 1 + PAKE_MSG_LEN;
pub const ACCEPT_LEN: usize = HELLO_LEN;
pub const INPUT_LEN: usize = 8;
pub const KEY_LEN: usize = 34;
pub const PONG_STATS_LEN: usize = 24;
pub const CONFIRM_PAYLOAD: &[u8] = b"GlideLink/v2/confirm";

const KEY_FLAG_ACTIVE: u8 = 1 << 0;
const KEY_FLAG_CAPS_LOCK: u8 = 1 << 1;
const KEY_EVENT_SHIFT: u8 = 2;
const KEY_EVENT_MASK: u8 = 0b11 << KEY_EVENT_SHIFT;
const KEY_EVENT_STATE: u8 = 0;
const KEY_EVENT_REPEAT: u8 = 1;

pub const PLATFORM_UNKNOWN: u8 = 0;
#[allow(dead_code)]
pub const PLATFORM_MACOS: u8 = 1;
#[allow(dead_code)]
pub const PLATFORM_WINDOWS: u8 = 2;

const C2A_LABEL: &[u8] = b"GlideLink/v2/controller-to-agent";
const A2C_LABEL: &[u8] = b"GlideLink/v2/agent-to-controller";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Side {
    Controller,
    Agent,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Handshake {
    pub sid: [u8; SID_LEN],
    pub screen: ScreenInfo,
    pub timestamp_ms: u64,
    pub platform: u8,
}

#[derive(Debug, Error)]
pub enum ProtocolError {
    #[error("malformed packet")]
    Malformed,
    #[error("packet authentication failed")]
    Authentication,
    #[error("session id mismatch")]
    SessionMismatch,
    #[error("replayed packet")]
    Replay,
}

pub fn random_sid() -> [u8; SID_LEN] {
    let mut sid = [0u8; SID_LEN];
    rand::rngs::OsRng.fill_bytes(&mut sid);
    sid
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Hash the full HELLO/ACCEPT exchange so the session keys are bound to both
/// PAKE messages, the session id, and the advertised screens.
pub fn transcript_hash(hello: &[u8], accept: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(hello);
    hasher.update(accept);
    hasher.finalize().into()
}

fn directional_key(
    shared: &[u8],
    sid: &[u8; SID_LEN],
    transcript: &[u8; 32],
    label: &[u8],
) -> [u8; 32] {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(shared).expect("HMAC accepts any key size");
    mac.update(label);
    mac.update(sid);
    mac.update(transcript);
    let bytes = mac.finalize().into_bytes();
    let mut key = [0u8; 32];
    key.copy_from_slice(&bytes);
    key
}

pub fn derive_session_keys(
    shared: &[u8],
    sid: &[u8; SID_LEN],
    transcript: &[u8; 32],
) -> ([u8; 32], [u8; 32]) {
    (
        directional_key(shared, sid, transcript, C2A_LABEL),
        directional_key(shared, sid, transcript, A2C_LABEL),
    )
}

fn header_prefix(kind: u8, sid: &[u8; SID_LEN], len: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(len);
    out.extend_from_slice(MAGIC);
    out.push(VERSION);
    out.push(kind);
    out.extend_from_slice(sid);
    out.extend_from_slice(&0u64.to_be_bytes());
    debug_assert_eq!(out.len(), HEADER_LEN);
    out
}

fn validate_prefix(
    packet: &[u8],
    expected_len: Option<usize>,
) -> Result<(u8, [u8; SID_LEN]), ProtocolError> {
    if packet.len() < 6 || &packet[0..4] != MAGIC || packet[4] != VERSION {
        return Err(ProtocolError::Malformed);
    }
    if let Some(len) = expected_len {
        if packet.len() != len {
            return Err(ProtocolError::Malformed);
        }
    }
    let kind = packet[5];
    let mut sid = [0u8; SID_LEN];
    sid.copy_from_slice(&packet[6..14]);
    Ok((kind, sid))
}

fn build_pake_packet(kind: u8, handshake: &Handshake, pake_msg: &[u8]) -> Vec<u8> {
    debug_assert_eq!(pake_msg.len(), PAKE_MSG_LEN);
    let mut out = header_prefix(kind, &handshake.sid, HELLO_LEN);
    out.extend_from_slice(&handshake.screen.width.to_le_bytes());
    out.extend_from_slice(&handshake.screen.height.to_le_bytes());
    out.extend_from_slice(&handshake.timestamp_ms.to_le_bytes());
    out.push(handshake.platform);
    out.extend_from_slice(pake_msg);
    debug_assert_eq!(out.len(), HELLO_LEN);
    out
}

fn parse_pake_packet(packet: &[u8], kind: u8) -> Result<(Handshake, Vec<u8>), ProtocolError> {
    let (actual_kind, sid) = validate_prefix(packet, Some(HELLO_LEN))?;
    if actual_kind != kind {
        return Err(ProtocolError::Malformed);
    }
    let width = u32::from_le_bytes(packet[HEADER_LEN..HEADER_LEN + 4].try_into().unwrap());
    let height = u32::from_le_bytes(packet[HEADER_LEN + 4..HEADER_LEN + 8].try_into().unwrap());
    let timestamp_ms =
        u64::from_le_bytes(packet[HEADER_LEN + 8..HEADER_LEN + 16].try_into().unwrap());
    let platform = packet[HEADER_LEN + 16];
    Ok((
        Handshake {
            sid,
            screen: ScreenInfo { width, height },
            timestamp_ms,
            platform,
        },
        packet[HEADER_LEN + 17..].to_vec(),
    ))
}

pub fn build_pake_hello(hello: &Handshake, pake_msg: &[u8]) -> Vec<u8> {
    build_pake_packet(TYPE_HELLO, hello, pake_msg)
}

pub fn build_pake_accept(accept: &Handshake, pake_msg: &[u8]) -> Vec<u8> {
    build_pake_packet(TYPE_ACCEPT, accept, pake_msg)
}

pub fn hello_sid(packet: &[u8]) -> Result<[u8; SID_LEN], ProtocolError> {
    let (kind, sid) = validate_prefix(packet, Some(HELLO_LEN))?;
    if kind != TYPE_HELLO {
        return Err(ProtocolError::Malformed);
    }
    Ok(sid)
}

pub fn parse_pake_hello(packet: &[u8]) -> Result<(Handshake, Vec<u8>), ProtocolError> {
    parse_pake_packet(packet, TYPE_HELLO)
}

pub fn parse_pake_accept(
    packet: &[u8],
    expected_sid: &[u8; SID_LEN],
) -> Result<(Handshake, Vec<u8>), ProtocolError> {
    let (accept, pake_msg) = parse_pake_packet(packet, TYPE_ACCEPT)?;
    if &accept.sid != expected_sid {
        return Err(ProtocolError::SessionMismatch);
    }
    Ok((accept, pake_msg))
}

#[derive(Clone, Debug, Default)]
pub struct ReplayWindow {
    highest: u64,
    bitmap: u64,
}

impl ReplayWindow {
    pub fn accept(&mut self, seq: u64) -> bool {
        if seq == 0 {
            return false;
        }
        if seq > self.highest {
            let shift = seq - self.highest;
            self.bitmap = if shift >= 64 { 0 } else { self.bitmap << shift };
            self.bitmap |= 1;
            self.highest = seq;
            true
        } else {
            let diff = self.highest - seq;
            if diff >= 64 {
                return false;
            }
            let bit = 1u64 << diff;
            if self.bitmap & bit != 0 {
                false
            } else {
                self.bitmap |= bit;
                true
            }
        }
    }
}

pub struct Session {
    sid: [u8; SID_LEN],
    tx: ChaCha20Poly1305,
    rx: ChaCha20Poly1305,
    tx_seq: u64,
    replay: ReplayWindow,
}

impl Session {
    pub fn from_keys(
        controller_key: &[u8; 32],
        agent_key: &[u8; 32],
        sid: [u8; SID_LEN],
        side: Side,
    ) -> Self {
        let (tx_key, rx_key) = match side {
            Side::Controller => (controller_key, agent_key),
            Side::Agent => (agent_key, controller_key),
        };
        Self {
            sid,
            tx: ChaCha20Poly1305::new(Key::from_slice(tx_key)),
            rx: ChaCha20Poly1305::new(Key::from_slice(rx_key)),
            tx_seq: 0,
            replay: ReplayWindow::default(),
        }
    }

    fn nonce_sid(&self) -> [u8; 4] {
        let mut prefix = [0u8; 4];
        prefix.copy_from_slice(&self.sid[..4]);
        prefix
    }

    fn nonce(&self, seq: u64) -> [u8; 12] {
        let mut nonce = [0u8; 12];
        nonce[..4].copy_from_slice(&self.nonce_sid());
        nonce[4..].copy_from_slice(&seq.to_be_bytes());
        nonce
    }

    pub fn encrypt_packet(&mut self, kind: u8, payload: &[u8]) -> Vec<u8> {
        self.tx_seq += 1;
        let seq = self.tx_seq;
        let mut out = Vec::with_capacity(HEADER_LEN + payload.len() + TAG_LEN);
        out.extend_from_slice(MAGIC);
        out.push(VERSION);
        out.push(kind);
        out.extend_from_slice(&self.sid);
        out.extend_from_slice(&seq.to_be_bytes());
        let header_len = out.len();
        debug_assert_eq!(header_len, HEADER_LEN);
        out.extend_from_slice(payload);

        let nonce = self.nonce(seq);
        let (header, body) = out.split_at_mut(header_len);
        let tag = self
            .tx
            .encrypt_in_place_detached(Nonce::from_slice(&nonce), header, body)
            .expect("in-place encryption cannot fail");
        out.extend_from_slice(tag.as_slice());
        out
    }

    pub fn decrypt_packet(&mut self, packet: &[u8]) -> Result<(u8, Vec<u8>), ProtocolError> {
        self.decrypt_packet_with_seq(packet)
            .map(|(kind, payload, _)| (kind, payload))
    }

    /// Like `decrypt_packet`, but also returns the accepted packet's own
    /// sequence number for RFC 3550-style loss accounting.
    pub fn decrypt_packet_with_seq(
        &mut self,
        packet: &[u8],
    ) -> Result<(u8, Vec<u8>, u64), ProtocolError> {
        if packet.len() < HEADER_LEN + TAG_LEN {
            return Err(ProtocolError::Malformed);
        }
        if &packet[0..4] != MAGIC || packet[4] != VERSION {
            return Err(ProtocolError::Malformed);
        }
        let kind = packet[5];
        if &packet[6..14] != self.sid {
            return Err(ProtocolError::SessionMismatch);
        }
        let seq = u64::from_be_bytes(packet[14..22].try_into().unwrap());
        if seq == 0 {
            return Err(ProtocolError::Malformed);
        }

        let cipher_len = packet.len() - HEADER_LEN;
        let payload_len = cipher_len - TAG_LEN;
        let mut payload = packet[HEADER_LEN..HEADER_LEN + payload_len].to_vec();
        let tag = Tag::from_slice(&packet[HEADER_LEN + payload_len..]);
        let nonce = self.nonce(seq);
        self.rx
            .decrypt_in_place_detached(
                Nonce::from_slice(&nonce),
                &packet[..HEADER_LEN],
                &mut payload,
                tag,
            )
            .map_err(|_| ProtocolError::Authentication)?;

        if !self.replay.accept(seq) {
            return Err(ProtocolError::Replay);
        }
        Ok((kind, payload, seq))
    }
}

pub fn encode_input(input: &RemoteInput) -> [u8; INPUT_LEN] {
    let x = (input.x.clamp(0.0, 1.0) * u16::MAX as f32).round() as u16;
    let y = (input.y.clamp(0.0, 1.0) * u16::MAX as f32).round() as u16;
    let mut out = [0u8; INPUT_LEN];
    out[0..2].copy_from_slice(&x.to_le_bytes());
    out[2..4].copy_from_slice(&y.to_le_bytes());
    out[4] = input.buttons;
    out[5] = 0;
    out[6..8].copy_from_slice(&input.wheel.to_le_bytes());
    out
}

pub fn decode_input(payload: &[u8]) -> Result<RemoteInput, ProtocolError> {
    if payload.len() != INPUT_LEN {
        return Err(ProtocolError::Malformed);
    }
    let x = u16::from_le_bytes(payload[0..2].try_into().unwrap()) as f32 / u16::MAX as f32;
    let y = u16::from_le_bytes(payload[2..4].try_into().unwrap()) as f32 / u16::MAX as f32;
    let buttons = payload[4];
    let wheel = i16::from_le_bytes(payload[6..8].try_into().unwrap());
    Ok(RemoteInput {
        x,
        y,
        buttons,
        wheel,
    })
}

pub fn encode_key(packet: &KeyPacket) -> [u8; KEY_LEN] {
    let mut out = [0u8; KEY_LEN];
    out[..KeyState::BITS].copy_from_slice(packet.state.bits());
    let mut flags = 0u8;
    if packet.active {
        flags |= KEY_FLAG_ACTIVE;
    }
    if packet.caps_lock {
        flags |= KEY_FLAG_CAPS_LOCK;
    }
    if let KeyEvent::Repeat(hid) = packet.event {
        flags |= KEY_EVENT_REPEAT << KEY_EVENT_SHIFT;
        out[33] = hid;
    }
    out[32] = flags;
    out
}

pub fn decode_key(payload: &[u8]) -> Result<KeyPacket, ProtocolError> {
    if payload.len() != KEY_LEN {
        return Err(ProtocolError::Malformed);
    }
    let flags = payload[32];
    if flags & 0b1111_0000 != 0 {
        return Err(ProtocolError::Malformed);
    }
    let mut bits = [0u8; KeyState::BITS];
    bits.copy_from_slice(&payload[..KeyState::BITS]);
    let event = match (flags & KEY_EVENT_MASK) >> KEY_EVENT_SHIFT {
        KEY_EVENT_STATE => {
            if payload[33] != 0 {
                return Err(ProtocolError::Malformed);
            }
            KeyEvent::None
        }
        KEY_EVENT_REPEAT => {
            if payload[33] == 0 {
                return Err(ProtocolError::Malformed);
            }
            KeyEvent::Repeat(payload[33])
        }
        _ => return Err(ProtocolError::Malformed),
    };
    Ok(KeyPacket {
        active: flags & KEY_FLAG_ACTIVE != 0,
        caps_lock: flags & KEY_FLAG_CAPS_LOCK != 0,
        state: KeyState::from_bits(bits),
        event,
    })
}

pub fn encode_u64(value: u64) -> [u8; 8] {
    value.to_le_bytes()
}

pub fn decode_u64(payload: &[u8]) -> Result<u64, ProtocolError> {
    if payload.len() != 8 {
        return Err(ProtocolError::Malformed);
    }
    Ok(u64::from_le_bytes(payload.try_into().unwrap()))
}

/// PONG carries the PING id plus the agent's receiver counts for the
/// controller-to-agent direction. Apps must treat liveness separately from
/// these optional statistics.
pub fn encode_pong(id: u64, expected: u64, received: u64) -> [u8; PONG_STATS_LEN] {
    let mut out = [0u8; PONG_STATS_LEN];
    out[0..8].copy_from_slice(&id.to_le_bytes());
    out[8..16].copy_from_slice(&expected.to_le_bytes());
    out[16..24].copy_from_slice(&received.to_le_bytes());
    out
}

/// Returns the PING id and, when present, `(expected, received)` counters.
/// An 8-byte legacy payload, or any payload shorter than the full stats
/// extension, is accepted without statistics.
pub fn decode_pong(payload: &[u8]) -> Result<(u64, Option<(u64, u64)>), ProtocolError> {
    if payload.len() < 8 {
        return Err(ProtocolError::Malformed);
    }
    let id = u64::from_le_bytes(payload[0..8].try_into().unwrap());
    if payload.len() < PONG_STATS_LEN {
        return Ok((id, None));
    }
    let expected = u64::from_le_bytes(payload[8..16].try_into().unwrap());
    let received = u64::from_le_bytes(payload[16..24].try_into().unwrap());
    Ok((id, Some((expected, received))))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(w: u32, h: u32) -> ScreenInfo {
        ScreenInfo {
            width: w,
            height: h,
        }
    }

    #[test]
    fn pake_handshake_messages_and_session_keys() {
        let sid = random_sid();
        let hello = Handshake {
            sid,
            screen: screen(2560, 1440),
            timestamp_ms: now_ms(),
            platform: PLATFORM_MACOS,
        };
        let accept = Handshake {
            sid,
            screen: screen(1920, 1080),
            timestamp_ms: now_ms(),
            platform: PLATFORM_WINDOWS,
        };
        let hello_packet = build_pake_hello(&hello, &[0x41; PAKE_MSG_LEN]);
        let accept_packet = build_pake_accept(&accept, &[0x42; PAKE_MSG_LEN]);
        assert_eq!(hello_packet.len(), HELLO_LEN);
        assert_eq!(accept_packet.len(), ACCEPT_LEN);
        assert_eq!(hello_sid(&hello_packet).unwrap(), sid);

        let (parsed_hello, hello_pake) = parse_pake_hello(&hello_packet).unwrap();
        assert_eq!(parsed_hello, hello);
        assert_eq!(hello_pake, vec![0x41; PAKE_MSG_LEN]);
        let (parsed_accept, accept_pake) = parse_pake_accept(&accept_packet, &sid).unwrap();
        assert_eq!(parsed_accept, accept);
        assert_eq!(accept_pake, vec![0x42; PAKE_MSG_LEN]);

        let transcript = transcript_hash(&hello_packet, &accept_packet);
        let (c2a, a2c) = derive_session_keys(&[7u8; 32], &sid, &transcript);
        let (c2a_other, a2c_other) = derive_session_keys(&[7u8; 32], &sid, &transcript);
        assert_eq!(c2a, c2a_other);
        assert_eq!(a2c, a2c_other);
        assert_ne!(c2a, a2c);
    }

    #[test]
    fn encrypted_input_round_trip_and_replay_rejection() {
        let sid = random_sid();
        let mut controller = Session::from_keys(&[7u8; 32], &[9u8; 32], sid, Side::Controller);
        let mut agent = Session::from_keys(&[7u8; 32], &[9u8; 32], sid, Side::Agent);

        let input = RemoteInput {
            x: 0.123,
            y: 0.987,
            buttons: 0b0000_0101,
            wheel: -2,
        };
        let packet = controller.encrypt_packet(TYPE_INPUT, &encode_input(&input));
        let (kind, payload, seq) = agent.decrypt_packet_with_seq(&packet).unwrap();
        assert_eq!(kind, TYPE_INPUT);
        assert_eq!(seq, 1);
        let decoded = decode_input(&payload).unwrap();
        assert!((decoded.x - input.x).abs() < 0.001);
        assert!((decoded.y - input.y).abs() < 0.001);
        assert_eq!(decoded.buttons, input.buttons);
        assert_eq!(decoded.wheel, input.wheel);

        assert!(matches!(
            agent.decrypt_packet(&packet),
            Err(ProtocolError::Replay)
        ));

        let mut wrong = Session::from_keys(&[8u8; 32], &[9u8; 32], sid, Side::Agent);
        let second = controller.encrypt_packet(TYPE_INPUT, &encode_input(&input));
        assert!(matches!(
            wrong.decrypt_packet(&second),
            Err(ProtocolError::Authentication)
        ));
    }

    #[test]
    fn replay_window_handles_reorder_and_old_packets() {
        let mut w = ReplayWindow::default();
        assert!(w.accept(1));
        assert!(w.accept(3));
        assert!(w.accept(2));
        assert!(!w.accept(2));
        assert!(!w.accept(1));
        assert!(w.accept(100));
        assert!(!w.accept(30));
        assert!(w.accept(99));
        assert!(!w.accept(0));
    }

    #[test]
    fn ping_payload_round_trip() {
        let v = 0xdead_beef_cafe_u64;
        assert_eq!(decode_u64(&encode_u64(v)).unwrap(), v);
        assert!(decode_u64(&[0u8; 7]).is_err());
    }

    #[test]
    fn key_packet_round_trip_and_validation() {
        let mut state = KeyState::default();
        for hid in [0x04, 0xE3, 0xE1] {
            state.insert(hid);
        }
        let packet = KeyPacket {
            active: true,
            caps_lock: true,
            state,
            event: KeyEvent::Repeat(0x04),
        };
        let encoded = encode_key(&packet);
        assert_eq!(encoded.len(), KEY_LEN);
        assert_eq!(decode_key(&encoded).unwrap(), packet);

        let inactive = KeyPacket {
            active: false,
            caps_lock: false,
            state: KeyState::default(),
            event: KeyEvent::None,
        };
        assert_eq!(decode_key(&encode_key(&inactive)).unwrap(), inactive);

        assert!(decode_key(&[]).is_err());
        assert!(decode_key(&[0u8; KEY_LEN - 1]).is_err());

        let mut reserved = encoded;
        reserved[32] |= 0b1000_0000;
        assert!(decode_key(&reserved).is_err());

        let mut bad_event = encoded;
        bad_event[32] = KEY_EVENT_REPEAT << KEY_EVENT_SHIFT;
        bad_event[33] = 0;
        assert!(decode_key(&bad_event).is_err());

        let mut state_with_key = inactive;
        state_with_key.event = KeyEvent::Repeat(0x04);
        let mut encoded_state = encode_key(&state_with_key);
        encoded_state[32] = 0;
        encoded_state[33] = 0x04;
        assert!(decode_key(&encoded_state).is_err());
    }

    #[test]
    fn pong_stats_round_trip_and_legacy_length() {
        let stats = decode_pong(&encode_pong(42, 100, 97)).unwrap();
        assert_eq!(stats, (42, Some((100, 97))));

        let legacy = decode_pong(&42u64.to_le_bytes()).unwrap();
        assert_eq!(legacy, (42, None));

        assert!(decode_pong(&[0u8; 7]).is_err());

        let mut extended = encode_pong(7, 8, 9).to_vec();
        extended.push(0xff);
        assert_eq!(decode_pong(&extended).unwrap(), (7, Some((8, 9))));
    }

    #[test]
    fn decrypt_with_seq_reports_accepted_order() {
        let sid = random_sid();
        let mut controller = Session::from_keys(&[7u8; 32], &[9u8; 32], sid, Side::Controller);
        let mut agent = Session::from_keys(&[7u8; 32], &[9u8; 32], sid, Side::Agent);
        let payload = encode_input(&RemoteInput::default());
        let first = controller.encrypt_packet(TYPE_INPUT, &payload);
        let second = controller.encrypt_packet(TYPE_INPUT, &payload);
        let third = controller.encrypt_packet(TYPE_INPUT, &payload);

        assert_eq!(agent.decrypt_packet_with_seq(&first).unwrap().2, 1);
        assert_eq!(agent.decrypt_packet_with_seq(&third).unwrap().2, 3);
        assert_eq!(agent.decrypt_packet_with_seq(&second).unwrap().2, 2);
        assert!(matches!(
            agent.decrypt_packet_with_seq(&second),
            Err(ProtocolError::Replay)
        ));
    }

    #[test]
    fn wrong_pairing_code_derives_different_pake_keys() {
        use spake2::{Ed25519Group, Identity, Password, Spake2};

        let (controller, controller_msg) = Spake2::<Ed25519Group>::start_a(
            &Password::new(b"AAAAAA"),
            &Identity::new(b"controller"),
            &Identity::new(b"agent"),
        );
        let (agent, agent_msg) = Spake2::<Ed25519Group>::start_b(
            &Password::new(b"BBBBBB"),
            &Identity::new(b"controller"),
            &Identity::new(b"agent"),
        );
        let controller_key = controller.finish(&agent_msg).unwrap();
        let agent_key = agent.finish(&controller_msg).unwrap();
        assert_ne!(controller_key, agent_key);
    }
}
