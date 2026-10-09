//! Multi-tool plugins: the `tools-plugin` world.
//!
//! A component that exports `toolbox` lists every tool at load time. Each name
//! becomes its own host tool and shares this package's config and state. A
//! component that only exports `tool` never enters this module.

use crate::PluginCapability;
use crate::component::bindings::tools::ToolsPlugin;
use crate::component::bindings::tools::exports::kinetic::plugin::toolbox::ToolResult as WitToolResult;
use crate::component::{
    PluginState, PluginStoreSpec, WarmPluginState, call_plugin, call_store, call_tool_execute,
    engine, load_component, wt, wt_instantiate,
};
use crate::host::AdmittedComponent;
use crate::instance::PluginInstanceScope;
use crate::runtime::{ToolMetadata, inject_config};
use crate::services::PluginHostServices;
use anyhow::{Context, Result};
use kinetic_api::tool::ToolResult;
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::OnceLock;
use tokio::sync::Mutex;
use wasmtime::Store;
use wasmtime::component::{Component, Linker};

const TOOLBOX_EXPORTS: &[&str] = &["kinetic:plugin/toolbox@0.1.0", "kinetic:plugin/toolbox"];

/// True when this component exports the multi-tool interface.
pub(crate) fn component_exports_toolbox(component: &Component) -> bool {
    TOOLBOX_EXPORTS
        .iter()
        .any(|name| component.get_export_index(None, *name).is_some())
}

/// A warm multi-tool guest. Fresh store per host call, same as a single tool.
pub(crate) struct Guest {
    state: Arc<Mutex<WarmPluginState<ToolsPlugin>>>,
}

fn base_linker(imports: crate::component::OptionalImports) -> Result<Linker<PluginState>> {
    let mut linker = Linker::new(engine());
    crate::component::add_wasi(&mut linker)?;
    if imports.http {
        crate::component::add_wasi_http(&mut linker)?;
    }
    let mut options = crate::component::bindings::tools::LinkOptions::default();
    options.plugins_wit_v0(true);
    options.plugins_wit_v0_sockets(imports.sockets);
    options.plugins_wit_v0_websocket(imports.websocket);
    wt(
        ToolsPlugin::add_to_linker::<_, wasmtime::component::HasSelf<_>>(
            &mut linker,
            &options,
            |s| s,
        ),
        "failed to add tools plugin imports to linker",
    )?;
    Ok(linker)
}

