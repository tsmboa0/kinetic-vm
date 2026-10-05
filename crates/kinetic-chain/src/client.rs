//! Reads the deployed KineticRegistry and DeviceVault. The contract hashes
//! claim tickets; this client only signs that digest.

use alloy::primitives::{Address, B256, U256, keccak256};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::sol;
use alloy::sol_types::SolValue;
use anyhow::Result;
use kinetic_config::schema::ChainConfig;

use crate::key::{DeviceSigner, SoftwareKey};

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
        function balanceOf(uint256 agentId) external view returns (uint256);
        function spentToday(uint256 agentId) external view returns (uint256);
        function loosenNonce(uint256 agentId) external view returns (uint256);
        function hashLoosenCaps(uint256 agentId, uint256 perTxCap, uint256 dailyCap, uint256 nonce, uint256 deadline) external view returns (bytes32);
        function hashAllowRecipient(uint256 agentId, address recipient, uint256 nonce, uint256 deadline) external view returns (bytes32);
        function hashUnpause(uint256 agentId, uint256 nonce, uint256 deadline) external view returns (bytes32);
        function pay(uint256 agentId, address recipient, uint256 amount) external;
        function loosenCaps(uint256 agentId, uint256 perTxCap, uint256 dailyCap, uint256 deadline, bytes signature) external;
        function allowRecipient(uint256 agentId, address recipient, uint256 deadline, bytes signature) external;
        function unpause(uint256 agentId, uint256 deadline, bytes signature) external;
    }

    #[sol(rpc)]
    interface IKineticAttestor {
        struct ActionProof {
            uint256 agentId;
            bytes32 action;
            bytes32 paramsHash;
            string requestURI;
            uint256 timestamp;
            uint256 nonce;
        }

        function hashActionProof(ActionProof calldata proof) external view returns (bytes32);
        function attest(ActionProof calldata proof, bytes calldata signature) external;
        function nonceUsed(uint256 agentId, uint256 nonce) external view returns (bool);
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
    rpc_url: String,
    chain_id: u64,
    registry: Address,
    vault: Address,
    attestor: Address,
}

/// Vault balance and caps. Spending still goes through `pay`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VaultStatus {
    pub balance: U256,
    pub spent_today: U256,
    pub per_tx_cap: U256,
    pub daily_cap: U256,
    pub paused: bool,
}

