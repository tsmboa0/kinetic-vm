//! Two-tool component used by the toolbox host test.
//!
//! `ping` ignores its arguments and returns `pong`. `echo` returns the `text`
//! argument. Both are registered from one `list-tools` export.

#[cfg(target_family = "wasm")]
mod component {
    wit_bindgen::generate!({
        path: "../../../../../wit/v0",
        world: "tools-plugin",
        features: ["plugins-wit-v0"],
    });

    use exports::kinetic::plugin::plugin_info::Guest as PluginInfo;
    use exports::kinetic::plugin::toolbox::{Guest, ToolInfo, ToolResult};

    struct Tools;

    impl PluginInfo for Tools {
        fn plugin_name() -> String {
            "tools-fixture".to_string()
        }

        fn plugin_version() -> String {
            "0.0.0".to_string()
        }
    }

    impl Guest for Tools {
        fn list_tools() -> Vec<ToolInfo> {
            vec![
                ToolInfo {
                    name: "ping".to_string(),
                    description: "Return pong.".to_string(),
                    parameters_schema: r#"{"type":"object","properties":{}}"#.to_string(),
                },
                ToolInfo {
                    name: "echo".to_string(),
                    description: "Return the text argument.".to_string(),
                    parameters_schema:
                        r#"{"type":"object","properties":{"text":{"type":"string"}},"required":["text"]}"#
                            .to_string(),
                },
            ]
        }

        fn execute(name: String, args: String) -> Result<ToolResult, String> {
            let output = match name.as_str() {
                "ping" => "pong".to_string(),
                "echo" => {
                    let parsed: serde_json::Value = serde_json::from_str(&args)
                        .map_err(|error| format!("args are not valid JSON: {error}"))?;
                    parsed
                        .get("text")
                        .and_then(|value| value.as_str())
                        .unwrap_or("")
                        .to_string()
                }
                other => return Err(format!("unknown tool {other}")),
            };
            Ok(ToolResult {
                success: true,
                output,
                error: None,
            })
        }
    }

    export!(Tools);
}
