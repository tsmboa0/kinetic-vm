//! Device-side Monad client: a software signing key, the boot read of the
//! device's agent, and a signed claim ticket once an owner is known.
//!
//! The secure-element backend is a later `DeviceSigner`. This crate does not
//! implement it.

mod client;
mod key;

pub use alloy::primitives::{Address, B256, U256};

/// Parse a MON amount such as `0.02` into wei.
pub fn parse_mon(amount: &str) -> Result<U256> {
    alloy::primitives::utils::parse_ether(amount)
        .map_err(|_| anyhow::Error::msg("amount is not a MON value"))
}

/// Render wei as a MON amount.
pub fn format_mon(amount: U256) -> String {
    alloy::primitives::utils::format_ether(amount)
}
pub use client::{
    ActionAttestation, Binding, BootReport, ChainClient, ClaimTicket, VaultStatus,
    action_request_hash, binding_from_device, unclaimed_claim_text,
};
pub use key::{DeviceSigner, SoftwareKey};

use std::path::Path;

use anyhow::Result;
use kinetic_config::schema::ChainConfig;

/// An open device key and the chain client that reads and signs for it.
pub struct DeviceChain {
    key: SoftwareKey,
    client: ChainClient,
}

impl DeviceChain {
    /// Load or create the device key, then connect. Fails while the chain
    /// section is disabled, before any key file is created.
    pub async fn open(config: &ChainConfig, key_dir: &Path) -> Result<Self> {
        if !config.enabled {
            anyhow::bail!("the Monad chain client is disabled in config");
        }
        let key = SoftwareKey::load_or_create(key_dir)?;
        let client = ChainClient::connect(config).await?;
        Ok(Self { key, client })
    }

    pub fn address(&self) -> Address {
        self.key.address()
    }

    pub async fn identity(&self) -> Result<BootReport> {
        self.client.report(&self.key).await
    }

    pub async fn vault_status(&self, agent_id: U256) -> Result<VaultStatus> {
        self.client.vault_status(agent_id).await
    }

    pub async fn pay(&self, agent_id: U256, recipient: Address, amount: U256) -> Result<B256> {
        self.client
            .pay(&self.key, agent_id, recipient, amount)
            .await
    }

    pub async fn attest(
        &self,
        agent_id: U256,
        action: &str,
        params: &str,
        request_uri: &str,
    ) -> Result<ActionAttestation> {
        self.client
            .attest(&self.key, agent_id, action, params, request_uri)
            .await
    }
}

/// Load or create the device key, then read the chain. `key_dir` is the
/// install config directory, so the key is encrypted with that install's
/// secret store. The chain's binding, owner, and vault limits are the
/// result; this function does not consult a local cache.
pub async fn boot(config: &ChainConfig, key_dir: &Path) -> Result<BootReport> {
    DeviceChain::open(config, key_dir).await?.identity().await
}
