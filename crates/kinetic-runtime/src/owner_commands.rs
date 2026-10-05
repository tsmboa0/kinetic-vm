//! Telegram commands for the onchain owner.
//!
//! The chat id is a local fact: the chain does not know Telegram. `owner.link`
//! records which chat the owner personal-signed. Every command still re-reads
//! `ownerOf` and stops if that address no longer matches.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use kinetic_chain::{
    Address, B256, BootReport, DeviceChain, U256, format_mon, owner_link_message, parse_mon,
    recover_personal_signer,
};
use kinetic_config::schema::Config;

use crate::i18n::{get_required_cli_string, get_required_cli_string_with_args};

const LINK_FILE: &str = "owner.link";
const SIGNATURE_TTL_SECS: u64 = 60 * 60;
const TESTNET_CHAIN_ID: u64 = 10143;

struct OwnerLink {
    telegram_id: String,
    owner: Address,
}

enum CapChange {
    Same,
    Tighten,
    Loosen,
    Mixed,
}

/// Reply for one Telegram owner command. Chain work runs only when `[chain]`
/// is enabled, and only after the local link check except for `/link`.
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
        "link" => link_command(config, dir, telegram_id, &args).await,
        "status" | "pause" | "resume" | "limits" | "approve" => {
            let Some(link) = read_link(dir) else {
                return text("cli-owner-not-linked");
            };
            if link.telegram_id != telegram_id {
                return text("cli-owner-not-you");
            }
            owned_command(config, dir, &link, &name, &args).await
        }
        _ => text("cli-owner-usage-link"),
    }
}

async fn link_command(config: &Config, dir: &Path, telegram_id: &str, args: &[String]) -> String {
    match args {
        [] => {}
        [signature] if looks_like_signature(signature) => {
            return finish_link(config, dir, telegram_id, signature).await;
        }
        _ => return text("cli-owner-usage-link"),
    }
    let Some(claimed) = open_claimed(config, dir).await else {
        return claimed_or_err(config, dir).await;
    };
    let message = owner_link_message(claimed.chain_id, claimed.agent_id, telegram_id);
    text_with("cli-owner-link-prompt", &[("message", &message)])
}

async fn finish_link(config: &Config, dir: &Path, telegram_id: &str, signature: &str) -> String {
    let Some(claimed) = open_claimed(config, dir).await else {
        return claimed_or_err(config, dir).await;
    };
    let message = owner_link_message(claimed.chain_id, claimed.agent_id, telegram_id);
    let recovered = match recover_personal_signer(&message, signature) {
        Ok(address) => address,
        Err(_) => return text("cli-owner-bad-signature"),
    };
    if recovered != claimed.owner {
        return text("cli-owner-bad-signature");
    }
    let link = OwnerLink {
        telegram_id: telegram_id.to_string(),
        owner: claimed.owner,
    };
    if write_link(dir, &link).is_err() {
        return chain_err();
    }
    let owner = claimed.owner.to_string();
    let agent = claimed.agent_id.to_string();
    text_with(
        "cli-owner-linked",
        &[("owner", owner.as_str()), ("agent", agent.as_str())],
    )
}

