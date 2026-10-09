//! `kinetic plugin create` writes a starter package a builder can replace.
//!
//! The directory holds several tools, one procedure, one skill, and an example
//! operation schema. State is not a file in the package. The starter tools
//! read and write one host key.

use std::fs;
use std::path::{Path, PathBuf};

const WIT_FILES: &[(&str, &str)] = &[
    ("channel.wit", include_str!("../wit/v0/channel.wit")),
    ("config.wit", include_str!("../wit/v0/config.wit")),
    ("inbound.wit", include_str!("../wit/v0/inbound.wit")),
    ("logging.wit", include_str!("../wit/v0/logging.wit")),
    ("memory.wit", include_str!("../wit/v0/memory.wit")),
    ("plugin-info.wit", include_str!("../wit/v0/plugin-info.wit")),
    ("secrets.wit", include_str!("../wit/v0/secrets.wit")),
    ("sockets.wit", include_str!("../wit/v0/sockets.wit")),
    ("state.wit", include_str!("../wit/v0/state.wit")),
    ("tool.wit", include_str!("../wit/v0/tool.wit")),
    ("types.wit", include_str!("../wit/v0/types.wit")),
    ("websocket.wit", include_str!("../wit/v0/websocket.wit")),
];

/// Why `kinetic plugin create` stopped.
#[derive(Debug, thiserror::Error)]
pub enum CreateError {
    /// The directory name is not a package name.
    #[error("bad plugin name {0}")]
    BadName(String),
    /// The path is already there.
    #[error("{} already exists", .0.display())]
    Exists(PathBuf),
    /// A file could not be written.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Create `path` and write the starter package. The directory name is the
/// plugin name.
pub fn create_package(path: &Path) -> Result<PathBuf, CreateError> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    if !package_name_ok(name) {
        return Err(CreateError::BadName(name.to_string()));
    }
    if path.exists() {
        return Err(CreateError::Exists(path.to_path_buf()));
    }
    fs::create_dir_all(path)?;
    if let Err(error) = write_tree(path, name) {
        let _ = fs::remove_dir_all(path);
        return Err(error);
    }
    Ok(path.to_path_buf())
}

fn package_name_ok(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) if first.is_ascii_lowercase() || first == '_' => {}
        _ => return false,
    }
    name.len() <= 64 && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

fn write_tree(root: &Path, name: &str) -> Result<(), CreateError> {
    write(root, "Cargo.toml", &fill(CARGO_TOML, name))?;
    write(root, "manifest.toml", &fill(MANIFEST_TOML, name))?;
    write(root, ".gitignore", GITIGNORE)?;
    write(root, "src/lib.rs", &fill(LIB_RS, name))?;
    write(root, "sop/SOP.toml", &fill(SOP_TOML, name))?;
    write(root, "sop/SOP.md", SOP_MD)?;
    write(
        root,
        &format!("skills/{name}/SKILL.md"),
        &fill(SKILL_MD, name),
    )?;
    write(root, "records.example.toml", RECORDS_EXAMPLE)?;
    for (file, contents) in WIT_FILES {
        write(root, &format!("wit/v0/{file}"), contents)?;
    }
    Ok(())
}

fn write(root: &Path, relative: &str, contents: &str) -> Result<(), CreateError> {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, contents)?;
    Ok(())
}

fn fill(template: &str, name: &str) -> String {
    template.replace("__PACKAGE__", name)
}

const CARGO_TOML: &str = r#"# Replace the tools in src/lib.rs. `kinetic build` compiles this crate to plugin.wasm.
# The host stores plugin state. There is no state file in this directory.
# This package is its own workspace, so a parent Cargo workspace does not absorb it.

[workspace]

[package]
name = "__PACKAGE__"
version = "0.1.0"
edition = "2021"
publish = false

[lib]
crate-type = ["cdylib"]

[target.'cfg(target_arch = "wasm32")'.dependencies]
serde_json = "1.0"
wit-bindgen = "0.51"
"#;

const MANIFEST_TOML: &str = r#"# Replace this starter.
# capabilities lists what the package provides. permissions lists what the
# tools may use. An egress host here is not a grant: the operator allows it
# at install.
# Delete skills/__PACKAGE__ and remove "skill" from capabilities when the
# model does not need extra guidance.
# State is not a file in this directory. The starter tools share one host key.

name = "__PACKAGE__"
version = "0.1.0"
description = "Replace this description."
wasm_path = "plugin.wasm"
capabilities = ["tool", "skill"]
permissions = ["state_read", "state_write"]

# Uncomment when a tool calls the network. Add config_read only together with
# config_schema. A property with x-secret = true is served as a secret.
#
# [egress]
# hosts = ["api.example.com"]
#
# [config_schema]
# type = "object"
# additionalProperties = false
#
# [config_schema.properties.api_token]
# type = "string"
# x-secret = true
"#;

const GITIGNORE: &str = "/target\n/plugin.wasm\n";

const LIB_RS: &str = r##"// Replace ping and note. Both tools live in this one package and share its
// config and its host state. State is not a file you edit.
// Do not put chat ids, wallets, or access codes in tool results or in state.

#[cfg(not(target_family = "wasm"))]
compile_error!("build this package for wasm32-wasip2");

#[cfg(target_family = "wasm")]
mod component {
    wit_bindgen::generate!({
        path: "wit/v0",
        world: "tools-plugin",
        features: ["plugins-wit-v0"],
    });

    use exports::kinetic::plugin::plugin_info::Guest as PluginInfo;
    use exports::kinetic::plugin::toolbox::{Guest, ToolInfo, ToolResult};
    use kinetic::plugin::state::{get as state_get, put as state_put};

