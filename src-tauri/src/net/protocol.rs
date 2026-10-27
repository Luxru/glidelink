use crate::input::RemoteInput;
use crate::status::ScreenInfo;
use chacha20poly1305::aead::AeadInPlace;
use chacha20poly1305::{ChaCha20Poly1305, Key, KeyInit, Nonce, Tag};
use hmac::{Hmac, Mac};
use pbkdf2::pbkdf2_hmac;
use rand::RngCore;
use sha2::Sha256;
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;

pub const MAGIC: &[u8; 4] = b"GLNK";
pub const VERSION: u8 = 1;

pub const TYPE_HELLO: u8 = 1;
pub const TYPE_ACCEPT: u8 = 2;
pub const TYPE_INPUT: u8 = 3;
pub const TYPE_PING: u8 = 4;
pub const TYPE_PONG: u8 = 5;
pub const TYPE_BYE: u8 = 6;

pub const SID_LEN: usize = 8;
pub const TAG_LEN: usize = 16;
pub const HEADER_LEN: usize = 4 + 1 + 1 + SID_LEN + 8;
pub const HELLO_LEN: usize = 4 + 1 + 1 + SID_LEN + 4 + 4 + 8 + 32;
pub const ACCEPT_LEN: usize = 4 + 1 + 1 + SID_LEN + 4 + 4 + 8 + 32;
pub const INPUT_LEN: usize = 8;

const KDF_ROUNDS: u32 = 100_000;
const C2A_LABEL: &[u8] = b"GlideLink/v1/controller-to-agent";
const A2C_LABEL: &[u8] = b"GlideLink/v1/agent-to-controller";

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

/// PBKDF2 keeps an offline guess expensive. Online guessing is additionally
/// bounded by the handshake rate and packet authentication failures.
pub fn derive_master(passcode: &str, sid: &[u8; SID_LEN]) -> [u8; 32] {
    let mut salt = Vec::with_capacity(24 + SID_LEN);
    salt.extend_from_slice(b"GlideLink/v1/pairing/");
    salt.extend_from_slice(sid);
    let mut out = [0u8; 32];
    pbkdf2_hmac::<Sha256>(passcode.as_bytes(), &salt, KDF_ROUNDS, &mut out);
    out
}

fn directional_key(master: &[u8; 32], sid: &[u8; SID_LEN], label: &[u8]) -> [u8; 32] {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(master).expect("HMAC accepts any key size");
    mac.update(label);
    mac.update(sid);
    let bytes = mac.finalize().into_bytes();
    let mut key = [0u8; 32];
    key.copy_from_slice(&bytes);
    key
}

fn handshake_mac(master: &[u8; 32], data: &[u8]) -> [u8; 32] {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(master).expect("HMAC accepts any key size");
    mac.update(data);
    let bytes = mac.finalize().into_bytes();
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    out
}

fn verify_handshake_mac(master: &[u8; 32], data: &[u8], tag: &[u8]) -> Result<(), ProtocolError> {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(master).expect("HMAC accepts any key size");
    mac.update(data);
    mac.verify_slice(tag)
        .map_err(|_| ProtocolError::Authentication)
}

