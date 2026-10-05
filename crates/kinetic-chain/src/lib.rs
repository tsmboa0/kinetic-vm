//! Device-side Monad client: a software signing key, the boot read of the
//! device's agent, and a signed claim ticket once an owner is known.
//!
//! The secure-element backend is a later `DeviceSigner`. This crate does not
//! implement it.

mod client;
mod key;

pub use client::{
    Binding, BootReport, ChainClient, ClaimTicket, binding_from_device, unclaimed_claim_text,
};
pub use key::{DeviceSigner, SoftwareKey};

use std::path::Path;

use anyhow::Result;
use kinetic_config::schema::ChainConfig;

/// Load or create the device key, then read the chain. `key_dir` is the
/// install config directory, so the key is encrypted with that install's
/// secret store. The chain's binding, owner, and vault limits are the
/// result; this function does not consult a local cache.
pub async fn boot(config: &ChainConfig, key_dir: &Path) -> Result<BootReport> {
    if !config.enabled {
        anyhow::bail!("the Monad chain client is disabled in config");
    }
    let key = SoftwareKey::load_or_create(key_dir)?;
    let client = ChainClient::connect(config).await?;
    client.report(&key).await
}
