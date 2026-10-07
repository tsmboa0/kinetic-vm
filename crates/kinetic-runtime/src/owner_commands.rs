//! Telegram commands for the onchain owner.
//!
//! The bound chat is the owner chat. Every command still re-reads `ownerOf`.
//! Spending changes still need an owner-wallet signature.

use std::path::Path;

use kinetic_chain::{Address, B256, BootReport, DeviceChain, U256, format_mon, parse_mon};
use kinetic_config::schema::Config;

use crate::i18n::{get_required_cli_string, get_required_cli_string_with_args};

const SIGNATURE_TTL_SECS: u64 = 60 * 60;
const TESTNET_CHAIN_ID: u64 = 10143;

enum CapChange {
    Same,
    Tighten,
    Loosen,
    Mixed,
}

/// Reply for one Telegram owner command. Chain work runs only when `[chain]`
/// is enabled and the sender is the paired Telegram chat.
pub async fn handle(config: &Config, telegram_id: &str, line: &str) -> String {
    if !config.chain.enabled {
        return text("cli-chain-disabled");
    }
    let Some((name, args)) = split_line(line) else {
        return text("cli-owner-usage-link");
    };
    if !valid_telegram_id(telegram_id) {
        return text("cli-owner-missing-sender");
    }
    let Some(dir) = config_dir(config) else {
        return chain_err();
    };
    match name.as_str() {
        "link" => text("cli-owner-link-retired"),
        "status" | "pause" | "resume" | "limits" | "approve" | "deposit" | "withdraw" => {
            if !bound_owner_chat(config, telegram_id) {
                return text("cli-owner-not-bound");
            }
            owned_command(config, dir, &name, &args).await
        }
        _ => text("cli-owner-usage-link"),
    }
}

fn bound_owner_chat(config: &Config, telegram_id: &str) -> bool {
    config.peer_groups.values().any(|group| {
        let channel = group.channel.as_str();
        let telegram = channel == "telegram" || channel.starts_with("telegram.");
        if !telegram {
            return false;
        }
        let ignored: Vec<String> = group
            .ignore
            .iter()
            .map(|peer| peer.as_str().trim().trim_start_matches('@').to_string())
            .collect();
        group.external_peers.iter().any(|peer| {
            let id = peer.as_str().trim().trim_start_matches('@');
            id == telegram_id && !ignored.iter().any(|item| item == id)
        })
    })
}

async fn owned_command(config: &Config, dir: &Path, name: &str, args: &[String]) -> String {
    let Some(claimed) = open_claimed(config, dir).await else {
        return claimed_or_err(config, dir).await;
    };
    match name {
        "status" => status_text(&claimed).await,
        "pause" => pause_text(&claimed),
        "resume" => resume_command(config, &claimed, args).await,
        "limits" => limits_command(config, &claimed, args).await,
        "approve" => approve_command(config, &claimed, args).await,
        "deposit" => vault_page(&claimed, "cli-owner-deposit", ""),
        "withdraw" => vault_page(&claimed, "cli-owner-withdraw", "withdraw"),
        _ => text("cli-owner-not-linked"),
    }
}

struct ClaimedSession {
    chain: DeviceChain,
    chain_id: u64,
    device: Address,
    agent_id: U256,
    owner: Address,
    per_tx_cap: U256,
    daily_cap: U256,
    paused: bool,
}

async fn open_claimed(config: &Config, dir: &Path) -> Option<ClaimedSession> {
    let chain = DeviceChain::open(&config.chain, dir).await.ok()?;
    let chain_id = chain.chain_id();
    let BootReport::Claimed {
        device,
        agent_id,
        owner,
        per_tx_cap,
        daily_cap,
        paused,
    } = chain.identity().await.ok()?
    else {
        return None;
    };
    Some(ClaimedSession {
        chain,
        chain_id,
        device,
        agent_id,
        owner,
        per_tx_cap,
        daily_cap,
        paused,
    })
}

