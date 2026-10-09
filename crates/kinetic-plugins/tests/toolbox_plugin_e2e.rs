//! One component exports two tools, and the host registers and runs both.

#![cfg(feature = "plugins-wasm-cranelift")]

mod support;

use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

use kinetic_api::tool::Tool;
use kinetic_plugins::component::PluginLimits;
use kinetic_plugins::config::{PluginConfigResolver, resolve_plugin_config};
use kinetic_plugins::instance::PluginInstanceScope;
use kinetic_plugins::services::PluginHostServices;
use kinetic_plugins::wasm_tool::WasmTool;
use kinetic_plugins::{PluginCapability, PluginManifest};
use serde_json::json;

use support::{admit_fixture, state_service};

fn fixture() -> PathBuf {
    static FIXTURE: OnceLock<PathBuf> = OnceLock::new();
    FIXTURE
        .get_or_init(|| {
            let fixture_dir =
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tools-fixture");
            let target_dir =
                PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("tools-plugin-fixture");
            let status = Command::new(env!("CARGO"))
                .current_dir(&fixture_dir)
                .args([
                    "build",
                    "--locked",
                    "--quiet",
                    "--package",
                    "kinetic-tools-plugin-fixture",
                    "--target",
                    "wasm32-wasip2",
                    "--target-dir",
                ])
                .arg(&target_dir)
                .status()
                .expect("run Cargo for the tools component fixture");
            assert!(
                status.success(),
                "tools fixture must build; install the wasm32-wasip2 target"
            );
            let wasm = target_dir.join("wasm32-wasip2/debug/kinetic_tools_plugin_fixture.wasm");
            assert!(wasm.is_file(), "tools fixture WASM was not produced");
            wasm
        })
        .clone()
}

fn manifest() -> PluginManifest {
    PluginManifest {
        name: "tools-fixture".to_string(),
        version: "0.0.0".to_string(),
        description: None,
        author: None,
        wasm_path: Some("tools-fixture.wasm".to_string()),
        wasm_sha256: None,
        capabilities: vec![PluginCapability::Tool],
        provides: None,
        permissions: Vec::new(),
        config_schema: None,
        signature: None,
        publisher_key: None,
        egress: Default::default(),
    }
}

#[tokio::test]
async fn one_component_registers_ping_and_echo() {
    let manifest = manifest();
    let component = admit_fixture(&fixture(), &manifest);
    let scope = PluginInstanceScope::from_manifest(&manifest, PluginCapability::Tool, "main", [])
        .expect("scope");
    let manifest_for_resolve = manifest.clone();
    let services = PluginHostServices::new(
        PluginConfigResolver::new(move |scope| {
            resolve_plugin_config(&manifest_for_resolve, scope, None)
        }),
        state_service(),
    );
    let limits = PluginLimits {
        call_fuel: 1_000_000_000,
        max_memory_bytes: 256 * 1024 * 1024,
        max_table_elements: 100_000,
        max_instances: 64,
        call_timeout: std::time::Duration::from_secs(30),
    };
    let mut tools =
        WasmTool::from_component(component, scope, services, limits, None).expect("list tools");
    tools.sort_by(|left, right| left.name().cmp(right.name()));
    let names: Vec<_> = tools.iter().map(|tool| tool.name().to_string()).collect();
    assert_eq!(names, vec!["echo".to_string(), "ping".to_string()]);

    let ping = tools
        .iter()
        .find(|tool| tool.name() == "ping")
        .expect("ping");
    let ping_result = ping.execute(json!({})).await.expect("ping call");
    assert!(ping_result.success, "{ping_result:?}");
    assert_eq!(ping_result.output.as_str(), "pong");

    let echo = tools
        .iter()
        .find(|tool| tool.name() == "echo")
        .expect("echo");
    let echo_result = echo
        .execute(json!({"text": "soil"}))
        .await
        .expect("echo call");
    assert!(echo_result.success, "{echo_result:?}");
    assert_eq!(echo_result.output.as_str(), "soil");
}
