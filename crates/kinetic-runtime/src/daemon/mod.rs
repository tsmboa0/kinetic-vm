use anyhow::Result;
use chrono::Utc;
use kinetic_config::schema::Config;
use std::path::PathBuf;
use tokio::task::JoinHandle;
use tokio::time::Duration;

use self::registry::ChannelRegistryClearer;

/// A daemon-run-owned channel generation. RPC config mutations prepare an
/// opaque drain for this exact generation; committing the mutation retires the
/// generation, clears channel admission, and waits for its channel task.
#[derive(Clone)]
pub struct ChannelGenerationControl {
    state: std::sync::Arc<ChannelGenerationState>,
    registry_clearer: Option<ChannelRegistryClearer>,
}

struct ChannelGenerationState {
    retired: std::sync::atomic::AtomicBool,
    active: parking_lot::Mutex<Option<ChannelGenerationAttemptState>>,
}

struct ChannelGenerationAttemptState {
    cancel: tokio_util::sync::CancellationToken,
    done: tokio::sync::oneshot::Receiver<()>,
}

pub struct PreparedChannelGenerationDrain {
    control: ChannelGenerationControl,
}

pub struct ChannelGenerationDrainWait {
    done: Option<tokio::sync::oneshot::Receiver<()>>,
}

pub(crate) struct ChannelGenerationAttempt {
    cancel: tokio_util::sync::CancellationToken,
    done: Option<tokio::sync::oneshot::Sender<()>>,
}

impl ChannelGenerationControl {
    pub(crate) fn new(registry_clearer: Option<ChannelRegistryClearer>) -> Self {
        Self {
            state: std::sync::Arc::new(ChannelGenerationState {
                retired: std::sync::atomic::AtomicBool::new(false),
                active: parking_lot::Mutex::new(None),
            }),
            registry_clearer,
        }
    }

    pub(crate) fn prepare(&self) -> Option<PreparedChannelGenerationDrain> {
        self.registry_clearer.as_ref()?;
        Some(PreparedChannelGenerationDrain {
            control: self.clone(),
        })
    }

    fn begin_attempt(
        &self,
        daemon_cancel: &tokio_util::sync::CancellationToken,
    ) -> Option<ChannelGenerationAttempt> {
        let mut active = self.state.active.lock();
        if self
            .state
            .retired
            .load(std::sync::atomic::Ordering::Acquire)
        {
            return None;
        }
        let (done_tx, done_rx) = tokio::sync::oneshot::channel();
        let cancel = daemon_cancel.child_token();
        active.replace(ChannelGenerationAttemptState {
            cancel: cancel.clone(),
            done: done_rx,
        });
        Some(ChannelGenerationAttempt {
            cancel,
            done: Some(done_tx),
        })
    }

    fn retire(&self) -> ChannelGenerationDrainWait {
        let active = {
            let mut slot = self.state.active.lock();
            if self
                .state
                .retired
                .swap(true, std::sync::atomic::Ordering::AcqRel)
            {
                return ChannelGenerationDrainWait { done: None };
            }
            slot.take()
        };
        if let Some(clear) = &self.registry_clearer {
            clear();
        }
        if let Some(active) = active {
            active.cancel.cancel();
            return ChannelGenerationDrainWait {
                done: Some(active.done),
            };
        }
        ChannelGenerationDrainWait { done: None }
    }

    #[cfg(test)]
    fn is_retired(&self) -> bool {
        self.state
            .retired
            .load(std::sync::atomic::Ordering::Acquire)
    }
}

impl ChannelGenerationDrainWait {
    pub async fn wait(mut self) {
        if let Some(done) = self.done.take() {
            let _ = done.await;
        }
    }
}

impl PreparedChannelGenerationDrain {
    pub fn begin(&self) -> ChannelGenerationDrainWait {
        self.control.retire()
    }
}

impl ChannelGenerationAttempt {
    fn cancel(&self) -> tokio_util::sync::CancellationToken {
        self.cancel.clone()
    }
}