/// `open_claimed` collapses "unclaimed" and "chain call failed". This second
/// look distinguishes them for the reply, without printing a signature.
async fn claimed_or_err(config: &Config, dir: &Path) -> String {
    let Ok(chain) = DeviceChain::open(&config.chain, dir).await else {
        return chain_err();
    };
    match chain.identity().await {
        Ok(BootReport::Unclaimed { .. }) => text("cli-chain-not-claimed"),
        Ok(BootReport::Claimed { .. }) => chain_err(),
        Err(_) => chain_err(),
    }
}

async fn status_text(claimed: &ClaimedSession) -> String {
    let vault = match claimed.chain.vault_status(claimed.agent_id).await {
        Ok(vault) => vault,
        Err(_) => return chain_err(),
    };
    let attestations = attestation_text(claimed).await;
    let agent = claimed.agent_id.to_string();
    let device = claimed.device.to_string();
    let owner = claimed.owner.to_string();
    let balance = format_mon(vault.balance);
    let spent = format_mon(vault.spent_today);
    let per_tx = format_mon(vault.per_tx_cap);
    let daily = format_mon(vault.daily_cap);
    let paused = paused_word(vault.paused);
    text_with(
        "cli-owner-status",
        &[
            ("agent", agent.as_str()),
            ("device", device.as_str()),
            ("owner", owner.as_str()),
            ("balance", balance.as_str()),
            ("spent", spent.as_str()),
            ("per_tx", per_tx.as_str()),
            ("daily", daily.as_str()),
            ("paused", paused.as_str()),
            ("attestations", attestations.as_str()),
        ],
    )
}

async fn attestation_text(claimed: &ClaimedSession) -> String {
    let url = explorer_address(claimed.chain_id, claimed.chain.attestor_address());
    match claimed.chain.recent_attestations(claimed.agent_id, 5).await {
        Ok(hashes) if !hashes.is_empty() => {
            let txs = hashes
                .iter()
                .map(|hash| explorer_tx(claimed.chain_id, *hash))
                .collect::<Vec<_>>()
                .join("\n");
            text_with("cli-owner-attestations", &[("txs", txs.as_str())])
        }
        _ => text_with("cli-owner-no-attestations", &[("url", url.as_str())]),
    }
}

fn vault_page(claimed: &ClaimedSession, key: &str, path: &str) -> String {
    let vault = claimed.chain.vault_address().to_string();
    let agent = claimed.agent_id.to_string();
    let url = if path.is_empty() {
        format!("https://vault-deposit.kineticvm.xyz?address={vault}&agent={agent}")
    } else {
        format!("https://vault-deposit.kineticvm.xyz/{path}?address={vault}&agent={agent}")
    };
    text_with(key, &[("url", url.as_str())])
}

/// The paired Telegram chat, and the bot that can post into it.
#[derive(Clone)]
pub struct OwnerPing {
    chat_id: String,
    bot_token: String,
}

impl OwnerPing {
    pub fn from_config(config: &Config) -> Option<Self> {
        let chat_id = config.peer_groups.values().find_map(|group| {
            let channel = group.channel.as_str();
            if channel != "telegram" && !channel.starts_with("telegram.") {
                return None;
            }
            group.external_peers.iter().find_map(|peer| {
                let id = peer.as_str().trim().trim_start_matches('@');
                if !id.is_empty() && id.chars().all(|c| c.is_ascii_digit()) {
                    Some(id.to_string())
                } else {
                    None
                }
            })
        })?;
        let channel = config
            .channels
            .telegram
            .get("default")
            .or_else(|| config.channels.telegram.values().next())?;
        let bot_token = channel.bot_token.trim();
        if !channel.enabled || bot_token.is_empty() {
            return None;
        }
        Some(Self {
            chat_id,
            bot_token: bot_token.to_string(),
        })
    }

