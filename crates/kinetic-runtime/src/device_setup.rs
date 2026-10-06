//! The short `kinetic quickstart` write.
//!
//! It stores the device name, owner wallet, Telegram bot, model provider, and
//! the starting caps. It does not create the device key.

use std::collections::HashMap;

use kinetic_chain::{Address, parse_mon};
use kinetic_config::presets::{
    AgentIdentity, BuilderSubmission, ChannelQuickStart, MemoryChoice, ModelProviderChoice,
    SelectorChoice,
};
use kinetic_config::schema::Config;

use crate::i18n::{get_required_cli_string, get_required_cli_string_with_args};
use crate::quickstart::{Surface, apply_with_surface, resolve_model_provider_type};

const DEVICE_AGENT: &str = "device";
const PROVIDER_ALIAS: &str = "default";
const TELEGRAM_ALIAS: &str = "default";

/// Answers collected by `kinetic quickstart`.
pub struct DeviceSetup {
    pub device_name: String,
    pub owner: String,
    pub bot_token: String,
    pub provider: String,
    pub model: String,
    pub api_key: Option<String>,
    pub per_tx_cap: String,
    pub daily_cap: String,
}

#[derive(Debug, PartialEq, Eq)]
pub enum SetupError {
    BadName,
    BadOwner,
    BadToken,
    BadProvider,
    NeedsKey,
    BadModel,
    BadCap,
    CapOrder,
    AgentExists,
    Failed(String),
}

impl SetupError {
    pub fn message(&self) -> String {
        match self {
            Self::BadName => text("cli-setup-bad-name"),
            Self::BadOwner => text("cli-setup-bad-owner"),
            Self::BadToken => text("cli-setup-bad-token"),
            Self::BadProvider => text("cli-setup-bad-provider"),
            Self::NeedsKey => text("cli-setup-needs-key"),
            Self::BadModel => text("cli-setup-bad-model"),
            Self::BadCap => text("cli-setup-bad-cap"),
            Self::CapOrder => text("cli-setup-cap-order"),
            Self::AgentExists => text("cli-setup-agent-exists"),
            Self::Failed(error) => text_with("cli-setup-failed", &[("error", error)]),
        }
    }
}

/// Write the device setup. Returns before any key file is created, and before
/// any config write when the answers are not usable.
pub async fn apply(config: &mut Config, setup: &DeviceSetup) -> Result<(), SetupError> {
    let device_name = setup.device_name.trim();
    if device_name.is_empty() || device_name.contains(['\n', '\r']) || device_name.len() > 128 {
        return Err(SetupError::BadName);
    }
    let owner = setup.owner.trim();
    if owner.parse::<Address>().is_err() {
        return Err(SetupError::BadOwner);
    }
    let token = setup.bot_token.trim();
    if token.is_empty() || token.chars().any(char::is_whitespace) {
        return Err(SetupError::BadToken);
    }
    let Some((provider, codex)) = resolve_model_provider_type(setup.provider.trim()) else {
        return Err(SetupError::BadProvider);
    };
    let local = kinetic_providers::list_model_providers()
        .into_iter()
        .any(|info| info.name == provider && info.local);
    let api_key = setup
        .api_key
        .as_deref()
        .map(str::trim)
        .filter(|key| !key.is_empty());
    if !local && !codex && api_key.is_none() {
        return Err(SetupError::NeedsKey);
    }
    let model = setup.model.trim();
    if model.is_empty() {
        return Err(SetupError::BadModel);
    }
    let per_tx = parse_mon(setup.per_tx_cap.trim()).map_err(|_| SetupError::BadCap)?;
    let daily = parse_mon(setup.daily_cap.trim()).map_err(|_| SetupError::BadCap)?;
    if per_tx > daily {
        return Err(SetupError::CapOrder);
    }
    if config.agents.contains_key(DEVICE_AGENT) {
        return Err(SetupError::AgentExists);
    }

    let mut fields = HashMap::new();
    if codex {
        fields.insert("auth_mode".to_string(), "codex".to_string());
    }
    if let Some(key) = api_key {
        fields.insert("api_key".to_string(), key.to_string());
    }
    let submission = BuilderSubmission {
        model_provider: SelectorChoice::Fresh(ModelProviderChoice {
            provider_type: provider.to_string(),
            alias: PROVIDER_ALIAS.to_string(),
            model: model.to_string(),
            fields,
        }),
        risk_profile: SelectorChoice::Fresh("balanced".to_string()),
        runtime_profile: SelectorChoice::Fresh("balanced".to_string()),
        memory: SelectorChoice::Fresh(MemoryChoice::Sqlite),
        channels: vec![SelectorChoice::Fresh(ChannelQuickStart {
            channel_type: "telegram".to_string(),
            alias: TELEGRAM_ALIAS.to_string(),
            fields: HashMap::from([("bot_token".to_string(), token.to_string())]),
        })],
        peer_groups: Vec::new(),
        agent: AgentIdentity {
            name: DEVICE_AGENT.to_string(),
            system_prompt: String::new(),
            personality_file: None,
            personality_files: Vec::new(),
        },
    };

    apply_with_surface(submission, config, Surface::Cli)
        .await
        .map_err(|errors| {
            SetupError::Failed(
                errors
                    .first()
                    .map(|error| error.message.clone())
                    .unwrap_or_else(|| "setup failed".to_string()),
            )
        })?;

    let writes = [
        ("chain.enabled", "true"),
        ("chain.owner", owner),
        ("chain.device_name", device_name),
        ("chain.per_tx_cap", setup.per_tx_cap.trim()),
        ("chain.daily_cap", setup.daily_cap.trim()),
    ];
    for (path, value) in writes {
        config
            .set_prop_persistent(path, value)
            .map_err(|error| SetupError::Failed(error.to_string()))?;
    }
    config
        .save_dirty()
        .await
        .map_err(|error| SetupError::Failed(error.to_string()))?;
    Ok(())
}