/// A device-signed action that landed in the validation registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionAttestation {
    pub transaction_hash: B256,
    pub request_hash: B256,
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
            rpc_url: config.rpc_url.clone(),
            chain_id,
            registry: parse_address("registry", &config.registry)?,
            vault: parse_address("vault", &config.vault)?,
            attestor: parse_address("attestor", &config.attestor)?,
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

    pub async fn vault_status(&self, agent_id: U256) -> Result<VaultStatus> {
        let vault = IDeviceVault::new(self.vault, &self.provider);
        let limits = vault
            .limitsOf(agent_id)
            .call()
            .await
            .map_err(|_| anyhow::Error::msg("failed to read the vault limits"))?;
        let balance = vault
            .balanceOf(agent_id)
            .call()
            .await
            .map_err(|_| anyhow::Error::msg("failed to read the vault balance"))?;
        let spent_today = vault
            .spentToday(agent_id)
            .call()
            .await
            .map_err(|_| anyhow::Error::msg("failed to read today's vault spend"))?;
        Ok(VaultStatus {
            balance,
            spent_today,
            per_tx_cap: limits.perTxCap,
            daily_cap: limits.dailyCap,
            paused: limits.paused,
        })
    }

    /// Send value through `DeviceVault.pay`. The device key is the sender, so
    /// the contract's caps and allowlist still apply.
    pub async fn pay(
        &self,
        key: &SoftwareKey,
        agent_id: U256,
        recipient: Address,
        amount: U256,
    ) -> Result<B256> {
        if recipient == Address::ZERO {
            anyhow::bail!("the vault cannot pay the zero address");
        }
        if amount.is_zero() {
            anyhow::bail!("the vault cannot pay zero");
        }
        let provider = self.signing_provider(key)?;
        let pending = IDeviceVault::new(self.vault, provider)
            .pay(agent_id, recipient, amount)
            .send()
            .await
            .map_err(|error| anyhow::Error::msg(format!("vault payment failed: {error}")))?;
        transaction_hash(pending).await
    }

    pub fn chain_id(&self) -> u64 {
        self.chain_id
    }

    pub fn vault_address(&self) -> Address {
        self.vault
    }

    pub fn attestor_address(&self) -> Address {
        self.attestor
    }

    pub async fn block_timestamp(&self) -> Result<u64> {
        self.latest_timestamp().await
    }

    pub async fn loosen_nonce(&self, agent_id: U256) -> Result<U256> {
        IDeviceVault::new(self.vault, &self.provider)
            .loosenNonce(agent_id)
            .call()
            .await
            .map_err(|_| anyhow::Error::msg("failed to read the vault nonce"))
    }

    pub async fn unpause_digest(
        &self,
        agent_id: U256,
        nonce: U256,
        deadline: U256,
    ) -> Result<B256> {
        IDeviceVault::new(self.vault, &self.provider)
            .hashUnpause(agent_id, nonce, deadline)
            .call()
            .await
            .map_err(|_| anyhow::Error::msg("failed to hash the unpause digest"))
    }

    pub async fn loosen_digest(
        &self,
        agent_id: U256,
        per_tx_cap: U256,
        daily_cap: U256,
        nonce: U256,
        deadline: U256,
    ) -> Result<B256> {
        IDeviceVault::new(self.vault, &self.provider)
            .hashLoosenCaps(agent_id, per_tx_cap, daily_cap, nonce, deadline)
            .call()
            .await
            .map_err(|_| anyhow::Error::msg("failed to hash the loosen digest"))
    }

    pub async fn allow_digest(
        &self,
        agent_id: U256,
        recipient: Address,
        nonce: U256,
        deadline: U256,
    ) -> Result<B256> {
        IDeviceVault::new(self.vault, &self.provider)
            .hashAllowRecipient(agent_id, recipient, nonce, deadline)
            .call()
            .await
            .map_err(|_| anyhow::Error::msg("failed to hash the allow digest"))
    }

    /// Relay an owner EIP-712 signature. The device key is not the owner, so
    /// the contract checks the signature and consumes `loosenNonce`.
    pub async fn relay_unpause(
        &self,
        key: &SoftwareKey,
        agent_id: U256,
        deadline: U256,
        signature_hex: &str,
    ) -> Result<B256> {
        let signature = signature_bytes(signature_hex)?;
        let provider = self.signing_provider(key)?;
        let pending = IDeviceVault::new(self.vault, provider)
            .unpause(agent_id, deadline, signature.into())
            .send()
            .await
            .map_err(|_| anyhow::Error::msg("the vault call failed"))?;
        transaction_hash(pending).await
    }

    pub async fn relay_loosen(
        &self,
        key: &SoftwareKey,
        agent_id: U256,
        per_tx_cap: U256,
        daily_cap: U256,
        deadline: U256,
        signature_hex: &str,
    ) -> Result<B256> {
        let signature = signature_bytes(signature_hex)?;
        let provider = self.signing_provider(key)?;
        let pending = IDeviceVault::new(self.vault, provider)
            .loosenCaps(agent_id, per_tx_cap, daily_cap, deadline, signature.into())
            .send()
            .await
            .map_err(|_| anyhow::Error::msg("the vault call failed"))?;
        transaction_hash(pending).await
    }

    pub async fn relay_allow(
        &self,
        key: &SoftwareKey,
        agent_id: U256,
        recipient: Address,
        deadline: U256,
        signature_hex: &str,
    ) -> Result<B256> {
        let signature = signature_bytes(signature_hex)?;
        let provider = self.signing_provider(key)?;
        let pending = IDeviceVault::new(self.vault, provider)
            .allowRecipient(agent_id, recipient, deadline, signature.into())
            .send()
            .await
            .map_err(|_| anyhow::Error::msg("the vault call failed"))?;
        transaction_hash(pending).await
    }

    /// Last few `Attested` transaction hashes for this agent. The chain is
    /// the source; this does not keep a local copy.
    pub async fn recent_attestation_txs(&self, agent_id: U256, limit: usize) -> Result<Vec<B256>> {
        let topic = B256::from(agent_id.to_be_bytes());
        let filter = alloy::rpc::types::Filter::new()
            .address(self.attestor)
            .event("Attested(uint256,address,bytes32,bytes32)")
            .topic1(topic)
            .from_block(0u64);
        let logs = self
            .provider
            .get_logs(&filter)
            .await
            .map_err(|_| anyhow::Error::msg("failed to read attestation logs"))?;
        let mut hashes = Vec::new();
        for log in logs {
            if let Some(hash) = log.transaction_hash {
                hashes.push(hash);
            }
        }
        if hashes.len() > limit {
            hashes.drain(0..hashes.len() - limit);
        }
        Ok(hashes)
    }

    /// Sign the action with the device key and submit it to the attestor.
    /// The contract hashes the proof; this method only signs that digest.
    pub async fn attest(
        &self,
        key: &SoftwareKey,
        agent_id: U256,
        action: &str,
        params: &str,
        request_uri: &str,
    ) -> Result<ActionAttestation> {
        if request_uri.is_empty() {
            anyhow::bail!("an attestation needs a request URI");
        }
        let timestamp = U256::from(self.latest_timestamp().await?);
        let nonce = self.fresh_nonce(agent_id, timestamp).await?;
        let action_hash = keccak256(action.as_bytes());
        let params_hash = keccak256(params.as_bytes());
        let proof = IKineticAttestor::ActionProof {
            agentId: agent_id,
            action: action_hash,
            paramsHash: params_hash,
            requestURI: request_uri.to_string(),
            timestamp,
            nonce,
        };
        let digest = IKineticAttestor::new(self.attestor, &self.provider)
            .hashActionProof(proof.clone())
            .call()
            .await
            .map_err(|_| anyhow::Error::msg("failed to hash the action proof"))?;
        let signature = key.sign_hash(&digest)?.as_bytes().to_vec();
        let provider = self.signing_provider(key)?;
        let pending = IKineticAttestor::new(self.attestor, provider)
            .attest(proof, signature.into())
            .send()
            .await
            .map_err(|error| anyhow::Error::msg(format!("attestation failed: {error}")))?;
        let transaction_hash = transaction_hash(pending).await?;
        Ok(ActionAttestation {
            transaction_hash,
            request_hash: action_request_hash(
                agent_id,
                action_hash,
                params_hash,
                request_uri,
                timestamp,
                nonce,
            ),
        })
    }

    fn signing_provider(&self, key: &SoftwareKey) -> Result<impl Provider + Clone> {
        let rpc_url = self
            .rpc_url
            .parse()
            .map_err(|_| anyhow::Error::msg("config chain.rpc_url is not a URL"))?;
        let wallet = alloy::network::EthereumWallet::from(key.transaction_signer());
        Ok(ProviderBuilder::new()
            .with_chain_id(self.chain_id)
            .wallet(wallet)
            .connect_http(rpc_url))
    }

    async fn latest_timestamp(&self) -> Result<u64> {
        let block = self
            .provider
            .get_block(alloy::eips::BlockId::latest())
            .await
            .map_err(|_| anyhow::Error::msg("failed to read the latest block"))?
            .ok_or_else(|| anyhow::Error::msg("the chain returned no latest block"))?;
        Ok(block.header.timestamp)
    }

    async fn fresh_nonce(&self, agent_id: U256, timestamp: U256) -> Result<U256> {
        let attestor = IKineticAttestor::new(self.attestor, &self.provider);
        for step in 0..4u64 {
            let nonce = timestamp + U256::from(step);
            let used = attestor
                .nonceUsed(agent_id, nonce)
                .call()
                .await
                .map_err(|_| anyhow::Error::msg("failed to read the attestation nonce"))?;
            if !used {
                return Ok(nonce);
            }
        }
        anyhow::bail!("the attestation nonce window is already used")
    }
}

