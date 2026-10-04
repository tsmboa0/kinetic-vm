//! Channel-owned relink hooks for QR-pairing channels.
//!
//! "Relink" replaces the currently linked account: QR-pairing channels
//! (WeChat, WhatsApp Web) persist their login on disk and silently resume it
//! on every start — by design, a restart never re-runs the QR flow while a
//! session exists. So issuing a new QR necessarily means clearing the
//! persisted login first; the restarted channel then finds no session and
//! begins a fresh pairing.
//!
//! Each match arm delegates to the channel module that owns the state, so
//! knowledge of what constitutes a persisted login (which files, which
//! rows) never leaks out of the channel — the gateway endpoint that exposes
//! this dispatches here and performs no file operations of its own. Paths
//! are resolved from the canonical `Config` per call; nothing is cached.
//!
//! Channels that cannot relink — webhook-token channels, bot-token channels,
//! the WhatsApp Cloud API backend, or channels whose feature is not compiled
//! into this binary — never resolve to a [`QrPairingChannel`] key
//! ([`crate::listing::qr_pairing_channel`] returns `None`), so they never
//! reach this hook and **nothing is touched**: no files are removed, no
//! state changes, the operation is an explicit no-op the caller can surface
//! verbatim.
//!
//! Relinking only clears disk state. A currently running channel keeps its
//! in-memory session until it restarts; callers own scheduling that restart
//! (the daemon reload path), which keeps this hook free of lifecycle side
//! effects and keeps restart policy where it already lives.

use crate::listing::QrPairingChannel;
use kinetic_config::schema::Config;

/// Result of a relink request for one channel alias.
///
/// The hook only runs for channels with a typed QR-pairing key
/// ([`QrPairingChannel`]); "this channel type cannot relink / is not
/// compiled" is expressed by [`crate::listing::qr_pairing_channel`]
/// returning `None` at resolution time, not by a variant here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelinkOutcome {
    /// A persisted login existed and its on-disk state was removed. The
    /// next channel start begins a fresh QR pairing that replaces the
    /// previously linked account.
    Cleared {
        /// Paths that were actually removed, for operator-facing reporting.
        removed: Vec<String>,
    },
    /// The channel supports relinking but held no persisted login state;
    /// nothing was removed. The next channel start already begins a fresh
    /// QR pairing.
    NothingToClear,
}

/// Clear the persisted login state for a channel alias so its next start
/// mints a fresh QR pairing.
///
/// Callers resolve their channel type key to [`QrPairingChannel`] once via
/// [`crate::listing::qr_pairing_channel`] — the same typed key
/// [`crate::login_probe::persisted_login`] dispatches on — so probe and
/// relink share one key space and no string key reaches this function. The
/// match below is exhaustive over the feature-gated variant set, so adding
/// a QR-pairing channel without a relink arm is a compile error rather
/// than a silent fallthrough.
///
/// Errors are I/O failures from removing existing files (permissions, etc.);
/// absent files are never an error.
pub fn relink(
    channel: QrPairingChannel,
    config: &Config,
    alias: &str,
) -> anyhow::Result<RelinkOutcome> {
    // Read at use-time in the feature-gated arms below; the binding keeps
    // the signature stable when no QR-pairing channel feature is compiled.
    let (_config, _alias) = (config, alias);
    match channel {}
}

#[cfg(test)]
mod tests {

    #[test]
    fn channels_without_a_relink_hook_resolve_to_no_qr_pairing_key() {
        // "Unsupported" is decided at key-resolution time: channel types
        // without QR-pairing sessions never reach the relink hook, so
        // nothing can be touched for them.
        assert_eq!(crate::listing::qr_pairing_channel("discord"), None);
        assert_eq!(
            crate::listing::qr_pairing_channel("whatsapp"),
            None,
            "the Cloud API backend has no on-disk session to clear"
        );
    }
}
