//! Background claim for a bound Telegram chat.
//!
//! The device key is created only after a chat is bound. The watcher then
//! sends the claim link once and polls until the mint lands. The bound chat
//! is the owner chat. Spending changes still need the owner wallet.

use std::time::Duration;

use kinetic_chain::{BootReport, DeviceChain, SoftwareKey};
use kinetic_config::multi_agent::PeerGroupConfig;
use kinetic_config::schema::Config;

use crate::claim_link;
use crate::i18n::{get_required_cli_string, get_required_cli_string_with_args};

/// How often the running device asks the chain whether the claim landed.
pub const POLL_EVERY: Duration = Duration::from_secs(2);

/// What the background watcher should do on this tick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Advance {
    /// Nothing to send. Keep polling.
    Wait,
    /// Send the claim link to the bound chat. The key now exists.
    Deliver {
        chat_id: String,
        text: String,
        url: String,
    },
    /// The mint just landed. Tell the chat, then show the device is ready.
    Claimed(ClaimFacts),
    /// The device was already claimed when this process started.
    Ready(ClaimFacts),
    /// Nothing to watch. Stop.
    Settled,
}

/// The claimed device, for the terminal and the Telegram summary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimFacts {
    pub chat_id: String,
    pub name: String,
    pub agent_id: String,
    pub device: String,
    pub owner: String,
    pub vault: String,
}

/// One tick of the claim watcher.
///
/// `link_sent` is true after the claim URL was delivered. A device that is
/// already claimed when the process starts prints the ready banner without
/// sending another Telegram note.
pub async fn advance(config: &Config, alias: &str, link_sent: bool) -> Advance {
    if !config.chain.enabled {
        return Advance::Settled;
    }
    let chat_id = bound_chat(config, alias);
    let Some(dir) = config.config_path.parent() else {
        return Advance::Settled;
    };
    if SoftwareKey::address_if_present(dir)
        .ok()
        .flatten()
        .is_some()
    {
        match Box::pin(DeviceChain::open_existing(&config.chain, dir)).await {
            Ok(chain) => match Box::pin(chain.identity()).await {
                Ok(BootReport::Claimed {
                    device,
                    agent_id,
                    owner,
                    ..
                }) => {
                    let facts = ClaimFacts {
                        chat_id: chat_id.unwrap_or_default(),
                        name: device_label(config),
                        agent_id: agent_id.to_string(),
                        device: device.to_string(),
                        owner: owner.to_string(),
                        vault: config.chain.vault.clone(),
                    };
                    return if link_sent {
                        Advance::Claimed(facts)
                    } else {
                        Advance::Ready(facts)
                    };
                }
                Ok(BootReport::Unclaimed { .. }) => {}
                Err(_) => return Advance::Wait,
            },
            Err(_) => return Advance::Wait,
        }
    }
    if link_sent {
        return Advance::Wait;
    }
    let Some(chat_id) = chat_id else {
        return Advance::Wait;
    };
    match Box::pin(claim_link::build(config)).await {
        Ok(url) => Advance::Deliver {
            chat_id,
            text: offer_message(&device_label(config)),
            url,
        },
        Err(_) => Advance::Wait,
    }
}

/// The Telegram user id stored when `/bind` succeeded.
///
/// Groups are named `telegram` or `telegram.<alias>`. The numeric sender id
/// is what `sendMessage` needs.
pub fn bound_chat(config: &Config, alias: &str) -> Option<String> {
    let dotted = format!("telegram.{alias}");
    config.peer_groups.values().find_map(|group| {
        let channel = group.channel.as_str();
        if channel != "telegram" && channel != dotted {
            return None;
        }
        first_peer(group)
    })
}

fn first_peer(group: &PeerGroupConfig) -> Option<String> {
    let ignored: Vec<String> = group
        .ignore
        .iter()
        .map(|peer| normalize(peer.as_str()))
        .collect();
    group.external_peers.iter().find_map(|peer| {
        let id = normalize(peer.as_str());
        if id.is_empty() || ignored.iter().any(|item| item == &id) {
            None
        } else {
            Some(id)
        }
    })
}

fn normalize(value: &str) -> String {
    value.trim().trim_start_matches('@').to_string()
}

pub fn device_label(config: &Config) -> String {
    let name = config.chain.device_name.trim();
    if name.is_empty() {
        "device".to_string()
    } else {
        name.to_string()
    }
}

/// Telegram copies at most this many characters from a button.
const COPY_TEXT_LIMIT: usize = 256;

/// The claim note and the buttons under it.
pub struct ClaimNote {
    pub text: String,
    pub markup: serde_json::Value,
}