async fn transaction_hash(
    pending: alloy::providers::PendingTransactionBuilder<alloy::network::Ethereum>,
) -> Result<B256> {
    let receipt = pending
        .get_receipt()
        .await
        .map_err(|_| anyhow::Error::msg("the chain transaction was not confirmed"))?;
    if !receipt.status() {
        anyhow::bail!("the chain transaction reverted");
    }
    Ok(receipt.transaction_hash)
}

/// `keccak256(abi.encode(agentId, action, paramsHash, keccak256(uri), timestamp, nonce))`.
/// This matches `KineticAttestor`'s request hash.
pub fn action_request_hash(
    agent_id: U256,
    action: B256,
    params_hash: B256,
    request_uri: &str,
    timestamp: U256,
    nonce: U256,
) -> B256 {
    let uri_hash = keccak256(request_uri.as_bytes());
    keccak256((agent_id, action, params_hash, uri_hash, timestamp, nonce).abi_encode())
}

/// Exact text the owner personal-signs to bind a Telegram chat. The chat id
/// is inside the signed text, so a signature for one chat cannot link another.
pub fn owner_link_message(chain_id: u64, agent_id: U256, telegram_id: &str) -> String {
    format!("KineticVM owner\nchain {chain_id}\nagent {agent_id}\ntelegram {telegram_id}")
}