impl Drop for ChannelGenerationAttempt {
    fn drop(&mut self) {
        if let Some(done) = self.done.take() {
            let _ = done.send(());
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct StartupReadiness {
    gateway_generation: u64,
    gateway_addr: Option<std::net::SocketAddr>,
    socket_generation: u64,
    socket_ready: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
enum SocketStartupState {
    #[default]
    Pending,
    Ready,
    Fatal {
        kind: std::io::ErrorKind,
        message: String,
    },
}

#[derive(Clone)]
struct SocketStartupTracker {
    state_tx: tokio::sync::watch::Sender<SocketStartupState>,
}

impl SocketStartupTracker {
    fn new() -> (Self, tokio::sync::watch::Receiver<SocketStartupState>) {
        let (state_tx, state_rx) = tokio::sync::watch::channel(SocketStartupState::Pending);
        (Self { state_tx }, state_rx)
    }

    fn reporter(&self, inner: Option<SocketReadinessReporter>) -> Option<SocketReadinessReporter> {
        let state_tx = self.state_tx.clone();
        Some(SocketReadinessReporter::new(move || {
            state_tx.send_if_modified(|state| {
                if matches!(state, SocketStartupState::Pending) {
                    *state = SocketStartupState::Ready;
                    true
                } else {
                    false
                }
            });
            if let Some(inner) = &inner {
                inner.report_ready();
            }
        }))
    }

    fn record_error(&self, error: &anyhow::Error) {
        let Some(kind) = fatal_socket_startup_kind(error) else {
            return;
        };

        self.state_tx.send_if_modified(|state| {
            if matches!(state, SocketStartupState::Pending) {
                *state = SocketStartupState::Fatal {
                    kind,
                    message: format!("{error:#}"),
                };
                true
            } else {
                false
            }
        });
    }
}

/// Startup errors no supervisor restart can recover from: the endpoint is
/// already owned by another daemon, or the configured path can never be bound
/// (for example a Unix socket path over the platform `sun_path` limit, which
/// `std` reports as `InvalidInput` before the syscall).
fn fatal_socket_startup_kind(error: &anyhow::Error) -> Option<std::io::ErrorKind> {
    error
        .downcast_ref::<std::io::Error>()
        .map(std::io::Error::kind)
        .filter(|kind| {
            matches!(
                kind,
                std::io::ErrorKind::AddrInUse | std::io::ErrorKind::InvalidInput
            )
        })
}

#[derive(Clone)]
pub struct GatewayReadinessReporter(std::sync::Arc<dyn Fn(std::net::SocketAddr) + Send + Sync>);

impl GatewayReadinessReporter {
    pub fn new(report: impl Fn(std::net::SocketAddr) + Send + Sync + 'static) -> Self {
        Self(std::sync::Arc::new(report))
    }

    pub fn report_ready(&self, addr: std::net::SocketAddr) {
        (self.0)(addr);
    }
}

/// Start the live-pricing refresher for a standalone command that owns the
/// runtime without a daemon (`kinetic gateway`, `kinetic channel start`).
///
/// The daemon does not use this: it starts the refresher on its generation's
/// shared live configuration. A standalone command has no such handle, so
/// this binds a copy of `config`; the standalone gateway then re-binds the
/// refresher to the live handle its config API writes. The refresher starts at
/// most once per process and is a no-op unless a provider sets
/// `live_pricing = true`.
pub fn spawn_pricing_refresher(config: &Config) {
    kinetic_providers::pricing::spawn_refresher(std::sync::Arc::new(parking_lot::RwLock::new(
        config.clone(),
    )));
}

/// Wrap a gateway readiness reporter so the `on_gateway_start` hook fires when
/// the gateway reports the address it actually bound.
///
/// The process that owns the gateway (the daemon, or the standalone
/// `kinetic gateway` command) owns the hook, not the gateway listener. The
/// hook fires at most once per reporter, and each gateway start gets a fresh
/// reporter, so every start fires it exactly once even if readiness is
/// reported again. `host` is the configured bind host; the port is the one the
/// listener actually bound, which differs from the configured port when that
/// is 0. The hook runs on its own task so a slow handler cannot delay the
/// listener. Returns `inner` unchanged when hooks are disabled.
pub fn gateway_start_hook_reporter(
    hooks: Option<std::sync::Arc<crate::hooks::HookRunner>>,
    host: String,
    inner: Option<GatewayReadinessReporter>,
) -> Option<GatewayReadinessReporter> {
    let Some(hooks) = hooks else {
        return inner;
    };
    let fired = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    Some(GatewayReadinessReporter::new(move |addr| {
        if !fired.swap(true, std::sync::atomic::Ordering::SeqCst) {
            let hooks = std::sync::Arc::clone(&hooks);
            let host = host.clone();
            kinetic_spawn::spawn!(async move {
                hooks.fire_gateway_start(&host, addr.port()).await;
            });
        }
        if let Some(inner) = &inner {
            inner.report_ready(addr);
        }
    }))
}

#[derive(Clone)]
pub struct SocketReadinessReporter(std::sync::Arc<dyn Fn() + Send + Sync>);

impl SocketReadinessReporter {
    pub fn new(report: impl Fn() + Send + Sync + 'static) -> Self {
        Self(std::sync::Arc::new(report))
    }

    pub fn report_ready(&self) {
        (self.0)();
    }
}

#[derive(Clone, Copy)]
enum StartupComponent {
    Gateway,
    Socket,
}

struct StartupReadinessAttempt {
    readiness_tx: tokio::sync::watch::Sender<StartupReadiness>,
    component: StartupComponent,
    generation: u64,
}

impl StartupReadinessAttempt {
    fn gateway(
        readiness_tx: Option<tokio::sync::watch::Sender<StartupReadiness>>,
    ) -> (Option<Self>, Option<GatewayReadinessReporter>) {
        match readiness_tx {
            Some(readiness_tx) => {
                let mut generation = 0;
                readiness_tx.send_modify(|state| {
                    state.gateway_generation = state.gateway_generation.wrapping_add(1);
                    state.gateway_addr = None;
                    generation = state.gateway_generation;
                });
                let report_tx = readiness_tx.clone();
                let reporter = GatewayReadinessReporter::new(move |addr| {
                    report_tx.send_modify(|state| {
                        if state.gateway_generation == generation {
                            state.gateway_addr = Some(addr);
                        }
                    });
                });
                (
                    Some(Self {
                        readiness_tx,
                        component: StartupComponent::Gateway,
                        generation,
                    }),
                    Some(reporter),
                )
            }
            None => (None, None),
        }
    }

    fn socket(
        readiness_tx: Option<tokio::sync::watch::Sender<StartupReadiness>>,
    ) -> (Option<Self>, Option<SocketReadinessReporter>) {
        match readiness_tx {
            Some(readiness_tx) => {
                let mut generation = 0;
                readiness_tx.send_modify(|state| {
                    state.socket_generation = state.socket_generation.wrapping_add(1);
                    state.socket_ready = false;
                    generation = state.socket_generation;
                });
                let report_tx = readiness_tx.clone();
                let reporter = SocketReadinessReporter::new(move || {
                    report_tx.send_modify(|state| {
                        if state.socket_generation == generation {
                            state.socket_ready = true;
                        }
                    });
                });
                (
                    Some(Self {
                        readiness_tx,
                        component: StartupComponent::Socket,
                        generation,
                    }),
                    Some(reporter),
                )
            }
            None => (None, None),
        }
    }
}

impl Drop for StartupReadinessAttempt {
    fn drop(&mut self) {
        self.readiness_tx.send_modify(|state| match self.component {
            StartupComponent::Gateway if state.gateway_generation == self.generation => {
                state.gateway_generation = state.gateway_generation.wrapping_add(1);
                state.gateway_addr = None;
            }
            StartupComponent::Socket if state.socket_generation == self.generation => {
                state.socket_generation = state.socket_generation.wrapping_add(1);
                state.socket_ready = false;
            }
            _ => {}
        });
    }
}

mod registry;
pub use registry::{DaemonInboundAuthority, DaemonRegistry, GatewayReloadControls};

const STATUS_FLUSH_SECONDS: u64 = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DaemonExit {
    Shutdown,
    Reload,
}

enum Waited {
    Exit(Result<DaemonExit>),
    Ready(Option<StartupReadiness>),
}

const EPHEMERAL_GRACE_SECS: u64 = 1;

#[cfg(test)]
static SCHEDULER_CLEAN_SHUTDOWN_OBSERVED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

#[cfg(test)]
pub(crate) fn reset_scheduler_clean_shutdown_observed() {
    SCHEDULER_CLEAN_SHUTDOWN_OBSERVED.store(false, std::sync::atomic::Ordering::SeqCst);
}

#[cfg(test)]
pub(crate) fn scheduler_clean_shutdown_observed() -> bool {
    SCHEDULER_CLEAN_SHUTDOWN_OBSERVED.load(std::sync::atomic::Ordering::SeqCst)
}

async fn wait_for_exit_signal(
    mut reload_rx: tokio::sync::watch::Receiver<bool>,
    ephemeral: bool,
    client_count: std::sync::Arc<std::sync::atomic::AtomicUsize>,
) -> Result<DaemonExit> {
    use std::sync::atomic::Ordering;

    // Future that resolves when ephemeral shutdown is triggered:
    // waits for at least one client to connect, then for all clients to
    // disconnect, then sleeps the grace period. Pending forever if not
    // ephemeral.
    let ephemeral_shutdown = async {
        if !ephemeral {
            return std::future::pending::<()>().await;
        }
        // Wait until at least one client has connected.
        loop {
            if client_count.load(Ordering::Relaxed) > 0 {
                break;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        // Wait until all clients disconnect.
        loop {
            if client_count.load(Ordering::Relaxed) == 0 {
                break;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        ::kinetic_log::record!(
            INFO,
            ::kinetic_log::Event::new(module_path!(), ::kinetic_log::Action::Note)
                .with_attrs(::serde_json::json!({"grace_secs": EPHEMERAL_GRACE_SECS})),
            "All socket clients disconnected; starting ephemeral grace period"
        );
        // Grace period — if a client reconnects, abort.
        for _ in 0..EPHEMERAL_GRACE_SECS {
            tokio::time::sleep(Duration::from_secs(1)).await;
            if client_count.load(Ordering::Relaxed) > 0 {
                // Client reconnected — restart the whole wait.
                return Box::pin(wait_for_ephemeral(client_count.clone())).await;
            }
        }
    };
    tokio::pin!(ephemeral_shutdown);

    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};

        let mut sigint = signal(SignalKind::interrupt())?;
        let mut sigterm = signal(SignalKind::terminate())?;
        let mut sighup = signal(SignalKind::hangup())?;

        loop {
            tokio::select! {
                _ = sigint.recv() => {
                    ::kinetic_log::record!(INFO, ::kinetic_log::Event::new(module_path!(), ::kinetic_log::Action::Note), "Received SIGINT, shutting down...");
                    return Ok(DaemonExit::Shutdown);
                }
                _ = sigterm.recv() => {
                    ::kinetic_log::record!(INFO, ::kinetic_log::Event::new(module_path!(), ::kinetic_log::Action::Note), "Received SIGTERM, shutting down...");
                    return Ok(DaemonExit::Shutdown);
                }
                _ = sighup.recv() => {
                    ::kinetic_log::record!(INFO, ::kinetic_log::Event::new(module_path!(), ::kinetic_log::Action::Note), "Received SIGHUP, ignoring (daemon stays running)");
                }
                changed = reload_rx.changed() => {
                    if changed.is_err() {
                        ::kinetic_log::record!(WARN, ::kinetic_log::Event::new(module_path!(), ::kinetic_log::Action::Note).with_outcome(::kinetic_log::EventOutcome::Unknown), "Reload sender dropped; shutting down");
                        return Ok(DaemonExit::Shutdown);
                    }
                    if *reload_rx.borrow_and_update() {
                        ::kinetic_log::record!(INFO, ::kinetic_log::Event::new(module_path!(), ::kinetic_log::Action::Note), "Reload requested via /admin/reload");
                        return Ok(DaemonExit::Reload);
                    }
                }
                _ = &mut ephemeral_shutdown => {
                    ::kinetic_log::record!(INFO, ::kinetic_log::Event::new(module_path!(), ::kinetic_log::Action::Note), "Ephemeral daemon: no clients remaining, shutting down");
                    return Ok(DaemonExit::Shutdown);
                }
            }
        }
    }

    #[cfg(not(unix))]
    {
        // In-process shutdown trigger (no SIGTERM on Windows): the gateway fires
        // this to request a graceful exit, e.g. for post-upgrade self-respawn.
        let respawn_shutdown = crate::restart::shutdown_notify().notified();
        tokio::pin!(respawn_shutdown);
        loop {
            tokio::select! {
                res = tokio::signal::ctrl_c() => {
                    res?;
                    ::kinetic_log::record!(INFO, ::kinetic_log::Event::new(module_path!(), ::kinetic_log::Action::Note), "Received Ctrl+C, shutting down...");
                    return Ok(DaemonExit::Shutdown);
                }
                _ = &mut respawn_shutdown => {
                    ::kinetic_log::record!(INFO, ::kinetic_log::Event::new(module_path!(), ::kinetic_log::Action::Note), "In-process shutdown requested, shutting down...");
                    return Ok(DaemonExit::Shutdown);
                }
                changed = reload_rx.changed() => {
                    if changed.is_err() {
                        ::kinetic_log::record!(WARN, ::kinetic_log::Event::new(module_path!(), ::kinetic_log::Action::Note).with_outcome(::kinetic_log::EventOutcome::Unknown), "Reload sender dropped; shutting down");
                        return Ok(DaemonExit::Shutdown);
                    }
                    if *reload_rx.borrow_and_update() {
                        ::kinetic_log::record!(INFO, ::kinetic_log::Event::new(module_path!(), ::kinetic_log::Action::Note), "Reload requested via /admin/reload");
                        return Ok(DaemonExit::Reload);
                    }
                }
                _ = &mut ephemeral_shutdown => {
                    ::kinetic_log::record!(INFO, ::kinetic_log::Event::new(module_path!(), ::kinetic_log::Action::Note), "Ephemeral daemon: no clients remaining, shutting down");
                    return Ok(DaemonExit::Shutdown);
                }
            }
        }
    }
}

/// Recursive helper: wait for clients to connect then all disconnect, with grace period.
async fn wait_for_ephemeral(client_count: std::sync::Arc<std::sync::atomic::AtomicUsize>) {
    use std::sync::atomic::Ordering;
    // Wait until all clients disconnect again.
    loop {
        if client_count.load(Ordering::Relaxed) == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    ::kinetic_log::record!(
        INFO,
        ::kinetic_log::Event::new(module_path!(), ::kinetic_log::Action::Note)
            .with_attrs(::serde_json::json!({"grace_secs": EPHEMERAL_GRACE_SECS})),
        "All socket clients disconnected; starting ephemeral grace period"
    );
    for _ in 0..EPHEMERAL_GRACE_SECS {
        tokio::time::sleep(Duration::from_secs(1)).await;
        if client_count.load(Ordering::Relaxed) > 0 {
            return Box::pin(wait_for_ephemeral(client_count)).await;
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GatewayBindMode {
    /// Address is free (or an ephemeral port): start and supervise our own gateway.
    StartFresh,
    /// A KineticVM gateway already holds the address (e.g. a standalone
    /// `kinetic gateway start`): fail fast rather than start a second gateway
    /// on the same port.
    GatewayAlreadyRunning,
    /// Address is held by some other process: fail fast rather than degrade into
    /// a supervisor retry loop on the bind.
    PortOccupied,
}

/// Map the configured gateway bind host to a concrete authority reachable for a
/// local `/health` probe, formatted for a URL. Mirrors the CLI `self_test`
/// probe: wildcard `0.0.0.0` -> `127.0.0.1`, IPv6 wildcard `::`/`[::]` ->
/// `[::1]`; a bare concrete IPv6 host is bracketed.
fn gateway_probe_authority(host: &str) -> String {
    match host {
        "0.0.0.0" => "127.0.0.1".to_string(),
        "::" | "[::]" => "[::1]".to_string(),
        other if other.contains(':') && !other.starts_with('[') => format!("[{other}]"),
        other => other.to_string(),
    }
}

/// Build the `/health` probe URL for the configured gateway, honouring the
/// gateway's TLS scheme and `path_prefix` so a prefixed or HTTPS gateway is
/// probed where it actually serves health.
fn gateway_health_probe_url(config: &Config, host: &str, port: u16) -> String {
    let scheme = if config.gateway.tls.as_ref().is_some_and(|tls| tls.enabled) {
        "https"
    } else {
        "http"
    };
    // `path_prefix` is validated to start with `/` and not end with `/`.
    let prefix = config.gateway.path_prefix.as_deref().unwrap_or("");
    format!(
        "{scheme}://{}:{port}{prefix}/health",
        gateway_probe_authority(host)
    )
}

async fn kinetic_gateway_responds(config: &Config, host: &str, port: u16) -> bool {
    let url = gateway_health_probe_url(config, host, port);
    let Ok(client) = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_millis(500))
        .build()
    else {
        return false;
    };
    let Ok(response) = client.get(&url).send().await else {
        return false;
    };
    if !response.status().is_success() {
        return false;
    }
    matches!(
        response.json::<serde_json::Value>().await,
        Ok(body)
            if body.get("status").and_then(|s| s.as_str()) == Some("ok")
                && body
                    .get("require_pairing")
                    .is_some_and(serde_json::Value::is_boolean)
                && body.get("runtime").is_some_and(serde_json::Value::is_object)
    )
}

pub async fn detect_gateway_bind_mode(config: &Config, host: &str, port: u16) -> GatewayBindMode {
    // Port 0 is a kernel-assigned ephemeral port: it cannot already be bound,
    // so always start fresh.
    if port == 0 {
        return GatewayBindMode::StartFresh;
    }

    // Mirror the gateway's own bind exactly. If host:port does not parse as a
    // socket address, defer to the gateway (it has its own fallback) rather
    // than pre-judging the address.
    let Ok(addr) = kinetic_infra::parse_gateway_bind_socket_addr(host, port) else {
        return GatewayBindMode::StartFresh;
    };

    classify_gateway_bind_outcome(
        tokio::net::TcpListener::bind(addr).await,
        config,
        host,
        port,
    )
    .await
}

async fn classify_gateway_bind_outcome(
    bind: std::io::Result<tokio::net::TcpListener>,
    config: &Config,
    host: &str,
    port: u16,
) -> GatewayBindMode {
    match bind {
        Ok(listener) => {
            drop(listener);
            GatewayBindMode::StartFresh
        }
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
            if kinetic_gateway_responds(config, host, port).await {
                GatewayBindMode::GatewayAlreadyRunning
            } else {
                GatewayBindMode::PortOccupied
            }
        }
        Err(_) => GatewayBindMode::StartFresh,
    }
}

pub async fn run(
    mut config: Config,
    host: String,
    port: u16,
    registry: DaemonRegistry,
    ephemeral: bool,
    startup_feedback_enabled: bool,
) -> Result<DaemonExit> {
    config.gateway.host = host.clone();
    if port != 0 {
        config.gateway.port = port;
    }
    let live_config_authority = crate::LiveConfigAuthority::new_owned(config.clone())?;
    let (exit, ownership) = run_with_authority(
        live_config_authority,
        host,
        port,
        registry,
        ephemeral,
        startup_feedback_enabled,
    )
    .await?;
    // `run` owns its guard for one invocation: shutdown released it inside
    // `run_with_authority`, and a reload releases it here — the caller re-enters
    // `run` (and re-acquires) for the next generation.
    drop(ownership);
    Ok(exit)
}

/// Use the authority acquired before constructing producers such as SOP maintenance.
///
/// On `DaemonExit::Reload` the process-ownership guard is drained with
/// ownership retained and returned to the caller, so the next daemon
/// generation can adopt it without an unlocked read/reacquire interval. On
/// shutdown or error the guard is released here.
pub async fn run_with_authority(
    live_config_authority: crate::LiveConfigAuthority,
    host: String,
    port: u16,
    mut registry: DaemonRegistry,
    ephemeral: bool,
    startup_feedback_enabled: bool,
) -> Result<(
    DaemonExit,
    Option<crate::live_config_authority::ConfigOwnershipGuard>,
)> {
    let config = live_config_authority.config().read().clone();
    let initial_backoff = config.reliability.channel_initial_backoff_secs.max(1);
    let max_backoff = config
        .reliability
        .channel_max_backoff_secs
        .max(initial_backoff);

    crate::health::mark_component_ok("daemon");

    // The daemon owns the event bus: every component (gateway, cron,
    // heartbeat, RPC) publishes on and reads from this one. Its observer is
    // installed as the process-wide broadcast hook here, exactly once per run,
    // so `logs/subscribe` and `events/history` carry agent, tool, and LLM
    // frames whether or not the gateway runs. The guard lives until `run`
    // returns.
    let event_bus = crate::observability::EventBus::new();
    let _event_hook_guard = event_bus.install_hook();
    let event_tx = event_bus.sender().clone();

    kinetic_log::set_broadcast_hook(event_tx.clone());

    if config.heartbeat.enabled
        && let Ok((_, heartbeat_workspace_dir)) = resolve_heartbeat_workspace_dir(&config)
    {
        let _ = crate::heartbeat::engine::HeartbeatEngine::ensure_heartbeat_file(
            &heartbeat_workspace_dir,
        )
        .await;
    }

    crate::agent::pricing_catalog::load_global_pricing_catalog(&config.data_dir);

    let mut handles: Vec<JoinHandle<()>> = vec![spawn_state_writer(config.clone())];
    let mut channels_handle: Option<JoinHandle<()>> = None;

    // Reload channel: gateway's /admin/reload writes here; our wait loop
    // (below) selects on it alongside OS signals. Cross-platform.
    let (reload_tx, reload_rx) = tokio::sync::watch::channel::<bool>(false);

    let channels_cancel = tokio_util::sync::CancellationToken::new();
    let channel_generation_control = std::sync::Arc::new(ChannelGenerationControl::new(
        registry.take_channel_registry_clearer(),
    ));
    let (gateway_shutdown_tx, _) = tokio::sync::watch::channel::<bool>(false);
    let (startup_readiness_tx, startup_readiness_rx) = if startup_feedback_enabled {
        let (tx, rx) = tokio::sync::watch::channel(StartupReadiness::default());
        (Some(tx), Some(rx))
    } else {
        (None, None)
    };
    let (socket_startup_tracker, socket_startup_rx) = SocketStartupTracker::new();
    let mut gateway_required = false;
    let mut socket_required = false;

    // Construct the TUI registry early so both the gateway (for /api/tuis)
    // and the RPC socket (for tui/list) share the same Arc.
    let tui_registry =
        std::sync::Arc::new(crate::rpc::tui_identity::TuiRegistry::new(&config.data_dir));

    // Canonical live pairing authority for this daemon generation. The
    // gateway serves /pair, rotation, and revocation from THIS instance
    // and the RPC native auth provider verifies against it, so a pairing
    // change reaches both surfaces immediately (no boot-time snapshot).
    let pairing_guard = std::sync::Arc::new(kinetic_config::pairing::PairingGuard::new(
        config.gateway.require_pairing,
        &config.gateway.paired_tokens,
        config.gateway.pairing_code,
    ));

    // One inbound-auth state for this daemon generation. The RPC context
    // and the supervised gateway authenticate against the same accepted
    // policy over the same live configuration, and both publish into it
    // under the process-wide config write lock. A revocation persisted
    // through either surface therefore binds the other before the writer
    // returns, not at the next daemon reload.
    let live_config = live_config_authority.config();
    // The daemon owns the live-pricing refresher, so it runs whether or not the
    // gateway is enabled. It follows this generation's live configuration, the
    // one the RPC context and the supervised gateway both write in place, so
    // an operator's change reaches the next refresh from either surface
    // without a reload. A reload starts a new generation and re-binds it.
    kinetic_providers::pricing::spawn_refresher(std::sync::Arc::clone(&live_config));
    let inbound_auth = std::sync::Arc::new(
        crate::rpc::auth::RpcInboundAuth::from_config(&config, pairing_guard.clone()).map_err(
            |e| anyhow::Error::msg(format!("building the inbound authentication layer: {e:#}")),
        )?,
    );

    if let Some(gateway_start) = registry.take_gateway_start() {
        gateway_required = true;
        let gateway_cfg = config.clone();
        let gateway_authority = DaemonInboundAuthority {
            pairing: pairing_guard.as_ref().clone(),
            inbound_auth: std::sync::Arc::clone(&inbound_auth),
            config: std::sync::Arc::clone(&live_config),
        };
        let gateway_host = host.clone();
        let gateway_event_bus = event_bus.clone();
        let gateway_reload_controls = GatewayReloadControls {
            shutdown_tx: gateway_shutdown_tx.clone(),
            reload_tx: reload_tx.clone(),
            channel_generation_control: Some(channel_generation_control.clone()),
        };
        let gateway_tui_registry = tui_registry.clone();
        let gateway_start = std::sync::Arc::new(gateway_start);
        let gateway_readiness_tx = startup_readiness_tx.clone();
        let gateway_live_config_authority = live_config_authority.clone();
        // The daemon owns the gateway-start hook. It is built once per daemon
        // generation and fired from each gateway start's readiness report.
        let gateway_hooks: Option<std::sync::Arc<crate::hooks::HookRunner>> = config
            .hooks
            .enabled
            .then(|| std::sync::Arc::new(crate::hooks::HookRunner::from_config(&config.hooks)));
        handles.push(spawn_component_supervisor(
            "gateway",
            initial_backoff,
            max_backoff,
            channels_cancel.clone(),
            move || {
                let cfg = gateway_cfg.clone();
                let host = gateway_host.clone();
                let bus = gateway_event_bus.clone();
                let reload_controls = gateway_reload_controls.clone();
                let tui_reg = gateway_tui_registry.clone();
                let start = gateway_start.clone();
                let live_config_authority = gateway_live_config_authority.clone();
                let authority = gateway_authority.clone();
                let (readiness_attempt, readiness_reporter) =
                    StartupReadinessAttempt::gateway(gateway_readiness_tx.clone());
                let readiness_reporter = gateway_start_hook_reporter(
                    gateway_hooks.clone(),
                    host.clone(),
                    readiness_reporter,
                );
                async move {
                    let _readiness_attempt = readiness_attempt;
                    start(
                        host,
                        port,
                        cfg,
                        live_config_authority,
                        Some(bus),
                        Some(reload_controls),
                        Some(tui_reg),
                        Some(authority),
                        readiness_reporter,
                    )
                    .await
                }
            },
        ));
    }

    if crate::control_plane::control_plane().is_none()
        && let Err(e) = crate::control_plane::ControlPlaneRecoveryOwner::start(&config.data_dir)
            .await
            .map(crate::control_plane::init_control_plane)
    {
        ::kinetic_log::record!(
            WARN,
            ::kinetic_log::Event::new(module_path!(), ::kinetic_log::Action::Note)
                .with_outcome(::kinetic_log::EventOutcome::Unknown)
                .with_attrs(::serde_json::json!({ "error": format!("{e:#}") })),
            "control-plane failed to start; supervision disabled for this run"
        );
    }
    // Respawn the reaper for THIS run iteration against the INSTALLED handle, so its
    // boot_id matches what producers stamp via `control_plane()`.
    if crate::control_plane::control_plane().is_some() {
        let _ = crate::control_plane::spawn_control_plane_reaper(
            crate::control_plane::reaper::DEFAULT_MAX_RUNTIME_SECS,
            channels_cancel.clone(),
        );
        crate::health::mark_component_ok("control-plane");
    }

    if let Some(channels_start) = registry.take_channels_start() {
        if has_supervised_channels(&config) {
            let channels_start = std::sync::Arc::new(channels_start);
            let cancel_for_supervisor = channels_cancel.clone();
            let generation_control = channel_generation_control.clone();
            let channels_live_config_authority = live_config_authority.clone();
            channels_handle = Some(spawn_component_supervisor(
                "channels",
                initial_backoff,
                max_backoff,
                channels_cancel.clone(),
                move || {
                    let start = channels_start.clone();
                    let cancel = cancel_for_supervisor.clone();
                    let generation_control = generation_control.clone();
                    let live_config_authority = channels_live_config_authority.clone();
                    async move {
                        let Some(attempt) = generation_control.begin_attempt(&cancel) else {
                            // A retired generation must never retry the old
                            // configuration. Wait for daemon shutdown/reload
                            // so the generic supervisor cannot hot-loop.
                            cancel.cancelled().await;
                            return Ok(());
                        };
                        let attempt_cancel = attempt.cancel();
                        let result = start(live_config_authority, attempt_cancel).await;
                        drop(attempt);
                        result
                    }
                },
            ));
        } else {
            crate::health::mark_component_ok("channels");
            ::kinetic_log::record!(
                INFO,
                ::kinetic_log::Event::new(module_path!(), ::kinetic_log::Action::Note),
                "No channels configured; channel supervisor disabled"
            );
        }
    } else {
        crate::health::mark_component_ok("channels");
        ::kinetic_log::record!(
            INFO,
            ::kinetic_log::Event::new(module_path!(), ::kinetic_log::Action::Note),
            "Channels subsystem not wired; channel supervisor disabled"
        );
    }

    // RPC transports: Unix socket and WSS (remote TUI connections).
    // Build the shared RpcContext if either transport is configured.
    let socket_client_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let need_rpc_ctx = registry.has_socket_start()
        || registry.has_wss_start()
        || registry.has_relay_start()
        || registry.has_enroll_start();

    // Extract shared SOP engine from registry for RpcContext.
    let (sop_engine, sop_audit, sop_driver_handles) = registry.take_sop_engine();

    let rpc_ctx = if need_rpc_ctx {
        use crate::rpc::context::RpcContext;
        use crate::rpc::session::SessionStore;
        use kinetic_infra::session_queue::SessionActorQueue;

        let session_queue = std::sync::Arc::new(SessionActorQueue::new(32, 30, 600));
        let sessions = std::sync::Arc::new(SessionStore::new(64, session_queue.clone()));

        {
            let reaper_queue = std::sync::Arc::clone(&session_queue);
            kinetic_spawn::spawn!(async move {
                const TICK: std::time::Duration = std::time::Duration::from_secs(60);
                let mut interval = tokio::time::interval(TICK);
                interval.tick().await;
                loop {
                    interval.tick().await;
                    let queue_evicted = reaper_queue.evict_idle().await;
                    if queue_evicted > 0 {
                        let span = ::kinetic_log::info_span!(
                            target: "kinetic_log_internal_scope",
                            "kinetic_scope",
                            channel = "rpc",
                        );
                        let _guard = span.enter();
                        ::kinetic_log::record!(
                            INFO,
                            ::kinetic_log::Event::new(module_path!(), ::kinetic_log::Action::Note,)
                                .with_category(::kinetic_log::EventCategory::Agent)
                                .with_attrs(::serde_json::json!({
                                    "evicted_queue_slots": queue_evicted,
                                })),
                            "Session queue: released idle actor-queue slots"
                        );
                        crate::util::release_freed_heap();
                    }
                }
            });
        }
        let session_backend =
            kinetic_infra::make_session_backend(&config.data_dir, &config.channels.session_backend)
                .ok();

        // Wire the memory subsystem so `memory/list` and `memory/search`
        // work over RPC transports (same pattern as the gateway).
        let rpc_memory: Option<std::sync::Arc<dyn kinetic_api::memory_traits::Memory>> =
            if config.agents.is_empty() {
                None
            } else {
                match kinetic_memory::create_memory_from_config(&config, None) {
                    Ok(mem) => Some(std::sync::Arc::from(mem)),
                    Err(_e) => {
                        ::kinetic_log::record!(
                            WARN,
                            ::kinetic_log::Event::new(module_path!(), ::kinetic_log::Action::Note),
                            "RPC memory subsystem unavailable"
                        );
                        None
                    }
                }
            };

        // Open the ACP session DB at boot so the file exists from the
        // moment the daemon is up, not when (if ever) `kinetic acp`
        // runs. Best-effort: on failure, log and continue with `None`.
        let acp_session_store: Option<
            std::sync::Arc<kinetic_infra::acp_session_store::AcpSessionStore>,
        > = match kinetic_infra::acp_session_store::AcpSessionStore::new(&config.data_dir) {
            Ok(s) => Some(std::sync::Arc::new(s)),
            Err(e) => {
                ::kinetic_log::record!(
                    WARN,
                    ::kinetic_log::Event::new(module_path!(), ::kinetic_log::Action::Note)
                        .with_outcome(::kinetic_log::EventOutcome::Failure)
                        .with_attrs(::serde_json::json!({"error": e.to_string()})),
                    "Failed to open ACP session store at daemon boot"
                );
                None
            }
        };

        // THE certificate audit logger for this daemon iteration. Built once
        // here and shared through RpcContext so enrollment, in-band renewal
        // and the issued-cert ledger all append through a single Merkle-chain
        // writer. A per-request logger recovers the same chain tip as its
        // siblings and races them into duplicate sequence numbers, which makes
        // `verify_chain` reject a file no single writer got wrong.
        //
        // Best-effort, like the ACP store above: a logger that cannot be
        // constructed (e.g. `sign_events` with no usable signing key) leaves
        // `cert_audit` unset, and the certificate paths then refuse to issue
        // rather than issue untraceably.
        let cert_audit: Option<std::sync::Arc<crate::security::audit::AuditLogger>> =
            match crate::security::audit::AuditLogger::open_shared(
                config.security.audit.clone(),
                config.data_dir.clone(),
            ) {
                Ok(logger) => Some(logger),
                Err(e) => {
                    ::kinetic_log::record!(
                        ERROR,
                        ::kinetic_log::Event::new(module_path!(), ::kinetic_log::Action::Fail)
                            .with_outcome(::kinetic_log::EventOutcome::Failure)
                            .with_attrs(::serde_json::json!({"error": e.to_string()})),
                        "certificate audit logger unavailable: enrollment and certificate \
                         renewal will refuse to issue"
                    );
                    None
                }
            };

        let hooks: Option<std::sync::Arc<crate::hooks::HookRunner>> = if config.hooks.enabled {
            Some(std::sync::Arc::new(
                crate::hooks::HookRunner::from_root_config(&config),
            ))
        } else {
            None
        };

        let (rpc_config, rpc_config_write_lock) =
            RpcContext::config_handles_for_authority(&live_config_authority);
        // The generation's shared inbound-auth state (see `inbound_auth`
        // above): the gateway authenticates and publishes against these same
        // instances.
        let rpc_auth = std::sync::Arc::clone(&inbound_auth);

        Some(std::sync::Arc::new(RpcContext {
            #[cfg(test)]
            config_commit_pause: None,
            config: rpc_config,
            config_write_lock: rpc_config_write_lock,
            agent_lifecycle: live_config_authority.agent_lifecycle(),
            channel_generation_control: Some(channel_generation_control.clone()),
            sessions,
            session_backend,
            memory: rpc_memory,
            // Process-global tracker shared with the gateway and channel
            // supervisor. Without this the RPC/zerocode-TUI turn path has no
            // tracker to record into and model cost is silently dropped
            cost_tracker: crate::cost::CostTracker::get_or_init_global(
                config.cost.clone(),
                &config.data_dir,
            ),
            event_tx: Some(event_tx.clone()),
            event_history: Some(std::sync::Arc::clone(event_bus.history())),
            subscriptions: {
                let hub = std::sync::Arc::new(crate::rpc::subscription::SubscriptionHub::new());
                hub.attach_bus(&event_tx);
                hub
            },
            reload_tx: Some(reload_tx.clone()),
            gateway_shutdown_tx: Some(gateway_shutdown_tx.clone()),
            approval_pending: std::sync::Arc::new(
                crate::rpc::context::ApprovalPendingMap::default(),
            ),
            tui_registry,
            acp_session_store,
            sop_engine,
            sop_audit,
            sop_driver_handles,
            hooks,
            cert_audit,
            auth: rpc_auth,
        }))
    } else {
        None
    };

    // Local IPC RPC listener (Unix socket on Unix, Named Pipe on Windows).
    if let Some(socket_start) = registry.take_socket_start() {
        socket_required = true;
        let rpc_ctx = rpc_ctx
            .clone()
            .expect("rpc_ctx built when socket_start is Some");
        let socket_start = std::sync::Arc::new(socket_start);
        let socket_cancel = channels_cancel.clone();
        let count = socket_client_count.clone();
        let socket_readiness_tx = startup_readiness_tx.clone();
        let startup_tracker = socket_startup_tracker.clone();
        handles.push(spawn_component_supervisor(
            "socket",
            initial_backoff,
            max_backoff,
            socket_cancel.clone(),
            move || {
                let ctx = rpc_ctx.clone();
                let start = socket_start.clone();
                let cancel = socket_cancel.clone();
                let count = count.clone();
                let (readiness_attempt, readiness_reporter) =
                    StartupReadinessAttempt::socket(socket_readiness_tx.clone());
                let readiness_reporter = startup_tracker.reporter(readiness_reporter);
                let startup_tracker = startup_tracker.clone();
                async move {
                    let _readiness_attempt = readiness_attempt;
                    let result = start(ctx, cancel, count, readiness_reporter).await;
                    if let Err(error) = &result {
                        startup_tracker.record_error(error);
                    }
                    result
                }
            },
        ));
    }

    // WSS RPC listener (remote TUI connections).
    if let Some(wss_start) = registry.take_wss_start() {
        let rpc_ctx = rpc_ctx
            .clone()
            .expect("rpc_ctx built when wss_start is Some");
        let wss_start = std::sync::Arc::new(wss_start);
        let wss_cancel = channels_cancel.clone();
        let count = socket_client_count.clone();
        handles.push(spawn_component_supervisor(
            "wss",
            initial_backoff,
            max_backoff,
            wss_cancel.clone(),
            move || {
                let ctx = rpc_ctx.clone();
                let start = wss_start.clone();
                let cancel = wss_cancel.clone();
                let count = count.clone();
                async move { start(ctx, cancel, count).await }
            },
        ));
    }

    // Relay bridge: keeps an outbound connection to a nominated relay so clients
    // can reach this daemon through it. Supervised like the WSS listener; the
    // starter parks when `[relay]` is disabled.
    if let Some(relay_start) = registry.take_relay_start() {
        let rpc_ctx = rpc_ctx
            .clone()
            .expect("rpc_ctx built when relay_start is Some");
        let relay_start = std::sync::Arc::new(relay_start);
        let relay_cancel = channels_cancel.clone();
        let count = socket_client_count.clone();
        handles.push(spawn_component_supervisor(
            "relay",
            initial_backoff,
            max_backoff,
            relay_cancel.clone(),
            move || {
                let ctx = rpc_ctx.clone();
                let start = relay_start.clone();
                let cancel = relay_cancel.clone();
                let count = count.clone();
                async move { start(ctx, cancel, count).await }
            },
        ));
    }

    // Certificate enrollment endpoint: the bootstrap surface a certless client
    // reaches for its first cert. Supervised like the WSS listener; the starter
    // parks when `[enroll]` is disabled.
    if let Some(enroll_start) = registry.take_enroll_start() {
        let rpc_ctx = rpc_ctx
            .clone()
            .expect("rpc_ctx built when enroll_start is Some");
        let enroll_start = std::sync::Arc::new(enroll_start);
        let enroll_cancel = channels_cancel.clone();
        let count = socket_client_count.clone();
        handles.push(spawn_component_supervisor(
            "enroll",
            initial_backoff,
            max_backoff,
            enroll_cancel.clone(),
            move || {
                let ctx = rpc_ctx.clone();
                let start = enroll_start.clone();
                let cancel = enroll_cancel.clone();
                let count = count.clone();
                async move { start(ctx, cancel, count).await }
            },
        ));
    }

    // Wire up MQTT SOP listener if configured and referenced by an enabled agent
    if let Some(mqtt_start) = registry.take_mqtt_start() {
        let active_mqtt: std::collections::HashSet<String> = config
            .agents
            .values()
            .filter(|a| a.enabled)
            .flat_map(|a| a.channels.iter().map(|c| c.as_str().to_string()))
            .collect();
        let mut mqtt_started = false;
        for (alias, mqtt_config) in &config.channels.mqtt {
            if !active_mqtt.contains(&format!("mqtt.{alias}")) {
                continue;
            }
            let mqtt_cfg = mqtt_config.clone();
            let mqtt_start = std::sync::Arc::new(mqtt_start);
            handles.push(spawn_component_supervisor(
                "mqtt",
                initial_backoff,
                max_backoff,
                channels_cancel.clone(),
                move || {
                    let cfg = mqtt_cfg.clone();
                    let start = mqtt_start.clone();
                    async move { start(cfg).await }
                },
            ));
            mqtt_started = true;
            break;
        }
        if !mqtt_started {
            crate::health::mark_component_ok("mqtt");
        }
    } else {
        crate::health::mark_component_ok("mqtt");
    }

    let daemon_execution_capability = live_config_authority.execution_capability();

    if config.heartbeat.enabled {
        let heartbeat_cfg = config.clone();
        let heartbeat_execution_capability = daemon_execution_capability.clone();
        handles.push(spawn_component_supervisor(
            "heartbeat",
            initial_backoff,
            max_backoff,
            channels_cancel.clone(),
            move || {
                let cfg = heartbeat_cfg.clone();
                let execution_capability = heartbeat_execution_capability.clone();
                async move { Box::pin(run_heartbeat_worker(cfg, execution_capability)).await }
            },
        ));
    }

    if config.scheduler.enabled {
        let scheduler_cfg = config.clone();
        let scheduler_event_tx = event_tx.clone();
        let scheduler_cancel = channels_cancel.clone();
        handles.push(spawn_component_supervisor(
            "scheduler",
            initial_backoff,
            max_backoff,
            channels_cancel.clone(),
            move || {
                let cfg = scheduler_cfg.clone();
                let tx = scheduler_event_tx.clone();
                let cancel = scheduler_cancel.clone();
                let execution_capability = daemon_execution_capability.clone();
                async move {
                    Box::pin(crate::cron::scheduler::run_with_capability(
                        cfg,
                        Some(tx),
                        cancel,
                        Some(execution_capability),
                    ))
                    .await
                }
            },
        ));
    } else {
        crate::health::mark_component_ok("scheduler");
        ::kinetic_log::record!(
            INFO,
            ::kinetic_log::Event::new(module_path!(), ::kinetic_log::Action::Note),
            "Cron disabled; scheduler supervisor not started"
        );
    }

    // Wait for shutdown (SIGINT/SIGTERM/Ctrl+C), reload (in-process channel),
    // or an unrecoverable initial socket ownership conflict.
    let rpc_connection_count = socket_client_count.clone();
    let exit = wait_for_exit_signal(reload_rx, ephemeral, socket_client_count);
    tokio::pin!(exit);

    let startup_result = if socket_required {
        let socket_startup = await_socket_startup(socket_startup_rx);
        tokio::pin!(socket_startup);
        tokio::select! {
            biased;
            exit = &mut exit => exit.map(Some),
            startup = &mut socket_startup => startup.map(|()| None),
        }
    } else {
        Ok(None)
    };

    let exit_result = match startup_result {
        Err(error) => Err(error),
        Ok(Some(exit)) => Ok(exit),
        Ok(None) if startup_feedback_enabled => {
            record_daemon_started(&config, &host, port);
            let bar = crate::brand::ConnectBar::start(
                crate::i18n::get_required_cli_string("cli-brand-connecting"),
                stderr_is_interactive_foreground(),
            );
            let readiness =
                await_startup_readiness(startup_readiness_rx, gateway_required, socket_required);
            tokio::pin!(readiness);

            let waited = tokio::select! {
                biased;
                exit = &mut exit => Waited::Exit(exit),
                readiness = &mut readiness => Waited::Ready(readiness),
            };
            bar.finish().await;
            match waited {
                Waited::Exit(exit) => exit,
                Waited::Ready(readiness) => {
                    if let Some(readiness) = readiness
                        && stderr_is_interactive_foreground()
                    {
                        let mut stderr = std::io::stderr().lock();
                        let _ = echo_daemon_ready_to_terminal(&config, readiness, &mut stderr);
                    }
                    exit.await
                }
            }
        }
        Ok(None) => {
            record_daemon_started(&config, &host, port);
            exit.await
        }
    };
    match &exit_result {
        Ok(exit) => crate::health::mark_component_error(
            "daemon",
            match exit {
                DaemonExit::Shutdown => "shutdown requested",
                DaemonExit::Reload => "reload requested",
            },
        ),
        Err(error) => crate::health::mark_component_error("daemon", format!("{error:#}")),
    }

    // Freeze admission before stopping ingress; detached final writes must
    // drain as well as component workers before process ownership is released.
    live_config_authority.close_agent_lifecycle();
    channels_cancel.cancel();

    // Retire the accepted RPC connections before the component handles are
    // touched. Each connection cancels and joins its prompts, and its listener
    // force-aborts it after `rpc::CONNECTION_DRAIN_GRACE`; the liveness token
    // every task started by the connection holds keeps the count above zero
    // until those tasks have actually ended, so waiting for zero is what stops
    // the replacement generation from admitting work for a session the
    // retiring generation is still unwinding. A reload that cannot establish
    // that is refused below instead of handing over. Only this wait carries
    // the listeners' budget: the per-component grace below stays as it was, so
    // an unrelated pending component adds no reload latency.
    let drain = await_rpc_connection_drain(&rpc_connection_count).await;
    let exit_result = settle_exit_against_drain(exit_result, drain);

    // Channel teardown owns listener cleanup plus all accepted message work.
    // Keep that supervisor out of the generic 500 ms component pool: its
    // internal absolute deadline is five seconds, and a reload may start a
    // replacement generation only after this owner has actually retired.
    let channels_retired = retire_channels_supervisor(channels_handle).await;
    let exit_result = settle_exit_against_channel_retirement(exit_result, channels_retired);

    // Grace window for cooperative shutdown of each component supervisor. The
    // RPC listeners are already past their own drain by this point, so this
    // only covers the supervisor loop returning after its component did.
    const GRACE_WINDOW: Duration = Duration::from_millis(500);
    let deadline = tokio::time::Instant::now() + GRACE_WINDOW;
    let mut remaining: Vec<JoinHandle<()>> = Vec::new();
    for mut handle in handles {
        tokio::select! {
            biased;
            _ = &mut handle => {
                // Cooperative handle exited cleanly during grace window.
            }
            _ = tokio::time::sleep_until(deadline) => {
                // Grace window expired; force-abort and re-join later.
                handle.abort();
                remaining.push(handle);
            }
        }
    }
    // Await remaining (aborted) handles. Already-completed handles from
    // the grace window are not re-await, so "JoinHandle polled after
    // completion" is avoided.
    for handle in remaining {
        let _ = handle.await;
    }

    drop(rpc_ctx);
    // A reload transfers process ownership continuously into the next daemon
    // generation: drain with the guard retained and hand it back to the
    // caller. Shutdown and errors release ownership here, as before.
    let transferred_ownership = if matches!(&exit_result, Ok(DaemonExit::Reload)) {
        live_config_authority
            .drain_agent_lifecycle_retaining_ownership()
            .await;
        live_config_authority.take_process_ownership()
    } else {
        live_config_authority.drain_agent_lifecycle().await;
        None
    };

    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    // SAFETY: glibc's parameter-free process allocator trim may release free
    // arenas after all daemon tasks have been joined; no Rust pointers are
    // retained across the call.
    unsafe {
        libc::malloc_trim(0);
    }

    exit_result.map(|exit| (exit, transferred_ownership))
}

pub fn state_file_path(config: &Config) -> PathBuf {
    config
        .config_path
        .parent()
        .map_or_else(|| PathBuf::from("."), PathBuf::from)
        .join("state")
        .join("daemon_state.json")
}

fn record_daemon_started(config: &Config, host: &str, port: u16) {
    ::kinetic_log::record!(
        INFO,
        ::kinetic_log::Event::new(module_path!(), ::kinetic_log::Action::Start)
            .with_category(::kinetic_log::EventCategory::System)
            .with_outcome(::kinetic_log::EventOutcome::Success)
            .with_attrs(::serde_json::json!({
                "requested_gateway": format!("http://{host}:{port}"),
                "socket": crate::rpc::local::socket_path(config).display().to_string(),
                "pairing_enabled": config.gateway.require_pairing,
                "stop_signal": "Ctrl+C or SIGTERM",
            })),
        "KineticVM daemon started"
    );
}

async fn await_startup_readiness(
    mut readiness_rx: Option<tokio::sync::watch::Receiver<StartupReadiness>>,
    gateway_required: bool,
    socket_required: bool,
) -> Option<StartupReadiness> {
    match &mut readiness_rx {
        Some(readiness_rx) => readiness_rx
            .wait_for(|state| {
                (!gateway_required || state.gateway_addr.is_some())
                    && (!socket_required || state.socket_ready)
            })
            .await
            .ok()
            .map(|state| *state),
        None => Some(StartupReadiness::default()),
    }
}

async fn await_socket_startup(
    mut state_rx: tokio::sync::watch::Receiver<SocketStartupState>,
) -> Result<()> {
    match state_rx
        .wait_for(|state| !matches!(state, SocketStartupState::Pending))
        .await
        .map(|state| state.clone())
    {
        Ok(SocketStartupState::Ready) => Ok(()),
        Ok(SocketStartupState::Fatal { kind, message }) => {
            Err(std::io::Error::new(kind, message).into())
        }
        Ok(SocketStartupState::Pending) => Err(std::io::Error::other(
            "socket startup remained pending after readiness wait",
        )
        .into()),
        Err(_) => Ok(()),
    }
}

/// Return whether stderr is an interactive terminal currently owned by this
/// process group. Job-control ownership can change while the daemon is running.
#[cfg(unix)]
pub fn stderr_is_interactive_foreground() -> bool {
    if !std::io::IsTerminal::is_terminal(&std::io::stderr()) {
        return false;
    }
    let (foreground_pgrp, own_pgrp) = {
        // SAFETY: these libc calls take no pointers. `tcgetpgrp` returns -1
        // when stderr has no controlling terminal; `getpgrp` cannot fail.
        unsafe { (libc::tcgetpgrp(libc::STDERR_FILENO), libc::getpgrp()) }
    };
    stderr_foreground_decision(foreground_pgrp, own_pgrp)
}

#[cfg(not(unix))]
pub fn stderr_is_interactive_foreground() -> bool {
    std::io::IsTerminal::is_terminal(&std::io::stderr())
}

#[cfg(unix)]
fn stderr_foreground_decision(foreground_pgrp: i32, own_pgrp: i32) -> bool {
    foreground_pgrp > 0 && foreground_pgrp == own_pgrp
}

/// Write the localized foreground startup banner before daemon initialization.
pub fn echo_daemon_starting_to_terminal<W: std::io::Write>(mut out: W) -> std::io::Result<()> {
    use crate::i18n::get_required_cli_string as cli_t;

    writeln!(out, "{}", cli_t("cli-daemon-starting-title"))?;
    writeln!(out, "   {}", cli_t("cli-daemon-starting-detail"))?;
    writeln!(out, "   {}", cli_t("cli-daemon-started-stop"))
}

fn echo_daemon_ready_to_terminal<W: std::io::Write>(
    config: &Config,
    readiness: StartupReadiness,
    mut out: W,
) -> std::io::Result<()> {
    use crate::i18n::{
        get_required_cli_string as cli_t, get_required_cli_string_with_args as cli_ta,
    };

    writeln!(out, "{}", cli_t("cli-daemon-started-title"))?;
    if let Some(addr) = readiness.gateway_addr {
        let scheme = if config.gateway.tls.as_ref().is_some_and(|tls| tls.enabled) {
            "https"
        } else {
            "http"
        };
        let prefix = config.gateway.path_prefix.as_deref().unwrap_or("");
        let gateway_url = format!("{scheme}://{addr}{prefix}");
        writeln!(
            out,
            "   {}",
            cli_ta("cli-daemon-started-gateway", &[("url", &gateway_url)])
        )?;
    }
    if readiness.socket_ready {
        let socket_path = crate::rpc::local::socket_path(config).display().to_string();
        writeln!(
            out,
            "   {}",
            cli_ta("cli-daemon-started-socket", &[("path", &socket_path)])
        )?;
    }
    if readiness.gateway_addr.is_some() && config.gateway.require_pairing {
        writeln!(out, "   {}", cli_t("cli-daemon-started-pairing"))?;
    }
    writeln!(out, "   {}", cli_t("cli-daemon-started-stop"))
}

fn spawn_state_writer(config: Config) -> JoinHandle<()> {
    kinetic_spawn::spawn!(async move {
        let path = state_file_path(&config);
        if let Some(parent) = path.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }

        let mut interval = tokio::time::interval(Duration::from_secs(STATUS_FLUSH_SECONDS));
        loop {
            interval.tick().await;
            let mut json = crate::health::snapshot_json();
            if let Some(obj) = json.as_object_mut() {
                obj.insert(
                    "written_at".into(),
                    serde_json::json!(Utc::now().to_rfc3339()),
                );
            }
            let data = serde_json::to_vec_pretty(&json).unwrap_or_else(|_| b"{}".to_vec());
            let _ = tokio::fs::write(&path, data).await;
        }
    })
}

/// Whether the RPC connections accepted by the retiring generation finished
/// draining within the shutdown budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RpcDrain {
    /// Every accepted connection, and every task it started, has ended.
    Complete,
    /// The budget expired with this many connections still unwinding.
    Outstanding(usize),
}

/// Wait for the connections the RPC listeners accepted to finish draining.
///
/// `count` is decremented when the last task started by a connection has
/// ended: the connection task, each prompt it spawned, and the nested turn task
/// each hold a clone of the connection's liveness token. A count of zero is
/// therefore the daemon-visible proof that no old-generation work is still
/// running, not merely that the connection task was aborted. The wait is
/// bounded just past the listeners' own forced-abort deadline
/// (`rpc::CONNECTION_DRAIN_GRACE`), the point at which a connection that
/// ignored cancellation is aborted. Returns immediately when no connection was
/// accepted, which is also the case when no RPC listener is running at all.
pub(crate) async fn await_rpc_connection_drain(count: &std::sync::atomic::AtomicUsize) -> RpcDrain {
    use std::sync::atomic::Ordering;

    const POLL_INTERVAL: Duration = Duration::from_millis(25);
    let deadline = tokio::time::Instant::now()
        + crate::rpc::CONNECTION_DRAIN_GRACE.saturating_add(Duration::from_millis(500));

    loop {
        let outstanding = count.load(Ordering::Relaxed);
        if outstanding == 0 {
            return RpcDrain::Complete;
        }
        if tokio::time::Instant::now() >= deadline {
            ::kinetic_log::record!(
                WARN,
                ::kinetic_log::Event::new(module_path!(), ::kinetic_log::Action::Note)
                    .with_outcome(::kinetic_log::EventOutcome::Unknown)
                    .with_attrs(::serde_json::json!({ "connections": outstanding })),
                "RPC connections still draining when the shutdown budget expired"
            );
            return RpcDrain::Outstanding(outstanding);
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Decide what a reload may do once the drain has been attempted.
///
/// A reload keeps the process alive and starts a replacement generation over
/// the same durable sessions, so it is admissible only when the retiring
/// generation is proven finished. When the drain budget expires with work
/// still unwinding, the reload is refused and downgraded to a shutdown: the
/// supervisor restarts a fresh process, which cannot overlap with work this
/// one never managed to retire. Shutdown exits and failures are returned
/// unchanged.
fn settle_exit_against_drain(exit: Result<DaemonExit>, drain: RpcDrain) -> Result<DaemonExit> {
    let RpcDrain::Outstanding(outstanding) = drain else {
        return exit;
    };
    if !matches!(exit, Ok(DaemonExit::Reload)) {
        return exit;
    }
    ::kinetic_log::record!(
        WARN,
        ::kinetic_log::Event::new(module_path!(), ::kinetic_log::Action::Note)
            .with_outcome(::kinetic_log::EventOutcome::Unknown)
            .with_attrs(::serde_json::json!({ "connections": outstanding })),
        "Reload refused: RPC work from the retiring generation is still unwinding; shutting down \
         instead so a replacement generation cannot overlap it"
    );
    Ok(DaemonExit::Shutdown)
}

const CHANNEL_SUPERVISOR_SHUTDOWN_GRACE: Duration = Duration::from_secs(6);

async fn retire_channels_supervisor(handle: Option<JoinHandle<()>>) -> bool {
    let Some(mut handle) = handle else {
        return true;
    };
    tokio::select! {
        biased;
        result = &mut handle => {
            if let Err(error) = result {
                ::kinetic_log::record!(
                    WARN,
                    ::kinetic_log::Event::new(module_path!(), ::kinetic_log::Action::Note)
                        .with_outcome(::kinetic_log::EventOutcome::Unknown)
                        .with_attrs(::serde_json::json!({ "error": error.to_string() })),
                    "Channel supervisor ended without establishing clean retirement"
                );
                false
            } else {
                true
            }
        }
        () = tokio::time::sleep(CHANNEL_SUPERVISOR_SHUTDOWN_GRACE) => {
            handle.abort();
            let _ = handle.await;
            ::kinetic_log::record!(
                WARN,
                ::kinetic_log::Event::new(module_path!(), ::kinetic_log::Action::Note)
                    .with_outcome(::kinetic_log::EventOutcome::Unknown),
                "Channel supervisor did not retire inside its shutdown allowance"
            );
            false
        }
    }
}

fn settle_exit_against_channel_retirement(
    exit: Result<DaemonExit>,
    channels_retired: bool,
) -> Result<DaemonExit> {
    if channels_retired || !matches!(exit, Ok(DaemonExit::Reload)) {
        return exit;
    }
    ::kinetic_log::record!(
        WARN,
        ::kinetic_log::Event::new(module_path!(), ::kinetic_log::Action::Note)
            .with_outcome(::kinetic_log::EventOutcome::Unknown),
        "Reload refused: channel work from the retiring generation is still unwinding; shutting \
         down instead so a replacement generation cannot overlap it"
    );
    Ok(DaemonExit::Shutdown)
}

fn spawn_component_supervisor<F, Fut>(
    name: &'static str,
    initial_backoff_secs: u64,
    max_backoff_secs: u64,
    cancel: tokio_util::sync::CancellationToken,
    mut run_component: F,
) -> JoinHandle<()>
where
    F: FnMut() -> Fut + Send + 'static,
    Fut: Future<Output = Result<()>> + Send + 'static,
{
    kinetic_spawn::spawn!(async move {
        let mut backoff = initial_backoff_secs.max(1);
        let max_backoff = max_backoff_secs.max(backoff);

        let stable_run = Duration::from_secs(initial_backoff_secs.max(1).saturating_mul(5));

        loop {
            crate::health::mark_component_ok(name);
            let run_started = std::time::Instant::now();
            let outcome = run_component().await;
            let ran_for = run_started.elapsed();
            match outcome {
                Ok(()) => {
                    if cancel.is_cancelled() {
                        crate::health::mark_component_ok(name);
                        #[cfg(test)]
                        if name == "scheduler" {
                            SCHEDULER_CLEAN_SHUTDOWN_OBSERVED
                                .store(true, std::sync::atomic::Ordering::SeqCst);
                        }
                        ::kinetic_log::record!(
                            INFO,
                            ::kinetic_log::Event::new(module_path!(), ::kinetic_log::Action::Note)
                                .with_outcome(::kinetic_log::EventOutcome::Success)
                                .with_attrs(::serde_json::json!({"name": name})),
                            &format!(
                                "Daemon component '{name}' shut down cleanly via cancellation token"
                            )
                        );
                        return;
                    }
                    crate::health::mark_component_error(name, "component exited unexpectedly");
                    ::kinetic_log::record!(
                        WARN,
                        ::kinetic_log::Event::new(module_path!(), ::kinetic_log::Action::Note)
                            .with_outcome(::kinetic_log::EventOutcome::Unknown)
                            .with_attrs(::serde_json::json!({
                                "name": name,
                                "ran_for_secs": ran_for.as_secs(),
                            })),
                        &format!("Daemon component '{name}' exited unexpectedly")
                    );
                    if ran_for >= stable_run {
                        backoff = initial_backoff_secs.max(1);
                    }
                }
                Err(e) => {
                    let error_chain = format!("{e:#}");
                    crate::health::mark_component_error(name, &error_chain);
                    ::kinetic_log::record!(
                        ERROR,
                        ::kinetic_log::Event::new(module_path!(), ::kinetic_log::Action::Fail)
                            .with_outcome(::kinetic_log::EventOutcome::Failure)
                            .with_attrs(::serde_json::json!({
                                "error": &error_chain,
                                "name": name,
                                "ran_for_secs": ran_for.as_secs(),
                            })),
                        &format!("Daemon component '{name}' failed: {error_chain}")
                    );
                    // A long-lived run that eventually errors is not a
                    // fast-fail loop; let it reset so a component that ran fine
                    // for hours and then hit a transient error retries quickly
                    // rather than inheriting a huge stale backoff.
                    if ran_for >= stable_run {
                        backoff = initial_backoff_secs.max(1);
                    }
                }
            }

            crate::health::bump_component_restart(name);
            crate::util::release_freed_heap();
            // The backoff sleep must yield to cancellation: a daemon shutting
            // down or reloading while a component is in its retry window would
            // otherwise wait out the whole window before the supervisor exits.
            tokio::select! {
                () = cancel.cancelled() => {
                    crate::health::mark_component_ok(name);
                    ::kinetic_log::record!(
                        INFO,
                        ::kinetic_log::Event::new(module_path!(), ::kinetic_log::Action::Note)
                            .with_outcome(::kinetic_log::EventOutcome::Success)
                            .with_attrs(::serde_json::json!({
                                "name": name,
                                "backoff_secs": backoff,
                            })),
                        &format!(
                            "Daemon component '{name}' cancelled during restart backoff; \
                             supervisor exiting"
                        )
                    );
                    return;
                }
                () = tokio::time::sleep(Duration::from_secs(backoff)) => {}
            }
            // Double backoff AFTER sleeping so first error uses initial_backoff
            backoff = backoff.saturating_mul(2).min(max_backoff);
        }
    })
}

fn resolve_heartbeat_workspace_dir(config: &Config) -> Result<(String, PathBuf)> {
    let agent_alias = config.heartbeat.agent.trim().to_string();
    if agent_alias.is_empty() {
        anyhow::bail!(
            "heartbeat worker requires `[heartbeat] agent = \"<alias>\"` naming a configured agent"
        );
    }
    if config.agent(&agent_alias).is_none() {
        anyhow::bail!(
            "[heartbeat] agent = {agent_alias:?} is not configured ([agents.{agent_alias}] missing)"
        );
    }
    let workspace_dir = config.agent_workspace_dir(&agent_alias);
    Ok((agent_alias, workspace_dir))
}

async fn run_heartbeat_worker(
    config: Config,
    execution_capability: crate::live_config_authority::AgentExecutionCapability,
) -> Result<()> {
    use crate::heartbeat::engine::{
        HeartbeatEngine, HeartbeatTask, TaskPriority, TaskStatus, compute_adaptive_interval,
    };
    use std::sync::Arc;

    let (agent_alias, heartbeat_workspace_dir) = resolve_heartbeat_workspace_dir(&config)?;

    let observer: std::sync::Arc<dyn crate::observability::Observer> =
        std::sync::Arc::from(crate::observability::create_observer(&config.observability));
    let engine = HeartbeatEngine::new(config.heartbeat.clone(), heartbeat_workspace_dir, observer);
    let metrics = engine.metrics();
    let delivery = resolve_heartbeat_delivery(&config)?;
    let two_phase = config.heartbeat.two_phase;
    let adaptive = config.heartbeat.adaptive;
    let start_time = std::time::Instant::now();

    // ── Deadman watcher ──────────────────────────────────────────
    let deadman_timeout = config.heartbeat.deadman_timeout_minutes;
    if deadman_timeout > 0 {
        let dm_metrics = Arc::clone(&metrics);
        let dm_config = config.clone();
        let dm_delivery = delivery.clone();
        kinetic_spawn::spawn!(async move {
            let check_interval = Duration::from_secs(60);
            let timeout = chrono::Duration::minutes(i64::from(deadman_timeout));
            loop {
                tokio::time::sleep(check_interval).await;
                let last_tick = dm_metrics.lock().last_tick_at;
                if let Some(last) = last_tick
                    && chrono::Utc::now() - last > timeout
                {
                    let alert = format!(
                        "⚠️ Heartbeat dead-man's switch: no tick in {deadman_timeout} minutes"
                    );
                    let (channel, target) = if let Some(ch) = &dm_config.heartbeat.deadman_channel {
                        let to = dm_config
                            .heartbeat
                            .deadman_to
                            .as_deref()
                            .or(dm_config.heartbeat.to.as_deref())
                            .unwrap_or_default();
                        (ch.clone(), to.to_string())
                    } else if let Some((ch, to)) = &dm_delivery {
                        (ch.clone(), to.clone())
                    } else {
                        continue;
                    };
                    let delivery_fut = crate::cron::scheduler::deliver_announcement(
                        &dm_config, &channel, &target, None, &alert,
                    );
                    match tokio::time::timeout(Duration::from_secs(30), delivery_fut).await {
                        Ok(Err(e)) => {
                            ::kinetic_log::record!(
                                WARN,
                                ::kinetic_log::Event::new(
                                    module_path!(),
                                    ::kinetic_log::Action::Note
                                )
                                .with_outcome(::kinetic_log::EventOutcome::Unknown)
                                .with_attrs(::serde_json::json!({"error": format!("{}", e)})),
                                "Deadman alert delivery failed"
                            );
                        }
                        Err(_) => {
                            ::kinetic_log::record!(
                                WARN,
                                ::kinetic_log::Event::new(
                                    module_path!(),
                                    ::kinetic_log::Action::Note
                                )
                                .with_outcome(::kinetic_log::EventOutcome::Unknown),
                                "Deadman alert delivery timed out (30s)"
                            );
                        }
                        Ok(Ok(())) => {}
                    }
                }
            }
        });
    }

    let base_interval = config.heartbeat.interval_minutes.max(1);
    let mut sleep_mins = base_interval;

    loop {
        tokio::time::sleep(Duration::from_secs(u64::from(sleep_mins) * 60)).await;

        // Update uptime
        {
            let mut m = metrics.lock();
            m.uptime_secs = start_time.elapsed().as_secs();
        }

        let tick_start = std::time::Instant::now();

        // Collect runnable tasks (active only, sorted by priority)
        let mut tasks = engine.collect_runnable_tasks().await?;
        let has_high_priority = tasks.iter().any(|t| t.priority == TaskPriority::High);

        if tasks.is_empty() {
            if let Some(fallback) = config
                .heartbeat
                .message
                .as_deref()
                .map(str::trim)
                .filter(|m| !m.is_empty())
            {
                tasks.push(HeartbeatTask {
                    text: fallback.to_string(),
                    priority: TaskPriority::Medium,
                    status: TaskStatus::Active,
                });
            } else {
                #[allow(clippy::cast_precision_loss)]
                let elapsed = tick_start.elapsed().as_millis() as f64;
                metrics.lock().record_success(elapsed);
                continue;
            }
        }

        // ── Phase 1: LLM decision (two-phase mode) ──────────────
        let tasks_to_run = if two_phase {
            let decision_prompt = format!(
                "[Heartbeat Task | decision] {}",
                HeartbeatEngine::build_decision_prompt(&tasks),
            );
            let phase1_fut = Box::pin(crate::agent::run(
                config.clone(),
                &agent_alias,
                Some(decision_prompt),
                None,
                None,
                Some(0.0),
                vec![],
                false,
                None,
                None,
                kinetic_api::ingress::TurnOrigin::Daemon,
                crate::agent::loop_::AgentRunOverrides {
                    execution_capability: Some(execution_capability.clone()),
                    internal_principal: Some(kinetic_api::ingress::InternalPrincipal::Daemon {
                        task: "heartbeat:decision".to_string(),
                    }),
                    ..crate::agent::loop_::AgentRunOverrides::default()
                },
            ));
            let phase1_result = if config.heartbeat.task_timeout_secs > 0 {
                match tokio::time::timeout(
                    Duration::from_secs(config.heartbeat.task_timeout_secs),
                    phase1_fut,
                )
                .await
                {
                    Ok(r) => r,
                    Err(_) => {
                        ::kinetic_log::record!(
                            WARN,
                            ::kinetic_log::Event::new(
                                module_path!(),
                                ::kinetic_log::Action::Timeout
                            )
                            .with_outcome(::kinetic_log::EventOutcome::Failure)
                            .with_attrs(::serde_json::json!({
                                "phase": "phase1_decision",
                                "timeout_secs": config.heartbeat.task_timeout_secs,
                            })),
                            "heartbeat: phase1 decision timed out"
                        );
                        Err(anyhow::Error::msg(format!(
                            "Phase 1 decision timed out ({}s)",
                            config.heartbeat.task_timeout_secs
                        )))
                    }
                }
            } else {
                phase1_fut.await
            };
            match phase1_result {
                Ok(response) => {
                    let indices = HeartbeatEngine::parse_decision_response(&response, tasks.len());
                    if indices.is_empty() {
                        ::kinetic_log::record!(
                            INFO,
                            ::kinetic_log::Event::new(module_path!(), ::kinetic_log::Action::Note),
                            "heartbeat phase 1: skip (nothing to do)"
                        );
                        crate::health::mark_component_ok("heartbeat");
                        #[allow(clippy::cast_precision_loss)]
                        let elapsed = tick_start.elapsed().as_millis() as f64;
                        metrics.lock().record_success(elapsed);
                        continue;
                    }
                    ::kinetic_log::record!(INFO, ::kinetic_log::Event::new(module_path!(), ::kinetic_log::Action::Note).with_attrs(::serde_json::json!({"selected": indices.len(), "total": tasks.len()})), "heartbeat phase 1: running task subset");
                    indices
                        .into_iter()
                        .filter_map(|i| tasks.get(i).cloned())
                        .collect()
                }
                Err(e) => {
                    ::kinetic_log::record!(
                        WARN,
                        ::kinetic_log::Event::new(module_path!(), ::kinetic_log::Action::Note)
                            .with_outcome(::kinetic_log::EventOutcome::Unknown)
                            .with_attrs(::serde_json::json!({"error": format!("{}", e)})),
                        "heartbeat phase 1 failed; running all tasks"
                    );
                    tasks
                }
            }
        } else {
            tasks
        };

        // ── Phase 2: Execute selected tasks ─────────────────────
        // Re-read session context on every tick so we pick up messages
        // that arrived since the daemon started.
        let session_context = if config.heartbeat.load_session_context {
            load_heartbeat_session_context(&config)
        } else {
            None
        };

        let heartbeat_memory: Option<Box<dyn kinetic_memory::Memory>> =
            kinetic_memory::create_memory_from_config(
                &config,
                config
                    .model_provider_for_agent(&agent_alias)
                    .and_then(|e| e.api_key.as_deref()),
            )
            .ok();

        let mut tick_had_error = false;
        for task in &tasks_to_run {
            let task_start = std::time::Instant::now();
            let task_prompt = format!("[Heartbeat Task | {}] {}", task.priority, task.text);

            // Memory context is injected once in the engine, keyed on the
            // Daemon origin (agent::memory_inject): Conversation entries are
            // excluded for scheduled origins. `heartbeat_memory` stays for
            // the post-run auto-save consolidation below.
            let prompt = match &session_context {
                Some(sc) => format!("{sc}\n\n{task_prompt}"),
                None => task_prompt,
            };
            let temp: Option<f64> = config
                .model_provider_for_agent(&agent_alias)
                .and_then(|e| e.temperature);
            let phase2_fut = Box::pin(crate::agent::run(
                config.clone(),
                &agent_alias,
                Some(prompt),
                None,
                None,
                temp,
                vec![],
                false,
                None,
                None,
                kinetic_api::ingress::TurnOrigin::Daemon,
                crate::agent::loop_::AgentRunOverrides {
                    execution_capability: Some(execution_capability.clone()),
                    // Heartbeat tasks have no runtime-owned id (their text is
                    // model-maintained content, which never enters the
                    // principal), so the stamp names the pipeline phase.
                    internal_principal: Some(kinetic_api::ingress::InternalPrincipal::Daemon {
                        task: "heartbeat:execute".to_string(),
                    }),
                    ..crate::agent::loop_::AgentRunOverrides::default()
                },
            ));
            let phase2_result = if config.heartbeat.task_timeout_secs > 0 {
                match tokio::time::timeout(
                    Duration::from_secs(config.heartbeat.task_timeout_secs),
                    phase2_fut,
                )
                .await
                {
                    Ok(r) => r,
                    Err(_) => {
                        ::kinetic_log::record!(
                            WARN,
                            ::kinetic_log::Event::new(
                                module_path!(),
                                ::kinetic_log::Action::Timeout
                            )
                            .with_outcome(::kinetic_log::EventOutcome::Failure)
                            .with_attrs(::serde_json::json!({
                                "phase": "phase2_heartbeat",
                                "timeout_secs": config.heartbeat.task_timeout_secs,
                            })),
                            "heartbeat task timed out"
                        );
                        Err(anyhow::Error::msg(format!(
                            "Heartbeat task timed out ({}s)",
                            config.heartbeat.task_timeout_secs
                        )))
                    }
                }
            } else {
                phase2_fut.await
            };
            match phase2_result {
                Ok(output) => {
                    crate::health::mark_component_ok("heartbeat");
                    #[allow(clippy::cast_possible_truncation)]
                    let duration_ms = task_start.elapsed().as_millis() as i64;
                    let now = chrono::Utc::now();
                    let _ = crate::heartbeat::store::record_run(
                        &config.data_dir,
                        &task.text,
                        &task.priority.to_string(),
                        now - chrono::Duration::milliseconds(duration_ms),
                        now,
                        "ok",
                        Some(output.as_str()),
                        duration_ms,
                        config.heartbeat.max_run_history,
                    );
                    // Consolidate heartbeat output to memory for cross-session awareness.
                    if config.memory.auto_save
                        && output.chars().count() >= 50
                        && let Some(ref mem) = heartbeat_memory
                    {
                        let key = format!("heartbeat_{}", uuid::Uuid::new_v4());
                        let summary = if output.len() > 500 {
                            // Find a valid UTF-8 char boundary at or before 500.
                            let mut end = 500;
                            while end > 0 && !output.is_char_boundary(end) {
                                end -= 1;
                            }
                            &output[..end]
                        } else {
                            &output
                        };
                        let _ = mem
                            .store(
                                &key,
                                &format!("Heartbeat task '{}': {}", task.text, summary),
                                kinetic_memory::MemoryCategory::Daily,
                                None,
                            )
                            .await;
                    }

                    let announcement = if output.trim().is_empty() {
                        format!("💓 heartbeat task completed: {}", task.text)
                    } else {
                        output
                    };
                    let suppress_delivery =
                        !crate::cron::scheduler::announce_delivery_decision(&announcement)
                            .should_deliver();
                    if suppress_delivery {
                        ::kinetic_log::record!(
                            DEBUG,
                            ::kinetic_log::Event::new(module_path!(), ::kinetic_log::Action::Note)
                                .with_outcome(::kinetic_log::EventOutcome::Success)
                                .with_attrs(::serde_json::json!({"task": task.text})),
                            "Heartbeat task returned NO_REPLY sentinel — skipping delivery"
                        );
                    }
                    if let Some((channel, target)) = &delivery
                        && !suppress_delivery
                    {
                        let delivery_result = tokio::time::timeout(
                            Duration::from_secs(30),
                            crate::cron::scheduler::deliver_announcement(
                                &config,
                                channel,
                                target,
                                None,
                                &announcement,
                            ),
                        )
                        .await;
                        match delivery_result {
                            Ok(Err(e)) => {
                                crate::health::mark_component_error(
                                    "heartbeat",
                                    format!("delivery failed: {e}"),
                                );
                                ::kinetic_log::record!(
                                    WARN,
                                    ::kinetic_log::Event::new(
                                        module_path!(),
                                        ::kinetic_log::Action::Note
                                    )
                                    .with_outcome(::kinetic_log::EventOutcome::Unknown)
                                    .with_attrs(::serde_json::json!({"error": format!("{}", e)})),
                                    "Heartbeat delivery failed"
                                );
                            }
                            Err(_) => {
                                crate::health::mark_component_error(
                                    "heartbeat",
                                    "delivery timed out (30s)".to_string(),
                                );
                                ::kinetic_log::record!(
                                    WARN,
                                    ::kinetic_log::Event::new(
                                        module_path!(),
                                        ::kinetic_log::Action::Note
                                    )
                                    .with_outcome(::kinetic_log::EventOutcome::Unknown),
                                    "Heartbeat delivery timed out (30s)"
                                );
                            }
                            Ok(Ok(())) => {}
                        }
                    }
                }
                Err(e) => {
                    tick_had_error = true;
                    #[allow(clippy::cast_possible_truncation)]
                    let duration_ms = task_start.elapsed().as_millis() as i64;
                    let now = chrono::Utc::now();
                    let _ = crate::heartbeat::store::record_run(
                        &config.data_dir,
                        &task.text,
                        &task.priority.to_string(),
                        now - chrono::Duration::milliseconds(duration_ms),
                        now,
                        "error",
                        Some(&e.to_string()),
                        duration_ms,
                        config.heartbeat.max_run_history,
                    );
                    crate::health::mark_component_error("heartbeat", e.to_string());
                    ::kinetic_log::record!(
                        WARN,
                        ::kinetic_log::Event::new(module_path!(), ::kinetic_log::Action::Note)
                            .with_outcome(::kinetic_log::EventOutcome::Unknown)
                            .with_attrs(::serde_json::json!({"error": format!("{}", e)})),
                        "Heartbeat task failed"
                    );
                }
            }
        }

        // Update metrics
        #[allow(clippy::cast_precision_loss)]
        let tick_elapsed = tick_start.elapsed().as_millis() as f64;
        {
            let mut m = metrics.lock();
            if tick_had_error {
                m.record_failure(tick_elapsed);
            } else {
                m.record_success(tick_elapsed);
            }
        }

        // Compute next sleep interval
        if adaptive {
            let failures = metrics.lock().consecutive_failures;
            sleep_mins = compute_adaptive_interval(
                base_interval,
                config.heartbeat.min_interval_minutes,
                config.heartbeat.max_interval_minutes,
                failures,
                has_high_priority,
            );
        } else {
            sleep_mins = base_interval;
        }
    }
}

/// Resolve delivery target: explicit config > auto-detect first configured channel.
fn resolve_heartbeat_delivery(config: &Config) -> Result<Option<(String, String)>> {
    let channel = config
        .heartbeat
        .target
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let target = config
        .heartbeat
        .to
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());

    match (channel, target) {
        // Both explicitly set — validate and use.
        (Some(channel), Some(target)) => {
            validate_heartbeat_channel_config(config, channel)?;
            Ok(Some((channel.to_string(), target.to_string())))
        }
        // Only one set — error.
        (Some(_), None) => anyhow::bail!("heartbeat.to is required when heartbeat.target is set"),
        (None, Some(_)) => anyhow::bail!("heartbeat.target is required when heartbeat.to is set"),
        // Neither set — try auto-detect the first configured channel.
        (None, None) => Ok(auto_detect_heartbeat_channel(config)),
    }
}

const HEARTBEAT_SESSION_CONTEXT_MESSAGES: usize = 20;

fn load_heartbeat_session_context(config: &Config) -> Option<String> {
    use kinetic_providers::traits::ChatMessage;

    let channel = config
        .heartbeat
        .target
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())?;
    let to = config
        .heartbeat
        .to
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())?;

    if channel.contains('/') || channel.contains('\\') || to.contains('/') || to.contains('\\') {
        ::kinetic_log::record!(
            WARN,
            ::kinetic_log::Event::new(module_path!(), ::kinetic_log::Action::Note)
                .with_outcome(::kinetic_log::EventOutcome::Unknown),
            "heartbeat session context: channel/to contains path separators, skipping"
        );
        return None;
    }

    let sessions_dir = config.data_dir.join("sessions");

    // Find the most recently modified JSONL file that belongs to this target.
    // Matches both `{channel}_{to}.jsonl` and `{channel}_{anything}_{to}.jsonl`.
    let prefix = format!("{channel}_");
    let suffix = format!("_{to}.jsonl");
    let exact = format!("{channel}_{to}.jsonl");
    let mid_prefix = format!("{channel}_{to}_");

    let path = std::fs::read_dir(&sessions_dir)
        .ok()?
        .filter_map(|e| e.ok())
        .filter(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            name.ends_with(".jsonl")
                && (name == exact
                    || (name.starts_with(&prefix) && name.ends_with(&suffix))
                    || name.starts_with(&mid_prefix))
        })
        .max_by_key(|e| {
            e.metadata()
                .and_then(|m| m.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
        })
        .map(|e| e.path())?;

    if !path.exists() {
        ::kinetic_log::record!(
            DEBUG,
            ::kinetic_log::Event::new(module_path!(), ::kinetic_log::Action::Note)
                .with_attrs(::serde_json::json!({"channel": channel, "to": to})),
            "heartbeat session context: no session file found"
        );
        return None;
    }

    let messages = load_jsonl_messages(&path);
    if messages.is_empty() {
        return None;
    }

    let recent: Vec<&ChatMessage> = messages
        .iter()
        .filter(|m| m.role == "user" || m.role == "assistant")
        .rev()
        .take(HEARTBEAT_SESSION_CONTEXT_MESSAGES)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();

    // Only inject context if there is at least one real user message in the
    // window. If the JSONL contains only assistant messages (e.g. previous
    // heartbeat outputs with no reply yet), skip context to avoid feeding
    // Monika's own messages back to her in a loop.
    let has_user_message = recent.iter().any(|m| m.role == "user");
    if !has_user_message {
        ::kinetic_log::record!(
            DEBUG,
            ::kinetic_log::Event::new(module_path!(), ::kinetic_log::Action::Note),
            "💓 Heartbeat session context: no user messages in recent history — skipping"
        );
        return None;
    }

    // Use the session file's mtime as a proxy for when the last message arrived.
    let last_message_age = std::fs::metadata(&path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|mtime| mtime.elapsed().ok());

    let silence_note = match last_message_age {
        Some(age) => {
            let mins = age.as_secs() / 60;
            if mins < 60 {
                format!("(last message ~{mins} minutes ago)\n")
            } else {
                let hours = mins / 60;
                let rem = mins % 60;
                if rem == 0 {
                    format!("(last message ~{hours}h ago)\n")
                } else {
                    format!("(last message ~{hours}h {rem}m ago)\n")
                }
            }
        }
        None => String::new(),
    };

    ::kinetic_log::record!(
        DEBUG,
        ::kinetic_log::Event::new(module_path!(), ::kinetic_log::Action::Note),
        &format!(
            "💓 Heartbeat session context: {} messages from {}, silence: {}",
            recent.len(),
            path.display().to_string(),
            silence_note.trim()
        )
    );

    let mut ctx = format!(
        "[Recent conversation history — use this for context when composing your message] {silence_note}",
    );
    for msg in &recent {
        let label = if msg.role == "user" { "User" } else { "You" };
        // Truncate very long messages to avoid bloating the prompt.
        // Use char_indices to avoid panicking on multi-byte UTF-8 characters.
        let content = if msg.content.len() > 500 {
            let truncate_at = msg
                .content
                .char_indices()
                .map(|(i, _)| i)
                .take_while(|&i| i <= 500)
                .last()
                .unwrap_or(0);
            format!("{}…", &msg.content[..truncate_at])
        } else {
            msg.content.clone()
        };
        ctx.push_str(label);
        ctx.push_str(": ");
        ctx.push_str(&content);
        ctx.push('\n');
    }

    Some(ctx)
}

/// Read the last `HEARTBEAT_SESSION_CONTEXT_MESSAGES` `ChatMessage` lines from
/// a JSONL session file using a bounded rolling window so we never hold the
/// entire file in memory.
fn load_jsonl_messages(path: &std::path::Path) -> Vec<kinetic_providers::traits::ChatMessage> {
    use std::collections::VecDeque;
    use std::io::BufRead;

    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return Vec::new(),
    };
    let reader = std::io::BufReader::new(file);
    let mut window: VecDeque<kinetic_providers::traits::ChatMessage> =
        VecDeque::with_capacity(HEARTBEAT_SESSION_CONTEXT_MESSAGES + 1);
    for line in reader.lines() {
        let Ok(line) = line else { continue };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Ok(msg) = serde_json::from_str::<kinetic_providers::traits::ChatMessage>(trimmed) {
            window.push_back(msg);
            if window.len() > HEARTBEAT_SESSION_CONTEXT_MESSAGES {
                window.pop_front();
            }
        }
    }
    window.into_iter().collect()
}

/// Auto-detect the best channel for heartbeat delivery by checking which
/// channels are configured. Returns the first match in priority order.
fn auto_detect_heartbeat_channel(config: &Config) -> Option<(String, String)> {
    // Priority order: telegram > discord > slack > mattermost
    // Find the first external peer authorized on a telegram channel
    // (peer authorization lives in peer_groups in V3, not on the
    // channel block).
    if !config.channels.telegram.is_empty() {
        for alias in config.channels.telegram.keys() {
            let peers = config.channel_addressable_peers("telegram", alias);
            if let Some(target) = peers.into_iter().next() {
                return Some(("telegram".to_string(), target));
            }
        }
    }
    if !config.channels.discord.is_empty() {
        // Discord requires explicit target — can't auto-detect
        return None;
    }
    if !config.channels.slack.is_empty() {
        // Slack requires explicit target
        return None;
    }
    if !config.channels.mattermost.is_empty() {
        // Mattermost requires explicit target
        return None;
    }
    None
}

fn validate_heartbeat_channel_config(config: &Config, channel: &str) -> Result<()> {
    // A heartbeat target may be a bare channel type ("telegram") or a
    // configured instance's composite key ("telegram.roy"). The channel
    // registry (is_known_channel / is_channel_configured / is_channel_deliverable)
    // is keyed by channel *type*, so validate the type segment. The delivery
    // path (deliver_announcement) resolves the instance alias at send time,
    // matching how cron delivery accepts `<type>.<alias>` refs — see
    // cron_delivery_channel_pattern.
    let channel_type = channel.split_once('.').map_or(channel, |(ty, _)| ty);
    if !config.channels.is_known_channel(channel_type) {
        anyhow::bail!("unsupported heartbeat.target channel: {channel}");
    }
    if !config.channels.is_channel_configured(channel_type) {
        anyhow::bail!(
            "heartbeat.target is set to {channel} but channels.{channel_type} is not configured"
        );
    }
    if !config.channels.is_channel_deliverable(channel_type) {
        anyhow::bail!(
            "heartbeat.target is set to {channel} but {channel_type} is an input-only channel that cannot deliver outbound messages"
        );
    }
    Ok(())
}

fn has_supervised_channels(config: &Config) -> bool {
    config.channels.has_any_enabled()
}

// run_mqtt_sop_listener has been moved to kinetic-channels::orchestrator::mqtt.
// The daemon now receives it as a starter via DaemonRegistry::register_mqtt.

#[cfg(test)]
mod tests {
    use super::*;
    use kinetic_config::schema::MattermostListenMode;
    use tempfile::TempDir;

    const DAEMON_DEADLOCK_GUARD: Duration = Duration::from_secs(30);

    #[tokio::test]
    async fn retiring_channel_generation_clears_admission_and_joins_active_attempt() {
        let cleared = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let clearer_state = std::sync::Arc::clone(&cleared);
        let control = ChannelGenerationControl::new(Some(std::sync::Arc::new(move || {
            clearer_state.store(true, std::sync::atomic::Ordering::Release);
        })));
        let daemon_cancel = tokio_util::sync::CancellationToken::new();
        let attempt = control
            .begin_attempt(&daemon_cancel)
            .expect("active generation should admit its channel attempt");
        let attempt_cancel = attempt.cancel();
        let active = kinetic_spawn::spawn!(async move {
            attempt_cancel.cancelled().await;
            drop(attempt);
        });

        let drain = control
            .prepare()
            .expect("registry clearer is available")
            .begin();
        assert!(control.is_retired());
        assert!(cleared.load(std::sync::atomic::Ordering::Acquire));
        tokio::time::timeout(std::time::Duration::from_secs(1), drain.wait())
            .await
            .expect("retired generation should join its active channel attempt");
        active.await.expect("channel attempt task should not panic");
        assert!(
            control.begin_attempt(&daemon_cancel).is_none(),
            "retired generation must not admit a replacement attempt"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn daemon_channels_shutdown_allows_cleanup_past_generic_grace() {
        let finished = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let finished_by_task = std::sync::Arc::clone(&finished);
        let handle = kinetic_spawn::spawn!(async move {
            tokio::time::sleep(Duration::from_millis(750)).await;
            finished_by_task.store(true, std::sync::atomic::Ordering::SeqCst);
        });

        assert!(
            retire_channels_supervisor(Some(handle)).await,
            "channel cleanup longer than the generic 500 ms grace must still retire cleanly"
        );
        assert!(
            finished.load(std::sync::atomic::Ordering::SeqCst),
            "the channel cleanup future must complete rather than be detached"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn daemon_channels_shutdown_refuses_reload_when_retirement_times_out() {
        struct RetirementProbe(std::sync::Arc<std::sync::atomic::AtomicBool>);
        impl Drop for RetirementProbe {
            fn drop(&mut self) {
                self.0.store(true, std::sync::atomic::Ordering::SeqCst);
            }
        }

        let dropped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let probe = RetirementProbe(std::sync::Arc::clone(&dropped));
        let handle = kinetic_spawn::spawn!(async move {
            let _probe = probe;
            std::future::pending::<()>().await;
        });

        let retired = retire_channels_supervisor(Some(handle)).await;
        assert!(!retired, "a timed-out channel generation is not retired");
        assert!(
            dropped.load(std::sync::atomic::Ordering::SeqCst),
            "the timed-out supervisor must be aborted and joined"
        );
        assert_eq!(
            settle_exit_against_channel_retirement(Ok(DaemonExit::Reload), retired).unwrap(),
            DaemonExit::Shutdown,
            "reload must be refused when channel retirement is unproven"
        );
    }

    fn test_config(tmp: &TempDir) -> Config {
        let config = Config {
            data_dir: tmp.path().join("data"),
            config_path: tmp.path().join("config.toml"),
            ..Config::default()
        };
        std::fs::create_dir_all(&config.data_dir).unwrap();
        config
    }

    fn add_agent_with_workspace(config: &mut Config, agent_alias: &str, workspace_dir: PathBuf) {
        let agent = kinetic_config::schema::AliasedAgentConfig {
            workspace: kinetic_config::multi_agent::AgentWorkspaceConfig {
                path: Some(workspace_dir),
                ..Default::default()
            },
            ..Default::default()
        };
        config.agents.insert(agent_alias.to_string(), agent);
    }

    /// Hold both process-global broadcast hooks still for a daemon lifecycle
    /// test.
    ///
    /// `run` calls `kinetic_log::set_broadcast_hook`, replacing the sender
    /// every log-assertion test subscribed to, and it installs the event bus
    /// as the observer broadcast hook, which would take observer events meant
    /// for a hook-capturing test. Those tests serialize on these two locks; a
    /// lifecycle test that calls `run` without them makes theirs miss events.
    ///
    /// Async because the observer lock is a `tokio` mutex. The name differs
    /// from the old synchronous `hold_log_broadcast` on purpose: a call written
    /// against that helper would otherwise compile to an un-awaited future
    /// that takes no lock at all.
    #[must_use]
    async fn hold_broadcast_hooks() -> (impl Drop, impl Drop) {
        let observer_hook = crate::observability::HOOK_TEST_LOCK.lock().await;
        (kinetic_log::__private_test_hook_lock(), observer_hook)
    }

    async fn recv_log_event(
        rx: &mut tokio::sync::broadcast::Receiver<serde_json::Value>,
        message: &str,
    ) -> serde_json::Value {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while std::time::Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            let step = remaining.min(std::time::Duration::from_millis(50));
            match tokio::time::timeout(step, rx.recv()).await {
                Ok(Ok(value))
                    if value
                        .get("message")
                        .and_then(|v| v.as_str())
                        .is_some_and(|candidate| candidate == message) =>
                {
                    return value;
                }
                Ok(Ok(_)) | Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => {}
                // A closed channel means the global broadcast hook was replaced
                // or cleared, not that the record was slow; keep that distinct
                // from a deadline miss so the failure names the real cause.
                Ok(Err(tokio::sync::broadcast::error::RecvError::Closed)) => {
                    panic!("log broadcast closed before event arrived: {message}");
                }
                Err(_elapsed) => {}
            }
        }
        panic!("did not find log event: {message}");
    }

    #[test]
    fn state_file_path_uses_config_state_directory() {
        let tmp = TempDir::new().unwrap();
        let config = test_config(&tmp);

        let path = state_file_path(&config);
        assert_eq!(path, tmp.path().join("state").join("daemon_state.json"));
    }

    #[tokio::test]
    async fn heartbeat_seed_uses_agent_workspace_not_data_dir() {
        let tmp = TempDir::new().unwrap();
        let mut config = test_config(&tmp);
        let agent_alias = "ops";
        let workspace_dir = tmp
            .path()
            .join("agents")
            .join(agent_alias)
            .join("workspace");
        std::fs::create_dir_all(&workspace_dir).unwrap();
        config.heartbeat.enabled = true;
        config.heartbeat.agent = agent_alias.to_string();
        add_agent_with_workspace(&mut config, agent_alias, workspace_dir.clone());

        let (_, resolved_workspace_dir) = resolve_heartbeat_workspace_dir(&config).unwrap();
        assert_eq!(resolved_workspace_dir, workspace_dir);
        assert_ne!(resolved_workspace_dir, config.data_dir);

        crate::heartbeat::engine::HeartbeatEngine::ensure_heartbeat_file(&resolved_workspace_dir)
            .await
            .unwrap();

        assert!(workspace_dir.join("HEARTBEAT.md").exists());
        assert!(!config.data_dir.join("HEARTBEAT.md").exists());
    }

    #[tokio::test]
    async fn heartbeat_engine_reads_agent_workspace_not_data_dir() {
        let tmp = TempDir::new().unwrap();
        let mut config = test_config(&tmp);
        let agent_alias = "ops";
        let workspace_dir = tmp
            .path()
            .join("agents")
            .join(agent_alias)
            .join("workspace");
        std::fs::create_dir_all(&workspace_dir).unwrap();
        config.heartbeat.enabled = true;
        config.heartbeat.agent = agent_alias.to_string();
        add_agent_with_workspace(&mut config, agent_alias, workspace_dir.clone());

        std::fs::write(config.data_dir.join("HEARTBEAT.md"), "- Data dir task").unwrap();
        std::fs::write(workspace_dir.join("HEARTBEAT.md"), "- Workspace task").unwrap();

        let (_, resolved_workspace_dir) = resolve_heartbeat_workspace_dir(&config).unwrap();
        let observer: std::sync::Arc<dyn crate::observability::Observer> =
            std::sync::Arc::new(crate::observability::NoopObserver);
        let engine = crate::heartbeat::engine::HeartbeatEngine::new(
            config.heartbeat.clone(),
            resolved_workspace_dir,
            observer,
        );

        let tasks = engine.collect_tasks().await.unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].text, "Workspace task");
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn daemon_startup_diagnostics_are_logged_as_structured_event() {
        let _writer_guard = kinetic_log::__private_test_writer_lock();
        let _hook_guard = kinetic_log::__private_test_hook_lock();
        kinetic_log::try_install_capture_subscriber();
        let mut rx = kinetic_log::subscribe_or_install();
        while rx.try_recv().is_ok() {}

        let tmp = TempDir::new().unwrap();
        let mut config = test_config(&tmp);
        config.gateway.require_pairing = true;

        record_daemon_started(&config, "127.0.0.1", 0);

        let value = recv_log_event(&mut rx, "KineticVM daemon started").await;
        assert_eq!(value["event"]["category"], "system");
        assert_eq!(value["event"]["action"], "start");
        assert_eq!(value["event"]["outcome"], "success");
        assert_eq!(
            value["attributes"]["requested_gateway"],
            "http://127.0.0.1:0"
        );
        assert_eq!(value["attributes"]["pairing_enabled"].as_bool(), Some(true));
        assert_eq!(value["attributes"]["stop_signal"], "Ctrl+C or SIGTERM");
        assert_eq!(
            value["attributes"]["socket"],
            crate::rpc::local::socket_path(&config)
                .display()
                .to_string()
        );
    }

    #[test]
    fn daemon_startup_banners_distinguish_starting_from_ready() {
        crate::i18n::init("en");
        let tmp = TempDir::new().unwrap();
        let mut config = test_config(&tmp);
        config.gateway.require_pairing = true;
        let actual_addr = "127.0.0.1:42617".parse().unwrap();

        let mut starting = Vec::new();
        echo_daemon_starting_to_terminal(&mut starting).unwrap();
        let starting = String::from_utf8(starting).unwrap();
        assert!(starting.contains("KineticVM"));
        assert!(!starting.contains("http://"));
        assert!(!starting.contains("Socket:"));

        let mut ready = Vec::new();
        echo_daemon_ready_to_terminal(
            &config,
            StartupReadiness {
                gateway_addr: Some(actual_addr),
                socket_ready: true,
                ..StartupReadiness::default()
            },
            &mut ready,
        )
        .unwrap();
        let ready = String::from_utf8(ready).unwrap();
        assert!(ready.contains("http://127.0.0.1:42617"));
        assert!(!ready.contains("http://127.0.0.1:0"));
        assert!(
            ready.contains(
                &crate::rpc::local::socket_path(&config)
                    .display()
                    .to_string()
            )
        );
        assert!(ready.contains("Ctrl+C"));
        assert!(ready.contains("current status"));
        assert!(!ready.contains("code appears"));
        assert!(!ready.contains("pairing code"));

        config.gateway.path_prefix = Some("/kinetic".to_string());
        config.gateway.tls = Some(kinetic_config::schema::GatewayTlsConfig {
            enabled: true,
            ..Default::default()
        });
        let mut tls_ready = Vec::new();
        echo_daemon_ready_to_terminal(
            &config,
            StartupReadiness {
                gateway_addr: Some(actual_addr),
                socket_ready: false,
                ..StartupReadiness::default()
            },
            &mut tls_ready,
        )
        .unwrap();
        let tls_ready = String::from_utf8(tls_ready).unwrap();
        assert!(tls_ready.contains("https://127.0.0.1:42617/kinetic"));
        assert!(!tls_ready.contains("Socket:"));

        let mut no_endpoints = Vec::new();
        echo_daemon_ready_to_terminal(
            &config,
            StartupReadiness {
                gateway_addr: None,
                socket_ready: false,
                ..StartupReadiness::default()
            },
            &mut no_endpoints,
        )
        .unwrap();
        let no_endpoints = String::from_utf8(no_endpoints).unwrap();
        assert!(!no_endpoints.contains("Gateway:"));
        assert!(!no_endpoints.contains("Socket:"));
        assert!(!no_endpoints.contains("Pairing:"));
    }

    #[tokio::test]
    async fn daemon_startup_readiness_waits_for_both_delayed_endpoints() {
        let (readiness_tx, readiness_rx) = tokio::sync::watch::channel(StartupReadiness::default());
        let (_gateway_attempt, gateway_reporter) =
            StartupReadinessAttempt::gateway(Some(readiness_tx.clone()));
        let (_socket_attempt, socket_reporter) =
            StartupReadinessAttempt::socket(Some(readiness_tx));
        let actual_addr = "127.0.0.1:42617".parse().unwrap();

        kinetic_spawn::spawn!(async move {
            tokio::time::sleep(Duration::from_millis(40)).await;
            socket_reporter.unwrap().report_ready();
            tokio::time::sleep(Duration::from_millis(40)).await;
            gateway_reporter.unwrap().report_ready(actual_addr);
        });

        let readiness = tokio::time::timeout(
            Duration::from_secs(1),
            await_startup_readiness(Some(readiness_rx), true, true),
        )
        .await
        .expect("delayed readiness should remain observable")
        .expect("both endpoints should report readiness");
        assert_eq!(readiness.gateway_addr, Some(actual_addr));
        assert!(readiness.socket_ready);
    }

    #[tokio::test]
    async fn daemon_startup_readiness_does_not_latch_a_stale_ephemeral_gateway() {
        let (readiness_tx, readiness_rx) = tokio::sync::watch::channel(StartupReadiness::default());
        let stale_addr = "127.0.0.1:41001".parse().unwrap();
        let current_addr = "127.0.0.1:41002".parse().unwrap();
        let (stale_attempt, stale_reporter) =
            StartupReadinessAttempt::gateway(Some(readiness_tx.clone()));
        let stale_reporter = stale_reporter.unwrap();
        stale_reporter.report_ready(stale_addr);

        let readiness = await_startup_readiness(Some(readiness_rx), true, true);
        tokio::pin!(readiness);
        assert_eq!(readiness_tx.borrow().gateway_addr, Some(stale_addr));
        assert!(!readiness_tx.borrow().socket_ready);

        drop(stale_attempt);
        stale_reporter.report_ready(stale_addr);
        assert_eq!(readiness_tx.borrow().gateway_addr, None);
        let (_socket_attempt, socket_reporter) =
            StartupReadinessAttempt::socket(Some(readiness_tx.clone()));
        socket_reporter.unwrap().report_ready();
        assert_eq!(readiness_tx.borrow().gateway_addr, None);
        assert!(readiness_tx.borrow().socket_ready);

        let (_current_attempt, current_reporter) =
            StartupReadinessAttempt::gateway(Some(readiness_tx.clone()));
        current_reporter.unwrap().report_ready(current_addr);
        stale_reporter.report_ready(stale_addr);
        assert_eq!(readiness_tx.borrow().gateway_addr, Some(current_addr));
        let result = tokio::time::timeout(Duration::from_secs(1), &mut readiness)
            .await
            .expect("current gateway and IPC readiness should complete")
            .expect("readiness channels should remain open");
        assert_eq!(result.gateway_addr, Some(current_addr));
        assert!(result.socket_ready);
    }

    #[tokio::test]
    async fn readiness_attempt_clears_gateway_state_when_task_is_aborted() {
        let (readiness_tx, mut readiness_rx) =
            tokio::sync::watch::channel(StartupReadiness::default());
        let handle = kinetic_spawn::spawn!(async move {
            let (attempt, reporter) = StartupReadinessAttempt::gateway(Some(readiness_tx));
            let _attempt = attempt;
            reporter
                .unwrap()
                .report_ready("127.0.0.1:41003".parse().unwrap());
            std::future::pending::<()>().await;
        });

        tokio::time::timeout(Duration::from_secs(1), async {
            readiness_rx
                .wait_for(|state| state.gateway_addr.is_some())
                .await
                .unwrap();
        })
        .await
        .expect("attempt should report its ephemeral address");
        handle.abort();
        let _ = handle.await;
        tokio::time::timeout(Duration::from_secs(1), async {
            readiness_rx
                .wait_for(|state| state.gateway_addr.is_none())
                .await
                .unwrap();
        })
        .await
        .expect("aborted attempt should clear gateway readiness");
    }

    #[tokio::test]
    async fn readiness_attempt_clears_gateway_state_when_task_returns() {
        let (readiness_tx, mut readiness_rx) =
            tokio::sync::watch::channel(StartupReadiness::default());
        let ready_reported = std::sync::Arc::new(tokio::sync::Notify::new());
        let ready_reported_by_task = ready_reported.clone();
        let release_return = std::sync::Arc::new(tokio::sync::Notify::new());
        let release_return_in_task = release_return.clone();
        let handle = kinetic_spawn::spawn!(async move {
            let (attempt, reporter) = StartupReadinessAttempt::gateway(Some(readiness_tx));
            let _attempt = attempt;
            reporter
                .unwrap()
                .report_ready("127.0.0.1:41004".parse().unwrap());
            ready_reported_by_task.notify_one();
            release_return_in_task.notified().await;
        });

        ready_reported.notified().await;
        assert!(readiness_rx.borrow().gateway_addr.is_some());
        release_return.notify_one();
        handle.await.unwrap();
        tokio::time::timeout(Duration::from_secs(1), async {
            readiness_rx
                .wait_for(|state| state.gateway_addr.is_none())
                .await
                .unwrap();
        })
        .await
        .expect("returned attempt should clear gateway readiness");
    }

    #[tokio::test]
    async fn readiness_attempt_clears_socket_state_when_task_panics() {
        let (readiness_tx, mut readiness_rx) =
            tokio::sync::watch::channel(StartupReadiness::default());
        let ready_reported = std::sync::Arc::new(tokio::sync::Notify::new());
        let ready_reported_by_task = ready_reported.clone();
        let release_panic = std::sync::Arc::new(tokio::sync::Notify::new());
        let release_panic_in_task = release_panic.clone();
        let handle = kinetic_spawn::spawn!(async move {
            let (attempt, reporter) = StartupReadinessAttempt::socket(Some(readiness_tx));
            let _attempt = attempt;
            reporter.unwrap().report_ready();
            ready_reported_by_task.notify_one();
            release_panic_in_task.notified().await;
            panic!("simulated component panic");
        });

        ready_reported.notified().await;
        assert!(readiness_rx.borrow().socket_ready);
        release_panic.notify_one();
        assert!(handle.await.unwrap_err().is_panic());
        tokio::time::timeout(Duration::from_secs(1), async {
            readiness_rx
                .wait_for(|state| !state.socket_ready)
                .await
                .unwrap();
        })
        .await
        .expect("panicked attempt should clear socket readiness");
    }

    #[test]
    #[cfg(unix)]
    fn daemon_startup_stderr_requires_current_process_group_ownership() {
        assert!(stderr_foreground_decision(123, 123));
        assert!(!stderr_foreground_decision(123, 456));
        assert!(!stderr_foreground_decision(-1, 123));
        assert!(!stderr_foreground_decision(0, 0));
    }

    #[tokio::test]
    async fn daemon_startup_readiness_preserves_absent_optional_endpoints() {
        assert_eq!(
            await_startup_readiness(None, false, false).await,
            Some(StartupReadiness {
                gateway_addr: None,
                socket_ready: false,
                ..StartupReadiness::default()
            })
        );
    }

    #[tokio::test]
    async fn daemon_startup_readiness_rejects_closed_endpoint_channel() {
        let (readiness_tx, readiness_rx) = tokio::sync::watch::channel(StartupReadiness::default());
        drop(readiness_tx);

        assert_eq!(
            await_startup_readiness(Some(readiness_rx), true, true).await,
            None
        );
    }

    #[tokio::test]
    async fn supervisor_marks_error_and_restart_on_failure() {
        let cancel = tokio_util::sync::CancellationToken::new();
        let handle =
            spawn_component_supervisor("daemon-test-fail", 1, 1, cancel.clone(), || async {
                anyhow::bail!("boom")
            });

        tokio::time::sleep(Duration::from_millis(50)).await;
        handle.abort();
        let _ = handle.await;

        let snapshot = crate::health::snapshot_json();
        let component = &snapshot["components"]["daemon-test-fail"];
        assert_eq!(component["status"], "error");
        assert!(component["restart_count"].as_u64().unwrap_or(0) >= 1);
        assert!(
            component["last_error"]
                .as_str()
                .unwrap_or("")
                .contains("boom")
        );
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn supervisor_preserves_component_error_chain() {
        let _writer_guard = kinetic_log::__private_test_writer_lock();
        let _hook_guard = kinetic_log::__private_test_hook_lock();
        kinetic_log::try_install_capture_subscriber();
        let mut rx = kinetic_log::subscribe_or_install();
        while rx.try_recv().is_ok() {}

        let cancel = tokio_util::sync::CancellationToken::new();
        let handle =
            spawn_component_supervisor("daemon-test-error-chain", 60, 60, cancel, || async {
                Err(anyhow::Error::msg("provider entry has no model")
                    .context("agents.ox.model_provider"))
            });

        let expected_chain = "agents.ox.model_provider: provider entry has no model";
        let expected_message =
            format!("Daemon component 'daemon-test-error-chain' failed: {expected_chain}");
        let value = recv_log_event(&mut rx, &expected_message).await;
        handle.abort();
        let _ = handle.await;

        assert_eq!(value["attributes"]["error"], expected_chain);
        let snapshot = crate::health::snapshot_json();
        assert_eq!(
            snapshot["components"]["daemon-test-error-chain"]["last_error"],
            expected_chain
        );
    }

    #[tokio::test]
    async fn supervisor_marks_unexpected_exit_as_error() {
        let cancel = tokio_util::sync::CancellationToken::new();
        let handle =
            spawn_component_supervisor("daemon-test-exit", 1, 1, cancel.clone(), || async {
                Ok(())
            });

        tokio::time::sleep(Duration::from_millis(50)).await;
        handle.abort();
        let _ = handle.await;

        let snapshot = crate::health::snapshot_json();
        let component = &snapshot["components"]["daemon-test-exit"];
        assert_eq!(component["status"], "error");
        assert!(component["restart_count"].as_u64().unwrap_or(0) >= 1);
        assert!(
            component["last_error"]
                .as_str()
                .unwrap_or("")
                .contains("component exited unexpectedly")
        );
    }

    #[tokio::test]
    async fn supervisor_marks_clean_shutdown_when_cancel_fires() {
        let cancel = tokio_util::sync::CancellationToken::new();
        let cancel_arc = std::sync::Arc::new(cancel.clone());
        let handle = spawn_component_supervisor("daemon-test-cancel", 1, 1, cancel.clone(), {
            let cancel_arc = std::sync::Arc::clone(&cancel_arc);
            move || {
                let cancel_arc = std::sync::Arc::clone(&cancel_arc);
                async move {
                    cancel_arc.cancelled().await;
                    Ok(())
                }
            }
        });

        // Give the supervisor a tick to call the component once (so
        // the component is parked in `cancel.cancelled().await`).
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Fire the cancellation. The component wakes up, returns
        // `Ok(())`, and the supervisor takes the clean-shutdown path
        // (mark ok + return) instead of the "exited unexpectedly"
        // path.
        cancel.cancel();

        // The supervisor's outer loop is `loop { run_component().await; ... }`
        // so a single Ok(()) while cancelled makes it `return`.
        let join = tokio::time::timeout(Duration::from_secs(1), handle).await;
        assert!(
            join.is_ok(),
            "supervisor should exit cooperatively within 1s of cancel; got: {join:?}"
        );
        let _ = join.unwrap();

        // Health snapshot must show the component as healthy (not error),
        // because the supervisor took the cancel-aware return path.
        let snapshot = crate::health::snapshot_json();
        let component = &snapshot["components"]["daemon-test-cancel"];
        assert_eq!(
            component["status"], "ok",
            "cooperative shutdown must mark the component healthy, not error; got snapshot: {component}"
        );
        assert_eq!(
            component["restart_count"].as_u64().unwrap_or(0),
            0,
            "cooperative shutdown must not trigger a restart; got snapshot: {component}"
        );
    }

    /// A cancellation that arrives while the supervisor is sleeping out a
    /// restart backoff must end the supervisor during that sleep, without a
    /// further component run and without waiting out the backoff window.
    #[tokio::test]
    async fn supervisor_exits_during_backoff_when_cancel_fires() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicU64, Ordering};

        let cancel = tokio_util::sync::CancellationToken::new();
        let calls = Arc::new(AtomicU64::new(0));
        let calls_inner = Arc::clone(&calls);
        // A 60 s initial backoff: if the sleep ignored the token, the join
        // below could not complete within its 1 s budget.
        let handle = spawn_component_supervisor(
            "daemon-test-cancel-in-backoff",
            60,
            60,
            cancel.clone(),
            move || {
                let calls = Arc::clone(&calls_inner);
                async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    anyhow::bail!("boom")
                }
            },
        );

        // Let the component fail once so the supervisor is parked in its
        // backoff sleep.
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        cancel.cancel();

        let join = tokio::time::timeout(Duration::from_secs(1), handle).await;
        assert!(
            join.is_ok(),
            "supervisor must exit during the backoff sleep once cancelled; got: {join:?}"
        );
        let _ = join.unwrap();

        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "a cancelled supervisor must not run the component again"
        );
        let snapshot = crate::health::snapshot_json();
        let component = &snapshot["components"]["daemon-test-cancel-in-backoff"];
        assert_eq!(
            component["status"], "ok",
            "a cancellation during backoff is a cooperative shutdown, not an error; got: {component}"
        );
    }

    #[tokio::test]
    async fn supervisor_backs_off_on_fast_ok_exit_loop() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicU64, Ordering};

        let cancel = tokio_util::sync::CancellationToken::new();
        let calls = Arc::new(AtomicU64::new(0));
        let calls_inner = Arc::clone(&calls);
        let handle =
            spawn_component_supervisor("daemon-test-fastok", 1, 60, cancel.clone(), move || {
                let calls = Arc::clone(&calls_inner);
                async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    // Return immediately — the fast-fail case.
                    Ok(())
                }
            });

        // Let ~3.5s elapse. With exponential backoff the sleeps are
        // 1s, 2s, 4s..., so at most ~3 invocations fit. Without the fix the
        // supervisor would spin at 1s and rack up ~4+ (really unbounded).
        tokio::time::sleep(Duration::from_millis(3500)).await;
        handle.abort();
        let _ = handle.await;

        let n = calls.load(Ordering::SeqCst);
        assert!(
            n <= 3,
            "fast Ok(()) exits must back off exponentially, not hot-loop; got {n} invocations in 3.5s"
        );
    }

    #[test]
    fn detects_no_supervised_channels() {
        let config = Config::default();
        assert!(!has_supervised_channels(&config));
    }

    #[test]
    fn all_disabled_channels_not_supervised() {
        let mut config = Config::default();
        config.channels.discord.insert(
            "clamps".to_string(),
            kinetic_config::schema::DiscordConfig {
                enabled: false,
                bot_token: "token".into(),
                guild_ids: vec![],
                channel_ids: vec![],
                listen_to_bots: false,
                mention_only: true,
                stream_mode: kinetic_config::schema::StreamMode::default(),
                draft_update_interval_ms: 0,
                multi_message_delay_ms: 0,
                stall_timeout_secs: 0,
                slash_commands: false,
                slash_command_scope: kinetic_config::schema::SlashCommandScope::default(),
                intents_mask: None,
                reaction_notifications: kinetic_config::schema::DiscordReactionScope::Off,
                interrupt_on_new_message: false,
                archive: false,
                approval_timeout_secs: 0,
                proxy_url: None,
                excluded_tools: vec![],
                reply_min_interval_secs: 0,
                reply_queue_depth_max: 0,
            },
        );
        config.channels.discord.insert(
            "glados".to_string(),
            kinetic_config::schema::DiscordConfig {
                enabled: false,
                bot_token: "token2".into(),
                guild_ids: vec![],
                channel_ids: vec![],
                listen_to_bots: false,
                mention_only: true,
                stream_mode: kinetic_config::schema::StreamMode::default(),
                draft_update_interval_ms: 0,
                multi_message_delay_ms: 0,
                stall_timeout_secs: 0,
                slash_commands: false,
                slash_command_scope: kinetic_config::schema::SlashCommandScope::default(),
                intents_mask: None,
                reaction_notifications: kinetic_config::schema::DiscordReactionScope::Off,
                interrupt_on_new_message: false,
                archive: false,
                approval_timeout_secs: 0,
                proxy_url: None,
                excluded_tools: vec![],
                reply_min_interval_secs: 0,
                reply_queue_depth_max: 0,
            },
        );
        assert!(!has_supervised_channels(&config));
    }

    #[test]
    fn detects_supervised_channels_present() {
        let mut config = Config::default();
        config.channels.telegram.insert(
            "default".to_string(),
            kinetic_config::schema::TelegramConfig {
                enabled: true,
                bot_token: "token".into(),
                api_base_url: kinetic_config::schema::TELEGRAM_OFFICIAL_API_BASE_URL.to_string(),
                stream_mode: kinetic_config::schema::StreamMode::default(),
                draft_update_interval_ms: 1000,
                multi_message_delay_ms: 800,
                interrupt_on_new_message: false,
                mention_only: false,
                per_user_session: true,
                passive_group_context: false,
                ack_reactions: None,
                proxy_url: None,
                approval_timeout_secs: 120,
                excluded_tools: vec![],
                reply_min_interval_secs: 0,
                reply_queue_depth_max: 0,
                debounce_ms: None,
            },
        );
        assert!(has_supervised_channels(&config));
    }

    #[test]
    fn detects_dingtalk_as_supervised_channel() {
        let mut config = Config::default();
        config.channels.dingtalk.insert(
            "default".to_string(),
            kinetic_config::schema::DingTalkConfig {
                enabled: true,
                client_id: "client_id".into(),
                client_secret: "client_secret".into(),
                proxy_url: None,
                excluded_tools: vec![],
            },
        );
        assert!(has_supervised_channels(&config));
    }

    #[test]
    fn detects_mattermost_as_supervised_channel() {
        let mut config = Config::default();
        config.channels.mattermost.insert(
            "default".to_string(),
            kinetic_config::schema::MattermostConfig {
                enabled: true,
                url: "https://mattermost.example.com".into(),
                bot_token: Some("token".into()),
                login_id: None,
                password: None,
                channel_ids: vec!["channel-id".into()],
                team_ids: vec![],
                discover_dms: None,
                thread_replies: Some(true),
                mention_only: Some(false),
                interrupt_on_new_message: false,
                proxy_url: None,
                listen_mode: MattermostListenMode::default(),
                excluded_tools: vec![],
                reply_min_interval_secs: 0,
                reply_queue_depth_max: 0,
                approval_timeout_secs: 300,
                purpose_as_instructions: false,
            },
        );
        assert!(has_supervised_channels(&config));
    }

    #[test]
    fn detects_qq_as_supervised_channel() {
        let mut config = Config::default();
        config.channels.qq.insert(
            "default".to_string(),
            kinetic_config::schema::QQConfig {
                enabled: true,
                app_id: "app-id".into(),
                app_secret: "app-secret".into(),
                proxy_url: None,
                excluded_tools: vec![],
            },
        );
        assert!(has_supervised_channels(&config));
    }

    #[test]
    fn detects_nextcloud_talk_as_supervised_channel() {
        let mut config = Config::default();
        config.channels.nextcloud_talk.insert(
            "default".to_string(),
            kinetic_config::schema::NextcloudTalkConfig {
                enabled: true,
                base_url: "https://cloud.example.com".into(),
                app_token: None,
                bot_token: None,
                webhook_secret: None,
                proxy_url: None,
                bot_name: None,
                excluded_tools: vec![],
                stream_mode: kinetic_config::schema::StreamMode::default(),
                draft_update_interval_ms: 1000,
            },
        );
        assert!(has_supervised_channels(&config));
    }

    #[test]
    fn webhook_only_config_is_supervised() {
        let mut config = Config::default();
        config.channels.webhook.insert(
            "default".to_string(),
            kinetic_config::schema::WebhookConfig {
                enabled: true,
                port: 8080,
                listen_path: None,
                send_url: None,
                send_method: None,
                auth_header: None,
                secret: None,
                excluded_tools: vec![],
                reply_min_interval_secs: 0,
                reply_queue_depth_max: 0,
                max_retries: None,
                retry_base_delay_ms: None,
                retry_max_delay_ms: None,
            },
        );
        assert!(has_supervised_channels(&config));
    }

    #[test]
    fn resolve_delivery_none_when_unset() {
        let config = Config::default();
        let target = resolve_heartbeat_delivery(&config).unwrap();
        assert!(target.is_none());
    }

    #[test]
    fn resolve_delivery_requires_to_field() {
        let mut config = Config::default();
        config.heartbeat.target = Some("telegram".into());
        let err = resolve_heartbeat_delivery(&config).unwrap_err();
        assert!(
            err.to_string()
                .contains("heartbeat.to is required when heartbeat.target is set")
        );
    }

    #[test]
    fn resolve_delivery_requires_target_field() {
        let mut config = Config::default();
        config.heartbeat.to = Some("123456".into());
        let err = resolve_heartbeat_delivery(&config).unwrap_err();
        assert!(
            err.to_string()
                .contains("heartbeat.target is required when heartbeat.to is set")
        );
    }

    #[test]
    fn resolve_delivery_rejects_unsupported_channel() {
        let mut config = Config::default();
        config.heartbeat.target = Some("carrier_pigeon".into());
        config.heartbeat.to = Some("ops@example.com".into());
        let err = resolve_heartbeat_delivery(&config).unwrap_err();
        assert!(
            err.to_string()
                .contains("unsupported heartbeat.target channel")
        );
    }

    #[test]
    fn resolve_delivery_accepts_matrix_target() {
        let mut config = Config::default();
        config.heartbeat.target = Some("matrix".into());
        config.heartbeat.to = Some("!room:example.org".into());
        config
            .channels
            .matrix
            .insert("default".to_string(), Default::default());

        let target = resolve_heartbeat_delivery(&config).unwrap();
        assert_eq!(
            target,
            Some(("matrix".to_string(), "!room:example.org".to_string()))
        );
    }

    #[test]
    fn resolve_delivery_rejects_configured_but_undeliverable_channel() {
        // review: a configured input-only channel (mqtt is a fan-in
        // listener whose Channel::send is a no-op) must not pass heartbeat
        // validation just because its table exists. Otherwise the validator
        // claims a target the delivery surface silently drops.
        let mut config = Config::default();
        config.heartbeat.target = Some("mqtt".into());
        config.heartbeat.to = Some("ops/heartbeat".into());
        config
            .channels
            .mqtt
            .insert("default".to_string(), Default::default());

        let err = resolve_heartbeat_delivery(&config).unwrap_err();
        assert!(
            err.to_string().contains("input-only channel"),
            "expected input-only rejection, got: {err}"
        );
    }

    #[test]
    fn resolve_delivery_accepts_composite_instance_target() {
        // review: a heartbeat target may name a specific channel instance
        // via its `<type>.<alias>` composite key. The delivery path requires
        // this form to route to a non-default instance in a multi-instance
        // setup, so validation must accept it rather than rejecting it as an
        // unknown channel. The composite key is passed through verbatim so the
        // delivery layer resolves the alias.
        let mut config = Config::default();
        config.heartbeat.target = Some("telegram.roy".into());
        config.heartbeat.to = Some("-1003233270107".into());
        config
            .channels
            .telegram
            .insert("roy".to_string(), Default::default());

        let target = resolve_heartbeat_delivery(&config).unwrap();
        assert_eq!(
            target,
            Some(("telegram.roy".to_string(), "-1003233270107".to_string()))
        );
    }

    #[test]
    fn resolve_delivery_rejects_composite_of_unknown_type() {
        // The type segment of a composite key must still be a known channel;
        // splitting on '.' must not let an unknown type slip through.
        let mut config = Config::default();
        config.heartbeat.target = Some("carrier_pigeon.roy".into());
        config.heartbeat.to = Some("ops@example.com".into());
        let err = resolve_heartbeat_delivery(&config).unwrap_err();
        assert!(
            err.to_string()
                .contains("unsupported heartbeat.target channel"),
            "expected unsupported-channel rejection, got: {err}"
        );
    }

    #[test]
    fn resolve_delivery_rejects_composite_of_undeliverable_type() {
        // Deliverability is a property of the channel type, so a composite key
        // whose type is input-only (mqtt) must be rejected just like the bare
        // form rather than passing on the alias suffix.
        let mut config = Config::default();
        config.heartbeat.target = Some("mqtt.sensors".into());
        config.heartbeat.to = Some("ops/heartbeat".into());
        config
            .channels
            .mqtt
            .insert("sensors".to_string(), Default::default());

        let err = resolve_heartbeat_delivery(&config).unwrap_err();
        assert!(
            err.to_string().contains("input-only channel"),
            "expected input-only rejection, got: {err}"
        );
    }

    #[test]
    fn resolve_delivery_rejects_voice_duplex_target() {
        // review: voice_duplex has a configured table and a WebSocket
        // event protocol but no Channel::send outbound path, so a heartbeat
        // target pointing at it must be rejected like the other input-only
        // transports rather than falling through to the dotted-ref error.
        let mut config = Config::default();
        config.heartbeat.target = Some("voice_duplex".into());
        config.heartbeat.to = Some("ops".into());
        config
            .channels
            .voice_duplex
            .insert("default".to_string(), Default::default());

        let err = resolve_heartbeat_delivery(&config).unwrap_err();
        assert!(
            err.to_string().contains("input-only channel"),
            "expected input-only rejection, got: {err}"
        );
    }

    #[test]
    fn resolve_delivery_requires_channel_configuration() {
        let mut config = Config::default();
        config.heartbeat.target = Some("telegram".into());
        config.heartbeat.to = Some("123456".into());
        let err = resolve_heartbeat_delivery(&config).unwrap_err();
        assert!(
            err.to_string()
                .contains("channels.telegram is not configured")
        );
    }

    #[test]
    fn resolve_delivery_accepts_telegram_configuration() {
        let mut config = Config::default();
        config.heartbeat.target = Some("telegram".into());
        config.heartbeat.to = Some("123456".into());
        config.channels.telegram.insert(
            "default".to_string(),
            kinetic_config::schema::TelegramConfig {
                enabled: true,
                bot_token: "bot-token".into(),
                api_base_url: kinetic_config::schema::TELEGRAM_OFFICIAL_API_BASE_URL.to_string(),
                stream_mode: kinetic_config::schema::StreamMode::default(),
                draft_update_interval_ms: 1000,
                multi_message_delay_ms: 800,
                interrupt_on_new_message: false,
                mention_only: false,
                per_user_session: true,
                passive_group_context: false,
                ack_reactions: None,
                proxy_url: None,
                approval_timeout_secs: 120,
                excluded_tools: vec![],
                reply_min_interval_secs: 0,
                reply_queue_depth_max: 0,
                debounce_ms: None,
            },
        );

        let target = resolve_heartbeat_delivery(&config).unwrap();
        assert_eq!(target, Some(("telegram".to_string(), "123456".to_string())));
    }

    #[test]
    fn auto_detect_telegram_when_configured() {
        use kinetic_config::multi_agent::{PeerGroupConfig, PeerUsername};

        let mut config = Config::default();
        config.channels.telegram.insert(
            "default".to_string(),
            kinetic_config::schema::TelegramConfig {
                enabled: true,
                bot_token: "bot-token".into(),
                api_base_url: kinetic_config::schema::TELEGRAM_OFFICIAL_API_BASE_URL.to_string(),
                stream_mode: kinetic_config::schema::StreamMode::default(),
                draft_update_interval_ms: 1000,
                multi_message_delay_ms: 800,
                interrupt_on_new_message: false,
                mention_only: false,
                per_user_session: true,
                passive_group_context: false,
                ack_reactions: None,
                proxy_url: None,
                approval_timeout_secs: 120,
                excluded_tools: vec![],
                reply_min_interval_secs: 0,
                reply_queue_depth_max: 0,
                debounce_ms: None,
            },
        );
        // Inbound peer authorization lives in peer_groups in V3.
        // Auto-detect picks the first external peer of the synthesized
        // `telegram_default` group as the heartbeat target.
        config.peer_groups.insert(
            "telegram_default".to_string(),
            PeerGroupConfig {
                channel: "telegram".into(),
                external_peers: vec![PeerUsername::new("user123")],
                ..PeerGroupConfig::default()
            },
        );

        let target = resolve_heartbeat_delivery(&config).unwrap();
        assert_eq!(
            target,
            Some(("telegram".to_string(), "user123".to_string()))
        );
    }

    #[test]
    fn auto_detect_none_when_no_channels() {
        let config = Config::default();
        let target = auto_detect_heartbeat_channel(&config);
        assert!(target.is_none());
    }

    #[test]
    fn auto_detect_skips_peers_that_are_not_addresses() {
        use kinetic_config::multi_agent::{PeerGroupConfig, PeerUsername};

        // The resolved peer list answers "who is authorized", so it carries a
        // wildcard and the deny markers for `ignore`. Neither is somewhere a
        // heartbeat can be sent, and an ignored peer least of all.
        let mut config = Config::default();
        config.channels.telegram.insert(
            "default".to_string(),
            kinetic_config::schema::TelegramConfig {
                enabled: true,
                bot_token: "bot-token".into(),
                api_base_url: kinetic_config::schema::TELEGRAM_OFFICIAL_API_BASE_URL.to_string(),
                stream_mode: kinetic_config::schema::StreamMode::default(),
                draft_update_interval_ms: 1000,
                interrupt_on_new_message: false,
                mention_only: false,
                ack_reactions: None,
                proxy_url: None,
                approval_timeout_secs: 120,
                excluded_tools: vec![],
                reply_min_interval_secs: 0,
                reply_queue_depth_max: 0,
                multi_message_delay_ms: 800,
                debounce_ms: None,
                per_user_session: true,
                passive_group_context: false,
            },
        );
        config.peer_groups.insert(
            "telegram_default".to_string(),
            PeerGroupConfig {
                channel: "telegram".into(),
                external_peers: vec![PeerUsername::new("*"), PeerUsername::new("user123")],
                ignore: vec![PeerUsername::new("user123")],
                ..PeerGroupConfig::default()
            },
        );

        assert!(
            auto_detect_heartbeat_channel(&config).is_none(),
            "a wildcard and an ignored peer leave no heartbeat target"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn sighup_does_not_shut_down_daemon() {
        use libc;
        use tokio::time::{Duration, timeout};

        let (_reload_tx, reload_rx) = tokio::sync::watch::channel(false);
        let count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let handle = kinetic_spawn::spawn!(wait_for_exit_signal(reload_rx, false, count));

        // Give the signal handler time to register
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Send SIGHUP to ourselves — should be ignored by the handler.
        // SAFETY: `raise` targets the current process with a valid signal
        // constant and passes no pointers across FFI.
        unsafe { libc::raise(libc::SIGHUP) };

        // The future should NOT complete within a short window
        let result = timeout(Duration::from_millis(200), handle).await;
        assert!(
            result.is_err(),
            "wait_for_exit_signal should not return after SIGHUP"
        );
    }

    #[tokio::test]
    async fn reload_channel_returns_reload() {
        use tokio::time::{Duration, timeout};

        let (reload_tx, reload_rx) = tokio::sync::watch::channel(false);
        let count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let handle = kinetic_spawn::spawn!(wait_for_exit_signal(reload_rx, false, count));
        tokio::time::sleep(Duration::from_millis(50)).await;
        reload_tx.send(true).expect("send reload");

        let result = timeout(Duration::from_secs(2), handle)
            .await
            .expect("wait_for_exit_signal should return after reload signal")
            .expect("task should not panic")
            .expect("signal handler should not error");
        assert_eq!(result, DaemonExit::Reload);
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn registry_gateway_starter_can_trigger_daemon_reload() {
        let _broadcast_guard = hold_broadcast_hooks().await;
        let tmp = TempDir::new().unwrap();
        let config = test_config(&tmp);
        let expected_data_dir = config.data_dir.clone();
        let (seen_tx, mut seen_rx) = tokio::sync::mpsc::unbounded_channel();

        let mut registry = DaemonRegistry::new();
        registry.register_gateway(Box::new(
            move |host,
                  port,
                  config,
                  _live_config_authority,
                  event_tx,
                  reload_controls,
                  tui_registry,
                  _pairing,
                  _ready_tx| {
                let seen_tx = seen_tx.clone();
                Box::pin(async move {
                    let has_event_tx = event_tx.is_some();
                    let has_gateway_shutdown_tx = reload_controls.is_some();
                    let reload_tx = reload_controls
                        .map(|controls| controls.reload_tx)
                        .expect("daemon should pass reload controls to gateway starter");
                    let has_reload_tx = !reload_tx.is_closed();
                    let has_tui_registry = tui_registry.is_some();
                    seen_tx
                        .send((
                            host,
                            port,
                            config.data_dir.clone(),
                            has_event_tx,
                            has_gateway_shutdown_tx,
                            has_reload_tx,
                            has_tui_registry,
                        ))
                        .expect("record gateway starter inputs");
                    reload_tx.send(true).expect("send reload signal");
                    std::future::pending::<Result<()>>().await
                })
            },
        ));

        let (exit, seen) = tokio::time::timeout(DAEMON_DEADLOCK_GUARD, async {
            tokio::join!(
                run(
                    config,
                    "127.0.0.1".to_string(),
                    4242,
                    registry,
                    false,
                    false,
                ),
                seen_rx.recv(),
            )
        })
        .await
        .expect("daemon must not deadlock after a gateway-triggered reload");
        let exit = exit.expect("daemon run should succeed");

        assert_eq!(exit, DaemonExit::Reload);
        let (
            host,
            port,
            data_dir,
            has_event_tx,
            has_gateway_shutdown_tx,
            has_reload_tx,
            has_tui_registry,
        ) = seen.expect("gateway starter should record its daemon inputs");
        assert_eq!(host, "127.0.0.1");
        assert_eq!(port, 4242);
        assert_eq!(data_dir, expected_data_dir);
        assert!(has_event_tx);
        assert!(has_gateway_shutdown_tx);
        assert!(has_reload_tx);
        assert!(has_tui_registry);
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn initial_socket_addr_in_use_fails_daemon_startup() {
        use std::io;

        let _broadcast_guard = hold_broadcast_hooks().await;
        for startup_feedback_enabled in [false, true] {
            let tmp = TempDir::new().unwrap();
            let config = test_config(&tmp);
            let (started_tx, mut started_rx) = tokio::sync::mpsc::unbounded_channel();

            let mut registry = DaemonRegistry::new();
            registry.register_socket(Box::new(move |_ctx, _cancel, _client_count, _readiness| {
                let started_tx = started_tx.clone();
                Box::pin(async move {
                    started_tx
                        .send(())
                        .expect("record initial socket startup attempt");
                    Err(io::Error::new(
                        io::ErrorKind::AddrInUse,
                        "local IPC endpoint lifecycle is already owned",
                    )
                    .into())
                })
            }));

            let (result, started) = tokio::time::timeout(DAEMON_DEADLOCK_GUARD, async {
                tokio::join!(
                    run(
                        config,
                        "127.0.0.1".to_string(),
                        0,
                        registry,
                        false,
                        startup_feedback_enabled,
                    ),
                    started_rx.recv(),
                )
            })
            .await
            .expect("daemon must not deadlock on an initial socket ownership conflict");
            started.expect("socket starter should observe the initial startup attempt");
            let error =
                result.expect_err("daemon startup should fail on an initially owned socket");

            assert!(
                error
                    .downcast_ref::<io::Error>()
                    .is_some_and(|error| error.kind() == io::ErrorKind::AddrInUse),
                "daemon should return the startup socket ownership error with startup_feedback_enabled={startup_feedback_enabled}, got: {error:#}"
            );
        }
    }

    #[test]
    fn fatal_socket_startup_kind_covers_unbindable_paths_through_context() {
        use std::io;

        let unbindable = anyhow::Error::from(io::Error::new(
            io::ErrorKind::InvalidInput,
            "local IPC socket path is 104 bytes but this platform allows at most 103",
        ))
        .context("binding local IPC endpoint");
        assert_eq!(
            fatal_socket_startup_kind(&unbindable),
            Some(io::ErrorKind::InvalidInput)
        );

        let owned = anyhow::Error::from(io::Error::from(io::ErrorKind::AddrInUse));
        assert_eq!(
            fatal_socket_startup_kind(&owned),
            Some(io::ErrorKind::AddrInUse)
        );

        let transient = anyhow::Error::from(io::Error::from(io::ErrorKind::PermissionDenied));
        assert_eq!(fatal_socket_startup_kind(&transient), None);
        assert_eq!(
            fatal_socket_startup_kind(&anyhow::Error::msg("not an io error")),
            None
        );
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn initial_socket_invalid_input_fails_daemon_startup() {
        use std::io;
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let _broadcast_guard = hold_broadcast_hooks().await;
        let tmp = TempDir::new().unwrap();
        let config = test_config(&tmp);
        let attempts = Arc::new(AtomicUsize::new(0));
        let attempts_for_socket = attempts.clone();

        let mut registry = DaemonRegistry::new();
        registry.register_socket(Box::new(move |_ctx, _cancel, _client_count, _readiness| {
            let attempts = attempts_for_socket.clone();
            Box::pin(async move {
                attempts.fetch_add(1, Ordering::SeqCst);
                Err(anyhow::Error::from(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "local IPC socket path is 104 bytes but this platform allows at most 103",
                ))
                .context("binding local IPC endpoint"))
            })
        }));

        let result = tokio::time::timeout(
            DAEMON_DEADLOCK_GUARD,
            run(config, "127.0.0.1".to_string(), 0, registry, false, false),
        )
        .await
        .expect("daemon must not restart-loop an unbindable socket path");
        let error = result.expect_err("daemon startup should fail on an unbindable socket path");

        assert_eq!(
            error.downcast_ref::<io::Error>().map(io::Error::kind),
            Some(io::ErrorKind::InvalidInput),
            "the startup error should keep the bind error kind, got: {error:#}"
        );
        let message = format!("{error:#}");
        assert!(message.contains("binding local IPC endpoint"), "{message}");
        assert!(message.contains("allows at most 103"), "{message}");
        assert_eq!(
            attempts.load(Ordering::SeqCst),
            1,
            "an unbindable path must fail closed instead of being retried"
        );
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn socket_addr_in_use_after_readiness_stays_supervised() {
        use std::io;
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tokio::time::{Duration, timeout};

        let _broadcast_guard = hold_broadcast_hooks().await;
        let tmp = TempDir::new().unwrap();
        let mut config = test_config(&tmp);
        config.reliability.channel_initial_backoff_secs = 1;
        config.reliability.channel_max_backoff_secs = 1;

        let attempts = Arc::new(AtomicUsize::new(0));
        let attempts_for_socket = attempts.clone();

        let mut registry = DaemonRegistry::new();
        registry.register_socket(Box::new(move |_ctx, _cancel, client_count, readiness| {
            let attempts = attempts_for_socket.clone();
            Box::pin(async move {
                let attempt = attempts.fetch_add(1, Ordering::SeqCst) + 1;
                if let Some(readiness) = readiness {
                    readiness.report_ready();
                }
                if attempt == 1 {
                    return Err(io::Error::new(
                        io::ErrorKind::AddrInUse,
                        "socket failed after it was already serving",
                    )
                    .into());
                }

                client_count.store(1, Ordering::SeqCst);
                let client_count_for_drop = client_count.clone();
                kinetic_spawn::spawn!(async move {
                    tokio::time::sleep(Duration::from_millis(1500)).await;
                    client_count_for_drop.store(0, Ordering::SeqCst);
                });
                std::future::pending::<Result<()>>().await
            })
        }));

        let exit = timeout(
            Duration::from_secs(8),
            run(config, "127.0.0.1".to_string(), 0, registry, true, true),
        )
        .await
        .expect("daemon should remain supervised after a post-readiness socket error")
        .expect("daemon run should succeed after supervised socket restart");

        assert_eq!(exit, DaemonExit::Shutdown);
        assert!(
            attempts.load(Ordering::SeqCst) >= 2,
            "socket supervisor should retry after a post-readiness AddrInUse"
        );
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn reload_waits_for_detached_write_before_releasing_process_ownership() {
        use std::sync::Arc;

        use crate::live_config_authority::{
            AgentAdmissionError, AgentExecutionError, ConfigOwnershipError, ConfigOwnershipGuard,
        };

        let _broadcast_guard = hold_broadcast_hooks().await;
        let tmp = TempDir::new().unwrap();
        let config = test_config(&tmp);
        let authority = crate::LiveConfigAuthority::new_owned(config.clone()).unwrap();
        let stale_capability = authority.execution_capability();
        let turn = authority.agent_lifecycle().reserve_turn("alpha").unwrap();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let result_path = config.data_dir.join("final-result");
        let write_path = result_path.clone();
        let background = kinetic_spawn::spawn!(async move {
            release_rx.await.unwrap();
            tokio::fs::write(write_path, b"finished").await.unwrap();
            drop(turn);
        });
        let (reload_sent_tx, reload_sent_rx) = tokio::sync::oneshot::channel();
        let reload_sent = Arc::new(parking_lot::Mutex::new(Some(reload_sent_tx)));
        let session_ready = Arc::new(tokio::sync::Semaphore::new(0));
        let socket_session_ready = session_ready.clone();
        let (sessions_tx, mut sessions_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut registry = DaemonRegistry::new();
        registry.register_socket(Box::new(move |ctx, cancel, _count, readiness| {
            let session_ready = socket_session_ready.clone();
            let sessions_tx = sessions_tx.clone();
            Box::pin(async move {
                let lease = ctx
                    .agent_lifecycle
                    .reserve_admission("idle-session")
                    .unwrap()
                    .publish()
                    .unwrap();
                let agent = crate::agent::agent::Agent::builder()
                    .model_provider(
                        kinetic_providers::create_model_provider("ollama", None).unwrap(),
                    )
                    .tools(crate::tools::scoped::ScopedToolRegistry::from_raw_for_test(
                        vec![],
                    ))
                    .memory(Arc::new(kinetic_memory::NoneMemory::new("none")))
                    .observer(Arc::new(crate::observability::noop::NoopObserver))
                    .tool_dispatcher(Box::new(crate::agent::dispatcher::NativeToolDispatcher))
                    .workspace_dir(std::env::temp_dir())
                    .build()
                    .unwrap();
                ctx.sessions
                    .insert(
                        "idle-session".into(),
                        crate::rpc::session::RpcSession::new(
                            agent,
                            "idle-session",
                            ".",
                            crate::rpc::types::ChatMode::Chat,
                        )
                        .with_lifecycle_lease(lease),
                    )
                    .await
                    .unwrap();
                sessions_tx.send(Arc::downgrade(&ctx.sessions)).unwrap();
                if let Some(readiness) = readiness {
                    readiness.report_ready();
                }
                session_ready.add_permits(1);
                cancel.cancelled().await;
                Ok(())
            })
        }));
        registry.register_gateway(Box::new(
            move |_host, _port, _config, _authority, _events, controls, _tui, _pairing, _ready| {
                let reload_sent = reload_sent.clone();
                let session_ready = session_ready.clone();
                Box::pin(async move {
                    session_ready.acquire().await.unwrap().forget();
                    controls.unwrap().reload_tx.send(true).unwrap();
                    if let Some(sent) = reload_sent.lock().take() {
                        sent.send(()).unwrap();
                    }
                    std::future::pending::<Result<()>>().await
                })
            },
        ));
        let mut daemon = kinetic_spawn::spawn!(run_with_authority(
            authority,
            "127.0.0.1".to_string(),
            0,
            registry,
            false,
            false,
        ));
        tokio::time::timeout(Duration::from_secs(5), reload_sent_rx)
            .await
            .unwrap()
            .unwrap();
        let sessions = sessions_rx.recv().await.unwrap();
        // Exceed component shutdown's grace window while the detached write is held.
        assert!(
            tokio::time::timeout(Duration::from_secs(1), &mut daemon)
                .await
                .is_err()
        );
        assert!(sessions.upgrade().is_none(), "idle RPC registry must drain");
        assert!(!result_path.exists());
        assert!(matches!(
            ConfigOwnershipGuard::acquire(&config.data_dir),
            Err(ConfigOwnershipError::AlreadyOwned { .. })
        ));
        assert!(matches!(
            stale_capability.admit("alpha"),
            Err(AgentExecutionError::Admission(
                AgentAdmissionError::GenerationClosing
            ))
        ));

        release_tx.send(()).unwrap();
        background.await.unwrap();
        let (exit, transferred_ownership) = tokio::time::timeout(Duration::from_secs(5), daemon)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(exit, DaemonExit::Reload);
        assert_eq!(tokio::fs::read(&result_path).await.unwrap(), b"finished");
        // Continuous ownership across reload generations: the guard returns
        // from `run_with_authority` instead of being released, and the next
        // generation adopts it without an unlocked reacquire interval.
        let ownership = transferred_ownership.expect("reload transfers process ownership");
        assert!(matches!(
            ConfigOwnershipGuard::acquire(&config.data_dir),
            Err(ConfigOwnershipError::AlreadyOwned { .. })
        ));
        let next = crate::LiveConfigAuthority::new_with_ownership(config.clone(), ownership);
        assert!(next.agent_lifecycle().reserve_turn("alpha").is_ok());
        assert!(matches!(
            stale_capability.admit("alpha"),
            Err(AgentExecutionError::Admission(
                AgentAdmissionError::GenerationClosing
            ))
        ));
        drop(stale_capability);
        assert!(matches!(
            ConfigOwnershipGuard::acquire(&config.data_dir),
            Err(ConfigOwnershipError::AlreadyOwned { .. })
        ));
    }

    #[tokio::test]
    async fn daemon_preload_ownership_blocks_offline_mutation_before_fresh_load() {
        use crate::live_config_authority::{ConfigOwnershipError, ConfigOwnershipGuard};

        let tmp = TempDir::new().unwrap();
        let config = test_config(&tmp);

        // Daemon startup: the data-directory identity is resolved and process
        // ownership is acquired BEFORE the executable config is loaded.
        let ownership = ConfigOwnershipGuard::acquire(&config.data_dir).unwrap();

        // A supported offline mutation cannot commit while startup holds the
        // guard, so the loaded snapshot can never be older than a mutation
        // that committed after the identity resolution.
        assert!(matches!(
            ConfigOwnershipGuard::acquire(&config.data_dir),
            Err(ConfigOwnershipError::AlreadyOwned { .. })
        ));

        // The fresh protected snapshot loads under the held guard and becomes
        // the generation's authority without reacquiring the lock.
        let mut fresh = test_config(&tmp);
        fresh.agents.insert("alpha".into(), Default::default());
        let authority = crate::LiveConfigAuthority::new_with_ownership(fresh, ownership);
        assert!(authority.agent_lifecycle().reserve_turn("alpha").is_ok());
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn scheduler_cooperative_shutdown_observed_through_daemon_reload() {
        use tokio::time::{Duration, timeout};

        let _broadcast_guard = hold_broadcast_hooks().await;
        let tmp = TempDir::new().unwrap();
        let mut config = test_config(&tmp);
        config.scheduler.enabled = true;

        reset_scheduler_clean_shutdown_observed();

        let mut registry = DaemonRegistry::new();
        registry.register_gateway(Box::new(
            move |_host,
                  _port,
                  _config,
                  _live_config_authority,
                  _event_tx,
                  reload_controls,
                  _tui_reg,
                  _pairing,
                  _ready_tx| {
                Box::pin(async move {
                    let reload_tx = reload_controls
                        .map(|controls| controls.reload_tx)
                        .expect("daemon should pass reload controls to gateway starter");
                    // Give the scheduler a tick to enter its select!
                    // loop and park at the next interval tick or cancel.
                    tokio::time::sleep(Duration::from_millis(300)).await;
                    reload_tx.send(true).expect("send reload signal");
                    std::future::pending::<Result<()>>().await
                })
            },
        ));

        let exit = timeout(
            Duration::from_secs(3),
            run(config, "127.0.0.1".to_string(), 0, registry, false, false),
        )
        .await
        .expect("daemon should return after gateway-triggered reload")
        .expect("daemon run should succeed");
        assert_eq!(exit, DaemonExit::Reload);

        assert!(
            scheduler_clean_shutdown_observed(),
            "scheduler supervisor must take the cancel-aware clean-return branch; \
             aborting the supervisor before it observes Ok(()) leaves this sentinel false"
        );

        let snapshot = crate::health::snapshot_json();
        let component = &snapshot["components"]["scheduler"];
        assert_eq!(
            component["status"], "ok",
            "scheduler health snapshot must show ok after cooperative shutdown; got: {component}"
        );
        assert_eq!(
            component["restart_count"].as_u64().unwrap_or(0),
            0,
            "scheduler must not have been restarted; \
             restart_count > 0 means the supervisor took the unexpected-Ok or Err branch \
             instead of the cancel-aware return, which is the regression this test pins"
        );
        assert!(
            component["last_error"].is_null(),
            "scheduler must have no last_error after cooperative shutdown; got: {component}"
        );
    }

    #[tokio::test]
    async fn ephemeral_does_not_exit_before_client_connects() {
        use tokio::time::{Duration, timeout};

        let (_reload_tx, reload_rx) = tokio::sync::watch::channel(false);
        let count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let handle = kinetic_spawn::spawn!(wait_for_exit_signal(reload_rx, true, count));

        // No clients ever connect — should NOT shut down.
        let result = timeout(Duration::from_millis(500), handle).await;
        assert!(
            result.is_err(),
            "ephemeral daemon should not exit before any client connects"
        );
    }

    #[tokio::test]
    async fn ephemeral_exits_after_client_disconnects() {
        use std::sync::atomic::Ordering;
        use tokio::time::{Duration, timeout};

        let (_reload_tx, reload_rx) = tokio::sync::watch::channel(false);
        let count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count2 = count.clone();
        let handle = kinetic_spawn::spawn!(wait_for_exit_signal(reload_rx, true, count2));

        // Simulate client connect then disconnect.
        count.store(1, Ordering::Relaxed);
        tokio::time::sleep(Duration::from_millis(100)).await;
        count.store(0, Ordering::Relaxed);

        // Should exit within grace period + buffer.
        let result = timeout(Duration::from_secs(EPHEMERAL_GRACE_SECS + 5), handle)
            .await
            .expect("ephemeral daemon should shut down after last client disconnects")
            .expect("task should not panic")
            .expect("signal handler should not error");
        assert_eq!(result, DaemonExit::Shutdown);
    }

    #[tokio::test]
    async fn ephemeral_grace_period_resets_on_reconnect() {
        use std::sync::atomic::Ordering;
        use tokio::time::{Duration, timeout};

        let (_reload_tx, reload_rx) = tokio::sync::watch::channel(false);
        let count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count2 = count.clone();
        let mut handle = kinetic_spawn::spawn!(wait_for_exit_signal(reload_rx, true, count2));

        // Client connects, disconnects.
        count.store(1, Ordering::Relaxed);
        tokio::time::sleep(Duration::from_millis(100)).await;
        count.store(0, Ordering::Relaxed);

        // Reconnect partway through the grace period — must be strictly
        // less than EPHEMERAL_GRACE_SECS so the daemon hasn't already
        // exited. With the 1s grace window we sleep ~200ms.
        tokio::time::sleep(Duration::from_millis(200)).await;
        count.store(1, Ordering::Relaxed);

        // Should NOT shut down while client is connected.
        let result = timeout(Duration::from_millis(500), &mut handle).await;
        assert!(
            result.is_err(),
            "ephemeral daemon should not exit while client is connected"
        );

        // Disconnect again — should eventually shut down.
        count.store(0, Ordering::Relaxed);
        let result = timeout(Duration::from_secs(EPHEMERAL_GRACE_SECS + 5), handle)
            .await
            .expect("ephemeral daemon should shut down after second disconnect")
            .expect("task should not panic")
            .expect("signal handler should not error");
        assert_eq!(result, DaemonExit::Shutdown);
    }

    // ── daemon gateway bind-mode detection (fail-fast) ────────────────

    /// Raw HTTP/1.1 `/health` body a real KineticVM gateway returns (shape
    /// mirrors `handle_health` in `kinetic-gateway`): `status: ok` plus the
    /// identity fields `require_pairing` and `runtime`.
    fn kinetic_health_ok_response() -> Vec<u8> {
        http_response(
            "200 OK",
            br#"{"status":"ok","paired":false,"require_pairing":true,"runtime":{"components":{}}}"#,
        )
    }

    /// Build a minimal HTTP/1.1 response with a JSON body.
    fn http_response(status_line: &str, body: &[u8]) -> Vec<u8> {
        let mut resp = format!(
            "HTTP/1.1 {status_line}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
            body.len()
        )
        .into_bytes();
        resp.extend_from_slice(body);
        resp
    }

    /// Spawn a one-shot HTTP responder on loopback. It answers the first
    /// request with `response`, then holds the listener bound until the
    /// returned guard (`oneshot::Sender`) is dropped — so the bind probe sees
    /// the port as occupied and the follow-up `/health` probe gets answered.
    async fn spawn_mock_gateway(response: Vec<u8>) -> (u16, tokio::sync::oneshot::Sender<()>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind mock listener");
        let port = listener.local_addr().expect("mock local addr").port();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
        kinetic_spawn::spawn!(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                let mut buf = [0_u8; 1024];
                let _ = stream.read(&mut buf).await;
                let _ = stream.write_all(&response).await;
                let _ = stream.flush().await;
            }
            // Keep `listener` in scope (port stays bound) until released.
            let _ = release_rx.await;
        });
        (port, release_tx)
    }

    #[test]
    fn gateway_probe_authority_maps_wildcards_and_brackets_ipv6() {
        // Wildcards map to loopback (IPv4 -> 127.0.0.1, IPv6 -> [::1]) the same
        // way the CLI self-test probe does.
        assert_eq!(gateway_probe_authority("0.0.0.0"), "127.0.0.1");
        assert_eq!(gateway_probe_authority("::"), "[::1]");
        assert_eq!(gateway_probe_authority("[::]"), "[::1]");
        // Concrete hosts pass through; a bare IPv6 host is bracketed for URLs.
        assert_eq!(gateway_probe_authority("127.0.0.1"), "127.0.0.1");
        assert_eq!(gateway_probe_authority("::1"), "[::1]");
        assert_eq!(gateway_probe_authority("[::1]"), "[::1]");
        assert_eq!(gateway_probe_authority("example.test"), "example.test");
    }

    #[test]
    fn gateway_health_probe_url_defaults_to_http_health() {
        let config = Config::default();
        assert_eq!(
            gateway_health_probe_url(&config, "127.0.0.1", 8080),
            "http://127.0.0.1:8080/health"
        );
    }

    #[test]
    fn gateway_health_probe_url_maps_ipv6_wildcard_to_loopback() {
        let config = Config::default();
        assert_eq!(
            gateway_health_probe_url(&config, "[::]", 8080),
            "http://[::1]:8080/health"
        );
        assert_eq!(
            gateway_health_probe_url(&config, "0.0.0.0", 8080),
            "http://127.0.0.1:8080/health"
        );
    }

    #[test]
    fn gateway_health_probe_url_honours_path_prefix() {
        let mut config = Config::default();
        config.gateway.path_prefix = Some("/api".to_string());
        assert_eq!(
            gateway_health_probe_url(&config, "127.0.0.1", 8080),
            "http://127.0.0.1:8080/api/health"
        );
    }

    #[test]
    fn gateway_health_probe_url_uses_https_when_tls_enabled() {
        let mut config = Config::default();
        config.gateway.tls = Some(kinetic_config::schema::GatewayTlsConfig {
            enabled: true,
            ..Default::default()
        });
        assert_eq!(
            gateway_health_probe_url(&config, "127.0.0.1", 8443),
            "https://127.0.0.1:8443/health"
        );
    }

    #[tokio::test]
    async fn detect_gateway_bind_mode_starts_fresh_on_ephemeral_port() {
        // Port 0 is kernel-assigned: it cannot already be bound.
        assert_eq!(
            detect_gateway_bind_mode(&Config::default(), "0.0.0.0", 0).await,
            GatewayBindMode::StartFresh
        );
    }

    #[tokio::test]
    async fn detect_gateway_bind_mode_starts_fresh_on_free_port() {
        // Reserve an ephemeral port, then release it so the address is free.
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("reserve port");
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        assert_eq!(
            detect_gateway_bind_mode(&Config::default(), "127.0.0.1", port).await,
            GatewayBindMode::StartFresh
        );
    }

    #[tokio::test]
    async fn detect_gateway_bind_mode_flags_existing_kinetic_gateway() {
        // A real KineticVM `/health` (status==ok + identity fields) on an
        // occupied port → fail fast with the "gateway already running" message.
        let (port, _release) = spawn_mock_gateway(kinetic_health_ok_response()).await;
        assert_eq!(
            detect_gateway_bind_mode(&Config::default(), "127.0.0.1", port).await,
            GatewayBindMode::GatewayAlreadyRunning,
            "a KineticVM /health on an occupied port is recognised as a gateway"
        );
    }

    #[tokio::test]
    async fn detect_gateway_bind_mode_flags_generic_status_ok_as_occupied() {
        // A foreign service answering the generic `{"status":"ok"}` (no
        // KineticVM identity fields) must NOT be taken for a gateway — it is a
        // plain occupied port.
        let (port, _release) =
            spawn_mock_gateway(http_response("200 OK", br#"{"status":"ok"}"#)).await;
        assert_eq!(
            detect_gateway_bind_mode(&Config::default(), "127.0.0.1", port).await,
            GatewayBindMode::PortOccupied,
            "a generic status:ok health response is not a KineticVM gateway"
        );
    }

    #[tokio::test]
    async fn detect_gateway_bind_mode_flags_non_gateway_404_as_occupied() {
        let (port, _release) = spawn_mock_gateway(http_response("404 Not Found", b"")).await;
        assert_eq!(
            detect_gateway_bind_mode(&Config::default(), "127.0.0.1", port).await,
            GatewayBindMode::PortOccupied,
            "a non-2xx /health on an occupied port fails fast as a foreign occupant"
        );
    }

    #[tokio::test]
    async fn detect_gateway_bind_mode_defers_on_non_addr_in_use_error() {
        let outcome = classify_gateway_bind_outcome(
            Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied)),
            &Config::default(),
            "0.0.0.0",
            80,
        )
        .await;
        assert_eq!(
            outcome,
            GatewayBindMode::StartFresh,
            "a non-AddrInUse bind error must defer to the gateway's own bind, not fail fast"
        );
    }

    // ── reconcile_heartbeat_mcp_registry unit tests ──────────────────────

    // ── Health check & reconnection for a stdio child that dies ────────

    /// The reload drain must report the connections that were still unwinding
    /// when its budget expired, not silently declare the generation retired.
    #[tokio::test(start_paused = true)]
    async fn rpc_drain_reports_connections_left_unwinding_when_the_budget_expires() {
        let count = std::sync::atomic::AtomicUsize::new(2);
        assert_eq!(
            await_rpc_connection_drain(&count).await,
            RpcDrain::Outstanding(2)
        );
    }

    #[tokio::test(start_paused = true)]
    async fn rpc_drain_completes_once_the_last_connection_task_ends() {
        let count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(1));
        let releaser = std::sync::Arc::clone(&count);
        let release = kinetic_spawn::spawn!(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            releaser.store(0, std::sync::atomic::Ordering::Relaxed);
        });
        assert_eq!(await_rpc_connection_drain(&count).await, RpcDrain::Complete);
        release.await.unwrap();
    }

    /// A reload hands the same durable sessions to a replacement generation, so
    /// it is admissible only on proof that the retiring generation is finished.
    #[test]
    fn reload_is_refused_when_rpc_work_is_still_unwinding() {
        assert_eq!(
            settle_exit_against_drain(Ok(DaemonExit::Reload), RpcDrain::Outstanding(1)).unwrap(),
            DaemonExit::Shutdown,
            "an undrained reload must shut the process down instead of handing over"
        );
        assert_eq!(
            settle_exit_against_drain(Ok(DaemonExit::Reload), RpcDrain::Complete).unwrap(),
            DaemonExit::Reload,
            "a drained reload must still reload"
        );
        assert_eq!(
            settle_exit_against_drain(Ok(DaemonExit::Shutdown), RpcDrain::Outstanding(1)).unwrap(),
            DaemonExit::Shutdown,
            "a shutdown is unaffected by the drain verdict"
        );
        assert!(
            settle_exit_against_drain(Err(anyhow::Error::msg("boom")), RpcDrain::Outstanding(1))
                .is_err(),
            "a failed daemon run must keep reporting its failure"
        );
    }

    #[tokio::test]
    async fn reload_waits_for_rpc_connection_drain_without_holding_other_components() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};
        use tokio::time::{Duration, Instant, timeout};

        let tmp = TempDir::new().unwrap();
        let config = test_config(&tmp);

        let accepted = Arc::new(AtomicBool::new(false));
        let drained = Arc::new(AtomicBool::new(false));
        let accepted_for_socket = accepted.clone();
        let drained_for_socket = drained.clone();

        let mut registry = DaemonRegistry::new();
        registry.register_socket(Box::new(move |_ctx, cancel, client_count, readiness| {
            let accepted = accepted_for_socket.clone();
            let drained = drained_for_socket.clone();
            Box::pin(async move {
                if let Some(readiness) = readiness {
                    readiness.report_ready();
                }
                // One accepted connection, counted the way a listener counts a
                // live client.
                client_count.store(1, Ordering::SeqCst);
                accepted.store(true, Ordering::SeqCst);
                cancel.cancelled().await;
                // A connection whose prompt takes longer than the per-component
                // grace to unwind before its client-count guard drops.
                tokio::time::sleep(Duration::from_millis(1200)).await;
                client_count.store(0, Ordering::SeqCst);
                drained.store(true, Ordering::SeqCst);
                Ok(())
            })
        }));
        // The gateway asks for the reload once the connection exists, then
        // parks: an unrelated pending component must not extend shutdown.
        registry.register_gateway(Box::new(
            move |_host,
                  _port,
                  _config,
                  _live_config_authority,
                  _event_tx,
                  reload_controls,
                  _tui_reg,
                  _pairing,
                  _ready_tx| {
                let accepted = accepted.clone();
                Box::pin(async move {
                    let reload_tx = reload_controls
                        .map(|controls| controls.reload_tx)
                        .expect("daemon should pass reload controls to gateway starter");
                    while !accepted.load(Ordering::SeqCst) {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                    reload_tx.send(true).expect("send reload signal");
                    std::future::pending::<Result<()>>().await
                })
            },
        ));

        let started = Instant::now();
        let exit = timeout(
            Duration::from_secs(8),
            run(config, "127.0.0.1".to_string(), 0, registry, false, false),
        )
        .await
        .expect("daemon should finish the reload inside the RPC connection drain budget")
        .expect("daemon run should succeed");
        let elapsed = started.elapsed();

        assert_eq!(exit, DaemonExit::Reload);
        assert!(
            drained.load(Ordering::SeqCst),
            "reload must wait for the accepted RPC connection to drain before retiring \
             the listener; aborting the listener at the component grace leaves the \
             connection's prompt work running into the replacement generation"
        );
        assert!(
            elapsed < Duration::from_secs(4),
            "a pending unrelated component must not inherit the RPC drain budget; \
             reload took {elapsed:?}"
        );
    }

    /// The daemon owns the live-pricing refresher, so it must run with the
    /// gateway disabled. This drives the real daemon with no gateway registered
    /// and a provider opted into live pricing, and waits for the refresher.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn pricing_refresher_runs_with_the_gateway_disabled() {
        use tokio::time::{Duration, Instant, sleep};

        let _broadcast_guard = hold_broadcast_hooks().await;
        let tmp = TempDir::new().unwrap();
        let mut config = test_config(&tmp);
        config.providers.models.ollama.insert(
            "priced".to_string(),
            kinetic_config::schema::OllamaModelProviderConfig {
                base: kinetic_config::schema::ModelProviderConfig {
                    // Discard port: the refresher's fetch fails fast, which
                    // keeps the previous (empty) snapshot. Only the start matters.
                    uri: Some("http://127.0.0.1:9".to_string()),
                    model: Some("priced-model".to_string()),
                    live_pricing: true,
                    ..Default::default()
                },
                ..Default::default()
            },
        );
        // No gateway is registered: the daemon runs with the gateway disabled.
        let registry = DaemonRegistry::new();
        let daemon = run(config, "127.0.0.1".to_string(), 0, registry, false, false);
        tokio::pin!(daemon);

        let deadline = Instant::now() + Duration::from_secs(5);
        // The refresher starts once per process, so another test may already
        // have started it. What this run must do is bind it to this daemon's
        // opted-in live configuration: the other daemon tests hold the same
        // lock and leave the binding opted out, so it only turns true here.
        loop {
            if kinetic_providers::pricing::refresher_running()
                && kinetic_providers::pricing::live_pricing_enabled()
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "the daemon must start the pricing refresher on its live configuration \
                 without a gateway"
            );
            tokio::select! {
                result = &mut daemon => panic!("daemon exited before the check: {result:?}"),
                () = sleep(Duration::from_millis(20)) => {}
            }
        }
    }

    /// Records every `on_gateway_start` call.
    struct RecordingGatewayStartHook(std::sync::Arc<std::sync::Mutex<Vec<(String, u16)>>>);

    #[async_trait::async_trait]
    impl crate::hooks::HookHandler for RecordingGatewayStartHook {
        fn name(&self) -> &str {
            "record-gateway-start"
        }
        async fn on_gateway_start(&self, host: &str, port: u16) {
            self.0.lock().unwrap().push((host.to_string(), port));
        }
    }

    /// The gateway-start hook fires exactly once per gateway start, with the
    /// port the listener actually bound, even if readiness is reported again,
    /// and the wrapped readiness reporter still sees every report.
    #[tokio::test]
    async fn gateway_start_hook_fires_once_per_gateway_start_with_the_bound_port() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tokio::time::{Duration, sleep};

        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut runner = crate::hooks::HookRunner::new();
        runner.register(Box::new(RecordingGatewayStartHook(seen.clone())));
        let runner = std::sync::Arc::new(runner);

        let inner_reports = std::sync::Arc::new(AtomicUsize::new(0));
        let inner = {
            let inner_reports = inner_reports.clone();
            GatewayReadinessReporter::new(move |_addr| {
                inner_reports.fetch_add(1, Ordering::SeqCst);
            })
        };

        // First gateway start. The configured port was 0; the listener bound 43210.
        let first = gateway_start_hook_reporter(
            Some(runner.clone()),
            "0.0.0.0".to_string(),
            Some(inner.clone()),
        )
        .expect("hooks enabled yields a reporter");
        let bound: std::net::SocketAddr = "0.0.0.0:43210".parse().unwrap();
        first.report_ready(bound);
        first.report_ready(bound);

        let wait_for = |n: usize| {
            let seen = seen.clone();
            async move {
                for _ in 0..100 {
                    if seen.lock().unwrap().len() >= n {
                        return;
                    }
                    sleep(Duration::from_millis(10)).await;
                }
            }
        };
        wait_for(1).await;
        // Give a wrongly repeated fire time to land before asserting it did not.
        sleep(Duration::from_millis(50)).await;
        assert_eq!(
            *seen.lock().unwrap(),
            vec![("0.0.0.0".to_string(), 43210)],
            "one gateway start fires the hook once, with the bound port"
        );
        assert_eq!(
            inner_reports.load(Ordering::SeqCst),
            2,
            "every readiness report still reaches the wrapped reporter"
        );

        // A later gateway start (supervisor restart or reload) gets a fresh
        // reporter and fires once more.
        let second = gateway_start_hook_reporter(Some(runner), "0.0.0.0".to_string(), Some(inner))
            .expect("hooks enabled yields a reporter");
        second.report_ready("0.0.0.0:43211".parse().unwrap());
        wait_for(2).await;
        sleep(Duration::from_millis(50)).await;
        assert_eq!(
            *seen.lock().unwrap(),
            vec![
                ("0.0.0.0".to_string(), 43210),
                ("0.0.0.0".to_string(), 43211),
            ],
            "each gateway start fires the hook exactly once, with its own bound port"
        );
    }

    #[test]
    fn gateway_start_hook_reporter_is_a_passthrough_when_hooks_are_disabled() {
        assert!(gateway_start_hook_reporter(None, "127.0.0.1".to_string(), None).is_none());
        let reported = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let inner = {
            let reported = reported.clone();
            GatewayReadinessReporter::new(move |addr| reported.lock().unwrap().push(addr))
        };
        let wrapped = gateway_start_hook_reporter(None, "127.0.0.1".to_string(), Some(inner))
            .expect("with hooks disabled the readiness reporter is kept");
        let addr: std::net::SocketAddr = "127.0.0.1:42617".parse().unwrap();
        wrapped.report_ready(addr);
        assert_eq!(
            *reported.lock().unwrap(),
            vec![addr],
            "with hooks disabled every readiness report still reaches the daemon"
        );
    }

    /// The daemon wraps each gateway start's readiness reporter with the hook
    /// when hooks are enabled. With startup feedback off there is no inner
    /// reporter, so the gateway starter receives one only through that
    /// wrapper; removing the daemon's wiring makes it arrive as `None`.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn daemon_hands_the_gateway_a_hook_reporter_when_hooks_are_enabled() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicU8, Ordering};
        use tokio::time::{Duration, Instant, sleep};

        let _broadcast_guard = hold_broadcast_hooks().await;
        for (hooks_enabled, expect_reporter) in [(true, true), (false, false)] {
            let tmp = TempDir::new().unwrap();
            let mut config = test_config(&tmp);
            config.hooks.enabled = hooks_enabled;

            // 0 = not started, 1 = started without a reporter, 2 = with one.
            let received = Arc::new(AtomicU8::new(0));
            let mut registry = DaemonRegistry::new();
            {
                let received = received.clone();
                registry.register_gateway(Box::new(
                    move |_host,
                          _port,
                          _config,
                          _live_config_authority,
                          _event_tx,
                          _reload,
                          _tui,
                          _pairing,
                          readiness| {
                        received.store(if readiness.is_some() { 2 } else { 1 }, Ordering::SeqCst);
                        Box::pin(std::future::pending::<Result<()>>())
                    },
                ));
            }

            // Startup feedback off: no readiness reporter of the daemon's own.
            let daemon = run(config, "127.0.0.1".to_string(), 0, registry, false, false);
            tokio::pin!(daemon);
            let deadline = Instant::now() + Duration::from_secs(5);
            while received.load(Ordering::SeqCst) == 0 {
                assert!(Instant::now() < deadline, "the gateway starter must run");
                tokio::select! {
                    result = &mut daemon => panic!("daemon exited early: {result:?}"),
                    () = sleep(Duration::from_millis(10)) => {}
                }
            }
            assert_eq!(
                received.load(Ordering::SeqCst) == 2,
                expect_reporter,
                "hooks enabled = {hooks_enabled}: the gateway starter's readiness reporter"
            );
        }
    }