fn header_prefix(kind: u8, sid: &[u8; SID_LEN], len: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(len);
    out.extend_from_slice(MAGIC);
    out.push(VERSION);
    out.push(kind);
    out.extend_from_slice(sid);
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

pub fn build_hello(master: &[u8; 32], hello: &Handshake) -> Vec<u8> {
    let mut out = header_prefix(TYPE_HELLO, &hello.sid, HELLO_LEN);
    out.extend_from_slice(&hello.screen.width.to_le_bytes());
    out.extend_from_slice(&hello.screen.height.to_le_bytes());
    out.extend_from_slice(&hello.timestamp_ms.to_le_bytes());
    let mac = handshake_mac(master, &out);
    out.extend_from_slice(&mac);
    debug_assert_eq!(out.len(), HELLO_LEN);
    out
}

pub fn hello_sid(packet: &[u8]) -> Result<[u8; SID_LEN], ProtocolError> {
    let (kind, sid) = validate_prefix(packet, Some(HELLO_LEN))?;
    if kind != TYPE_HELLO {
        return Err(ProtocolError::Malformed);
    }
    Ok(sid)
}

pub fn parse_hello(packet: &[u8], master: &[u8; 32]) -> Result<Handshake, ProtocolError> {
    let (kind, sid) = validate_prefix(packet, Some(HELLO_LEN))?;
    if kind != TYPE_HELLO {
        return Err(ProtocolError::Malformed);
    }
    let mac_at = HELLO_LEN - 32;
    verify_handshake_mac(master, &packet[..mac_at], &packet[mac_at..])?;
    let width = u32::from_le_bytes(packet[14..18].try_into().unwrap());
    let height = u32::from_le_bytes(packet[18..22].try_into().unwrap());
    let timestamp_ms = u64::from_le_bytes(packet[22..30].try_into().unwrap());
    Ok(Handshake {
        sid,
        screen: ScreenInfo { width, height },
        timestamp_ms,
    })
}

pub fn build_accept(master: &[u8; 32], accept: &Handshake) -> Vec<u8> {
    let mut out = header_prefix(TYPE_ACCEPT, &accept.sid, ACCEPT_LEN);
    out.extend_from_slice(&accept.screen.width.to_le_bytes());
    out.extend_from_slice(&accept.screen.height.to_le_bytes());
    out.extend_from_slice(&accept.timestamp_ms.to_le_bytes());
    let mac = handshake_mac(master, &out);
    out.extend_from_slice(&mac);
    debug_assert_eq!(out.len(), ACCEPT_LEN);
    out
}

pub fn parse_accept(
    packet: &[u8],
    master: &[u8; 32],
    expected_sid: &[u8; SID_LEN],
) -> Result<Handshake, ProtocolError> {
    let (kind, sid) = validate_prefix(packet, Some(ACCEPT_LEN))?;
    if kind != TYPE_ACCEPT {
        return Err(ProtocolError::Malformed);
    }
    if &sid != expected_sid {
        return Err(ProtocolError::SessionMismatch);
    }
    let mac_at = ACCEPT_LEN - 32;
    verify_handshake_mac(master, &packet[..mac_at], &packet[mac_at..])?;
    let width = u32::from_le_bytes(packet[14..18].try_into().unwrap());
    let height = u32::from_le_bytes(packet[18..22].try_into().unwrap());
    let timestamp_ms = u64::from_le_bytes(packet[22..30].try_into().unwrap());
    Ok(Handshake {
        sid,
        screen: ScreenInfo { width, height },
        timestamp_ms,
    })
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
    pub fn from_master(master: &[u8; 32], sid: [u8; SID_LEN], side: Side) -> Self {
        let controller_key = directional_key(master, &sid, C2A_LABEL);
        let agent_key = directional_key(master, &sid, A2C_LABEL);
        let (tx_key, rx_key) = match side {
            Side::Controller => (controller_key, agent_key),
            Side::Agent => (agent_key, controller_key),
        };
        Self {
            sid,
            tx: ChaCha20Poly1305::new(Key::from_slice(&tx_key)),
            rx: ChaCha20Poly1305::new(Key::from_slice(&rx_key)),
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
        Ok((kind, payload))
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

pub fn encode_u64(value: u64) -> [u8; 8] {
    value.to_le_bytes()
}

pub fn decode_u64(payload: &[u8]) -> Result<u64, ProtocolError> {
    if payload.len() != 8 {
        return Err(ProtocolError::Malformed);
    }
    Ok(u64::from_le_bytes(payload.try_into().unwrap()))
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
    fn handshake_authenticates_and_parses() {
        let code = "A7k2mQ9xLp4v";
        let sid = random_sid();
        let master = derive_master(code, &sid);
        let hello = Handshake {
            sid,
            screen: screen(2560, 1440),
            timestamp_ms: now_ms(),
        };
        let hello_packet = build_hello(&master, &hello);
        let agent_master = derive_master(code, &hello_sid(&hello_packet).unwrap());
        let parsed = parse_hello(&hello_packet, &agent_master).unwrap();
        assert_eq!(parsed, hello);

        let accept = Handshake {
            sid,
            screen: screen(1920, 1080),
            timestamp_ms: now_ms(),
        };
        let accept_packet = build_accept(&agent_master, &accept);
        let parsed_accept = parse_accept(&accept_packet, &master, &sid).unwrap();
        assert_eq!(parsed_accept, accept);

        let mut tampered = accept_packet;
        let last = tampered.len() - 1;
        tampered[last] ^= 1;
        assert!(parse_accept(&tampered, &master, &sid).is_err());
    }

    #[test]
    fn encrypted_input_round_trip_and_replay_rejection() {
        let master = [7u8; 32];
        let sid = random_sid();
        let mut controller = Session::from_master(&master, sid, Side::Controller);
        let mut agent = Session::from_master(&master, sid, Side::Agent);

        let input = RemoteInput {
            x: 0.123,
            y: 0.987,
            buttons: 0b0000_0101,
            wheel: -2,
        };
        let packet = controller.encrypt_packet(TYPE_INPUT, &encode_input(&input));
        let (kind, payload) = agent.decrypt_packet(&packet).unwrap();
        assert_eq!(kind, TYPE_INPUT);
        let decoded = decode_input(&payload).unwrap();
        assert!((decoded.x - input.x).abs() < 0.001);
        assert!((decoded.y - input.y).abs() < 0.001);
        assert_eq!(decoded.buttons, input.buttons);
        assert_eq!(decoded.wheel, input.wheel);

        assert!(matches!(
            agent.decrypt_packet(&packet),
            Err(ProtocolError::Replay)
        ));

        let wrong = Session::from_master(&[8u8; 32], sid, Side::Agent);
        let mut wrong = wrong;
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
    fn handshake_rejects_wrong_passcode() {
        let sid = random_sid();
        let hello = Handshake {
            sid,
            screen: screen(800, 600),
            timestamp_ms: now_ms(),
        };
        let packet = build_hello(&derive_master("the-right-code", &sid), &hello);
        let wrong = derive_master("the-wrong-code", &sid);
        assert!(matches!(
            parse_hello(&packet, &wrong),
            Err(ProtocolError::Authentication)
        ));
    }
}
