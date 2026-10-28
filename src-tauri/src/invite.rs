use rand::RngCore;
use std::net::Ipv4Addr;

/// Crockford base32: unambiguous digits and uppercase letters.
const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
pub const CODE_LEN: usize = 20;
const VERSION: u8 = 2;
pub const PORT_RANGE_START: u16 = 45_890;
pub const PORT_RANGE_END: u16 = 45_999;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Invite {
    pub ip: Ipv4Addr,
    pub port: u16,
    pub token: [u8; 4],
}

impl Invite {
    pub fn generate(ip: Ipv4Addr, port: u16) -> Self {
        let mut token = [0u8; 4];
        rand::rngs::OsRng.fill_bytes(&mut token);
        Self { ip, port, token }
    }

    pub fn encode(&self) -> String {
        let mut bytes = [0u8; 12];
        bytes[0] = VERSION;
        bytes[1..5].copy_from_slice(&self.ip.octets());
        bytes[5..7].copy_from_slice(&self.port.to_be_bytes());
        bytes[7..11].copy_from_slice(&self.token);
        bytes[11] = crc8(&bytes[..11]);
        encode_base32(&bytes)
    }

    pub fn decode(code: &str) -> Result<Self, String> {
        let code = normalize_code(code);
        if code.len() != CODE_LEN {
            return Err("E_PAIRING_CODE".into());
        }
        let bytes = decode_base32(&code).ok_or_else(|| "E_PAIRING_CODE".to_string())?;
        if bytes[0] != VERSION || crc8(&bytes[..11]) != bytes[11] {
            return Err("E_PAIRING_CODE".into());
        }
        let ip = Ipv4Addr::new(bytes[1], bytes[2], bytes[3], bytes[4]);
        let port = u16::from_be_bytes([bytes[5], bytes[6]]);
        if !(PORT_RANGE_START..=PORT_RANGE_END).contains(&port) {
            return Err("E_PAIRING_CODE".into());
        }
        Ok(Self {
            ip,
            port,
            token: [bytes[7], bytes[8], bytes[9], bytes[10]],
        })
    }
}

pub fn normalize_code(input: &str) -> String {
    input
        .chars()
        .filter_map(|ch| {
            let ch = ch.to_ascii_uppercase();
            match ch {
                '0' | 'O' => Some('0'),
                '1' | 'I' | 'L' => Some('1'),
                '2'..='9' | 'A'..='Z' => Some(ch),
                _ => None,
            }
        })
        .collect()
}

fn crc8(data: &[u8]) -> u8 {
    let mut crc = 0u8;
    for byte in data {
        crc ^= *byte;
        for _ in 0..8 {
            crc = if crc & 0x80 != 0 {
                (crc << 1) ^ 0x07
            } else {
                crc << 1
            };
        }
    }
    crc
}

fn encode_base32(bytes: &[u8; 12]) -> String {
    let mut bits = 0u128;
    for byte in bytes {
        bits = (bits << 8) | u128::from(*byte);
    }
    // 12 bytes = 96 bits; the final 4 bits are zero padding to reach 20 chars.
    bits <<= 4;
    let mut out = String::with_capacity(CODE_LEN);
    for shift in (0..CODE_LEN).rev() {
        let index = ((bits >> (shift * 5)) & 0x1f) as usize;
        out.push(ALPHABET[index] as char);
    }
    out
}

fn decode_base32(code: &str) -> Option<[u8; 12]> {
    if code.len() != CODE_LEN {
        return None;
    }
    let mut bits = 0u128;
    for ch in code.bytes() {
        let index = ALPHABET.iter().position(|c| *c == ch)?;
        bits = (bits << 5) | index as u128;
    }
    if bits & 0xf != 0 {
        return None;
    }
    let data = bits >> 4;
    let raw = data.to_be_bytes();
    let mut out = [0u8; 12];
    out.copy_from_slice(&raw[4..]);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invite_round_trip() {
        let invite = Invite {
            ip: Ipv4Addr::new(192, 37, 0, 72),
            port: 45_912,
            token: [0x12, 0x34, 0x56, 0x78],
        };
        let code = invite.encode();
        assert_eq!(code.len(), CODE_LEN);
        assert_eq!(Invite::decode(&code).unwrap(), invite);
    }

    #[test]
    fn invite_rejects_typos() {
        let invite = Invite {
            ip: Ipv4Addr::new(192, 37, 0, 72),
            port: 45_912,
            token: [1, 2, 3, 4],
        };
        let mut chars: Vec<char> = invite.encode().chars().collect();
        chars[5] = if chars[5] == 'A' { 'B' } else { 'A' };
        assert!(Invite::decode(&chars.into_iter().collect::<String>()).is_err());
        assert!(Invite::decode("ABC").is_err());
    }

    #[test]
    fn invite_normalizes_separators_and_ambiguous_characters() {
        let invite = Invite {
            ip: Ipv4Addr::new(192, 37, 0, 72),
            port: 45_912,
            token: [1, 2, 3, 4],
        };
        let code = invite.encode();
        let spaced = code
            .chars()
            .collect::<Vec<_>>()
            .chunks(4)
            .map(|chunk| chunk.iter().collect::<String>())
            .collect::<Vec<_>>()
            .join("-")
            .to_lowercase();
        assert_eq!(Invite::decode(&spaced).unwrap(), invite);
    }
}
