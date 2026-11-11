use crate::error;
use crate::layout::{self, DeviceId, DevicePlacement};
use serde::{Deserialize, Serialize};
use std::fs;
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

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
    /// Controller-only: IPv4 address of the listening interface. Agent
    /// connects to the address encoded in the pairing code.
    #[serde(default)]
    pub local_ip: String,
    /// Controller-only: map Command <-> Control when a peer runs a different
    /// operating system. Off by default (strict physical keys).
    #[serde(default)]
    pub adapt_modifiers: bool,
    /// UI language chosen in Settings. Empty means no explicit choice, so the
    /// UI follows the system locale on the next launch.
    #[serde(default)]
    pub language: String,
    /// Runtime log level: off/error/warn/info/debug/trace.
    #[serde(default = "default_log_level")]
    pub log_level: String,
    /// Stable per-installation identity used by the Controller to remember
    /// devices across reconnects and app restarts.
    #[serde(default)]
    pub device_id: String,
    /// Controller-only: persisted 2D screen arrangement.
    #[serde(default)]
    pub layout: Vec<DevicePlacement>,
    /// Controller-only: last generated listening code. Reused when the
    /// selected interface and port are still usable.
    #[serde(default)]
    pub last_invite: String,
    /// Agent-only: last pairing code entered, restored in the UI after a
    /// disconnect or restart.
    #[serde(default)]
    pub last_pairing_code: String,
}

/// Partial update sent by the Settings page. Missing fields are left alone.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsPatch {
    pub language: Option<String>,
    pub adapt_modifiers: Option<bool>,
    pub local_ip: Option<String>,
    pub log_level: Option<String>,
}

pub fn default_log_level() -> String {
    "info".to_string()
}