fn offer_message(name: &str) -> String {
    let name = escape_html(name);
    let title = get_required_cli_string_with_args(
        "cli-claim-watch-offer-title",
        &[("name", name.as_str())],
    );
    let body = get_required_cli_string("cli-claim-watch-offer-body");
    format!("<b>{title}</b>\n\n{body}")
}

pub fn offer_delivery(name: &str, page: &str) -> ClaimNote {
    let open = get_required_cli_string("cli-claim-watch-button");
    let mut rows = vec![vec![serde_json::json!({ "text": open, "url": page })]];
    let mut text = offer_message(name);
    if page.chars().count() <= COPY_TEXT_LIMIT {
        let copy = get_required_cli_string("cli-claim-watch-copy-link");
        rows.push(vec![
            serde_json::json!({ "text": copy, "copy_text": { "text": page } }),
        ]);
    } else {
        let hint = get_required_cli_string("cli-claim-watch-copy-hint");
        let page = escape_html(page);
        text = format!("{text}\n\n{hint}\n<code>{page}</code>");
    }
    ClaimNote {
        text,
        markup: serde_json::json!({ "inline_keyboard": rows }),
    }
}

pub fn funding_markup(facts: &ClaimFacts) -> serde_json::Value {
    let deposit = get_required_cli_string("cli-claim-watch-deposit-button");
    let page = format!(
        "https://vault-deposit.kineticvm.xyz?address={}&agent={}",
        facts.vault, facts.agent_id
    );
    serde_json::json!({
        "inline_keyboard": [
            [{ "text": deposit, "url": page }]
        ]
    })
}

/// Print the claimed device, then the line that means it is waiting for work.
pub fn print_ready(facts: &ClaimFacts) {
    println!(
        "{}",
        get_required_cli_string_with_args(
            "cli-daemon-claim-done",
            &[("name", facts.name.as_str())]
        )
    );
    println!(
        "   {}",
        get_required_cli_string_with_args(
            "cli-daemon-agent-id",
            &[("agent", facts.agent_id.as_str())]
        )
    );
    println!(
        "   {}",
        get_required_cli_string_with_args(
            "cli-daemon-agent-key",
            &[("key", facts.device.as_str())]
        )
    );
    println!(
        "   {}",
        get_required_cli_string_with_args("cli-daemon-vault", &[("vault", facts.vault.as_str())])
    );
    println!();
    println!("{}", get_required_cli_string("cli-daemon-ready-title"));
    println!("   {}", get_required_cli_string("cli-daemon-listening"));
}