/// Recover the signer of an EIP-191 personal signature over `message`.
pub fn recover_personal_signer(message: &str, signature_hex: &str) -> Result<Address> {
    let signature: alloy::primitives::Signature = signature_hex
        .trim()
        .parse()
        .map_err(|_| anyhow::Error::msg("the signature is not a 65-byte signature"))?;
    signature
        .recover_address_from_msg(message.as_bytes())
        .map_err(|_| anyhow::Error::msg("the signature does not recover an address"))
}

fn signature_bytes(raw: &str) -> Result<Vec<u8>> {
    let signature: alloy::primitives::Signature = raw
        .trim()
        .parse()
        .map_err(|_| anyhow::Error::msg("the signature is not a 65-byte signature"))?;
    Ok(signature.as_bytes().to_vec())
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
    fn the_smoke_attestation_request_hash_matches_the_contract_encoding() {
        let hash = action_request_hash(
            U256::from(2005),
            keccak256("vend"),
            keccak256("slot-1"),
            "ipfs://kinetic-smoke-action",
            U256::from(1_791_189_746u64),
            U256::from(1),
        );
        let expected: B256 = "0xa1ac0a2eceae764ce6d004d64644053989210274006583536782a5d0d4a32ed9"
            .parse()
            .expect("request hash");
        assert_eq!(hash, expected);
    }

    #[test]
    fn a_personal_signature_binds_the_telegram_id() {
        let dir = tempfile::tempdir().expect("temp dir");
        let key = SoftwareKey::load_or_create(dir.path()).expect("key");
        let message = owner_link_message(10143, U256::from(2005), "42");
        let other = owner_link_message(10143, U256::from(2005), "99");
        let digest = alloy::primitives::eip191_hash_message(message.as_bytes());
        let signature = key.sign_hash(&digest).expect("sign");
        let hex = format!("0x{}", hex::encode(signature.as_bytes()));
        let recovered = recover_personal_signer(&message, &hex).expect("recover");
        assert_eq!(recovered, key.address());
        let mismatched = recover_personal_signer(&other, &hex).expect("recover other");
        assert_ne!(mismatched, key.address());
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