    const NOTE_KEY: &str = "note";

    struct Tools;

    impl PluginInfo for Tools {
        fn plugin_name() -> String {
            "__PACKAGE__".to_string()
        }

        fn plugin_version() -> String {
            "0.1.0".to_string()
        }
    }

    impl Guest for Tools {
        fn list_tools() -> Vec<ToolInfo> {
            vec![
                ToolInfo {
                    name: "ping".to_string(),
                    description: "Return pong. Replace this tool.".to_string(),
                    parameters_schema: r#"{"type":"object","properties":{}}"#.to_string(),
                },
                ToolInfo {
                    name: "note".to_string(),
                    description: "Store text in host state and return the previous value. Replace this tool.".to_string(),
                    parameters_schema: r#"{"type":"object","properties":{"text":{"type":"string"}},"required":["text"]}"#.to_string(),
                },
            ]
        }

        fn execute(name: String, args: String) -> Result<ToolResult, String> {
            let output = match name.as_str() {
                "ping" => "pong".to_string(),
                "note" => remember(&args)?,
                other => return Err(format!("unknown tool {other}")),
            };
            Ok(ToolResult {
                success: true,
                output,
                error: None,
            })
        }
    }

    fn remember(args: &str) -> Result<String, String> {
        let parsed: serde_json::Value = serde_json::from_str(args)
            .map_err(|error| format!("args are not valid JSON: {error}"))?;
        let text = parsed
            .get("text")
            .and_then(|value| value.as_str())
            .ok_or_else(|| "`text` must be a string".to_string())?;
        let current = state_get(NOTE_KEY).map_err(|_| "state read failed".to_string())?;
        let previous = current
            .as_ref()
            .map(|entry| String::from_utf8_lossy(&entry.value).into_owned())
            .unwrap_or_else(|| "empty".to_string());
        let expected = current.as_ref().map(|entry| entry.revision);
        state_put(NOTE_KEY, text.as_bytes(), expected)
            .map_err(|_| "state write failed".to_string())?;
        Ok(previous)
    }

    export!(Tools);
}
"##;

const SOP_TOML: &str = r#"# Replace this procedure. It calls tools by name. The tool code stays in src/lib.rs.

[sop]
name = "__PACKAGE__"
description = "Replace this. The starter step calls ping."
version = "0.1.0"

[[triggers]]
type = "manual"
"#;

const SOP_MD: &str = r#"<!-- Replace this procedure. Delete the starter step and write your own. -->

## Steps

1. **Ping** - Call the starter tool. Replace this step.
   - tools: ping
   - call: {"tool":"ping","args":{}}
"#;

const SKILL_MD: &str = r#"---
name: __PACKAGE__
description: Replace this skill. Delete this directory and remove skill from the manifest when the model does not need extra guidance.
---

Replace this body. A skill is guidance the model can read. The SOP is the procedure that calls the tools.
"#;

const RECORDS_EXAMPLE: &str = r#"# kinetic plugin install writes these fields into the device config.
# Installing a plugin again replaces them.
# The host checks these fields, signs that JSON, and stores the line.
# Leave the list empty and recording stays off.
# Do not add chat ids, wallets, or access codes.

[[records.fields]]
name = "subject"
type = "string"
required = true
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn create_writes_a_starter_named_after_the_directory() {
        let root = tempfile::tempdir().expect("temp");
        let path = root.path().join("sample_pkg");
        create_package(&path).expect("create");

        let manifest = fs::read_to_string(path.join("manifest.toml")).expect("manifest");
        assert!(manifest.contains("name = \"sample_pkg\""));
        assert!(manifest.contains("capabilities = [\"tool\", \"skill\"]"));
        assert!(manifest.contains("state_read"));

        let source = fs::read_to_string(path.join("src/lib.rs")).expect("lib");
        assert!(
            source.contains("fn plugin_name() -> String {\n            \"sample_pkg\".to_string()")
        );
        assert!(source.contains("state_put"));
        assert!(source.contains("\"ping\""));
        assert!(source.contains("\"note\""));

        let sop = fs::read_to_string(path.join("sop/SOP.toml")).expect("sop");
        assert!(sop.contains("name = \"sample_pkg\""));
        let steps = fs::read_to_string(path.join("sop/SOP.md")).expect("steps");
        assert!(steps.contains("\"tool\":\"ping\""));

        let skill = fs::read_to_string(path.join("skills/sample_pkg/SKILL.md")).expect("skill");
        assert!(skill.starts_with("---\nname: sample_pkg\n"));
        assert!(skill.contains("description:"));

        let records = fs::read_to_string(path.join("records.example.toml")).expect("records");
        assert!(records.contains("name = \"subject\""));
        assert!(records.contains("type = \"string\""));

        let wit = fs::read_to_string(path.join("wit/v0/tool.wit")).expect("wit");
        assert!(wit.contains("interface toolbox"));
        assert!(path.join(".gitignore").is_file());
    }

    #[test]
    fn a_second_create_and_a_bad_name_are_refused() {
        let root = tempfile::tempdir().expect("temp");
        let path = root.path().join("sample_pkg");
        create_package(&path).expect("create");
        assert!(matches!(create_package(&path), Err(CreateError::Exists(_))));
        assert!(matches!(
            create_package(&root.path().join("1bad")),
            Err(CreateError::BadName(_))
        ));
        assert!(matches!(
            create_package(&root.path().join("HasCaps")),
            Err(CreateError::BadName(_))
        ));
    }
}
