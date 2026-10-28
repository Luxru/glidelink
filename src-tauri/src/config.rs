use crate::error;
use crate::layout::Edge;
use serde::{Deserialize, Serialize};
use std::fs;
use std::net::Ipv4Addr;
use std::path::Path;

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
    /// Agent-only: IPv4 address of the selected network interface. The
    /// controller's target is carried inside the invite code.
    #[serde(default)]
    pub local_ip: String,
    pub edge: Edge,
    /// Offset of the peer screen along the shared edge, normalized to the
    /// host edge. Controller-only; defaults to zero for older configs.
    #[serde(default)]
    pub peer_offset: f32,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            role: Role::Controller,
            local_ip: String::new(),
            edge: Edge::Right,
            peer_offset: 0.0,
        }
    }
}

impl AppConfig {
    pub fn local_ipv4(&self) -> Result<Ipv4Addr, String> {
        self.local_ip
            .trim()
            .parse::<Ipv4Addr>()
            .map_err(|_| error::code("E_LOCAL_IP"))
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.role == Role::Agent {
            let ip = self.local_ipv4()?;
            if ip.is_loopback() || ip.is_unspecified() {
                return Err(error::code("E_LOCAL_IP"));
            }
        }
        Ok(())
    }
}

pub fn load_or_create(path: &Path) -> Result<AppConfig, String> {
    if path.exists() {
        let raw = fs::read_to_string(path).map_err(|e| error::detail("E_CONFIG_READ", e))?;
        let config: AppConfig =
            serde_json::from_str(&raw).map_err(|e| error::detail("E_CONFIG_PARSE", e))?;
        // Drop fields from older formats (including the old passcode).
        let _ = save(path, &config);
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
    let raw =
        serde_json::to_string_pretty(config).map_err(|e| error::detail("E_CONFIG_WRITE", e))?;
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
    fn old_config_fields_get_safe_defaults() {
        let old = r#"{
            "role": "agent",
            "peerAddr": "192.168.1.42:45890",
            "listenPort": 45890,
            "passcode": "oldlongpasscode",
            "edge": "right"
        }"#;
        let cfg: AppConfig = serde_json::from_str(old).expect("old config parses");
        assert!(cfg.local_ip.is_empty());
        assert_eq!(cfg.peer_offset, 0.0);
    }

    #[test]
    fn validation_checks_local_ip() {
        let controller = AppConfig {
            role: Role::Controller,
            ..AppConfig::default()
        };
        assert!(controller.validate().is_ok());

        let agent = AppConfig {
            role: Role::Agent,
            local_ip: "192.168.1.10".into(),
            ..AppConfig::default()
        };
        assert!(agent.validate().is_ok());
        assert_eq!(agent.local_ipv4().unwrap(), Ipv4Addr::new(192, 168, 1, 10));

        let mut invalid = agent;
        invalid.local_ip = "127.0.0.1".into();
        assert!(invalid.validate().is_err());
        invalid.local_ip = "not-an-ip".into();
        assert!(invalid.validate().is_err());
    }
}
