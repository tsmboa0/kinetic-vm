//! Build the public URL the owner wallet opens to claim this device.
//!
//! The signature is the device key over the claim ticket. The page submits
//! it. This module never sends the key.

use kinetic_chain::{
    Address, ClaimPageQuery, DeviceChain, U256, claim_page_url, ensure_public_claim_url,
    ensure_supported_network, parse_mon,
};
use kinetic_config::schema::Config;

use crate::i18n::{get_required_cli_string, get_required_cli_string_with_args};

/// Inside the registry's one-day maximum, with room for a little clock skew.
const CLAIM_LINK_LIFETIME_SECS: u64 = 23 * 60 * 60;

#[derive(Debug, PartialEq, Eq)]
pub enum ClaimLinkError {
    Disabled,
    MissingOwner,
    BadOwner,
    BadCap,
    LocalPage,
    UnpublishedMainnet,
    BadNetwork,
    Failed,
}

impl ClaimLinkError {
    pub fn message(&self) -> String {
        match self {
            Self::Disabled => text("cli-claim-link-disabled"),
            Self::MissingOwner => text("cli-claim-link-missing-owner"),
            Self::BadOwner => text("cli-claim-link-bad-owner"),
            Self::BadCap => text("cli-claim-link-bad-cap"),
            Self::LocalPage => text("cli-claim-link-local"),
            Self::UnpublishedMainnet => text("cli-claim-link-mainnet"),
            Self::BadNetwork => text("cli-claim-link-network"),
            Self::Failed => text_with("cli-chain-failed", &[("error", "request failed")]),
        }
    }
}

/// Sign a claim ticket and return the public page URL.
///
/// Local mistakes return before the device key is created.
pub async fn build(config: &Config) -> Result<String, ClaimLinkError> {
    if !config.chain.enabled {
        return Err(ClaimLinkError::Disabled);
    }
    let owner = config.chain.owner.trim();
    if owner.is_empty() {
        return Err(ClaimLinkError::MissingOwner);
    }
    let owner: Address = owner.parse().map_err(|_| ClaimLinkError::BadOwner)?;
    if parse_mon(&config.chain.per_tx_cap).is_err() || parse_mon(&config.chain.daily_cap).is_err() {
        return Err(ClaimLinkError::BadCap);
    }
    if ensure_public_claim_url(&config.chain.claim_url).is_err() {
        return Err(ClaimLinkError::LocalPage);
    }
    match config.chain.network.as_str() {
        "testnet" | "mainnet" => {}
        _ => return Err(ClaimLinkError::BadNetwork),
    }
    if ensure_supported_network(&config.chain).is_err() {
        return Err(ClaimLinkError::UnpublishedMainnet);
    }
    let Some(dir) = config.config_path.parent() else {
        return Err(ClaimLinkError::Failed);
    };
    let chain = DeviceChain::open(&config.chain, dir)
        .await
        .map_err(|_| ClaimLinkError::Failed)?;
    let now = chain
        .block_timestamp()
        .await
        .map_err(|_| ClaimLinkError::Failed)?;
    let deadline = U256::from(now.saturating_add(CLAIM_LINK_LIFETIME_SECS));
    let ticket = chain
        .sign_claim(owner, deadline)
        .await
        .map_err(|_| ClaimLinkError::Failed)?;
    claim_page_url(&ClaimPageQuery {
        claim_url: &config.chain.claim_url,
        device: chain.address(),
        owner,
        signature: &ticket.signature,
        deadline: ticket.deadline,
        per_tx_cap: &config.chain.per_tx_cap,
        daily_cap: &config.chain.daily_cap,
        chain_id: config.chain.chain_id,
        device_name: &config.chain.device_name,
        registry: &config.chain.registry,
        vault: &config.chain.vault,
        attestor: &config.chain.attestor,
        rpc_url: &config.chain.rpc_url,
    })
    .map_err(|_| ClaimLinkError::LocalPage)
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
    use std::path::Path;

    fn test_config(dir: &Path) -> Config {
        Config {
            chain: kinetic_config::schema::ChainConfig {
                enabled: true,
                owner: "0x00000000000000000000000000000000000000b2".into(),
                ..kinetic_config::schema::ChainConfig::default()
            },
            config_path: dir.join("config.toml"),
            ..Config::default()
        }
    }

    #[tokio::test]
    async fn a_disabled_chain_does_not_create_a_device_key() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut config = test_config(dir.path());
        config.chain.enabled = false;
        let error = build(&config).await.expect_err("disabled");
        assert_eq!(error, ClaimLinkError::Disabled);
        assert_eq!(error.message(), text("cli-claim-link-disabled"));
        assert!(!dir.path().join("device.key").exists());
    }

    #[tokio::test]
    async fn a_missing_owner_does_not_create_a_device_key() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut config = test_config(dir.path());
        config.chain.owner.clear();
        let error = build(&config).await.expect_err("owner");
        assert_eq!(error, ClaimLinkError::MissingOwner);
        assert!(!dir.path().join("device.key").exists());
    }

    #[tokio::test]
    async fn a_bad_owner_does_not_create_a_device_key() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut config = test_config(dir.path());
        config.chain.owner = "not-an-address".into();
        let error = build(&config).await.expect_err("owner");
        assert_eq!(error, ClaimLinkError::BadOwner);
        assert!(!dir.path().join("device.key").exists());
    }

    #[tokio::test]
    async fn a_bad_cap_does_not_create_a_device_key() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut config = test_config(dir.path());
        config.chain.per_tx_cap = "lots".into();
        let error = build(&config).await.expect_err("cap");
        assert_eq!(error, ClaimLinkError::BadCap);
        assert!(!dir.path().join("device.key").exists());
    }

    #[tokio::test]
    async fn a_local_claim_page_does_not_create_a_device_key() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut config = test_config(dir.path());
        config.chain.claim_url = "https://localhost/claim".into();
        let error = build(&config).await.expect_err("localhost");
        assert_eq!(error, ClaimLinkError::LocalPage);
        assert!(!dir.path().join("device.key").exists());
    }

    #[tokio::test]
    async fn unpublished_mainnet_does_not_create_a_device_key() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut config = test_config(dir.path());
        config.chain.network = "mainnet".into();
        config.chain.chain_id = 143;
        let error = build(&config).await.expect_err("mainnet");
        assert_eq!(error, ClaimLinkError::UnpublishedMainnet);
        assert_eq!(error.message(), text("cli-claim-link-mainnet"));
        assert!(!dir.path().join("device.key").exists());
    }
}