pub fn claimed_html(facts: &ClaimFacts) -> String {
    let name = escape_html(&facts.name);
    let title = get_required_cli_string_with_args(
        "cli-claim-watch-claimed-title",
        &[("name", name.as_str())],
    );
    let summary = get_required_cli_string("cli-claim-watch-claimed-summary");
    let owner = escape_html(&facts.owner);
    let owner_label = get_required_cli_string("cli-claim-watch-owner");
    let owner_line = format!("{owner_label} <code>{owner}</code>");
    let agent = get_required_cli_string_with_args(
        "cli-daemon-agent-id",
        &[("agent", facts.agent_id.as_str())],
    );
    let next = get_required_cli_string("cli-claim-watch-next");
    let deposit = get_required_cli_string("cli-claim-watch-deposit");
    let vault_label = get_required_cli_string("cli-claim-watch-device-vault");
    let vault = escape_html(&facts.vault);
    format!(
        "✅ <b>{title}</b>\n\n{summary}\n{agent}\n{owner_line}\n\n<b>{next}</b>\n\n{deposit}\n{vault_label}: <code>{vault}</code>"
    )
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use kinetic_config::multi_agent::{PeerGroupConfig, PeerUsername};
    use kinetic_config::providers::ChannelRef;
    use kinetic_config::schema::Config;

    use super::{
        Advance, ClaimFacts, advance, bound_chat, claimed_html, funding_markup, offer_delivery,
        offer_message,
    };

    fn config() -> (tempfile::TempDir, Config) {
        let dir = tempfile::tempdir().expect("temp");
        let mut config = Config {
            config_path: dir.path().join("config.toml"),
            ..Config::default()
        };
        config.chain.enabled = true;
        config.chain.owner = "0x64772107fC23f7370C90EA0aBd29ee6117B97f77".to_string();
        (dir, config)
    }

    #[test]
    fn a_bound_chat_is_the_numeric_sender() {
        let (_dir, mut config) = config();
        config.peer_groups.insert(
            "owner".to_string(),
            PeerGroupConfig {
                channel: ChannelRef::new("telegram.default".to_string()),
                external_peers: vec![PeerUsername::new("4242")],
                ..PeerGroupConfig::default()
            },
        );
        assert_eq!(bound_chat(&config, "default").as_deref(), Some("4242"));
        assert!(bound_chat(&config, "other").is_none());
    }

    #[tokio::test]
    async fn a_missing_chat_does_not_create_the_key() {
        let (dir, config) = config();
        assert_eq!(advance(&config, "default", false).await, Advance::Wait);
        assert!(!dir.path().join("device.key").exists());
    }

    #[tokio::test]
    async fn a_disabled_chain_settles_without_a_key() {
        let (dir, mut config) = config();
        config.chain.enabled = false;
        assert_eq!(advance(&config, "default", false).await, Advance::Settled);
        assert!(!dir.path().join("device.key").exists());
    }

    #[tokio::test]
    async fn a_bad_owner_does_not_create_the_key() {
        let (dir, mut config) = config();
        config.chain.owner = "not-a-wallet".to_string();
        config.peer_groups.insert(
            "owner".to_string(),
            PeerGroupConfig {
                channel: ChannelRef::new("telegram".to_string()),
                external_peers: vec![PeerUsername::new("4242")],
                ..PeerGroupConfig::default()
            },
        );
        assert_eq!(advance(&config, "default", false).await, Advance::Wait);
        assert!(!dir.path().join("device.key").exists());
    }

    #[test]
    fn the_claim_offer_is_html_with_paragraphs() {
        let text = offer_message("Vending <lab>");
        assert!(text.starts_with("<b>Claim Vending &lt;lab&gt;</b>"));
        assert!(text.contains("Tap the button to claim this device"));
        assert!(text.contains("\n\n"));
        assert!(!text.contains("{cli-"));
    }

    #[test]
    fn the_claimed_note_names_the_vault_and_the_agent_key() {
        let text = claimed_html(&ClaimFacts {
            chat_id: "42".to_string(),
            name: "Newton".to_string(),
            agent_id: "7".to_string(),
            device: "0x1111111111111111111111111111111111111111".to_string(),
            owner: "0x2222222222222222222222222222222222222222".to_string(),
            vault: "0x3333333333333333333333333333333333333333".to_string(),
        });
        assert!(text.contains("Newton is claimed"));
        assert!(text.contains("The identity NFT is in the owner wallet."));
        assert!(text.contains("<b>Next step</b>"));
        assert!(text.contains("Click the link below to send MON to the device vault"));
        assert!(
            text.contains("Device Vault: <code>0x3333333333333333333333333333333333333333</code>")
        );
        assert!(!text.contains("pay gas"));
        assert!(text.contains("<code>0x2222222222222222222222222222222222222222</code>"));
        assert!(!text.contains("{cli-"));
    }

    #[test]
    fn the_claim_note_has_a_button_that_copies_the_link() {
        let page = "https://claim.kineticvm.xyz?t=abc";
        let note = offer_delivery("Vending <lab>", page);
        let rows = note.markup["inline_keyboard"].as_array().expect("rows");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0][0]["url"], page);
        assert_eq!(rows[1][0]["text"], "[ Click to copy link ]");
        assert_eq!(rows[1][0]["copy_text"]["text"], page);
        assert!(!note.text.contains("http"));
    }

    #[test]
    fn a_long_claim_link_is_copied_from_the_message() {
        let page = format!("https://claim.kineticvm.xyz?x={}", "a".repeat(300));
        let note = offer_delivery("Vending", &page);
        assert!(note.text.contains("Tap the link to copy it"));
        assert!(note.text.contains("<code>https://claim.kineticvm.xyz?x="));
        let rows = note.markup["inline_keyboard"].as_array().expect("rows");
        assert_eq!(rows.len(), 1);
    }

    #[test]
    fn the_funding_buttons_copy_the_vault_and_the_agent_key() {
        let markup = funding_markup(&ClaimFacts {
            chat_id: "42".to_string(),
            name: "Newton".to_string(),
            agent_id: "7".to_string(),
            device: "0x1111111111111111111111111111111111111111".to_string(),
            owner: "0x2222222222222222222222222222222222222222".to_string(),
            vault: "0x3333333333333333333333333333333333333333".to_string(),
        });
        let rows = markup["inline_keyboard"].as_array().expect("rows");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0][0]["text"], "Deposit");
        assert_eq!(
            rows[0][0]["url"],
            "https://vault-deposit.kineticvm.xyz?address=0x3333333333333333333333333333333333333333&agent=7"
        );
    }
}