async fn owned_command(
    config: &Config,
    dir: &Path,
    link: &OwnerLink,
    name: &str,
    args: &[String],
) -> String {
    let Some(claimed) = open_claimed(config, dir).await else {
        return claimed_or_err(config, dir).await;
    };
    if claimed.owner != link.owner {
        return text("cli-owner-changed");
    }
    match name {
        "status" => status_text(&claimed).await,
        "pause" => pause_text(&claimed),
        "resume" => resume_command(&claimed, args).await,
        "limits" => limits_command(&claimed, args).await,
        "approve" => approve_command(&claimed, args).await,
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

fn pause_text(claimed: &ClaimedSession) -> String {
    let agent = claimed.agent_id.to_string();
    let vault = claimed.chain.vault_address().to_string();
    text_with(
        "cli-owner-pause",
        &[("agent", agent.as_str()), ("vault", vault.as_str())],
    )
}

async fn resume_command(claimed: &ClaimedSession, args: &[String]) -> String {
    match args {
        [] => prepare_unpause(claimed).await,
        [deadline, signature] if looks_like_signature(signature) => {
            let Some(deadline) = parse_deadline(deadline) else {
                return text("cli-owner-usage-resume");
            };
            relay_text(
                claimed
                    .chain
                    .relay_unpause(claimed.agent_id, U256::from(deadline), signature)
                    .await,
                "cli-owner-resumed",
            )
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

async fn limits_command(claimed: &ClaimedSession, args: &[String]) -> String {
    match args {
        [] => limits_view(claimed),
        [per_tx, daily, deadline, signature] if looks_like_signature(signature) => {
            submit_loosen(claimed, per_tx, daily, deadline, signature).await
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
    relay_text(
        claimed
            .chain
            .relay_loosen(
                claimed.agent_id,
                per_tx_cap,
                daily_cap,
                U256::from(deadline),
                signature,
            )
            .await,
        "cli-owner-loosened",
    )
}

async fn approve_command(claimed: &ClaimedSession, args: &[String]) -> String {
    match args {
        [] => text("cli-owner-approve-none"),
        [recipient, deadline, signature] if looks_like_signature(signature) => {
            submit_allow(claimed, recipient, deadline, signature).await
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

fn read_link(dir: &Path) -> Option<OwnerLink> {
    let stored = fs::read_to_string(dir.join(LINK_FILE)).ok()?;
    let mut lines = stored.lines();
    if lines.next()? != "v1" {
        return None;
    }
    let telegram_id = lines.next()?.to_string();
    if !valid_telegram_id(&telegram_id) {
        return None;
    }
    let owner = lines.next()?.parse().ok()?;
    Some(OwnerLink { telegram_id, owner })
}

fn write_link(dir: &Path, link: &OwnerLink) -> std::io::Result<()> {
    let body = format!("v1\n{}\n{}\n", link.telegram_id, link.owner);
    write_private(&dir.join(LINK_FILE), body.as_bytes())
}

fn write_private(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    let tmp = temp_path(path);
    let _ = fs::remove_file(&tmp);
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&tmp)?;
    file.write_all(contents)?;
    file.sync_all()?;
    if let Err(error) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(error);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

fn temp_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_else(|| LINK_FILE.into());
    name.push(".tmp");
    path.with_file_name(name)
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
    async fn status_without_a_link_does_not_open_the_chain() {
        let dir = tempfile::tempdir().expect("temp dir");
        let config = test_config(dir.path(), true);
        let reply = handle(&config, "42", "/status@kinetic_bot").await;
        assert_eq!(reply, text("cli-owner-not-linked"));
        assert!(!dir.path().join("device.key").exists());
        assert!(!dir.path().join(LINK_FILE).exists());
    }

    #[tokio::test]
    async fn a_different_chat_is_rejected_before_the_chain() {
        let dir = tempfile::tempdir().expect("temp dir");
        let config = test_config(dir.path(), true);
        fs::write(
            dir.path().join(LINK_FILE),
            "v1\n42\n0x64772107fC23f7370C90EA0aBd29ee6117B97f77\n",
        )
        .expect("link");
        let reply = handle(&config, "99", "/pause").await;
        assert_eq!(reply, text("cli-owner-not-you"));
        assert!(!dir.path().join("device.key").exists());
    }

    #[tokio::test]
    async fn a_corrupt_link_file_asks_for_a_new_link() {
        let dir = tempfile::tempdir().expect("temp dir");
        let config = test_config(dir.path(), true);
        fs::write(dir.path().join(LINK_FILE), "nope\n").expect("link");
        let reply = handle(&config, "42", "/limits").await;
        assert_eq!(reply, text("cli-owner-not-linked"));
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

    #[test]
    fn the_link_file_round_trips_the_chat_and_owner() {
        let dir = tempfile::tempdir().expect("temp dir");
        let owner: Address = "0x64772107fC23f7370C90EA0aBd29ee6117B97f77"
            .parse()
            .expect("address");
        write_link(
            dir.path(),
            &OwnerLink {
                telegram_id: "42".to_string(),
                owner,
            },
        )
        .expect("write");
        let loaded = read_link(dir.path()).expect("read");
        assert_eq!(loaded.telegram_id, "42");
        assert_eq!(loaded.owner, owner);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(dir.path().join(LINK_FILE))
                .expect("metadata")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
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