    /// In daemon mode the refresher follows the generation's shared live
    /// configuration: the handle the RPC context writes and hands to the
    /// supervised gateway. A write through that handle, as either surface's
    /// config API makes, must reach the refresher without a reload.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn a_live_config_write_reaches_the_daemon_pricing_refresher_without_a_reload() {
        use std::sync::Arc;
        use tokio::time::{Duration, Instant, sleep};

        let _broadcast_guard = hold_broadcast_hooks().await;
        let tmp = TempDir::new().unwrap();
        let mut config = test_config(&tmp);
        config.providers.models.ollama.insert(
            "priced".to_string(),
            kinetic_config::schema::OllamaModelProviderConfig {
                base: kinetic_config::schema::ModelProviderConfig {
                    uri: Some("http://127.0.0.1:9".to_string()),
                    model: Some("priced-model".to_string()),
                    live_pricing: true,
                    ..Default::default()
                },
                ..Default::default()
            },
        );

        // The gateway starter captures the live configuration the daemon hands it.
        let handed = Arc::new(std::sync::Mutex::new(None));
        let mut registry = DaemonRegistry::new();
        {
            let handed = handed.clone();
            registry.register_gateway(Box::new(
                move |_host,
                      _port,
                      _config,
                      _live_config_authority,
                      _event_tx,
                      _reload,
                      _tui,
                      authority,
                      _ready| {
                    *handed.lock().unwrap() = authority.map(|authority| authority.config);
                    Box::pin(std::future::pending::<Result<()>>())
                },
            ));
        }

        let daemon = run(config, "127.0.0.1".to_string(), 0, registry, false, false);
        tokio::pin!(daemon);
        let deadline = Instant::now() + Duration::from_secs(5);
        let live = loop {
            if let Some(live) = handed.lock().unwrap().clone() {
                break live;
            }
            assert!(Instant::now() < deadline, "the gateway starter must run");
            tokio::select! {
                result = &mut daemon => panic!("daemon exited early: {result:?}"),
                () = sleep(Duration::from_millis(10)) => {}
            }
        };
        assert!(
            kinetic_providers::pricing::live_pricing_enabled(),
            "the refresher follows the generation's live configuration, which opts in"
        );

        live.write()
            .providers
            .models
            .ollama
            .get_mut("priced")
            .expect("the opted-in provider exists")
            .base
            .live_pricing = false;
        assert!(
            !kinetic_providers::pricing::live_pricing_enabled(),
            "a write to the shared live configuration must reach the refresher without a reload"
        );
    }
}