    pub async fn tell_vault_needs_funds(&self, vault: &str, agent_id: &str) {
        let page = format!("https://vault-deposit.kineticvm.xyz?address={vault}&agent={agent_id}");
        let text = text_with("cli-owner-gas-short", &[("url", page.as_str())]);
        let endpoint = format!("https://api.telegram.org/bot{}/sendMessage", self.bot_token);
        let Ok(client) = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(8))
            .build()
        else {
            return;
        };
        if client
            .post(endpoint)
            .json(&serde_json::json!({
                "chat_id": self.chat_id,
                "text": text,
            }))
            .send()
            .await
            .is_err()
        {
            ::kinetic_log::record!(
                WARN,
                ::kinetic_log::Event::new(module_path!(), ::kinetic_log::Action::Note),
                "failed to tell the owner the vault needs funds"
            );
        }
    }
}

fn pause_text(claimed: &ClaimedSession) -> String {
    let agent = claimed.agent_id.to_string();
    let vault = claimed.chain.vault_address().to_string();
    text_with(
        "cli-owner-pause",
        &[("agent", agent.as_str()), ("vault", vault.as_str())],
    )
}

async fn resume_command(config: &Config, claimed: &ClaimedSession, args: &[String]) -> String {
    match args {
        [] => prepare_unpause(claimed).await,
        [deadline, signature] if looks_like_signature(signature) => {
            let Some(deadline) = parse_deadline(deadline) else {
                return text("cli-owner-usage-resume");
            };
            let result = claimed
                .chain
                .relay_unpause(claimed.agent_id, U256::from(deadline), signature)
                .await;
            note_short(config, claimed, &result).await;
            relay_text(result, "cli-owner-resumed")
        }
        _ => text("cli-owner-usage-resume"),
    }
}

async fn prepare_unpause(claimed: &ClaimedSession) -> String {
    let Some(parts) = digest_parts(claimed).await else {
        return chain_err();
    };
    let digest = match claimed
        .chain
        .unpause_digest(claimed.agent_id, parts.nonce, parts.deadline)
        .await
    {
        Ok(digest) => digest,
        Err(_) => return chain_err(),
    };
    let command = format!("/resume {} SIGNATURE", parts.deadline_secs);
    sign_reply(&command, digest, parts.deadline_secs)
}

async fn limits_command(config: &Config, claimed: &ClaimedSession, args: &[String]) -> String {
    match args {
        [] => limits_view(claimed),
        [per_tx, daily, deadline, signature] if looks_like_signature(signature) => {
            submit_loosen(config, claimed, per_tx, daily, deadline, signature).await
        }
        [per_tx, daily] => propose_caps(claimed, per_tx, daily).await,
        _ => text("cli-owner-usage-limits"),
    }
}

fn limits_view(claimed: &ClaimedSession) -> String {
    let per_tx = format_mon(claimed.per_tx_cap);
    let daily = format_mon(claimed.daily_cap);
    let paused = paused_word(claimed.paused);
    text_with(
        "cli-owner-limits",
        &[
            ("per_tx", per_tx.as_str()),
            ("daily", daily.as_str()),
            ("paused", paused.as_str()),
        ],
    )
}

async fn propose_caps(claimed: &ClaimedSession, per_tx: &str, daily: &str) -> String {
    let (Ok(next_per), Ok(next_daily)) = (parse_mon(per_tx), parse_mon(daily)) else {
        return text("cli-chain-bad-amount");
    };
    match classify(claimed.per_tx_cap, claimed.daily_cap, next_per, next_daily) {
        CapChange::Same => text("cli-owner-same-caps"),
        CapChange::Mixed => text("cli-owner-mixed-caps"),
        CapChange::Tighten => tighten_text(claimed, next_per, next_daily),
        CapChange::Loosen => prepare_loosen(claimed, per_tx, daily, next_per, next_daily).await,
    }
}

fn tighten_text(claimed: &ClaimedSession, per_tx: U256, daily: U256) -> String {
    let agent = claimed.agent_id.to_string();
    let per_tx_wei = per_tx.to_string();
    let daily_wei = daily.to_string();
    let vault = claimed.chain.vault_address().to_string();
    let per_tx_mon = format_mon(per_tx);
    let daily_mon = format_mon(daily);
    text_with(
        "cli-owner-tighten",
        &[
            ("agent", agent.as_str()),
            ("per_tx", per_tx_wei.as_str()),
            ("daily", daily_wei.as_str()),
            ("vault", vault.as_str()),
            ("per_tx_mon", per_tx_mon.as_str()),
            ("daily_mon", daily_mon.as_str()),
        ],
    )
}

