//! Monad identity, vault, and attestation tools. Payments go through the vault.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use kinetic_api::tool::{Tool, ToolOutput, ToolResult, ToolSpec};
use kinetic_chain::{
    Address, BootReport, DeviceChain, format_mon, parse_mon, unclaimed_claim_text,
};
use kinetic_config::schema::{ChainConfig, Config, RecordsConfig};

use crate::owner_commands::OwnerPing;
use serde_json::json;

const ACTUATORS: &[&str] = &[
    "gpio_write",
    "gpio_rpi_write",
    "gpio_rpi_blink",
    "set_device",
];

pub fn chain_tools(config: &Config, key_dir: PathBuf) -> Vec<Arc<dyn Tool>> {
    let chain = config.chain.clone();
    let ping = OwnerPing::from_config(config);
    vec![
        Arc::new(MonadIdentityTool::new(chain.clone(), key_dir.clone())),
        Arc::new(MonadAttestTool::new(
            chain.clone(),
            key_dir.clone(),
            ping.clone(),
            config.records.clone(),
            config.operation_records_path(),
        )),
        Arc::new(VaultStatusTool::new(chain.clone(), key_dir.clone())),
        Arc::new(VaultPayTool::new(chain, key_dir, ping)),
    ]
}

pub fn is_actuator(name: &str) -> bool {
    ACTUATORS.contains(&name)
}

/// After a successful pin or serial action, record it on Monad.
pub fn attest_physical_actions(
    config: &kinetic_config::schema::Config,
    tools: Vec<Box<dyn Tool>>,
) -> Vec<Box<dyn Tool>> {
    if !config.chain.enabled {
        return tools;
    }
    let Some(key_dir) = config.config_path.parent().map(PathBuf::from) else {
        return tools;
    };
    let chain = config.chain.clone();
    let ping = OwnerPing::from_config(config);
    tools
        .into_iter()
        .map(|tool| -> Box<dyn Tool> {
            if is_actuator(tool.name()) {
                Box::new(AttestingTool::new(
                    tool,
                    chain.clone(),
                    key_dir.clone(),
                    ping.clone(),
                ))
            } else {
                tool
            }
        })
        .collect()
}

struct ChainHandle {
    chain: ChainConfig,
    key_dir: PathBuf,
}

impl ChainHandle {
    fn new(chain: ChainConfig, key_dir: PathBuf) -> Self {
        Self { chain, key_dir }
    }

    async fn open(&self) -> Result<DeviceChain, ToolResult> {
        if !self.chain.enabled {
            return Err(ToolResult::err(text("cli-chain-disabled")));
        }
        DeviceChain::open(&self.chain, &self.key_dir)
            .await
            .map_err(|error| ToolResult::err(failed(&error.to_string())))
    }
}

fn text(key: &str) -> String {
    crate::i18n::get_required_cli_string(key)
}

fn text_with(key: &str, args: &[(&str, &str)]) -> String {
    crate::i18n::get_required_cli_string_with_args(key, args)
}

fn failed(error: &str) -> String {
    text_with("cli-chain-failed", &[("error", error)])
}

fn missing(field: &str) -> ToolResult {
    ToolResult::err(text_with("cli-chain-missing", &[("field", field)]))
}

fn paused_word(paused: bool) -> String {
    text(if paused {
        "cli-chain-yes"
    } else {
        "cli-chain-no"
    })
}

fn identity_text(report: &BootReport) -> String {
    match report {
        BootReport::Unclaimed { device } => text_with(
            "cli-chain-unclaimed",
            &[
                ("address", &device.to_string()),
                ("claim", &unclaimed_claim_text(*device)),
            ],
        ),
        BootReport::Claimed {
            device,
            agent_id,
            owner,
            per_tx_cap,
            daily_cap,
            paused,
        } => text_with(
            "cli-chain-identity",
            &[
                ("agent", &agent_id.to_string()),
                ("owner", &owner.to_string()),
                ("device", &device.to_string()),
                ("per_tx", &format_mon(*per_tx_cap)),
                ("daily", &format_mon(*daily_cap)),
                ("paused", &paused_word(*paused)),
            ],
        ),
    }
}