fn normalize_log_level(value: &str) -> String {
    match value.trim().to_ascii_lowercase().as_str() {
        "off" | "error" | "warn" | "warning" | "info" | "debug" | "trace" => {
            let value = value.trim().to_ascii_lowercase();
            if value == "warning" {
                "warn".to_string()
            } else {
                value
            }
        }
        _ => default_log_level(),
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            role: Role::Controller,
            local_ip: String::new(),
            adapt_modifiers: false,
            language: String::new(),
            log_level: default_log_level(),
            device_id: String::new(),
            layout: Vec::new(),
            last_invite: String::new(),
            last_pairing_code: String::new(),
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

    pub fn device_id_bytes(&self) -> Result<DeviceId, String> {
        layout::device_id_from_hex(&self.device_id).ok_or_else(|| error::code("E_DEVICE_ID"))
    }

    /// Ensure a stable id exists. Used at startup before any handshake.
    pub fn ensure_device_id(&mut self) -> bool {
        if layout::device_id_from_hex(&self.device_id).is_some() {
            return false;
        }
        self.device_id = layout::device_id_to_hex(&layout::new_device_id());
        true
    }

    pub fn validate(&self) -> Result<(), String> {
        let _ = self.device_id_bytes()?;
        if self.role == Role::Controller {
            let ip = self.local_ipv4()?;
            if ip.is_loopback() || ip.is_unspecified() {
                return Err(error::code("E_LOCAL_IP"));
            }
        }
        Ok(())
    }

    /// Trim free-form values and keep `language` within the supported set.
    pub fn normalize(&mut self) {
        self.local_ip = self.local_ip.trim().to_string();
        self.language = normalize_language(&self.language);
        self.log_level = normalize_log_level(&self.log_level);
    }

    /// Apply a Settings-page change without touching session fields.
    pub fn apply_patch(&mut self, patch: &SettingsPatch) -> Result<(), String> {
        if let Some(language) = &patch.language {
            self.language = normalize_language(language);
        }
        if let Some(adapt_modifiers) = patch.adapt_modifiers {
            self.adapt_modifiers = adapt_modifiers;
        }
        if let Some(log_level) = &patch.log_level {
            self.log_level = normalize_log_level(log_level);
        }
        if let Some(local_ip) = &patch.local_ip {
            let trimmed = local_ip.trim();
            if !trimmed.is_empty() {
                let ip = trimmed
                    .parse::<Ipv4Addr>()
                    .map_err(|_| error::code("E_LOCAL_IP"))?;
                if ip.is_loopback() || ip.is_unspecified() {
                    return Err(error::code("E_LOCAL_IP"));
                }
            }
            self.local_ip = trimmed.to_string();
        }
        Ok(())
    }

    /// Keep settings that `start_session` does not own. The UI sends a full
    /// config record, but older clients (and controller starts) omit the
    /// language and the agent-only interface choice.
    pub fn inherit_persisted_settings(&mut self, previous: &AppConfig) {
        if self.language.trim().is_empty() {
            self.language = previous.language.clone();
        }
        if self.log_level.trim().is_empty() {
            self.log_level = previous.log_level.clone();
        }
        if self.local_ip.trim().is_empty() {
            self.local_ip = previous.local_ip.clone();
        }
        if self.device_id.trim().is_empty() {
            self.device_id = previous.device_id.clone();
        }
        if self.layout.is_empty() {
            self.layout = previous.layout.clone();
        }
        if self.last_invite.is_empty() {
            self.last_invite = previous.last_invite.clone();
        }
        if self.last_pairing_code.is_empty() {
            self.last_pairing_code = previous.last_pairing_code.clone();
        }
        self.normalize();
    }
}

/// Shared, persist-on-write settings handle. The network thread also updates
/// device placements, so a single lock protects both UI and hub writes.
#[derive(Clone)]
pub struct ConfigHandle {
    path: Arc<PathBuf>,
    config: Arc<Mutex<AppConfig>>,
}

impl ConfigHandle {
    pub fn new(path: PathBuf, config: AppConfig) -> Self {
        Self {
            path: Arc::new(path),
            config: Arc::new(Mutex::new(config)),
        }
    }

    pub fn snapshot(&self) -> Result<AppConfig, String> {
        self.config
            .lock()
            .map(|config| config.clone())
            .map_err(|_| error::code("E_INTERNAL"))
    }

    pub fn update<F>(&self, mutate: F) -> Result<AppConfig, String>
    where
        F: FnOnce(&mut AppConfig) -> Result<(), String>,
    {
        let mut current = self.config.lock().map_err(|_| error::code("E_INTERNAL"))?;
        let mut next = current.clone();
        mutate(&mut next)?;
        save(&self.path, &next)?;
        *current = next.clone();
        Ok(next)
    }
}

fn normalize_language(value: &str) -> String {
    match value.trim().to_ascii_lowercase().as_str() {
        "zh" => "zh".to_string(),
        "en" => "en".to_string(),
        _ => String::new(),
    }
}

pub fn load_or_create(settings_path: &Path) -> Result<AppConfig, String> {
    if settings_path.exists() {
        let mut config = match read(settings_path) {
            Ok(config) => config,
            Err(err) => {
                let _ = fs::rename(settings_path, settings_path.with_extension("json.bak"));
                let _ = save(settings_path, &AppConfig::default());
                return Err(err);
            }
        };
        config.normalize();
        if config.ensure_device_id() {
            let _ = save(settings_path, &config);
        }
        // Drop unknown fields from older formats.
        let _ = save(settings_path, &config);
        return Ok(config);
    }

    let mut config = AppConfig::default();
    config.ensure_device_id();
    save(settings_path, &config)?;
    Ok(config)
}

fn read(path: &Path) -> Result<AppConfig, String> {
    let raw = fs::read_to_string(path).map_err(|e| error::detail("E_CONFIG_READ", e))?;
    serde_json::from_str(&raw).map_err(|e| error::detail("E_CONFIG_PARSE", e))
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
    use std::path::PathBuf;

    fn test_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("glidelink-config-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create test dir");
        dir
    }

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
        assert!(cfg.layout.is_empty());
        assert!(cfg.device_id.is_empty());
        assert!(!cfg.adapt_modifiers);
        assert!(cfg.language.is_empty());
    }

    #[test]
    fn patch_updates_only_supplied_fields() {
        let mut config = AppConfig {
            role: Role::Agent,
            local_ip: "192.168.1.10".into(),
            ..AppConfig::default()
        };

        config
            .apply_patch(&SettingsPatch {
                language: Some(" ZH ".into()),
                ..SettingsPatch::default()
            })
            .unwrap();
        assert_eq!(config.language, "zh");
        assert_eq!(config.local_ip, "192.168.1.10");
        assert!(!config.adapt_modifiers);

        config
            .apply_patch(&SettingsPatch {
                adapt_modifiers: Some(true),
                ..SettingsPatch::default()
            })
            .unwrap();
        assert!(config.adapt_modifiers);
        assert_eq!(config.language, "zh");

        config
            .apply_patch(&SettingsPatch {
                language: Some("fr".into()),
                ..SettingsPatch::default()
            })
            .unwrap();
        assert!(config.language.is_empty());
    }

    #[test]
    fn patch_validates_local_ip() {
        let mut config = AppConfig::default();
        config
            .apply_patch(&SettingsPatch {
                local_ip: Some("  ".into()),
                ..SettingsPatch::default()
            })
            .unwrap();
        assert!(config.local_ip.is_empty());

        for bad in ["127.0.0.1", "0.0.0.0", "not-an-ip"] {
            let patch = SettingsPatch {
                local_ip: Some(bad.into()),
                ..SettingsPatch::default()
            };
            assert!(config.apply_patch(&patch).is_err(), "{bad} should fail");
        }

        config
            .apply_patch(&SettingsPatch {
                local_ip: Some(" 192.168.1.20 ".into()),
                ..SettingsPatch::default()
            })
            .unwrap();
        assert_eq!(config.local_ip, "192.168.1.20");
    }

    #[test]
    fn start_session_inherits_language_and_interface() {
        let previous = AppConfig {
            local_ip: "192.168.1.10".into(),
            language: "en".into(),
            ..AppConfig::default()
        };

        let mut controller = AppConfig {
            role: Role::Controller,
            ..AppConfig::default()
        };
        controller.inherit_persisted_settings(&previous);
        assert_eq!(controller.language, "en");
        assert_eq!(controller.local_ip, "192.168.1.10");

        let mut agent = AppConfig {
            role: Role::Agent,
            local_ip: "192.168.1.30".into(),
            language: "zh".into(),
            ..AppConfig::default()
        };
        agent.inherit_persisted_settings(&previous);
        assert_eq!(agent.language, "zh");
        assert_eq!(agent.local_ip, "192.168.1.30");
    }

    #[test]
    fn creates_default_settings_when_missing() {
        let dir = test_dir("create");
        let settings = dir.join("settings.json");

        let config = load_or_create(&settings).expect("create");
        assert_eq!(config.role, Role::Controller);
        assert!(settings.exists());

        let reloaded = load_or_create(&settings).expect("reload");
        assert_eq!(reloaded.role, Role::Controller);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_settings_are_backed_up() {
        let dir = test_dir("corrupt");
        let settings = dir.join("settings.json");
        fs::write(&settings, "{not json").unwrap();

        let err = load_or_create(&settings).unwrap_err();
        assert!(err.contains("E_CONFIG_PARSE"), "unexpected error: {err}");
        assert!(dir.join("settings.json.bak").exists());

        let recovered = load_or_create(&settings).expect("defaults");
        assert_eq!(recovered.role, Role::Controller);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn validation_checks_controller_local_ip() {
        let controller = AppConfig {
            role: Role::Controller,
            local_ip: "192.168.1.20".into(),
            ..AppConfig::default()
        };
        assert!(controller.local_ipv4().unwrap() == Ipv4Addr::new(192, 168, 1, 20));

        let mut invalid = controller.clone();
        invalid.local_ip = "127.0.0.1".into();
        invalid.ensure_device_id();
        assert!(invalid.validate().is_err());
        invalid.local_ip = "not-an-ip".into();
        assert!(invalid.validate().is_err());

        let mut agent = AppConfig {
            role: Role::Agent,
            ..AppConfig::default()
        };
        agent.ensure_device_id();
        assert!(agent.validate().is_ok());
        assert!(agent.local_ipv4().is_err());
    }

    #[test]
    fn device_id_is_generated_once() {
        let mut config = AppConfig::default();
        assert!(config.ensure_device_id());
        let first = config.device_id.clone();
        assert!(!config.ensure_device_id());
        assert_eq!(config.device_id, first);
        assert!(config.device_id_bytes().is_ok());
    }
}