async fn prepare_loosen(
    claimed: &ClaimedSession,
    per_tx: &str,
    daily: &str,
    next_per: U256,
    next_daily: U256,
) -> String {
    let Some(parts) = digest_parts(claimed).await else {
        return chain_err();
    };
    let digest = match claimed
        .chain
        .loosen_digest(
            claimed.agent_id,
            next_per,
            next_daily,
            parts.nonce,
            parts.deadline,
        )
        .await
    {
        Ok(digest) => digest,
        Err(_) => return chain_err(),
    };
    let command = format!("/limits {per_tx} {daily} {} SIGNATURE", parts.deadline_secs);
    sign_reply(&command, digest, parts.deadline_secs)
}

async fn submit_loosen(
    config: &Config,
    claimed: &ClaimedSession,
    per_tx: &str,
    daily: &str,
    deadline: &str,
    signature: &str,
) -> String {
    let (Ok(per_tx_cap), Ok(daily_cap)) = (parse_mon(per_tx), parse_mon(daily)) else {
        return text("cli-chain-bad-amount");
    };
    let Some(deadline) = parse_deadline(deadline) else {
        return text("cli-owner-usage-limits");
    };
    let result = claimed
        .chain
        .relay_loosen(
            claimed.agent_id,
            per_tx_cap,
            daily_cap,
            U256::from(deadline),
            signature,
        )
        .await;
    note_short(config, claimed, &result).await;
    relay_text(result, "cli-owner-loosened")
}

async fn approve_command(config: &Config, claimed: &ClaimedSession, args: &[String]) -> String {
    match args {
        [] => text("cli-owner-approve-none"),
        [recipient, deadline, signature] if looks_like_signature(signature) => {
            submit_allow(config, claimed, recipient, deadline, signature).await
        }
        [recipient] => prepare_allow(claimed, recipient).await,
        _ => text("cli-owner-usage-approve"),
    }
}

async fn prepare_allow(claimed: &ClaimedSession, recipient: &str) -> String {
    let Ok(recipient) = recipient.parse::<Address>() else {
        return text("cli-chain-bad-address");
    };
    if recipient == Address::ZERO {
        return text("cli-chain-bad-address");
    }
    let Some(parts) = digest_parts(claimed).await else {
        return chain_err();
    };
    let digest = match claimed
        .chain
        .allow_digest(claimed.agent_id, recipient, parts.nonce, parts.deadline)
        .await
    {
        Ok(digest) => digest,
        Err(_) => return chain_err(),
    };
    let command = format!("/approve {recipient} {} SIGNATURE", parts.deadline_secs);
    sign_reply(&command, digest, parts.deadline_secs)
}

async fn submit_allow(
    config: &Config,
    claimed: &ClaimedSession,
    recipient: &str,
    deadline: &str,
    signature: &str,
) -> String {
    let Ok(recipient_address) = recipient.parse::<Address>() else {
        return text("cli-chain-bad-address");
    };
    if recipient_address == Address::ZERO {
        return text("cli-chain-bad-address");
    }
    let Some(deadline) = parse_deadline(deadline) else {
        return text("cli-owner-usage-approve");
    };
    let result = claimed
        .chain
        .relay_allow(
            claimed.agent_id,
            recipient_address,
            U256::from(deadline),
            signature,
        )
        .await;
    note_short(config, claimed, &result).await;
    match result {
        Ok(tx) => {
            let tx = tx.to_string();
            text_with(
                "cli-owner-allowed",
                &[("recipient", recipient), ("tx", tx.as_str())],
            )
        }
        Err(error) => relay_failure(error),
    }
}

struct DigestParts {
    nonce: U256,
    deadline: U256,
    deadline_secs: u64,
}

async fn digest_parts(claimed: &ClaimedSession) -> Option<DigestParts> {
    let now = claimed.chain.block_timestamp().await.ok()?;
    let deadline_secs = now.saturating_add(SIGNATURE_TTL_SECS);
    let nonce = claimed.chain.loosen_nonce(claimed.agent_id).await.ok()?;
    Some(DigestParts {
        nonce,
        deadline: U256::from(deadline_secs),
        deadline_secs,
    })
}