pub struct MonadIdentityTool {
    handle: ChainHandle,
}

impl MonadIdentityTool {
    pub fn new(chain: ChainConfig, key_dir: PathBuf) -> Self {
        Self {
            handle: ChainHandle::new(chain, key_dir),
        }
    }
}

kinetic_api::tool_attribution!(
    MonadIdentityTool,
    kinetic_api::attribution::ToolKind::Plugin
);

#[async_trait]
impl Tool for MonadIdentityTool {
    fn name(&self) -> &str {
        "monad_identity"
    }

    fn description(&self) -> &str {
        "Read this device's Monad agent, owner, vault caps, and whether it still needs to be claimed."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({"type": "object", "properties": {}})
    }

    async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<ToolResult> {
        let session = match self.handle.open().await {
            Ok(session) => session,
            Err(result) => return Ok(result),
        };
        let report = match session.identity().await {
            Ok(report) => report,
            Err(error) => return Ok(ToolResult::err(failed(&error.to_string()))),
        };
        Ok(ToolResult::ok(identity_text(&report)))
    }
}

pub struct VaultStatusTool {
    handle: ChainHandle,
}

impl VaultStatusTool {
    pub fn new(chain: ChainConfig, key_dir: PathBuf) -> Self {
        Self {
            handle: ChainHandle::new(chain, key_dir),
        }
    }
}

kinetic_api::tool_attribution!(VaultStatusTool, kinetic_api::attribution::ToolKind::Plugin);

#[async_trait]
impl Tool for VaultStatusTool {
    fn name(&self) -> &str {
        "vault_status"
    }

    fn description(&self) -> &str {
        "Read the device vault balance, the rolling daily spend, the caps, and whether spending is paused."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({"type": "object", "properties": {}})
    }

    async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<ToolResult> {
        let session = match self.handle.open().await {
            Ok(session) => session,
            Err(result) => return Ok(result),
        };
        let report = match session.identity().await {
            Ok(report) => report,
            Err(error) => return Ok(ToolResult::err(failed(&error.to_string()))),
        };
        let BootReport::Claimed { agent_id, .. } = report else {
            return Ok(ToolResult::err(text("cli-chain-not-claimed")));
        };
        let status = match session.vault_status(agent_id).await {
            Ok(status) => status,
            Err(error) => return Ok(ToolResult::err(failed(&error.to_string()))),
        };
        Ok(ToolResult::ok(text_with(
            "cli-chain-vault",
            &[
                ("balance", &format_mon(status.balance)),
                ("spent", &format_mon(status.spent_today)),
                ("per_tx", &format_mon(status.per_tx_cap)),
                ("daily", &format_mon(status.daily_cap)),
                ("paused", &paused_word(status.paused)),
            ],
        )))
    }
}

pub struct VaultPayTool {
    handle: ChainHandle,
    ping: Option<OwnerPing>,
}

impl VaultPayTool {
    pub fn new(chain: ChainConfig, key_dir: PathBuf, ping: Option<OwnerPing>) -> Self {
        Self {
            handle: ChainHandle::new(chain, key_dir),
            ping,
        }
    }
}

kinetic_api::tool_attribution!(VaultPayTool, kinetic_api::attribution::ToolKind::Plugin);

#[async_trait]
impl Tool for VaultPayTool {
    fn name(&self) -> &str {
        "vault_pay"
    }