pub fn saved_message(device_name: &str) -> String {
    text_with("cli-setup-saved", &[("name", device_name.trim())])
}

pub fn accepts_name(value: &str) -> bool {
    let name = value.trim();
    !name.is_empty() && name.len() <= 128 && !name.contains(['\n', '\r'])
}

pub fn accepts_owner(value: &str) -> bool {
    value.trim().parse::<Address>().is_ok()
}

pub fn accepts_cap(value: &str) -> bool {
    parse_mon(value.trim()).is_ok()
}

pub fn caps_fit(per_tx: &str, daily: &str) -> bool {
    match (parse_mon(per_tx.trim()), parse_mon(daily.trim())) {
        (Ok(per_tx), Ok(daily)) => per_tx <= daily,
        _ => false,
    }
}

fn text(key: &str) -> String {
    get_required_cli_string(key)
}

fn text_with(key: &str, args: &[(&str, &str)]) -> String {
    get_required_cli_string_with_args(key, args)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> DeviceSetup {
        DeviceSetup {
            device_name: "Lobby machine".into(),
            owner: "0x64772107fC23f7370C90EA0aBd29ee6117B97f77".into(),
            bot_token: "123456:telegram-token".into(),
            provider: "anthropic".into(),
            model: "claude-sonnet-4-5".into(),
            api_key: Some("sk-test".into()),
            per_tx_cap: "0.05".into(),
            daily_cap: "1".into(),
        }
    }

    async fn blank_config(dir: &std::path::Path) -> Config {
        let config = Config {
            config_path: dir.join("config.toml"),
            data_dir: dir.join("data"),
            ..Config::default()
        };
        config.save().await.expect("save blank config");
        config
    }

    #[tokio::test]
    async fn setup_stores_the_device_and_does_not_create_the_key() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut config = blank_config(dir.path()).await;
        apply(&mut config, &sample()).await.expect("apply");

        let raw = std::fs::read_to_string(dir.path().join("config.toml")).expect("read");
        let saved: Config = toml::from_str(&raw).expect("parse");
        assert!(saved.chain.enabled);
        assert_eq!(saved.chain.device_name, "Lobby machine");
        assert_eq!(
            saved.chain.owner,
            "0x64772107fC23f7370C90EA0aBd29ee6117B97f77"
        );
        assert_eq!(saved.chain.per_tx_cap, "0.05");
        assert_eq!(saved.chain.daily_cap, "1");
        assert!(saved.agents.contains_key(DEVICE_AGENT));
        assert!(saved.channels.telegram["default"].enabled);
        let store = kinetic_config::secrets::SecretStore::new(dir.path(), true);
        assert_eq!(
            store
                .decrypt(&saved.channels.telegram["default"].bot_token)
                .expect("decrypt"),
            "123456:telegram-token"
        );
        assert!(!dir.path().join("device.key").exists());
    }

    #[tokio::test]
    async fn a_bad_owner_is_refused_before_any_write() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut config = blank_config(dir.path()).await;
        let before = std::fs::read_to_string(dir.path().join("config.toml")).expect("read");
        let mut setup = sample();
        setup.owner = "not-a-wallet".into();
        let error = apply(&mut config, &setup).await.expect_err("owner");
        assert_eq!(error, SetupError::BadOwner);
        let after = std::fs::read_to_string(dir.path().join("config.toml")).expect("read");
        assert_eq!(before, after);
        assert!(!dir.path().join("device.key").exists());
    }

    #[tokio::test]
    async fn a_per_transaction_cap_above_the_daily_cap_is_refused() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut config = blank_config(dir.path()).await;
        let mut setup = sample();
        setup.per_tx_cap = "2".into();
        setup.daily_cap = "1".into();
        let error = apply(&mut config, &setup).await.expect_err("caps");
        assert_eq!(error, SetupError::CapOrder);
        assert!(!dir.path().join("device.key").exists());
    }
}
