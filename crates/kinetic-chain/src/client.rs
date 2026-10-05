//! Reads the deployed KineticRegistry and DeviceVault. The contract hashes
//! claim tickets; this client only signs that digest.

use alloy::primitives::{Address, U256};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::sol;
use anyhow::Result;
use kinetic_config::schema::ChainConfig;

use crate::key::DeviceSigner;

sol! {
    #[sol(rpc)]
    interface IKineticRegistry {
        function deviceToAgent(address device) external view returns (uint256 agentId, bool bound);
        function ownerOfAgent(uint256 agentId) external view returns (address);
        function activeDevice(uint256 agentId) external view returns (address);
        function claimNonce(address device) external view returns (uint256);
        function hashClaim(address device, address owner_, uint256 nonce, uint256 deadline) external view returns (bytes32);
    }

    #[sol(rpc)]
    interface IDeviceVault {
        function limitsOf(uint256 agentId) external view returns (uint256 perTxCap, uint256 dailyCap, bool paused);
    }
}

/// What the chain says about this device. Chain state wins over any local note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BootReport {
    Unclaimed {
        device: Address,
    },
    Claimed {
        device: Address,
        agent_id: U256,
        owner: Address,
        per_tx_cap: U256,
        daily_cap: U256,
        paused: bool,
    },
}

/// Device signature over a claim ticket that names the owner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimTicket {
    pub owner: Address,
    pub nonce: U256,
    pub deadline: U256,
    pub signature: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Binding {
    Unclaimed,
    Claimed { agent_id: U256 },
}

/// `bound` is the fact. An agent id of zero can still be claimed.
pub fn binding_from_device(agent_id: U256, bound: bool) -> Binding {
    if bound {
        Binding::Claimed { agent_id }
    } else {
        Binding::Unclaimed
    }
}

pub struct ChainClient {
    provider: alloy::providers::DynProvider,
    registry: Address,
    vault: Address,
}

impl ChainClient {
    pub async fn connect(config: &ChainConfig) -> Result<Self> {
        if !config.enabled {
            anyhow::bail!("the Monad chain client is disabled in config");
        }
        let rpc_url = config
            .rpc_url
            .parse()
            .map_err(|_| anyhow::Error::msg("config chain.rpc_url is not a URL"))?;
        let provider = ProviderBuilder::new().connect_http(rpc_url).erased();
        let chain_id = provider
            .get_chain_id()
            .await
            .map_err(|_| anyhow::Error::msg("failed to read the chain id"))?;
        if chain_id != config.chain_id {
            anyhow::bail!(
                "RPC chain id {chain_id} does not match config chain.chain_id {}",
                config.chain_id
            );
        }
        Ok(Self {
            provider,
            registry: parse_address("registry", &config.registry)?,
            vault: parse_address("vault", &config.vault)?,
        })
    }

    pub async fn report(&self, signer: &impl DeviceSigner) -> Result<BootReport> {
        let device = signer.address();
        let found = IKineticRegistry::new(self.registry, &self.provider)
            .deviceToAgent(device)
            .call()
            .await
            .map_err(|_| anyhow::Error::msg("failed to read the device binding"))?;
        ::kinetic_log::record!(
            INFO,
            ::kinetic_log::Event::new(module_path!(), ::kinetic_log::Action::Note)
                .with_outcome(::kinetic_log::EventOutcome::Success)
                .with_attrs(::serde_json::json!({
                    "device": format!("{device}"),
                    "bound": found.bound,
                })),
            "read device binding from the chain"
        );
        let Binding::Claimed { agent_id } = binding_from_device(found.agentId, found.bound) else {
            return Ok(BootReport::Unclaimed { device });
        };
        let active = IKineticRegistry::new(self.registry, &self.provider)
            .activeDevice(agent_id)
            .call()
            .await
            .map_err(|_| {
                anyhow::Error::msg(
                    "device binding is not active; the owner may have transferred the agent",
                )
            })?;
        if active != device {
            anyhow::bail!("the chain bound this agent to a different device");
        }
        let owner = IKineticRegistry::new(self.registry, &self.provider)
            .ownerOfAgent(agent_id)
            .call()
            .await
            .map_err(|_| anyhow::Error::msg("failed to read the agent owner"))?;
        let limits = IDeviceVault::new(self.vault, &self.provider)
            .limitsOf(agent_id)
            .call()
            .await
            .map_err(|_| anyhow::Error::msg("failed to read the vault limits"))?;
        Ok(BootReport::Claimed {
            device,
            agent_id,
            owner,
            per_tx_cap: limits.perTxCap,
            daily_cap: limits.dailyCap,
            paused: limits.paused,
        })
    }