    fn description(&self) -> &str {
        "Pay a recipient from the device vault. The payment always goes through the vault caps and allowlist."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "recipient": {"type": "string", "description": "Address allowed to receive the payment"},
                "amount": {"type": "string", "description": "Amount in MON, such as 0.02"}
            },
            "required": ["recipient", "amount"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        let Some(recipient) = args.get("recipient").and_then(|value| value.as_str()) else {
            return Ok(missing("recipient"));
        };
        let Some(amount) = args.get("amount").and_then(|value| value.as_str()) else {
            return Ok(missing("amount"));
        };
        let Ok(recipient) = recipient.parse::<Address>() else {
            return Ok(ToolResult::err(text("cli-chain-bad-address")));
        };
        let Ok(amount) = parse_mon(amount) else {
            return Ok(ToolResult::err(text("cli-chain-bad-amount")));
        };
        if recipient == Address::ZERO {
            return Ok(ToolResult::err(text("cli-chain-bad-address")));
        }
        if amount.is_zero() {
            return Ok(ToolResult::err(text("cli-chain-bad-amount")));
        }
        let session = match self.handle.open().await {
            Ok(session) => session,
            Err(result) => return Ok(result),
        };
        let report = match session.identity().await {
            Ok(report) => report,
            Err(error) => return Ok(ToolResult::err(failed(&error.to_string()))),
        };
        let BootReport::Claimed { agent_id, .. } = report else {
            return Ok(ToolResult::err(text("cli-chain-not-claimed")));
        };
        match session.pay(agent_id, recipient, amount).await {
            Ok(tx) => Ok(ToolResult::ok(text_with(
                "cli-chain-paid",
                &[
                    ("amount", &format_mon(amount)),
                    ("recipient", &recipient.to_string()),
                    ("tx", &tx.to_string()),
                ],
            ))),
            Err(error) => {
                note_gas(&self.ping, &error, session.vault_address(), agent_id).await;
                Ok(ToolResult::err(failed(&error.to_string())))
            }
        }
    }
}

pub struct MonadAttestTool {
    handle: ChainHandle,
    ping: Option<OwnerPing>,
    records: RecordsConfig,
    records_path: PathBuf,
    description: String,
}

impl MonadAttestTool {
    pub fn new(
        chain: ChainConfig,
        key_dir: PathBuf,
        ping: Option<OwnerPing>,
        records: RecordsConfig,
        records_path: PathBuf,
    ) -> Self {
        let description = attest_description(&records);
        Self {
            handle: ChainHandle::new(chain, key_dir),
            ping,
            records,
            records_path,
            description,
        }
    }
}

fn attest_description(records: &RecordsConfig) -> String {
    if !records.recording() {
        return "Record a device action on Monad. The device key signs the action, the parameters, and the request URI.".to_string();
    }
    format!(
        "Record a completed operation on Monad. params must be one JSON object with these fields: {}. The host stores that canonical JSON as one line and signs those same bytes. The host does not add chat ids, wallets, or access codes.",
        crate::records::field_summary(records)
    )
}

kinetic_api::tool_attribution!(MonadAttestTool, kinetic_api::attribution::ToolKind::Plugin);

