use crate::error;
use crate::layout::Edge;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use std::fs;
use std::net::SocketAddr;
use std::path::Path;

pub const DEFAULT_PORT: u16 = 45_890;
const PASSCODE_ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz23456789";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Controller,
    Agent,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Controller => "controller",
            Self::Agent => "agent",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppConfig {
    pub role: Role,
    pub peer_addr: String,
    pub listen_port: u16,
    pub passcode: String,
    pub edge: Edge,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            role: Role::Controller,
            peer_addr: String::new(),
            listen_port: DEFAULT_PORT,
            passcode: generate_passcode(12),
            edge: Edge::Right,
        }
    }
}

impl AppConfig {
    pub fn peer_socket_addr(&self) -> Result<SocketAddr, String> {
        self.peer_addr
            .parse::<SocketAddr>()
            .map_err(|_| error::code("E_PEER_ADDRESS"))
    }

    pub fn validate(&self) -> Result<(), String> {
        let pass_len = self.passcode.trim().len();
        if !(8..=256).contains(&pass_len) {
            return Err(error::code("E_PASSCODE_LENGTH"));
        }
        match self.role {
            Role::Controller => {
                self.peer_socket_addr()?;
                if self.peer_addr.ends_with(":0") {
                    return Err(error::code("E_PEER_ADDRESS"));
                }
            }
            Role::Agent => {
                if self.listen_port < 1024 {
                    return Err(error::code("E_PORT_RANGE"));
                }
            }
        }
        Ok(())
    }

    /// Hash only enough to avoid keeping the pairing code in logs. The actual
    /// config file still contains the code, protected by the user's OS account.
    pub fn redacted_passcode(&self) -> String {
        let n = self.passcode.chars().count();
        if n <= 4 {
            return "*".repeat(n);
        }
        let visible: String = self.passcode.chars().take(2).collect();
        format!(
            "{visible}{}{}",
            "*".repeat(n.saturating_sub(2).min(8)),
            if n > 10 { "…" } else { "" }
        )
    }
}

pub fn generate_passcode(len: usize) -> String {
    let mut bytes = vec![0u8; len];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    bytes
        .into_iter()
        .map(|b| PASSCODE_ALPHABET[b as usize % PASSCODE_ALPHABET.len()] as char)
        .collect()
}

pub fn load_or_create(path: &Path) -> Result<AppConfig, String> {
    if path.exists() {
        let raw =
            fs::read_to_string(path).map_err(|e| error::detail("E_CONFIG_READ", e))?;
        let config: AppConfig =
            serde_json::from_str(&raw).map_err(|e| error::detail("E_CONFIG_PARSE", e))?;
        return Ok(config);
    }

    let config = AppConfig::default();
    save(path, &config)?;
    Ok(config)
}

pub fn save(path: &Path, config: &AppConfig) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| error::detail("E_CONFIG_WRITE", e))?;
    }
    let raw = serde_json::to_string_pretty(config)
        .map_err(|e| error::detail("E_CONFIG_WRITE", e))?;
    fs::write(path, raw).map_err(|e| error::detail("E_CONFIG_WRITE", e))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_passcodes_are_valid() {
        let code = generate_passcode(12);
        assert_eq!(code.len(), 12);
        assert!(code.chars().all(|c| PASSCODE_ALPHABET.contains(&(c as u8))));
        assert_ne!(code, generate_passcode(12));
    }

    #[test]
    fn validation_checks_roles() {
        let mut cfg = AppConfig {
            role: Role::Controller,
            peer_addr: "192.168.1.20:45890".into(),
            passcode: "abcdefgh".into(),
            ..AppConfig::default()
        };
        assert!(cfg.validate().is_ok());
        cfg.peer_addr = "not-an-address".into();
        assert!(cfg.validate().is_err());

        let agent = AppConfig {
            role: Role::Agent,
            listen_port: 80,
            passcode: "abcdefgh".into(),
            ..AppConfig::default()
        };
        assert!(agent.validate().is_err());
    }
}
