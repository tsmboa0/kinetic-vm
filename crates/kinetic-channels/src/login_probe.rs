//! Channel-owned persisted-login probes for QR-pairing channels.
//!
//! Answers "does this channel alias hold a persisted login/session on
//! disk?" by delegating to the channel module that owns the state — the
//! same signal each channel's startup path uses to decide between resuming
//! an existing session and starting a fresh QR pairing. Nothing is cached
//! and nothing is written: every call resolves paths from the canonical
//! `Config` and probes read-only.
//!
//! The gateway consumes this from `/api/channels` to report
//! `readiness.authenticated`. Keeping the probe here (rather than in the
//! gateway) keeps session-state knowledge inside the owning channel, and
//! keeps it out of the login lifecycle event payloads, which stay
//! lifecycle-only.

use crate::listing::QrPairingChannel;
use kinetic_config::schema::Config;

/// Result of a persisted-login probe for one channel alias.
///
/// The probe only runs for channels with a typed QR-pairing key
/// ([`QrPairingChannel`]); "this channel type has no probe / is not
/// compiled" is expressed by [`crate::listing::qr_pairing_channel`]
/// returning `None` at resolution time, not by a variant here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PersistedLogin {
    /// The channel found its persisted login/session signal on disk; it
    /// will resume the linked session instead of asking for a QR scan.
    Present,
    /// The channel supports persisted logins but none is stored; the next
    /// channel start begins a fresh QR pairing.
    Absent,
}

/// Probe the persisted login state for a channel alias.
///
/// Callers resolve their channel type key to [`QrPairingChannel`] once via
/// [`crate::listing::qr_pairing_channel`] and dispatch on the typed value;
/// no string key reaches this function. The match below is exhaustive over
/// the feature-gated variant set, so adding a QR-pairing channel without a
/// probe arm is a compile error rather than a silent fallthrough.
pub fn persisted_login(channel: QrPairingChannel, config: &Config, alias: &str) -> PersistedLogin {
    // Read at use-time in the feature-gated arms below; the binding keeps
    // the signature stable when no QR-pairing channel feature is compiled.
    let (_config, _alias) = (config, alias);
    match channel {}
}

#[cfg(test)]
mod tests {

    #[test]
    fn channels_without_a_probe_resolve_to_no_qr_pairing_key() {
        // "Unsupported" is decided at key-resolution time: channel types
        // without channel-owned QR login state never reach the probe.
        assert_eq!(crate::listing::qr_pairing_channel("discord"), None);
        assert_eq!(
            crate::listing::qr_pairing_channel("whatsapp"),
            None,
            "the Cloud API backend has no on-disk session to probe"
        );
    }
}