fn sign_reply(command: &str, digest: B256, deadline: u64) -> String {
    let digest = digest.to_string();
    let deadline = deadline.to_string();
    text_with(
        "cli-owner-sign-digest",
        &[
            ("command", command),
            ("digest", digest.as_str()),
            ("deadline", deadline.as_str()),
        ],
    )
}

async fn note_short(
    config: &Config,
    claimed: &ClaimedSession,
    result: &Result<B256, anyhow::Error>,
) {
    if result
        .as_ref()
        .err()
        .is_some_and(kinetic_chain::is_vault_gas_short)
    {
        if let Some(ping) = OwnerPing::from_config(config) {
            ping.tell_vault_needs_funds(
                &claimed.chain.vault_address().to_string(),
                &claimed.agent_id.to_string(),
            )
            .await;
        }
    }
}

fn relay_text(result: Result<B256, anyhow::Error>, key: &str) -> String {
    match result {
        Ok(tx) => {
            let tx = tx.to_string();
            text_with(key, &[("tx", tx.as_str())])
        }
        Err(error) => relay_failure(error),
    }
}

fn relay_failure(error: anyhow::Error) -> String {
    if error.to_string().contains("reverted") {
        text("cli-owner-rejected")
    } else {
        chain_err()
    }
}

fn classify(current_per: U256, current_daily: U256, next_per: U256, next_daily: U256) -> CapChange {
    use std::cmp::Ordering::{Equal, Greater, Less};
    match (next_per.cmp(&current_per), next_daily.cmp(&current_daily)) {
        (Equal, Equal) => CapChange::Same,
        (Greater, Less) | (Less, Greater) => CapChange::Mixed,
        (Greater, _) | (_, Greater) => CapChange::Loosen,
        _ => CapChange::Tighten,
    }
}

fn paused_word(paused: bool) -> String {
    text(if paused {
        "cli-chain-yes"
    } else {
        "cli-chain-no"
    })
}

fn explorer_tx(chain_id: u64, hash: B256) -> String {
    let hash = hash.to_string();
    if chain_id == TESTNET_CHAIN_ID {
        format!("https://testnet.monadvision.com/tx/{hash}")
    } else {
        hash
    }
}

fn explorer_address(chain_id: u64, address: Address) -> String {
    let address = address.to_string();
    if chain_id == TESTNET_CHAIN_ID {
        format!("https://testnet.monadvision.com/address/{address}")
    } else {
        address
    }
}

fn split_line(line: &str) -> Option<(String, Vec<String>)> {
    let mut parts = line.split_whitespace();
    let token = parts.next()?;
    let name = token.split('@').next().unwrap_or(token);
    let name = name.trim_start_matches('/').to_ascii_lowercase();
    if name.is_empty() {
        return None;
    }
    Some((name, parts.map(str::to_string).collect()))
}

fn looks_like_signature(token: &str) -> bool {
    let hex = token.strip_prefix("0x").unwrap_or(token);
    hex.len() == 130 && hex.chars().all(|c| c.is_ascii_hexdigit())
}

fn parse_deadline(token: &str) -> Option<u64> {
    let deadline = token.parse::<u64>().ok()?;
    if deadline == 0 { None } else { Some(deadline) }
}

fn valid_telegram_id(telegram_id: &str) -> bool {
    let len = telegram_id.len();
    (1..=20).contains(&len) && telegram_id.chars().all(|c| c.is_ascii_digit())
}

fn config_dir(config: &Config) -> Option<&Path> {
    let parent = config.config_path.parent()?;
    if parent.as_os_str().is_empty() {
        None
    } else {
        Some(parent)
    }
}

fn text(key: &str) -> String {
    get_required_cli_string(key)
}

fn text_with(key: &str, args: &[(&str, &str)]) -> String {
    get_required_cli_string_with_args(key, args)
}

