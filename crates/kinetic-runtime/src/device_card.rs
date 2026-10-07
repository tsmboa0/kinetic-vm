//! The card `kinetic config get` prints when no property path is given.
//!
//! The device address comes from `device.key` when that file already exists.
//! Reading the card never creates the key.

use kinetic_chain::{BootReport, DeviceChain, SoftwareKey, format_mon};
use kinetic_config::schema::{Config, ModelProviderConfig};
use serde::Serialize;

const DEVICE_AGENT: &str = "device";

use crate::i18n::{get_required_cli_string, get_required_cli_string_with_args};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaimState {
    No,
    Yes,
    Unread,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeviceCard {
    pub device_name: String,
    pub address: Option<String>,
    pub claimed: ClaimState,
    pub owner: String,
    pub per_tx_cap: String,
    pub daily_cap: String,
    pub network: String,
    pub chain_enabled: bool,
    pub telegram: bool,
    pub provider: String,
    pub model: String,
    pub api_key_set: bool,
}

/// Paths the short `kinetic config set --api-key` and `--model` commands write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceModelPaths {
    pub api_key: String,
    pub model: String,
}

/// Local card. A missing key stays missing.
pub fn from_config(config: &Config) -> Result<DeviceCard, String> {
    let address = config
        .config_path
        .parent()
        .map(SoftwareKey::address_if_present)
        .transpose()
        .map_err(|_| text("cli-device-card-key-unreadable"))?
        .flatten();
    let (provider, model, api_key_set) = model_view(config);
    Ok(DeviceCard {
        device_name: config.chain.device_name.clone(),
        claimed: if address.is_none() {
            ClaimState::No
        } else {
            ClaimState::Unread
        },
        address: address.map(|address| address.to_string()),
        owner: config.chain.owner.clone(),
        per_tx_cap: config.chain.per_tx_cap.clone(),
        daily_cap: config.chain.daily_cap.clone(),
        network: config.chain.network.clone(),
        chain_enabled: config.chain.enabled,
        telegram: config
            .channels
            .telegram
            .values()
            .any(|channel| channel.enabled),
        provider,
        model,
        api_key_set,
    })
}

fn model_view(config: &Config) -> (String, String, bool) {
    let Some((family, _, entry)) = config.resolved_model_provider_for_agent(DEVICE_AGENT) else {
        return (String::new(), String::new(), false);
    };
    let model = entry
        .model
        .as_deref()
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .unwrap_or_default()
        .to_string();
    let api_key_set = entry
        .api_key
        .as_deref()
        .map(str::trim)
        .is_some_and(|key| !key.is_empty());
    (family.to_string(), model, api_key_set)
}

/// The provider profile quickstart stored for this device.
pub fn device_model_paths(config: &Config) -> Result<DeviceModelPaths, String> {
    let (family, alias, _) = resolved_device_model(config)?;
    Ok(DeviceModelPaths {
        api_key: format!("providers.models.{family}.{alias}.api_key"),
        model: format!("providers.models.{family}.{alias}.model"),
    })
}

/// Property path for `kinetic config set --provider`.
pub fn device_provider_path() -> &'static str {
    "agents.device.model_provider"
}

/// Turn `openai` or `openai.default` into a profile this install already has.
pub fn provider_assignment(config: &Config, raw: &str) -> Result<String, String> {
    if !config.agents.contains_key(DEVICE_AGENT) {
        return Err(text("cli-config-set-no-model"));
    }
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(text_with(
            "cli-config-set-provider-unknown",
            &[("provider", raw)],
        ));
    }
    let (family_raw, alias_raw) = match raw.split_once('.') {
        Some((family, alias)) => (family, Some(alias)),
        None => (raw, None),
    };
    let hits: Vec<(&'static str, &str)> = config
        .providers
        .models
        .iter_entries()
        .filter(|(family, alias, _)| {
            family.eq_ignore_ascii_case(family_raw)
                && alias_raw.is_none_or(|wanted| alias.eq_ignore_ascii_case(wanted))
        })
        .map(|(family, alias, _)| (family, alias))
        .collect();
    let chosen = if alias_raw.is_some() {
        hits.first().copied()
    } else {
        hits.iter()
            .copied()
            .find(|(_, alias)| *alias == "default")
            .or_else(|| (hits.len() == 1).then(|| hits[0]))
    };
    chosen
        .map(|(family, alias)| format!("{family}.{alias}"))
        .ok_or_else(|| text_with("cli-config-set-provider-unknown", &[("provider", raw)]))
}

