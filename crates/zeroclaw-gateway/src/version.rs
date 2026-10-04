//! Restart classification shown by `/api/status`.
//!
//! Advisory only: it tells the operator which restart command applies in this
//! environment. The gateway does not restart itself.

use serde::Serialize;
use std::sync::OnceLock;
use zeroclaw_runtime::i18n::get_required_cli_string;

/// How a restart is achieved in this environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "schema-export", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum RestartMode {
    /// The ZeroClaw Desktop supervisor relaunches us after the dedicated exit.
    DesktopSupervised,
    /// A supervisor (systemd/launchd) relaunches us after a clean exit.
    Supervised,
    /// No supervisor, but the process can relaunch itself after teardown.
    SelfRespawn,
    /// The operator must restart manually (container PID 1, or non-unix bare).
    Manual,
}

impl RestartMode {
    pub fn as_str(self) -> &'static str {
        match self {
            RestartMode::DesktopSupervised => "desktop_supervised",
            RestartMode::Supervised => "supervised",
            RestartMode::SelfRespawn => "self_respawn",
            RestartMode::Manual => "manual",
        }
    }

    /// Whether this environment can relaunch the process after a clean exit.
    pub fn auto_restartable(self) -> bool {
        matches!(
            self,
            RestartMode::DesktopSupervised | RestartMode::Supervised | RestartMode::SelfRespawn
        )
    }
}

/// Detected restart mode plus the command to show the operator.
#[derive(Clone)]
pub struct RestartInfo {
    pub mode: RestartMode,
    pub hint: String,
}

fn env_present(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|v| !v.is_empty())
}

fn is_container() -> bool {
    // Any positive signal wins: a false "not a container" is the dangerous
    // case (exiting PID 1 with no restart policy tears the container down).
    std::path::Path::new("/.dockerenv").exists()
        || std::process::id() == 1
        || std::fs::read_to_string("/proc/1/cgroup").is_ok_and(|s| {
            s.contains("docker") || s.contains("containerd") || s.contains("kubepods")
        })
}

/// Classify the runtime environment to pick the hint text.
///
/// The classification is static for the process lifetime (env vars + cgroup), so
/// it is computed once and cached — `/api/status` calls this on every poll.
pub fn detect_restart() -> RestartInfo {
    static CACHE: OnceLock<RestartInfo> = OnceLock::new();
    CACHE.get_or_init(detect_restart_uncached).clone()
}

fn detect_restart_uncached() -> RestartInfo {
    if zeroclaw_runtime::restart::is_desktop_supervised() {
        return RestartInfo {
            mode: RestartMode::DesktopSupervised,
            hint: get_required_cli_string("cli-gateway-restart-hint-process"),
        };
    }
    // Container first — default to manual since we can't see a restart policy.
    if is_container() {
        let hint = if env_present("KUBERNETES_SERVICE_HOST") {
            get_required_cli_string("cli-gateway-restart-hint-kubernetes")
        } else {
            get_required_cli_string("cli-gateway-restart-hint-container")
        };
        return RestartInfo {
            mode: RestartMode::Manual,
            hint,
        };
    }
    // systemd: a clean exit is relaunched when the unit sets Restart=on-success.
    if env_present("INVOCATION_ID") || env_present("JOURNAL_STREAM") {
        return RestartInfo {
            mode: RestartMode::Supervised,
            hint: get_required_cli_string("cli-gateway-restart-hint-systemd"),
        };
    }
    // launchd (macOS): KeepAlive relaunches on exit.
    if cfg!(target_os = "macos") && env_present("XPC_SERVICE_NAME") {
        return RestartInfo {
            mode: RestartMode::Supervised,
            hint: get_required_cli_string("cli-gateway-restart-hint-launchd"),
        };
    }
    // Bare process. On unix/windows we can relaunch ourselves after teardown;
    // elsewhere there's no safe self-relaunch, so stay manual.
    if cfg!(unix) || cfg!(windows) {
        RestartInfo {
            mode: RestartMode::SelfRespawn,
            hint: get_required_cli_string("cli-gateway-restart-hint-process"),
        }
    } else {
        RestartInfo {
            mode: RestartMode::Manual,
            hint: get_required_cli_string("cli-gateway-restart-hint-process"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restart_mode_wire_values_are_stable() {
        for (mode, expected) in [
            (RestartMode::DesktopSupervised, "desktop_supervised"),
            (RestartMode::Supervised, "supervised"),
            (RestartMode::SelfRespawn, "self_respawn"),
            (RestartMode::Manual, "manual"),
        ] {
            assert_eq!(mode.as_str(), expected);
            assert_eq!(
                serde_json::to_value(mode).expect("serialize mode"),
                expected
            );
        }
        assert!(RestartMode::Supervised.auto_restartable());
        assert!(RestartMode::DesktopSupervised.auto_restartable());
        assert!(RestartMode::SelfRespawn.auto_restartable());
        assert!(!RestartMode::Manual.auto_restartable());
    }

    #[test]
    fn detect_restart_returns_a_nonempty_hint() {
        let info = detect_restart();
        assert!(!info.hint.is_empty());
    }

    #[test]
    fn restart_hint_keys_resolve_via_fluent() {
        for key in [
            "cli-gateway-restart-hint-kubernetes",
            "cli-gateway-restart-hint-container",
            "cli-gateway-restart-hint-systemd",
            "cli-gateway-restart-hint-launchd",
            "cli-gateway-restart-hint-process",
        ] {
            let resolved = get_required_cli_string(key);
            assert!(
                !resolved.is_empty() && !resolved.starts_with('{'),
                "missing Fluent string for {key}: {resolved}"
            );
        }
    }
}