#[async_trait]
impl Tool for MonadAttestTool {
    fn name(&self) -> &str {
        "monad_attest"
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn parameters_schema(&self) -> serde_json::Value {
        let params = if self.records.recording() {
            format!(
                "JSON object with these fields: {}",
                crate::records::field_summary(&self.records)
            )
        } else {
            "Parameters the device signs".to_string()
        };
        json!({
            "type": "object",
            "properties": {
                "action": {"type": "string", "description": "Short name of the completed operation"},
                "params": {"type": "string", "description": params},
                "request_uri": {"type": "string", "description": "URI stored inside the signed proof"}
            },
            "required": ["action", "params", "request_uri"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        let Some(action) = args.get("action").and_then(|value| value.as_str()) else {
            return Ok(missing("action"));
        };
        let Some(params) = args.get("params").and_then(|value| value.as_str()) else {
            return Ok(missing("params"));
        };
        let Some(request_uri) = args.get("request_uri").and_then(|value| value.as_str()) else {
            return Ok(missing("request_uri"));
        };
        if action.is_empty() {
            return Ok(missing("action"));
        }
        if request_uri.is_empty() {
            return Ok(missing("request_uri"));
        }
        let prepared = match crate::records::prepare_operation(&self.records, params) {
            Ok(prepared) => prepared,
            Err(reason) => {
                return Ok(ToolResult::err(text_with(
                    "cli-chain-record-rejected",
                    &[("reason", &reason)],
                )));
            }
        };
        let attested_params = match &prepared {
            crate::records::PreparedOperation::Off => params,
            crate::records::PreparedOperation::Line(line) => line.as_str(),
        };
        let session = match self.handle.open().await {
            Ok(session) => session,
            Err(result) => return Ok(result),
        };
        let report = match session.identity().await {
            Ok(report) => report,
            Err(error) => return Ok(ToolResult::err(failed(&error.to_string()))),
        };
        let BootReport::Claimed { agent_id, .. } = report else {
            return Ok(ToolResult::err(text("cli-chain-not-claimed")));
        };
        match session
            .attest(agent_id, action, attested_params, request_uri)
            .await
        {
            Ok(attestation) => {
                let tx = attestation.transaction_hash.to_string();
                let request = attestation.request_hash.to_string();
                if let crate::records::PreparedOperation::Line(line) = &prepared {
                    if let Err(error) =
                        crate::records::append_operation_line(&self.records_path, line)
                    {
                        ::kinetic_log::record!(
                            WARN,
                            ::kinetic_log::Event::new(module_path!(), ::kinetic_log::Action::Write)
                                .with_outcome(::kinetic_log::EventOutcome::Failure)
                                .with_attrs(json!({"tx": &tx, "line": line})),
                            "operation record was attested but the local file was not written"
                        );
                        return Ok(ToolResult::err(text_with(
                            "cli-chain-record-unstored",
                            &[("tx", &tx), ("error", &error), ("line", line)],
                        )));
                    }
                }
                let key = match &prepared {
                    crate::records::PreparedOperation::Line(_) => "cli-chain-attested-stored",
                    crate::records::PreparedOperation::Off => "cli-chain-attested",
                };
                Ok(ToolResult::ok(text_with(
                    key,
                    &[("action", action), ("tx", &tx), ("request", &request)],
                )))
            }
            Err(error) => {
                note_gas(&self.ping, &error, session.vault_address(), agent_id).await;
                Ok(ToolResult::err(failed(&error.to_string())))
            }
        }
    }
}

pub struct AttestingTool {
    inner: Box<dyn Tool>,
    handle: ChainHandle,
    ping: Option<OwnerPing>,
}

impl AttestingTool {
    pub fn new(
        inner: Box<dyn Tool>,
        chain: ChainConfig,
        key_dir: PathBuf,
        ping: Option<OwnerPing>,
    ) -> Self {
        Self {
            inner,
            handle: ChainHandle::new(chain, key_dir),
            ping,
        }
    }
}

impl kinetic_api::attribution::Attributable for AttestingTool {
    fn role(&self) -> kinetic_api::attribution::Role {
        self.inner.role()
    }

    fn alias(&self) -> &str {
        self.inner.alias()
    }

    fn tool_provenance(&self) -> kinetic_api::attribution::ToolProvenance {
        self.inner.tool_provenance()
    }
}

#[async_trait]
impl Tool for AttestingTool {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn description(&self) -> &str {
        self.inner.description()
    }

    fn parameters_schema(&self) -> serde_json::Value {
        self.inner.parameters_schema()
    }

    fn spec(&self) -> ToolSpec {
        self.inner.spec()
    }

    fn requires_unrestricted_principal(&self) -> bool {
        self.inner.requires_unrestricted_principal()
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        let mut result = self.inner.execute(args.clone()).await?;
        if !result.success {
            return Ok(result);
        }
        let params = args.to_string();
        let request_uri = format!("kinetic://actuator/{}", self.inner.name());
        let note = match self.record(self.inner.name(), &params, &request_uri).await {
            Ok(tx) => text_with("cli-chain-actuator-attested", &[("tx", &tx)]),
            Err(error) => text_with("cli-chain-actuator-attest-failed", &[("error", &error)]),
        };
        result.output = append_note(result.output, &note);
        Ok(result)
    }
}

impl AttestingTool {
    async fn record(
        &self,
        action: &str,
        params: &str,
        request_uri: &str,
    ) -> Result<String, String> {
        let session = self
            .handle
            .open()
            .await
            .map_err(|result| result.error.unwrap_or_else(|| failed("open")))?;
        let report = session
            .identity()
            .await
            .map_err(|error| failed(&error.to_string()))?;
        let BootReport::Claimed { agent_id, .. } = report else {
            return Err(text("cli-chain-not-claimed"));
        };
        let attestation = match session.attest(agent_id, action, params, request_uri).await {
            Ok(attestation) => attestation,
            Err(error) => {
                note_gas(&self.ping, &error, session.vault_address(), agent_id).await;
                return Err(failed(&error.to_string()));
            }
        };
        Ok(attestation.transaction_hash.to_string())
    }
}

async fn note_gas(
    ping: &Option<OwnerPing>,
    error: &anyhow::Error,
    vault: Address,
    agent_id: kinetic_chain::U256,
) {
    if !kinetic_chain::is_vault_gas_short(error) {
        return;
    }
    if let Some(ping) = ping {
        ping.tell_vault_needs_funds(&vault.to_string(), &agent_id.to_string())
            .await;
    }
}

fn append_note(output: ToolOutput, note: &str) -> ToolOutput {
    let text = format!("{output}\n{note}");
    match output.data().cloned() {
        Some(data) => ToolOutput::json_with_text(data, text),
        None => ToolOutput::text(text),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kinetic_api::attribution::ToolKind;

    kinetic_api::tool_attribution!(EchoTool, ToolKind::Plugin);

    struct EchoTool {
        name: &'static str,
        ok: bool,
    }

    #[async_trait]
    impl Tool for EchoTool {
        fn name(&self) -> &str {
            self.name
        }

        fn description(&self) -> &str {
            "test actuator"
        }

        fn parameters_schema(&self) -> serde_json::Value {
            json!({})
        }

        async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<ToolResult> {
            if self.ok {
                Ok(ToolResult::ok("pin set"))
            } else {
                Ok(ToolResult::err("pin failed"))
            }
        }
    }

    #[test]
    fn pin_writes_are_the_actions_that_get_recorded() {
        assert!(is_actuator("gpio_write"));
        assert!(is_actuator("gpio_rpi_write"));
        assert!(!is_actuator("gpio_read"));
    }

    #[test]
    fn an_unclaimed_device_names_its_address() {
        let device: Address = "0x9Df116134f653ec363F64667D39edea4B3649C16"
            .parse()
            .expect("address");
        let text = identity_text(&BootReport::Unclaimed { device });
        assert!(text.contains("0x9Df116134f653ec363F64667D39edea4B3649C16"));
        assert!(text.contains("kinetic:"));
    }

    #[tokio::test]
    async fn vault_pay_rejects_a_bad_amount_before_opening_the_chain() {
        let tool = VaultPayTool::new(
            ChainConfig::default(),
            PathBuf::from("/tmp/kinetic-chain-test"),
            None,
        );
        let result = tool
            .execute(json!({"recipient": "0x0000000000000000000000000000000000000001", "amount": "nope"}))
            .await
            .expect("tool result");
        assert!(!result.success);
        assert_eq!(
            result.error.as_deref(),
            Some(text("cli-chain-bad-amount").as_str())
        );
    }

    #[tokio::test]
    async fn a_record_outside_the_schema_never_opens_the_chain() {
        let dir = tempfile::tempdir().expect("temp");
        let tool = MonadAttestTool::new(
            ChainConfig {
                enabled: true,
                ..ChainConfig::default()
            },
            dir.path().to_path_buf(),
            None,
            RecordsConfig {
                fields: vec![kinetic_config::schema::OperationRecordField {
                    name: "subject".to_string(),
                    kind: kinetic_config::schema::OperationFieldKind::String,
                    required: true,
                }],
            },
            dir.path().join("records").join("operations.jsonl"),
        );
        let result = tool
            .execute(json!({
                "action": "observe",
                "params": "{\"chat_id\":\"709\"}",
                "request_uri": "kinetic://operation/observe"
            }))
            .await
            .expect("tool result");
        assert!(!result.success);
        let error = result.error.expect("error");
        assert!(error.contains("chat_id"), "{error}");
        assert!(!dir.path().join("records").join("operations.jsonl").exists());
    }

    #[tokio::test]
    async fn a_failed_pin_write_is_not_recorded() {
        let tool = AttestingTool::new(
            Box::new(EchoTool {
                name: "gpio_write",
                ok: false,
            }),
            ChainConfig {
                enabled: true,
                ..ChainConfig::default()
            },
            PathBuf::from("/tmp/kinetic-chain-test"),
            None,
        );
        let result = tool.execute(json!({"pin": 13})).await.expect("result");
        assert!(!result.success);
        assert!(result.output.is_empty());
    }
}