    /// Sign the digest `hashClaim` returns. The ticket names `owner`, so it
    /// cannot be built until that address is known.
    pub async fn sign_claim(
        &self,
        signer: &impl DeviceSigner,
        owner: Address,
        deadline: U256,
    ) -> Result<ClaimTicket> {
        let device = signer.address();
        if owner == device {
            anyhow::bail!("the device cannot be the owner of its own agent");
        }
        let registry = IKineticRegistry::new(self.registry, &self.provider);
        let nonce = registry
            .claimNonce(device)
            .call()
            .await
            .map_err(|_| anyhow::Error::msg("failed to read the claim nonce"))?;
        let digest = registry
            .hashClaim(device, owner, nonce, deadline)
            .call()
            .await
            .map_err(|_| anyhow::Error::msg("failed to hash the claim ticket"))?;
        let signature = signer.sign_hash(&digest)?;
        Ok(ClaimTicket {
            owner,
            nonce,
            deadline,
            signature: signature.as_bytes().to_vec(),
        })
    }
}

fn parse_address(field: &str, value: &str) -> Result<Address> {
    value
        .parse::<Address>()
        .map_err(|_| anyhow::Error::msg(format!("config chain.{field} is not an address")))
}

/// Text a later claim screen can put in a QR code. The claim ticket itself
/// is signed only after the owner address is known.
pub fn unclaimed_claim_text(device: Address) -> String {
    format!("kinetic:{device}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use kinetic_config::schema::ChainConfig;

    #[test]
    fn agent_id_zero_stays_claimed_when_the_binding_says_so() {
        let binding = binding_from_device(U256::ZERO, true);
        assert_eq!(
            binding,
            Binding::Claimed {
                agent_id: U256::ZERO
            }
        );
    }

    #[test]
    fn an_unbound_device_ignores_a_leftover_agent_id() {
        let binding = binding_from_device(U256::from(2005), false);
        assert_eq!(binding, Binding::Unclaimed);
    }

    #[test]
    fn defaults_match_the_published_testnet_deployment() {
        let published: serde_json::Value =
            serde_json::from_str(include_str!("../../../contracts/deployments/10143.json"))
                .expect("deployment json");
        let config = ChainConfig::default();
        assert!(!config.enabled);
        assert_eq!(
            config.chain_id,
            published["chainId"].as_u64().expect("chain id")
        );
        assert_eq!(
            config.registry,
            published["kineticRegistry"].as_str().expect("registry")
        );
        assert_eq!(
            config.vault,
            published["deviceVault"].as_str().expect("vault")
        );
        assert_eq!(
            config.attestor,
            published["kineticAttestor"].as_str().expect("attestor")
        );
    }

    #[test]
    fn a_config_without_a_chain_section_keeps_the_published_defaults() {
        let config: ChainConfig = toml::from_str("").expect("empty chain config");
        assert!(config.is_default());
    }

    #[test]
    fn unclaimed_text_names_the_device_and_not_a_ticket() {
        let device: Address = "0x9Df116134f653ec363F64667D39edea4B3649C16"
            .parse()
            .expect("address");
        let text = unclaimed_claim_text(device);
        assert!(text.starts_with("kinetic:0x"));
        assert!(!text.contains("signature"));
    }
}
