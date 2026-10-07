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
        function topUpGas(uint256 agentId, uint256 amount) external;
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

/// The device acts while it holds more than 1 MON, and a refill never aims above 5 MON.
pub const MIN_DEVICE_GAS_WEI: u128 = 1_000_000_000_000_000_000;
pub const MAX_DEVICE_GAS_WEI: u128 = 5_000_000_000_000_000_000;
const _: () = assert!(MAX_DEVICE_GAS_WEI > MIN_DEVICE_GAS_WEI);

/// True when the device must take gas from the vault before the next transaction.
pub fn needs_gas_refill(balance: U256) -> bool {
    balance <= U256::from(MIN_DEVICE_GAS_WEI)
}

/// The vault cannot cover a gas top-up that would leave the device above 1 MON.
#[derive(Debug, PartialEq, Eq)]
pub struct VaultGasShort;

impl std::fmt::Display for VaultGasShort {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the vault does not have enough MON for device gas")
    }
}

impl std::error::Error for VaultGasShort {}

pub fn is_vault_gas_short(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| cause.is::<VaultGasShort>())
}

/// Amount the device should pass to `topUpGas`.
///
/// `Ok(None)` means the device already holds more than 1 MON, or the spending
/// caps cannot lift it above 1 MON. `Err(VaultGasShort)` means the vault
/// itself cannot.
pub fn gas_top_up_amount(
    balance: U256,
    vault_balance: U256,
    per_tx_cap: U256,
    spent_today: U256,
    daily_cap: U256,
) -> Result<Option<U256>, VaultGasShort> {
    if !needs_gas_refill(balance) {
        return Ok(None);
    }
    let room = U256::from(MAX_DEVICE_GAS_WEI).saturating_sub(balance);
    let daily_left = daily_cap.saturating_sub(spent_today);
    let cap_amount = room.min(per_tx_cap).min(daily_left);
    let amount = cap_amount.min(vault_balance);
    if !amount.is_zero() && balance.saturating_add(amount) > U256::from(MIN_DEVICE_GAS_WEI) {
        return Ok(Some(amount));
    }
    let minimum = U256::from(MIN_DEVICE_GAS_WEI)
        .saturating_sub(balance)
        .saturating_add(U256::from(1u64));
    if vault_balance < minimum {
        return Err(VaultGasShort);
    }
    Ok(None)
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
        ensure_supported_network(config)?;
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
        self.ensure_gas(key, agent_id).await?;
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
        self.ensure_gas(key, agent_id).await?;
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
        self.ensure_gas(key, agent_id).await?;
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
        self.ensure_gas(key, agent_id).await?;
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
        self.ensure_gas(key, agent_id).await?;
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

    /// Call `topUpGas` when the device holds 1 MON or less.
    /// The amount stays inside the per-transaction cap and never aims above 5 MON.
    async fn ensure_gas(&self, key: &SoftwareKey, agent_id: U256) -> Result<()> {
        let balance = self
            .provider
            .get_balance(key.address())
            .await
            .map_err(|_| anyhow::Error::msg("failed to read the device gas balance"))?;
        if !needs_gas_refill(balance) {
            return Ok(());
        }
        let status = self.vault_status(agent_id).await?;
        let amount = match gas_top_up_amount(
            balance,
            status.balance,
            status.per_tx_cap,
            status.spent_today,
            status.daily_cap,
        ) {
            Ok(Some(amount)) => amount,
            Ok(None) => {
                return Err(anyhow::Error::msg(
                    "the spending cap is too low to top up device gas",
                ));
            }
            Err(short) => return Err(anyhow::Error::new(short)),
        };
        let provider = self.signing_provider(key)?;
        let pending = IDeviceVault::new(self.vault, provider)
            .topUpGas(agent_id, amount)
            .send()
            .await
            .map_err(|error| {
                anyhow::Error::msg(format!(
                    "the device could not take gas from the vault: {error}"
                ))
            })?;
        transaction_hash(pending).await?;
        let balance = self
            .provider
            .get_balance(key.address())
            .await
            .map_err(|_| anyhow::Error::msg("failed to read the device gas balance"))?;
        if needs_gas_refill(balance) {
            return Err(anyhow::Error::new(VaultGasShort));
        }
        Ok(())
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
/// is signed only after the owner address is known, and that signature goes
/// in the public claim link rather than the QR.
pub fn unclaimed_claim_text(device: Address) -> String {
    format!("kinetic:{device}")
}

/// Refuse mainnet while the config still names the published testnet
/// contracts, and refuse a testnet label pointed at chain id 143 with those
/// same addresses.
pub fn ensure_supported_network(config: &ChainConfig) -> Result<()> {
    match config.network.as_str() {
        "testnet" => {
            if config.chain_id == 143 && points_at_published_testnet(config) {
                anyhow::bail!(
                    "chain id 143 is Monad mainnet, and these contract addresses are the testnet deployment"
                );
            }
            Ok(())
        }
        "mainnet" => {
            if config.chain_id != 143 || points_at_published_testnet(config) {
                anyhow::bail!("Monad mainnet has no KineticVM contracts yet");
            }
            Ok(())
        }
        _ => anyhow::bail!("chain.network must be testnet or mainnet"),
    }
}

fn points_at_published_testnet(config: &ChainConfig) -> bool {
    let published = ChainConfig::default();
    config.registry.eq_ignore_ascii_case(&published.registry)
        || config.vault.eq_ignore_ascii_case(&published.vault)
        || config.attestor.eq_ignore_ascii_case(&published.attestor)
}

/// The claim page has to be a public https origin. A phone wallet opens it.
pub fn ensure_public_claim_url(claim_url: &str) -> Result<()> {
    let Some(host) = claim_url_host(claim_url) else {
        anyhow::bail!("chain.claim_url must be a public https address");
    };
    let blocked = host.eq_ignore_ascii_case("localhost")
        || host.eq_ignore_ascii_case("0.0.0.0")
        || host == "::1"
        || host.starts_with("127.")
        || host.ends_with(".local")
        || host.ends_with(".localhost");
    if blocked {
        anyhow::bail!("chain.claim_url must be a public https address");
    }
    Ok(())
}

/// Query the shared claim page can submit without calling the device.
pub struct ClaimPageQuery<'a> {
    pub claim_url: &'a str,
    pub device: Address,
    pub owner: Address,
    pub signature: &'a [u8],
    pub deadline: U256,
    pub per_tx_cap: &'a str,
    pub daily_cap: &'a str,
    pub chain_id: u64,
    pub device_name: &'a str,
    pub registry: &'a str,
    pub vault: &'a str,
    pub attestor: &'a str,
    pub rpc_url: &'a str,
}

pub fn claim_page_url(query: &ClaimPageQuery<'_>) -> Result<String> {
    ensure_public_claim_url(query.claim_url)?;
    if let Some(short) = compact_claim_url(query) {
        return Ok(short);
    }
    let mut url = claim_base(query.claim_url);
    let mut first = !url.contains('?');
    let signature = format!("0x{}", hex::encode(query.signature));
    let deadline = query.deadline.to_string();
    let chain_id = query.chain_id.to_string();
    let fields = [
        ("device", query.device.to_string()),
        ("owner", query.owner.to_string()),
        ("sig", signature),
        ("deadline", deadline),
        ("perTx", query.per_tx_cap.to_string()),
        ("daily", query.daily_cap.to_string()),
        ("chainId", chain_id),
        ("name", query.device_name.to_string()),
        ("registry", query.registry.to_string()),
        ("vault", query.vault.to_string()),
        ("attestor", query.attestor.to_string()),
        ("rpc", query.rpc_url.to_string()),
    ];
    for (key, value) in fields {
        url.push(if first { '?' } else { '&' });
        first = false;
        url.push_str(key);
        url.push('=');
        url.push_str(&query_component(&value));
    }
    Ok(url)
}

/// Telegram's copy button holds 256 characters. The long query string does
/// not, so a published testnet ticket is packed into `?t=`.
const COPY_LINK_LIMIT: usize = 256;

fn compact_claim_url(query: &ClaimPageQuery<'_>) -> Option<String> {
    let payload = compact_payload(query)?;
    let mut token = String::new();
    encode_base64url(&payload, &mut token);
    let mut url = claim_base(query.claim_url);
    url.push(if url.contains('?') { '&' } else { '?' });
    url.push_str("t=");
    url.push_str(&token);
    (url.chars().count() <= COPY_LINK_LIMIT).then_some(url)
}

fn compact_payload(query: &ClaimPageQuery<'_>) -> Option<Vec<u8>> {
    if !published_testnet(query) {
        return None;
    }
    let deadline = u64::try_from(query.deadline).ok()?;
    let chain_id = u32::try_from(query.chain_id).ok()?;
    let mut buf = Vec::with_capacity(160);
    buf.push(1);
    buf.extend_from_slice(&chain_id.to_be_bytes());
    buf.extend_from_slice(&deadline.to_be_bytes());
    buf.extend_from_slice(query.device.as_slice());
    buf.extend_from_slice(query.owner.as_slice());
    push_counted(&mut buf, query.signature)?;
    push_counted(&mut buf, query.per_tx_cap.as_bytes())?;
    push_counted(&mut buf, query.daily_cap.as_bytes())?;
    push_counted(&mut buf, query.device_name.as_bytes())?;
    Some(buf)
}

fn published_testnet(query: &ClaimPageQuery<'_>) -> bool {
    // contracts/deployments/10143.json. A custom deployment keeps the long URL.
    query.chain_id == 10_143
        && same_address(query.registry, "0xa361931269F7e0b1957175a48669F14735E61EDA")
        && same_address(query.vault, "0x77102fCAC7927bB64507DF82bf1ae659465B4EE5")
        && same_address(query.attestor, "0x6e46d149b7d3396b42E874765Cf5a1bb26BD5bA5")
}

fn same_address(left: &str, right: &str) -> bool {
    left.trim().eq_ignore_ascii_case(right)
}

fn push_counted(buf: &mut Vec<u8>, bytes: &[u8]) -> Option<()> {
    let len = u8::try_from(bytes.len()).ok()?;
    buf.push(len);
    buf.extend_from_slice(bytes);
    Some(())
}

fn encode_base64url(data: &[u8], out: &mut String) {
    const ALPHA: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut index = 0;
    while index + 3 <= data.len() {
        let n =
            ((data[index] as u32) << 16) | ((data[index + 1] as u32) << 8) | data[index + 2] as u32;
        out.push(ALPHA[((n >> 18) & 63) as usize] as char);
        out.push(ALPHA[((n >> 12) & 63) as usize] as char);
        out.push(ALPHA[((n >> 6) & 63) as usize] as char);
        out.push(ALPHA[(n & 63) as usize] as char);
        index += 3;
    }
    let rest = &data[index..];
    if rest.len() == 1 {
        let n = (rest[0] as u32) << 16;
        out.push(ALPHA[((n >> 18) & 63) as usize] as char);
        out.push(ALPHA[((n >> 12) & 63) as usize] as char);
    } else if rest.len() == 2 {
        let n = ((rest[0] as u32) << 16) | ((rest[1] as u32) << 8);
        out.push(ALPHA[((n >> 18) & 63) as usize] as char);
        out.push(ALPHA[((n >> 12) & 63) as usize] as char);
        out.push(ALPHA[((n >> 6) & 63) as usize] as char);
    }
}

#[cfg(test)]
fn decode_base64url(token: &str) -> Option<Vec<u8>> {
    fn val(byte: u8) -> Option<u8> {
        match byte {
            b'A'..=b'Z' => Some(byte - b'A'),
            b'a'..=b'z' => Some(byte - b'a' + 26),
            b'0'..=b'9' => Some(byte - b'0' + 52),
            b'-' => Some(62),
            b'_' => Some(63),
            _ => None,
        }
    }
    let raw = token.as_bytes();
    if raw.is_empty() || raw.len() % 4 == 1 {
        return None;
    }
    let mut out = Vec::new();
    let mut index = 0;
    while index + 4 <= raw.len() {
        let n = ((val(raw[index])? as u32) << 18)
            | ((val(raw[index + 1])? as u32) << 12)
            | ((val(raw[index + 2])? as u32) << 6)
            | val(raw[index + 3])? as u32;
        out.push((n >> 16) as u8);
        out.push((n >> 8) as u8);
        out.push(n as u8);
        index += 4;
    }
    match raw.len() - index {
        0 => {}
        2 => {
            let n = ((val(raw[index])? as u32) << 18) | ((val(raw[index + 1])? as u32) << 12);
            out.push((n >> 16) as u8);
        }
        3 => {
            let n = ((val(raw[index])? as u32) << 18)
                | ((val(raw[index + 1])? as u32) << 12)
                | ((val(raw[index + 2])? as u32) << 6);
            out.push((n >> 16) as u8);
            out.push((n >> 8) as u8);
        }
        _ => return None,
    }
    Some(out)
}

fn claim_base(claim_url: &str) -> String {
    let mut url = claim_url.trim().to_string();
    if let Some(hash) = url.find('#') {
        url.truncate(hash);
    }
    while url.ends_with('?') || url.ends_with('&') {
        url.pop();
    }
    url
}

fn claim_url_host(claim_url: &str) -> Option<&str> {
    let rest = claim_url.trim().strip_prefix("https://")?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let hostport = authority.rsplit('@').next().unwrap_or(authority);
    if let Some(inner) = hostport.strip_prefix('[') {
        return inner.split(']').next().filter(|host| !host.is_empty());
    }
    hostport.split(':').next().filter(|host| !host.is_empty())
}

fn query_component(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char);
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;
    use kinetic_config::schema::ChainConfig;

    #[test]
    fn gas_top_up_stays_inside_the_per_transaction_cap_and_five_mon() {
        let one = U256::from(MIN_DEVICE_GAS_WEI);
        let five = U256::from(MAX_DEVICE_GAS_WEI);
        assert_eq!(
            gas_top_up_amount(
                one,
                five,
                U256::from(10) * one,
                U256::ZERO,
                U256::from(50) * one
            ),
            Ok(Some(U256::from(4) * one))
        );
        assert_eq!(
            gas_top_up_amount(one, five, one, U256::ZERO, U256::from(2) * one),
            Ok(Some(one))
        );
        assert_eq!(
            gas_top_up_amount(U256::ZERO, U256::ZERO, one, U256::ZERO, one),
            Err(VaultGasShort)
        );
        assert_eq!(
            gas_top_up_amount(
                U256::from(500_000_000_000_000_000u128),
                one / U256::from(10),
                one,
                U256::ZERO,
                one
            ),
            Err(VaultGasShort)
        );
        assert_eq!(
            gas_top_up_amount(
                one / U256::from(2),
                U256::from(10) * one,
                one / U256::from(10),
                U256::ZERO,
                U256::from(10) * one
            ),
            Ok(None)
        );
        assert!(is_vault_gas_short(&anyhow::Error::new(VaultGasShort)));
    }

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
    fn the_claim_page_carries_the_ticket_and_the_deployment() {
        let mut config = ChainConfig::default();
        config.registry = "0x0000000000000000000000000000000000000001".into();
        let device: Address = "0x00000000000000000000000000000000000000a1"
            .parse()
            .expect("device");
        let owner: Address = "0x00000000000000000000000000000000000000b2"
            .parse()
            .expect("owner");
        let url = claim_page_url(&ClaimPageQuery {
            claim_url: &config.claim_url,
            device,
            owner,
            signature: &[0x01, 0x02, 0x03],
            deadline: U256::from(1_700_000_000u64),
            per_tx_cap: "0.05",
            daily_cap: "1",
            chain_id: config.chain_id,
            device_name: "vending 1",
            registry: &config.registry,
            vault: &config.vault,
            attestor: &config.attestor,
            rpc_url: &config.rpc_url,
        })
        .expect("claim url");
        assert!(url.starts_with("https://claim.kineticvm.xyz?"));
        assert!(url.contains(&format!("device={device}")));
        assert!(url.contains(&format!("owner={owner}")));
        assert!(url.contains("sig=0x010203"));
        assert!(url.contains("deadline=1700000000"));
        assert!(url.contains("perTx=0.05"));
        assert!(url.contains("daily=1"));
        assert!(url.contains("chainId=10143"));
        assert!(url.contains("name=vending%201"));
        assert!(url.contains(&format!("registry={}", config.registry)));
        assert!(url.contains(&format!("attestor={}", config.attestor)));
        assert!(!url.contains("never-the-key"));
        assert!(!url.contains("?t="));
    }

    #[test]
    fn a_published_claim_link_is_short_enough_to_copy() {
        let config = ChainConfig::default();
        let device: Address = "0x00000000000000000000000000000000000000a1"
            .parse()
            .expect("device");
        let owner: Address = "0x00000000000000000000000000000000000000b2"
            .parse()
            .expect("owner");
        let signature = [0x11u8; 65];
        let url = claim_page_url(&ClaimPageQuery {
            claim_url: &config.claim_url,
            device,
            owner,
            signature: &signature,
            deadline: U256::from(1_900_000_000u64),
            per_tx_cap: "5",
            daily_cap: "50",
            chain_id: config.chain_id,
            device_name: "Newton Vending Machine",
            registry: &config.registry,
            vault: &config.vault,
            attestor: &config.attestor,
            rpc_url: &config.rpc_url,
        })
        .expect("claim url");
        let token = url
            .strip_prefix("https://claim.kineticvm.xyz?t=")
            .expect("short token");
        assert!(url.chars().count() <= 256);
        let bytes = decode_base64url(token).expect("token");
        assert_eq!(bytes[0], 1);
        assert_eq!(
            u32::from_be_bytes(bytes[1..5].try_into().expect("chain")),
            10_143
        );
        assert_eq!(
            u64::from_be_bytes(bytes[5..13].try_into().expect("deadline")),
            1_900_000_000
        );
        assert_eq!(&bytes[13..33], device.as_slice());
        assert_eq!(&bytes[33..53], owner.as_slice());
        assert_eq!(bytes[53], 65);
        assert_eq!(&bytes[54..119], &signature);
        assert_eq!(&bytes[120..121], b"5");
        assert_eq!(&bytes[122..124], b"50");
        assert_eq!(&bytes[125..], b"Newton Vending Machine");
    }

    #[test]
    fn a_local_claim_page_is_refused() {
        let error = ensure_public_claim_url("http://localhost:3000/claim")
            .expect_err("http localhost")
            .to_string();
        assert!(error.contains("public https"));
        assert!(ensure_public_claim_url("https://127.0.0.1/claim").is_err());
        assert!(ensure_public_claim_url("https://localhost/claim").is_err());
        assert!(ensure_public_claim_url("https://[::1]/claim").is_err());
    }

    #[test]
    fn mainnet_keeps_the_published_testnet_contracts_offline() {
        let mainnet = ChainConfig {
            network: "mainnet".into(),
            chain_id: 143,
            ..ChainConfig::default()
        };
        assert!(ensure_supported_network(&mainnet).is_err());

        let mislabeled = ChainConfig {
            chain_id: 143,
            ..ChainConfig::default()
        };
        assert!(ensure_supported_network(&mislabeled).is_err());
        assert!(ensure_supported_network(&ChainConfig::default()).is_ok());
    }

    #[test]
    fn a_private_mainnet_deployment_is_accepted() {
        let config = ChainConfig {
            network: "mainnet".into(),
            chain_id: 143,
            registry: "0x1111111111111111111111111111111111111111".into(),
            vault: "0x2222222222222222222222222222222222222222".into(),
            attestor: "0x3333333333333333333333333333333333333333".into(),
            ..ChainConfig::default()
        };
        assert!(ensure_supported_network(&config).is_ok());
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