fn chain_err() -> String {
    text_with("cli-chain-failed", &[("error", "the chain call failed")])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config(dir: &Path, enabled: bool) -> Config {
        let mut config = Config::default();
        config.chain.enabled = enabled;
        config.config_path = dir.join("config.toml");
        config
    }

    #[tokio::test]
    async fn a_disabled_chain_does_not_create_a_device_key() {
        let dir = tempfile::tempdir().expect("temp dir");
        let config = test_config(dir.path(), false);
        let reply = handle(&config, "42", "/status").await;
        assert_eq!(reply, text("cli-chain-disabled"));
        assert!(!dir.path().join("device.key").exists());
    }

    #[tokio::test]
    async fn status_without_a_bound_chat_does_not_open_the_chain() {
        let dir = tempfile::tempdir().expect("temp dir");
        let config = test_config(dir.path(), true);
        let reply = handle(&config, "42", "/status@kinetic_bot").await;
        assert_eq!(reply, text("cli-owner-not-bound"));
        assert!(!dir.path().join("device.key").exists());
    }

    #[tokio::test]
    async fn deposit_and_withdraw_without_a_bound_chat_do_not_open_the_chain() {
        let dir = tempfile::tempdir().expect("temp dir");
        let config = test_config(dir.path(), true);
        let deposit = handle(&config, "42", "/deposit").await;
        let withdraw = handle(&config, "42", "/withdraw").await;
        assert_eq!(deposit, text("cli-owner-not-bound"));
        assert_eq!(withdraw, text("cli-owner-not-bound"));
        assert!(!dir.path().join("device.key").exists());
    }

    #[tokio::test]
    async fn a_different_chat_is_rejected_before_the_chain() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut config = test_config(dir.path(), true);
        bind_chat(&mut config, "42");
        let reply = handle(&config, "99", "/pause").await;
        assert_eq!(reply, text("cli-owner-not-bound"));
        assert!(!dir.path().join("device.key").exists());
    }

    #[tokio::test]
    async fn a_stale_link_file_does_not_open_the_chain() {
        let dir = tempfile::tempdir().expect("temp dir");
        let config = test_config(dir.path(), true);
        std::fs::write(dir.path().join("owner.link"), "nope\n").expect("link");
        let reply = handle(&config, "42", "/limits").await;
        assert_eq!(reply, text("cli-owner-not-bound"));
        assert!(!dir.path().join("device.key").exists());
    }

    #[tokio::test]
    async fn link_is_retired() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut config = test_config(dir.path(), true);
        bind_chat(&mut config, "42");
        let reply = handle(&config, "42", "/link").await;
        assert_eq!(reply, text("cli-owner-link-retired"));
        assert!(!dir.path().join("device.key").exists());
    }

    #[tokio::test]
    async fn a_missing_sender_never_links() {
        let dir = tempfile::tempdir().expect("temp dir");
        let config = test_config(dir.path(), true);
        let reply = handle(&config, "", "/link").await;
        assert_eq!(reply, text("cli-owner-missing-sender"));
        assert!(!dir.path().join("device.key").exists());
    }

    fn bind_chat(config: &mut Config, id: &str) {
        use kinetic_config::multi_agent::{PeerGroupConfig, PeerUsername};
        use kinetic_config::providers::ChannelRef;
        config.peer_groups.insert(
            "owner".to_string(),
            PeerGroupConfig {
                channel: ChannelRef::new("telegram.default".to_string()),
                external_peers: vec![PeerUsername::new(id)],
                ..PeerGroupConfig::default()
            },
        );
    }

    #[test]
    fn cap_changes_stay_in_one_direction() {
        let current = U256::from(10u64);
        assert!(matches!(
            classify(current, current, current, current),
            CapChange::Same
        ));
        assert!(matches!(
            classify(current, current, U256::from(4u64), current),
            CapChange::Tighten
        ));
        assert!(matches!(
            classify(current, current, U256::from(12u64), current),
            CapChange::Loosen
        ));
        assert!(matches!(
            classify(current, current, U256::from(12u64), U256::from(4u64)),
            CapChange::Mixed
        ));
    }
}