fn tools_linker(imports: crate::component::OptionalImports) -> Result<Arc<Linker<PluginState>>> {
    type Linkers = std::sync::Mutex<
        std::collections::HashMap<crate::component::OptionalImports, Arc<Linker<PluginState>>>,
    >;
    static LINKERS: OnceLock<Linkers> = OnceLock::new();
    let mut linkers = LINKERS
        .get_or_init(Linkers::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(linker) = linkers.get(&imports) {
        return Ok(Arc::clone(linker));
    }
    let linker = Arc::new(base_linker(imports)?);
    linkers.insert(imports, Arc::clone(&linker));
    Ok(linker)
}

/// Instantiate a multi-tool component. Does not call a guest export.
pub(crate) async fn create_plugin(
    component: &AdmittedComponent,
    scope: &PluginInstanceScope,
    services: &PluginHostServices,
    limits: crate::component::PluginLimits,
    egress: Option<crate::egress::EgressHostService>,
) -> Result<Guest> {
    scope.require_capability(PluginCapability::Tool)?;
    let component = load_component(component)?;
    let mut store = crate::component::new_store(
        PluginStoreSpec::new(scope.clone(), services.clone(), limits)
            .with_granted_http()
            .with_egress_policy(egress),
    );
    let imports = crate::component::OptionalImports::for_store(store.data());
    let linker = tools_linker(imports)?;
    crate::component::ensure_imports_coherent(&store, imports)?;
    let bindings = call_store!(store, async move |store: &mut Store<PluginState>| {
        wt_instantiate(
            ToolsPlugin::instantiate_async(store, &component, &linker).await,
            "failed to instantiate tools plugin",
        )
    })?;
    Ok(Guest {
        state: Arc::new(Mutex::new(Some((store, bindings)))),
    })
}

/// Read `list-tools` and refuse a list the host cannot register.
pub(crate) async fn list_tools(guest: &mut Guest) -> Result<Vec<ToolMetadata>> {
    let listed = call_plugin!(
        guest,
        async move |store: &mut Store<PluginState>, bindings: &mut ToolsPlugin| {
            wt(
                bindings
                    .kinetic_plugin_toolbox()
                    .call_list_tools(store)
                    .await,
                "toolbox.list-tools failed",
            )
        }
    )?;
    let mut tools = Vec::with_capacity(listed.len());
    for tool in listed {
        let parameters_schema =
            serde_json::from_str(&tool.parameters_schema).with_context(|| {
                format!(
                    "toolbox tool {} parameters-schema is not valid JSON",
                    tool.name
                )
            })?;
        tools.push(ToolMetadata {
            name: tool.name,
            description: tool.description,
            parameters_schema,
        });
    }
    check_listed_tools(&tools)?;
    Ok(tools)
}

/// Run one named tool, injecting the package's public config.
pub(crate) async fn call_execute(
    guest: &mut Guest,
    name: &str,
    args_json: &[u8],
) -> Result<ToolResult> {
    let name = name.to_string();
    call_tool_execute!(
        guest,
        async move |store: &mut Store<PluginState>, bindings: &mut ToolsPlugin| {
            let config = store.data_mut().public_config()?;
            let input = inject_config(args_json, &config)?;
            let result = wt(
                bindings
                    .kinetic_plugin_toolbox()
                    .call_execute(store, &name, &input)
                    .await,
                "toolbox.execute trapped",
            )?
            .map_err(|e| anyhow::Error::msg(format!("plugin execute returned error: {e}")))?;
            Ok(into_tool_result(result))
        }
    )
}

fn into_tool_result(result: WitToolResult) -> ToolResult {
    ToolResult {
        success: result.success,
        output: result.output.into(),
        error: result.error,
    }
}

/// Refuse an empty list, a blank or illegal name, or a duplicate.
pub(crate) fn check_listed_tools(tools: &[ToolMetadata]) -> Result<()> {
    if tools.is_empty() {
        return Err(anyhow::Error::msg("toolbox list-tools returned no tools"));
    }
    let mut seen = HashSet::new();
    for tool in tools {
        if !tool_name_ok(&tool.name) {
            return Err(anyhow::Error::msg(format!(
                "toolbox tool name {:?} must be letters, digits, and underscores, and must not start with a digit",
                tool.name
            )));
        }
        if !seen.insert(tool.name.as_str()) {
            return Err(anyhow::Error::msg(format!(
                "toolbox tool {} is listed more than once",
                tool.name
            )));
        }
    }
    Ok(())
}

fn tool_name_ok(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphabetic() || first == '_' => {}
        _ => return false,
    }
    name.len() <= 64 && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn meta(name: &str) -> ToolMetadata {
        ToolMetadata {
            name: name.to_string(),
            description: "d".to_string(),
            parameters_schema: json!({"type": "object"}),
        }
    }

    #[test]
    fn an_empty_tool_list_is_refused() {
        let error = check_listed_tools(&[]).expect_err("empty");
        assert!(error.to_string().contains("no tools"), "{error}");
    }

    #[test]
    fn a_duplicate_or_illegal_name_is_refused() {
        let duplicate = check_listed_tools(&[meta("ping"), meta("ping")]).expect_err("dup");
        assert!(
            duplicate.to_string().contains("more than once"),
            "{duplicate}"
        );
        let illegal = check_listed_tools(&[meta("1ping")]).expect_err("digit");
        assert!(illegal.to_string().contains("1ping"), "{illegal}");
    }

    #[test]
    fn two_distinct_names_are_accepted() {
        check_listed_tools(&[meta("ping"), meta("echo")]).expect("names");
    }
}
