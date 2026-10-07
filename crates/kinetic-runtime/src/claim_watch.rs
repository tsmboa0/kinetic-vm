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
    /// The mint landed after the link was sent.
    Claimed { chat_id: String, text: String },
    /// Already claimed, or there is nothing to watch. Stop.
    Settled,
}

/// One tick of the claim watcher.
///
/// `link_sent` is true after the claim URL was delivered. A device that is
/// already claimed when the process starts settles without nagging the chat.
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
                Ok(BootReport::Claimed { .. }) => {
                    return if link_sent {
                        match chat_id {
                            Some(chat_id) => Advance::Claimed {
                                chat_id,
                                text: claimed_message(&device_label(config)),
                            },
                            None => Advance::Settled,
                        }
                    } else {
                        Advance::Settled
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

fn offer_message(name: &str) -> String {
    let name = escape_html(name);
    get_required_cli_string_with_args("cli-claim-watch-offer", &[("name", name.as_str())])
}

fn claimed_message(name: &str) -> String {
    let name = escape_html(name);
    get_required_cli_string_with_args("cli-claim-watch-claimed", &[("name", name.as_str())])
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

    use super::{Advance, advance, bound_chat, offer_message};

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
        assert!(text.contains("<b>Claim Vending &lt;lab&gt;</b>"));
        assert!(text.contains("\n\n"));
        assert!(!text.contains("http"));
    }
}