fn resolved_device_model(
    config: &Config,
) -> Result<(&'static str, &str, &ModelProviderConfig), String> {
    config
        .resolved_model_provider_for_agent(DEVICE_AGENT)
        .ok_or_else(|| text("cli-config-set-no-model"))
}

/// Ask the chain whether this device is claimed. Does nothing when the key
/// file is absent, so a status read cannot create one.
pub async fn read_claim(config: &Config, card: &mut DeviceCard) {
    if card.address.is_none() || !config.chain.enabled {
        return;
    }
    let Some(dir) = config.config_path.parent() else {
        card.claimed = ClaimState::Unread;
        return;
    };
    let Ok(chain) = DeviceChain::open_existing(&config.chain, dir).await else {
        card.claimed = ClaimState::Unread;
        return;
    };
    match chain.identity().await {
        Ok(report) => apply_report(card, &report),
        Err(_) => card.claimed = ClaimState::Unread,
    }
}

pub fn apply_report(card: &mut DeviceCard, report: &BootReport) {
    match report {
        BootReport::Unclaimed { .. } => card.claimed = ClaimState::No,
        BootReport::Claimed {
            owner,
            per_tx_cap,
            daily_cap,
            ..
        } => {
            card.claimed = ClaimState::Yes;
            card.owner = owner.to_string();
            card.per_tx_cap = format_mon(*per_tx_cap);
            card.daily_cap = format_mon(*daily_cap);
        }
    }
}

pub fn render(card: &DeviceCard) -> String {
    let address = card
        .address
        .clone()
        .unwrap_or_else(|| text("cli-device-card-address-missing"));
    let owner = if card.owner.trim().is_empty() {
        text("cli-device-card-owner-missing")
    } else {
        card.owner.clone()
    };
    let claim = text(match card.claimed {
        ClaimState::No => "cli-device-card-claim-no",
        ClaimState::Yes => "cli-device-card-claim-yes",
        ClaimState::Unread => "cli-device-card-claim-unread",
    });
    let chain_state = text(if card.chain_enabled {
        "cli-device-card-chain-on"
    } else {
        "cli-device-card-chain-off"
    });
    let telegram = text(if card.telegram {
        "cli-device-card-on"
    } else {
        "cli-device-card-off"
    });
    let provider = shown(&card.provider);
    let model = shown(&card.model);
    let api_key = text(if card.api_key_set {
        "cli-device-card-set"
    } else {
        "cli-device-card-unset"
    });
    [
        text_with("cli-device-card-name", &[("value", &card.device_name)]),
        text_with("cli-device-card-address", &[("value", &address)]),
        text_with("cli-device-card-claimed", &[("value", &claim)]),
        text_with("cli-device-card-owner", &[("value", &owner)]),
        text_with("cli-device-card-per-tx", &[("value", &card.per_tx_cap)]),
        text_with("cli-device-card-daily", &[("value", &card.daily_cap)]),
        text_with(
            "cli-device-card-chain",
            &[("network", &card.network), ("state", &chain_state)],
        ),
        text_with("cli-device-card-telegram", &[("value", &telegram)]),
        text_with("cli-device-card-provider", &[("value", &provider)]),
        text_with("cli-device-card-model", &[("value", &model)]),
        text_with("cli-device-card-api-key", &[("value", &api_key)]),
    ]
    .join("\n")
}

