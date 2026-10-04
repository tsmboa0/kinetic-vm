//! Channel implementations and orchestration for messaging platform integrations.

#![allow(
    clippy::to_string_in_format_args,
    clippy::useless_format,
    clippy::explicit_auto_deref
)]

pub mod allowlist;
// Ungated: the reader-aligned paired-identity writer is shared by every
// writer of an `external_peers` grant, including the CLI/API bind core and the
// Telegram pairing path.
pub(crate) mod identity_persist;
pub mod listing;
pub mod login_events;
pub mod login_probe;
pub mod login_relink;
#[cfg(feature = "channel-telegram")]
pub(crate) mod model_picker_delivery;
pub mod orchestrator;
pub mod paced_channel;
pub mod util;

// Always-compiled channels and utilities (no feature gate)
#[cfg(feature = "channel-acp-server")]
pub mod acp_channel;
pub mod cli;
pub mod link_enricher;
pub mod transcription;
pub mod tts;
pub mod voice;

// Feature-gated channels
#[cfg(feature = "channel-filesystem")]
pub mod filesystem;
#[cfg(feature = "channel-telegram")]
pub mod telegram;
#[cfg(feature = "voice-wake")]
pub mod voice_wake;
#[cfg(feature = "channel-webhook")]
pub mod webhook;