fn shown(value: &str) -> String {
    if value.trim().is_empty() {
        text("cli-device-card-unset")
    } else {
        value.to_string()
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
    use kinetic_chain::U256;

    fn config_in(dir: &std::path::Path) -> Config {
        Config {
            config_path: dir.join("config.toml"),
            ..Config::default()
        }
    }

    #[test]
    fn a_missing_key_stays_missing() {
        let dir = tempfile::tempdir().expect("temp dir");
        let config = config_in(dir.path());
        let card = from_config(&config).expect("card");
        assert_eq!(card.address, None);
        assert_eq!(card.claimed, ClaimState::No);
        assert!(!dir.path().join("device.key").exists());
        let rendered = render(&card);
        assert!(rendered.contains("not created yet"));
        assert!(rendered.contains("Claimed: no"));
    }

    #[test]
    fn an_existing_key_is_shown_and_left_in_place() {
        let dir = tempfile::tempdir().expect("temp dir");
        let key = SoftwareKey::load_or_create(dir.path()).expect("create");
        let before = std::fs::read(dir.path().join("device.key")).expect("read");
        let config = config_in(dir.path());
        let card = from_config(&config).expect("card");
        assert_eq!(
            card.address.as_deref(),
            Some(key.address().to_string().as_str())
        );
        assert_eq!(card.claimed, ClaimState::Unread);
        assert_eq!(
            std::fs::read(dir.path().join("device.key")).expect("read"),
            before
        );
    }

    #[test]
    fn a_claimed_report_replaces_the_local_owner_and_caps() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut config = config_in(dir.path());
        config.chain.owner = "0x0000000000000000000000000000000000000001".into();
        config.chain.per_tx_cap = "0.01".into();
        let mut card = from_config(&config).expect("card");
        apply_report(
            &mut card,
            &BootReport::Claimed {
                device: "0x0000000000000000000000000000000000000002"
                    .parse()
                    .expect("address"),
                agent_id: U256::from(9u64),
                owner: "0x64772107fC23f7370C90EA0aBd29ee6117B97f77"
                    .parse()
                    .expect("owner"),
                per_tx_cap: U256::from(50_000_000_000_000_000u64),
                daily_cap: U256::from(1_000_000_000_000_000_000u64),
                paused: false,
            },
        );
        assert_eq!(card.claimed, ClaimState::Yes);
        assert_eq!(card.owner, "0x64772107fC23f7370C90EA0aBd29ee6117B97f77");
        assert_eq!(
            kinetic_chain::parse_mon(&card.per_tx_cap).expect("per tx"),
            U256::from(50_000_000_000_000_000u64)
        );
        assert_eq!(
            kinetic_chain::parse_mon(&card.daily_cap).expect("daily"),
            U256::from(1_000_000_000_000_000_000u64)
        );
    }

    #[test]
    fn the_card_names_the_model_and_whether_the_key_is_set() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut config = config_in(dir.path());
        config.agents.insert(
            "device".into(),
            kinetic_config::schema::AliasedAgentConfig {
                model_provider: "openai.default".into(),
                ..Default::default()
            },
        );
        config.providers.models.openai.insert(
            "default".into(),
            kinetic_config::schema::OpenAIModelProviderConfig {
                base: kinetic_config::schema::ModelProviderConfig {
                    model: Some("gpt-5.4-mini".into()),
                    api_key: Some("sk-test-secret".into()),
                    ..Default::default()
                },
            },
        );
        let card = from_config(&config).expect("card");
        let rendered = render(&card);
        assert!(rendered.contains("Provider: openai"));
        assert!(rendered.contains("Model: gpt-5.4-mini"));
        assert!(rendered.contains("API key: set"));
        assert!(!rendered.contains("sk-test-secret"));
        let paths = device_model_paths(&config).expect("paths");
        assert_eq!(paths.api_key, "providers.models.openai.default.api_key");
        assert_eq!(paths.model, "providers.models.openai.default.model");
        assert_eq!(
            provider_assignment(&config, "OpenAI").expect("provider"),
            "openai.default"
        );
    }

    #[test]
    fn a_missing_model_stays_unset_and_cannot_be_retargeted() {
        let dir = tempfile::tempdir().expect("temp dir");
        let config = config_in(dir.path());
        let card = from_config(&config).expect("card");
        let rendered = render(&card);
        assert!(rendered.contains("Provider: not set"));
        assert!(rendered.contains("Model: not set"));
        assert!(rendered.contains("API key: not set"));
        assert!(device_model_paths(&config).is_err());
        assert!(provider_assignment(&config, "openai").is_err());
    }
}
