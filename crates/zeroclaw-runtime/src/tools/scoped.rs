//! `ScopedToolRegistry` - the one gated seam that mints the per-agent tool set.
//!
//! Assembly applies peripherals, built-in policy, ACP memory stripping, MCP
//! scope and policy, capability tools, pinned resources, and skills in that
//! order. This is the only production construction path: the engine's
//! turn-entry carriers (`ResolvedIo` / `ResolvedAgentExecution`), `Agent`, and
//! the channel runtime context all store the sealed type, so an unscoped
//! registry cannot reach a turn without going through [`ScopedToolRegistry::assemble`].

use std::collections::HashSet;
use std::sync::Arc;

use zeroclaw_api::runtime_traits::RuntimeAdapter;
use zeroclaw_config::policy::SecurityPolicy;
use zeroclaw_config::schema::Config;

use crate::agent::loop_::{
    append_pinned_mcp_section, apply_policy_tool_filter, load_peripheral_tools,
};
use crate::skills::Skill;
use crate::tools::{
    self, ActivatedToolSet, AllToolsResult, DelegateParentToolsHandle, PerToolChannelHandle, Tool,
    register_skill_tools_with_context_and_runtime_optional_nat64,
};

/// A per-agent tool registry that has been scoped and gated. The inner field is
/// private and production code can only mint one through
/// [`ScopedToolRegistry::assemble`]. The engine's turn-entry carriers
/// (`ResolvedIo` / `ResolvedAgentExecution`), `Agent`, and the channel runtime
/// context store this type directly, so handing the engine an unfiltered
/// registry is a compile error, not a review-checklist item. Read sites are
/// unchanged: the registry [`std::ops::Deref`]s to the same `[Box<dyn Tool>]`
/// slice the raw `Vec` used to expose.
pub struct ScopedToolRegistry(Vec<Box<dyn Tool>>);

impl std::ops::Deref for ScopedToolRegistry {
    type Target = [Box<dyn Tool>];
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl ScopedToolRegistry {
    /// Consume the assembled registry into the owned `Vec` for non-turn
    /// consumers such as the gateway's listing-only registry. Turn carriers
    /// accept only the sealed type.
    pub fn into_inner(self) -> Vec<Box<dyn Tool>> {
        self.0
    }

    /// Narrow an ALREADY-sealed registry in place. A passthrough to
    /// [`Vec::retain`] on the private inner vector. This is a mutator on an
    /// existing sealed registry (it removes tools, never adds), so it cannot
    /// mint a scope from raw tools and does not weaken the seal - the only way
    /// to obtain a `ScopedToolRegistry` to call this on is still
    /// [`Self::assemble`] (or the test-only constructor).
    pub(crate) fn retain(&mut self, f: impl FnMut(&Box<dyn Tool>) -> bool) {
        self.0.retain(f);
    }

    /// Rebind the memory-backed tools of an ALREADY-sealed registry to a new
    /// backend handle. Session memory follows its owner: when a session is
    /// pinned to a principal's private plane the memory tools — which each
    /// captured a clone of the shared handle at assembly — must be re-pointed
    /// at the routed handle, or an owned session would keep issuing shared-plane
    /// recall/store/export/delete while the agent reports its memory as private.
    ///
    /// This replaces the SAME named memory tools in place with fresh instances
    /// over `memory`; it never introduces a new tool name, so the surface the
    /// seal admitted is unchanged. Any memory tool already withdrawn by policy
    /// narrowing stays withdrawn (only tools currently present are rebound).
    pub(crate) fn rebind_memory_tools(
        &mut self,
        memory: Arc<dyn zeroclaw_memory::Memory>,
        security: Arc<SecurityPolicy>,
    ) {
        use zeroclaw_tools::{
            memory_export::MemoryExportTool, memory_forget::MemoryForgetTool,
            memory_purge::MemoryPurgeTool, memory_recall::MemoryRecallTool,
            memory_store::MemoryStoreTool,
        };
        for tool in self.0.iter_mut() {
            let replacement: Option<Box<dyn Tool>> = match tool.name() {
                "memory_store" => Some(Box::new(MemoryStoreTool::new(
                    Arc::clone(&memory),
                    Arc::clone(&security),
                ))),
                "memory_recall" => Some(Box::new(MemoryRecallTool::new(Arc::clone(&memory)))),
                "memory_forget" => Some(Box::new(MemoryForgetTool::new(
                    Arc::clone(&memory),
                    Arc::clone(&security),
                ))),
                "memory_export" => Some(Box::new(MemoryExportTool::new(Arc::clone(&memory)))),
                "memory_purge" => Some(Box::new(MemoryPurgeTool::new(
                    Arc::clone(&memory),
                    Arc::clone(&security),
                ))),
                _ => None,
            };
            if let Some(replacement) = replacement {
                *tool = replacement;
            }
        }
    }

    /// Test-only constructor that mints a registry directly from raw tools,
    /// bypassing [`Self::assemble`]. Gated to `test` (this crate's own unit
    /// tests) OR the `test-util` feature (so OTHER crates' test builds -
    /// notably `zeroclaw-channels`, whose `ChannelRuntimeContext` fixtures need
    /// a sealed registry - can construct one). The `test-util` feature is never
    /// enabled by a production dependency edge, so this raw mint does not exist
    /// in shipped builds and the seal holds where it matters.
    #[cfg(any(test, feature = "test-util"))]
    pub fn from_raw_for_test(tools: Vec<Box<dyn Tool>>) -> Self {
        Self(tools)
    }
}

/// Inputs to [`ScopedToolRegistry::assemble`]. The eager built-ins arrive already
/// built (`built`); `assemble` does the policy-bearing steps the sites used to repeat.
pub struct ScopedAssembly<'a> {
    pub config: &'a Config,
    pub agent_alias: &'a str,
    pub security: &'a Arc<SecurityPolicy>,
    /// Eager built-in tools + the channel/delegate handle bundle, consumed here.
    pub built: AllToolsResult,
    /// Skills loaded by the caller's (single) loader; registered under the same gate.
    pub skills: &'a [Skill],
    pub runtime: Arc<dyn RuntimeAdapter>,
    /// Documented divergence: a per-run caller allowlist. It only NARROWS, and is
    /// threaded into BOTH the built-in filter and the MCP tool-access policy. `None`
    /// on every path except `run`.
    pub caller_allowed: Option<&'a [String]>,
    /// Documented divergence: loading peripherals physically connects hardware (the
    /// daemon's loader opens serial ports, exclusively for real devices). Listing-only
    /// surfaces (the gateway's `/api/tools` registries) MUST pass `false` so they never
    /// hold devices the live turn paths need; execution surfaces pass `true`.
    pub connect_peripherals: bool,
    /// Documented divergence: ACP excludes persistent memory tools.
    pub exclude_memory: bool,
    /// `deliver_file` hands the client a typed file attachment that only an
    /// ACP-capable turn actually transports (the model history, WS, and RPC
    /// paths all drop the artifact). Every non-ACP assembly passes `false` so
    /// the tool is absent rather than returning a false success on a channel
    /// that cannot deliver it. Only the ACP turn path passes `true`.
    pub acp_delivery: bool,
    pub emit_assembly_logs: bool,
}

/// Output of [`ScopedToolRegistry::assemble`]: the scoped registry plus the
/// side-channel handles + the deferred-MCP prompt section the callers thread on.
pub struct ScopedAssembled {
    pub registry: ScopedToolRegistry,
    pub delegate_handle: Option<DelegateParentToolsHandle>,
    pub ask_user_handle: Option<PerToolChannelHandle>,
    pub reaction_handle: PerToolChannelHandle,
    pub poll_handle: Option<PerToolChannelHandle>,
    pub escalate_handle: Option<PerToolChannelHandle>,
    pub channel_room_handle: Option<PerToolChannelHandle>,
    /// The deferred-MCP tool-search listing on its own (deferred mode only): the
    /// `## Deferred Tools` section that names the policy-admitted `<server>__<tool>`
    /// stubs and instructs the model to call `tool_search`. Empty when deferred loading
    /// is off, no stubs are admitted, or `tool_search` itself is in `excluded_tools`
    /// (the registry and prompt surfaces move together).
    ///
    /// Private - deliberately not destructurable. Every caller that has ever needed
    /// this field also needs [`Self::pinned_section`] threaded correctly alongside it,
    /// and a `..` (or an unaware full destructure) silently drops it - which is exactly
    /// how the independent-delegate path lost `pinned_section` when the field was split
    /// out. Use [`Self::combined_mcp_prompt_section`] for the
    /// single-block shape (`run`, `process_message`, independent delegation) or
    /// [`Self::deferred_section`]/[`Self::pinned_section`] for the two-slot shape
    /// (`from_config`'s `Agent`, which injects each separately per-turn).
    deferred_section: String,
    /// The pinned-MCP-resources system-prompt section on its own. Empty when no pinned
    /// resources are granted. Private for the same reason as [`Self::deferred_section`]
    /// above - access via the same two accessor patterns.
    pinned_section: String,
    /// The same pinned resources as attributed blocks (`<server>__<uri>` key plus the
    /// rendered text), for holders that must be able to withdraw a block when the
    /// caller's tool selector later narrows past its key. Always renders to exactly
    /// [`Self::pinned_section`].
    pinned_blocks: Vec<zeroclaw_tools::mcp_context::PinnedResourceBlock>,
    /// Live handle to the activated deferred-MCP set (present only when a deferred
    /// `tool_search` tool was registered).
    pub activated_handle: Option<Arc<std::sync::Mutex<ActivatedToolSet>>>,
    pub mcp_tool_names: HashSet<String>,
}

impl ScopedAssembled {
    /// The deferred-MCP tool-search listing and the pinned-MCP-resources section,
    /// composed into ONE prompt block. For callers that inject a single combined MCP
    /// prompt section: `run`, `process_message`, and independent delegation.
    ///
    /// Centralizing the composition here (instead of each caller hand-rolling
    /// `append_pinned_mcp_section(&mut deferred_section, &pinned_section)` after its own
    /// destructure) is what makes dropping `pinned_section` a thing that can no longer
    /// happen silently - the field isn't reachable except through this method or
    /// [`Self::pinned_section`], so a caller must consciously pick one.
    pub fn combined_mcp_prompt_section(&self) -> String {
        let mut combined = self.deferred_section.clone();
        append_pinned_mcp_section(&mut combined, &self.pinned_section);
        combined
    }

    /// The deferred-MCP tool-search listing on its own, for callers with two distinct
    /// prompt slots that inject each separately (`Agent::from_config`'s `Agent`, whose
    /// prompt is composed per-turn later rather than at `assemble`-call time). See
    /// [`Self::pinned_section`] for its counterpart, and
    /// [`Self::combined_mcp_prompt_section`] for the single-block shape.
    pub fn deferred_section(&self) -> &str {
        &self.deferred_section
    }

    /// The pinned-MCP-resources section on its own. See [`Self::deferred_section`].
    pub fn pinned_section(&self) -> &str {
        &self.pinned_section
    }

    /// The pinned resources as attributed blocks, for a holder that re-renders the
    /// section itself and prunes blocks on live tool narrowing (`Agent`).
    pub fn pinned_blocks(&self) -> &[zeroclaw_tools::mcp_context::PinnedResourceBlock] {
        &self.pinned_blocks
    }
}

fn tool_allowed_in_context(name: &str, exclude_memory: bool, acp_delivery: bool) -> bool {
    (!exclude_memory || !zeroclaw_tools::MEMORY_TOOL_NAMES.contains(&name))
        && (acp_delivery || name != "deliver_file")
}

impl ScopedToolRegistry {
    /// Mint a scoped, gated registry from already-built eager tools. The single seam
    /// every construction path goes through.
    pub async fn assemble(spec: ScopedAssembly<'_>) -> ScopedAssembled {
        let ScopedAssembly {
            config,
            agent_alias,
            security,
            built,
            skills,
            runtime,
            caller_allowed,
            connect_peripherals,
            exclude_memory,
            acp_delivery,
            emit_assembly_logs,
        } = spec;

        let AllToolsResult {
            tools: mut tools_registry,
            delegate_handle,
            ask_user_handle,
            reaction_handle,
            poll_handle,
            escalate_handle,
            channel_room_handle,
            unfiltered_tool_arcs,
            // Test-only capture of the concrete delegate instance; `assemble`
            // has no use for it and must keep destructuring exhaustively so a
            // new field cannot be silently dropped here.
            #[cfg(test)]
                delegate_tool: _,
        } = built;

        // 1. Peripherals. Loading CONNECTS hardware (serial opens are exclusive for
        //    real devices), so this is gated: execution surfaces pass
        //    `connect_peripherals: true`; listing-only surfaces pass `false` and
        //    enumerate without holding devices.
        if connect_peripherals {
            let peripheral_tools = load_peripheral_tools(config.peripherals.clone()).await;
            if emit_assembly_logs && !peripheral_tools.is_empty() {
                ::zeroclaw_log::record!(
                    INFO,
                    ::zeroclaw_log::Event::new(module_path!(), ::zeroclaw_log::Action::Load)
                        .with_category(::zeroclaw_log::EventCategory::Tool)
                        .with_attrs(::serde_json::json!({"count": peripheral_tools.len()})),
                    "Peripheral tools added"
                );
            }
            tools_registry.extend(peripheral_tools);
        }

        // Mint the pipeline only after the effective caller policy is known. The
        // same immutable Arc is used for top-level registration and any
        // skill-scoped builtin elevation, so no unrestricted copy can escape.
        let context_filtered_tool_arcs: Vec<Arc<dyn Tool>> = unfiltered_tool_arcs
            .iter()
            .filter(|tool| tool_allowed_in_context(tool.name(), exclude_memory, acp_delivery))
            .cloned()
            .collect();
        let pipeline_tool = config.pipeline.enabled.then(|| {
            Arc::new(tools::PipelineTool::with_access_policy(
                config.pipeline.clone(),
                context_filtered_tool_arcs.clone(),
                zeroclaw_tools::tool_access::ToolAccessPolicy::from_security(
                    security.allowed_tools.as_deref(),
                    security.excluded_tools.as_deref(),
                    caller_allowed,
                ),
            )) as Arc<dyn Tool>
        });
        if let Some(tool) = pipeline_tool.as_ref() {
            tools_registry.push(Box::new(tools::ArcToolRef(Arc::clone(tool))));
        }

        // 2. Built-in allow/deny filter (uniform: the gateway used to skip it entirely).
        //    `caller_allowed` narrows on top of the policy, for the `run` path only.
        let before_filter = tools_registry.len();
        apply_policy_tool_filter(&mut tools_registry, Some(security.as_ref()), caller_allowed);
        if emit_assembly_logs && tools_registry.len() != before_filter {
            ::zeroclaw_log::record!(
                INFO,
                ::zeroclaw_log::Event::new(module_path!(), ::zeroclaw_log::Action::Load)
                    .with_category(::zeroclaw_log::EventCategory::Agent)
                    .with_attrs(::serde_json::json!({
                        "agent_alias": agent_alias,
                        "before": before_filter,
                        "retained": tools_registry.len(),
                        "policy_allowed": security.allowed_tools.as_ref().map(|v| v.len()),
                        "policy_excluded": security.excluded_tools.as_ref().map(|v| v.len()),
                        "caller_allowed": caller_allowed.map(|v| v.len()),
                    })),
                "Applied capability-based tool access filter"
            );
        }

        // 3. Apply the assembly context to every executable view. Pipeline children
        //    were minted above from this same predicate, so nested execution cannot
        //    recover memory or delivery tools removed from the outer registry.
        tools_registry
            .retain(|tool| tool_allowed_in_context(tool.name(), exclude_memory, acp_delivery));

        let mut deferred_section = String::new();
        let pinned_section = String::new();
        let pinned_blocks: Vec<zeroclaw_tools::mcp_context::PinnedResourceBlock> = Vec::new();
        let activated_handle = None;
        let mcp_tool_names: HashSet<String> = HashSet::new();

        // 5. Skills (uniform: the gateway used to skip them). Registered under the same
        //    `SecurityPolicy`, resolving builtin elevation against context-filtered arcs.
        let resolution_registry: Vec<Arc<dyn Tool>> = context_filtered_tool_arcs
            .iter()
            .cloned()
            .chain(pipeline_tool.iter().cloned())
            .collect();
        let nat64_prefixes = match zeroclaw_infra::net_guard::parse_nat64_prefixes(
            &config.security.nat64_prefixes,
            "security.nat64_prefixes",
        ) {
            Ok(prefixes) => Some(prefixes),
            Err(error) => {
                ::zeroclaw_log::record!(
                    ERROR,
                    ::zeroclaw_log::Event::new(module_path!(), ::zeroclaw_log::Action::Fail)
                        .with_category(::zeroclaw_log::EventCategory::Tool)
                        .with_outcome(::zeroclaw_log::EventOutcome::Failure)
                        .with_attrs(::serde_json::json!({"error": error.to_string()})),
                    "Skipping skill HTTP tools because security.nat64_prefixes is invalid"
                );
                None
            }
        };
        register_skill_tools_with_context_and_runtime_optional_nat64(
            &mut tools_registry,
            skills,
            Arc::clone(security),
            &resolution_registry,
            runtime,
            nat64_prefixes.as_deref(),
        );

        // Skills and deferred MCP helpers are registered after the built-in filter,
        // so the explicit denylist must subtract once more at the final boundary.
        if let Some(excluded) = security.excluded_tools.as_deref() {
            tools_registry.retain(|t| !excluded.iter().any(|ex| ex == t.name()));
            // The registry and prompt surfaces must move together: if `tool_search`
            // itself is excluded, the deferred-MCP prompt section - which always
            // instructs the model to call `tool_search` - must not survive either,
            // or the model is told to call a tool the policy just removed.
            if excluded.iter().any(|ex| ex == "tool_search") {
                deferred_section.clear();
            }
        }

        if caller_allowed.is_some_and(|allowed| !allowed.iter().any(|name| name == "tool_search")) {
            tools_registry.retain(|tool| tool.name() != "tool_search");
            deferred_section.clear();
        }

        ScopedAssembled {
            registry: ScopedToolRegistry(tools_registry),
            delegate_handle,
            ask_user_handle,
            reaction_handle,
            poll_handle,
            escalate_handle,
            channel_room_handle,
            deferred_section,
            pinned_section,
            pinned_blocks,
            activated_handle,
            mcp_tool_names,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::SkillTool;
    use crate::tools::{ToolOutput, ToolResult};
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct MockTool(&'static str);

    impl zeroclaw_api::attribution::Attributable for MockTool {
        fn role(&self) -> zeroclaw_api::attribution::Role {
            zeroclaw_api::attribution::Role::Tool(zeroclaw_api::attribution::ToolKind::Plugin)
        }
        fn alias(&self) -> &str {
            self.0
        }
    }

    struct CountingTool {
        name: &'static str,
        calls: Arc<AtomicUsize>,
    }

    zeroclaw_api::mock_tool_attribution!(CountingTool);

    #[async_trait]
    impl Tool for CountingTool {
        fn name(&self) -> &str {
            self.name
        }

        fn description(&self) -> &str {
            "count calls"
        }

        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }

        async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<ToolResult> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(ToolResult {
                success: true,
                output: "ran".into(),
                error: None,
            })
        }
    }

    #[async_trait]
    impl Tool for MockTool {
        fn name(&self) -> &str {
            self.0
        }
        fn description(&self) -> &str {
            ""
        }
        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({})
        }
        async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<ToolResult> {
            Ok(ToolResult {
                success: true,
                output: ToolOutput::default(),
                error: None,
            })
        }
    }

    fn built_with(tools: Vec<Box<dyn Tool>>) -> AllToolsResult {
        AllToolsResult {
            tools,
            delegate_handle: None,
            ask_user_handle: None,
            reaction_handle: Arc::new(parking_lot::RwLock::new(std::collections::HashMap::new())),
            poll_handle: None,
            escalate_handle: None,
            channel_room_handle: None,
            unfiltered_tool_arcs: Vec::new(),
            delegate_tool: None,
        }
    }

    fn built_with_counting_tools(
        calls: Arc<AtomicUsize>,
        names: &[&'static str],
    ) -> AllToolsResult {
        let unfiltered_tool_arcs: Vec<Arc<dyn Tool>> = names
            .iter()
            .map(|name| {
                Arc::new(CountingTool {
                    name,
                    calls: Arc::clone(&calls),
                }) as Arc<dyn Tool>
            })
            .collect();
        let tools = unfiltered_tool_arcs
            .iter()
            .cloned()
            .map(|tool| Box::new(tools::ArcToolRef(tool)) as Box<dyn Tool>)
            .collect();
        AllToolsResult {
            tools,
            delegate_handle: None,
            ask_user_handle: None,
            reaction_handle: Arc::new(parking_lot::RwLock::new(std::collections::HashMap::new())),
            poll_handle: None,
            escalate_handle: None,
            channel_room_handle: None,
            unfiltered_tool_arcs,
            delegate_tool: None,
        }
    }

    fn built_with_pipeline(calls: Arc<AtomicUsize>) -> AllToolsResult {
        built_with_counting_tools(calls, &["shell", "file_write"])
    }

    async fn assemble_pipeline(
        security: Arc<SecurityPolicy>,
        skills: &[Skill],
        calls: Arc<AtomicUsize>,
        caller_allowed: Option<&[String]>,
    ) -> ScopedAssembled {
        let mut config = Config::default();
        config.pipeline.enabled = true;
        config.pipeline.max_steps = 20;
        config.pipeline.allowed_tools = vec!["shell".to_string(), "file_write".to_string()];
        ScopedToolRegistry::assemble(ScopedAssembly {
            config: &config,
            agent_alias: "default",
            security: &security,
            built: built_with_pipeline(calls),
            skills,
            runtime: Arc::new(crate::platform::NativeRuntime::new()),
            caller_allowed,
            connect_peripherals: false,
            exclude_memory: false,
            acp_delivery: false,
            emit_assembly_logs: false,
        })
        .await
    }

    #[tokio::test]
    async fn assembled_pipeline_rejects_agent_denied_step_before_execution() {
        let calls = Arc::new(AtomicUsize::new(0));
        let security = Arc::new(SecurityPolicy {
            allowed_tools: Some(vec![tools::PipelineTool::NAME.to_string()]),
            ..SecurityPolicy::default()
        });
        let assembled = assemble_pipeline(security, &[], Arc::clone(&calls), None).await;
        let pipeline = assembled
            .registry
            .iter()
            .find(|tool| tool.name() == tools::PipelineTool::NAME)
            .expect("policy-admitted pipeline must be registered");

        let result = pipeline
            .execute(serde_json::json!({
                "steps": [{"tool": "shell", "args": {}}]
            }))
            .await
            .expect("pipeline denial is a tool result, not a transport error");

        assert!(!result.success);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn pipeline_omitted_when_top_level_policy_denies_it() {
        let calls = Arc::new(AtomicUsize::new(0));
        let security = Arc::new(SecurityPolicy {
            allowed_tools: Some(vec!["shell".to_string()]),
            ..SecurityPolicy::default()
        });
        let assembled = assemble_pipeline(security, &[], calls, None).await;

        assert!(
            assembled
                .registry
                .iter()
                .all(|tool| tool.name() != tools::PipelineTool::NAME)
        );
    }

    #[tokio::test]
    async fn skill_elevated_pipeline_keeps_the_same_agent_policy_ceiling() {
        let calls = Arc::new(AtomicUsize::new(0));
        let skill = Skill {
            name: "ops".to_string(),
            description: "pipeline wrapper".to_string(),
            description_localizations: Default::default(),
            version: "1.0.0".to_string(),
            author: None,
            tags: Vec::new(),
            tools: vec![SkillTool {
                name: "chain".to_string(),
                description: "run a pipeline".to_string(),
                kind: "builtin".to_string(),
                command: String::new(),
                args: Default::default(),
                target: Some(tools::PipelineTool::NAME.to_string()),
                locked_args: Default::default(),
                timeout_secs: None,
            }],
            prompts: Vec::new(),
            slash_options: Vec::new(),
            always: false,
            location: None,
        };
        let security = Arc::new(SecurityPolicy {
            allowed_tools: Some(vec!["ops__chain".to_string()]),
            ..SecurityPolicy::default()
        });
        let assembled = assemble_pipeline(
            security,
            std::slice::from_ref(&skill),
            Arc::clone(&calls),
            None,
        )
        .await;
        assert!(
            assembled
                .registry
                .iter()
                .all(|tool| tool.name() != tools::PipelineTool::NAME)
        );
        let elevated = assembled
            .registry
            .iter()
            .find(|tool| tool.name() == "ops__chain")
            .expect("skill elevation must resolve the scoped pipeline target");

        let result = elevated
            .execute(serde_json::json!({
                "steps": [{"tool": "shell", "args": {}}]
            }))
            .await
            .expect("pipeline denial is a tool result, not a transport error");

        assert!(!result.success);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    async fn assert_mixed_pipeline_is_prevalidated(parallel: bool) {
        let calls = Arc::new(AtomicUsize::new(0));
        let security = Arc::new(SecurityPolicy {
            allowed_tools: Some(vec![
                tools::PipelineTool::NAME.to_string(),
                "shell".to_string(),
            ]),
            ..SecurityPolicy::default()
        });
        let assembled = assemble_pipeline(security, &[], Arc::clone(&calls), None).await;
        let pipeline = assembled
            .registry
            .iter()
            .find(|tool| tool.name() == tools::PipelineTool::NAME)
            .expect("policy-admitted pipeline must be registered");

        let result = pipeline
            .execute(serde_json::json!({
                "steps": [
                    {"tool": "shell", "args": {}},
                    {"tool": "file_write", "args": {}}
                ],
                "parallel": parallel
            }))
            .await
            .expect("pipeline denial is a tool result, not a transport error");

        assert!(!result.success);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn sequential_pipeline_prevalidates_every_step() {
        assert_mixed_pipeline_is_prevalidated(false).await;
    }

    #[tokio::test]
    async fn parallel_pipeline_prevalidates_every_step() {
        assert_mixed_pipeline_is_prevalidated(true).await;
    }

    #[tokio::test]
    async fn pipeline_steps_respect_the_run_caller_allowlist() {
        let calls = Arc::new(AtomicUsize::new(0));
        let security = Arc::new(SecurityPolicy::default());
        let caller_allowed = vec![tools::PipelineTool::NAME.to_string(), "shell".to_string()];
        let assembled =
            assemble_pipeline(security, &[], Arc::clone(&calls), Some(&caller_allowed)).await;
        let pipeline = assembled
            .registry
            .iter()
            .find(|tool| tool.name() == tools::PipelineTool::NAME)
            .expect("caller-admitted pipeline must be registered");

        let result = pipeline
            .execute(serde_json::json!({
                "steps": [{"tool": "file_write", "args": {}}]
            }))
            .await
            .expect("pipeline denial is a tool result, not a transport error");

        assert!(!result.success);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    async fn assert_pipeline_context_prevalidates_excluded_tool(
        child_name: &'static str,
        exclude_memory: bool,
        acp_delivery: bool,
        parallel: bool,
    ) {
        let calls = Arc::new(AtomicUsize::new(0));
        let mut config = Config::default();
        config.pipeline.enabled = true;
        config.pipeline.allowed_tools = vec!["shell".to_string(), child_name.to_string()];
        let security = Arc::new(SecurityPolicy {
            allowed_tools: Some(vec![
                tools::PipelineTool::NAME.to_string(),
                "shell".to_string(),
                child_name.to_string(),
            ]),
            ..SecurityPolicy::default()
        });
        let assembled = ScopedToolRegistry::assemble(ScopedAssembly {
            config: &config,
            agent_alias: "default",
            security: &security,
            built: built_with_counting_tools(Arc::clone(&calls), &["shell", child_name]),
            skills: &[],
            runtime: Arc::new(crate::platform::NativeRuntime::new()),
            caller_allowed: None,
            connect_peripherals: false,
            exclude_memory,
            acp_delivery,
            emit_assembly_logs: false,
        })
        .await;

        assert!(
            assembled
                .registry
                .iter()
                .all(|tool| tool.name() != child_name),
            "context-excluded tool must be absent from the outer registry"
        );
        let pipeline = assembled
            .registry
            .iter()
            .find(|tool| tool.name() == tools::PipelineTool::NAME)
            .expect("context filtering must not remove the admitted pipeline");
        let result = pipeline
            .execute(serde_json::json!({
                "steps": [
                    {"tool": "shell", "args": {}},
                    {"tool": child_name, "args": {}}
                ],
                "parallel": parallel
            }))
            .await
            .expect("pipeline denial is a tool result, not a transport error");

        assert!(!result.success);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn sequential_pipeline_prevalidates_memory_excluded_by_assembly_context() {
        assert_pipeline_context_prevalidates_excluded_tool("memory_recall", true, true, false)
            .await;
    }

    #[tokio::test]
    async fn parallel_pipeline_prevalidates_memory_excluded_by_assembly_context() {
        assert_pipeline_context_prevalidates_excluded_tool("memory_recall", true, true, true).await;
    }

    #[tokio::test]
    async fn sequential_pipeline_prevalidates_delivery_outside_acp_context() {
        assert_pipeline_context_prevalidates_excluded_tool("deliver_file", false, false, false)
            .await;
    }

    #[tokio::test]
    async fn parallel_pipeline_prevalidates_delivery_outside_acp_context() {
        assert_pipeline_context_prevalidates_excluded_tool("deliver_file", false, false, true)
            .await;
    }

    async fn assert_skill_context_excludes_tool(
        child_name: &'static str,
        exclude_memory: bool,
        acp_delivery: bool,
    ) {
        let calls = Arc::new(AtomicUsize::new(0));
        let skill = Skill {
            name: "ops".to_string(),
            description: "context-filtered builtin wrapper".to_string(),
            description_localizations: Default::default(),
            version: "1.0.0".to_string(),
            author: None,
            tags: Vec::new(),
            tools: vec![SkillTool {
                name: "restricted".to_string(),
                description: "wrap a context-restricted builtin".to_string(),
                kind: "builtin".to_string(),
                command: String::new(),
                args: Default::default(),
                target: Some(child_name.to_string()),
                locked_args: Default::default(),
                timeout_secs: None,
            }],
            prompts: Vec::new(),
            slash_options: Vec::new(),
            always: false,
            location: None,
        };
        let security = Arc::new(SecurityPolicy {
            allowed_tools: Some(vec!["ops__restricted".to_string()]),
            ..SecurityPolicy::default()
        });
        let config = Config::default();
        let assembled = ScopedToolRegistry::assemble(ScopedAssembly {
            config: &config,
            agent_alias: "default",
            security: &security,
            built: built_with_counting_tools(Arc::clone(&calls), &[child_name]),
            skills: std::slice::from_ref(&skill),
            runtime: Arc::new(crate::platform::NativeRuntime::new()),
            caller_allowed: None,
            connect_peripherals: false,
            exclude_memory,
            acp_delivery,
            emit_assembly_logs: false,
        })
        .await;

        assert!(
            assembled
                .registry
                .iter()
                .all(|tool| tool.name() != "ops__restricted")
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn skill_cannot_recover_memory_excluded_by_assembly_context() {
        assert_skill_context_excludes_tool("memory_recall", true, true).await;
    }

    #[tokio::test]
    async fn skill_cannot_recover_delivery_outside_acp_context() {
        assert_skill_context_excludes_tool("deliver_file", false, false).await;
    }

    async fn assemble_names(
        security: Arc<SecurityPolicy>,
        tools: Vec<Box<dyn Tool>>,
        caller_allowed: Option<&[String]>,
    ) -> Vec<String> {
        let config = Config::default();
        let out = ScopedToolRegistry::assemble(ScopedAssembly {
            config: &config,
            agent_alias: "default",
            security: &security,
            built: built_with(tools),
            skills: &[],
            runtime: Arc::new(crate::platform::NativeRuntime::new()),
            caller_allowed,
            connect_peripherals: false,
            exclude_memory: false,
            acp_delivery: true, // keep deliver_file so name-filter tests are unaffected
            emit_assembly_logs: false,
        })
        .await;
        out.registry.iter().map(|t| t.name().to_string()).collect()
    }

    #[tokio::test]
    async fn scoped_assembly_threads_configured_nat64_prefixes_to_skill_http() {
        let mut config = Config::default();
        config.security.nat64_prefixes = vec!["2001:4860:4860::/96".to_string()];
        let skill = Skill {
            name: "net".to_string(),
            description: "network skill".to_string(),
            description_localizations: Default::default(),
            version: "1".to_string(),
            author: None,
            tags: Vec::new(),
            tools: vec![SkillTool {
                name: "fetch".to_string(),
                description: "fetch".to_string(),
                kind: "http".to_string(),
                command: "https://[2001:4860:4860::a00:1]/".to_string(),
                args: Default::default(),
                target: None,
                locked_args: Default::default(),
                timeout_secs: None,
            }],
            prompts: Vec::new(),
            slash_options: Vec::new(),
            always: false,
            location: None,
        };
        let security = Arc::new(SecurityPolicy::default());
        let assembled = ScopedToolRegistry::assemble(ScopedAssembly {
            config: &config,
            agent_alias: "default",
            security: &security,
            built: built_with(Vec::new()),
            skills: std::slice::from_ref(&skill),
            runtime: Arc::new(crate::platform::NativeRuntime::new()),
            caller_allowed: None,
            connect_peripherals: false,
            exclude_memory: false,
            acp_delivery: true,
            emit_assembly_logs: false,
        })
        .await;
        let tool = assembled
            .registry
            .iter()
            .find(|tool| tool.name() == "net__fetch")
            .expect("HTTP skill must be registered");
        let result = tool.execute(serde_json::json!({})).await.unwrap();
        assert!(!result.success);
        assert_eq!(
            result.error.as_deref(),
            Some("HTTP destination rejected by network policy")
        );
    }

    #[tokio::test]
    async fn assemble_applies_the_builtin_filter_uniformly() {
        // The gateway path historically SKIPPED the built-in allow/deny filter, leaking
        // excluded tools. Through the one seam the filter ALWAYS runs - the leak is fixed
        // by construction, not by remembering to call it.
        let security = Arc::new(SecurityPolicy {
            excluded_tools: Some(vec!["spawn_subagent".into()]),
            ..SecurityPolicy::default()
        });
        let names = assemble_names(
            security,
            vec![
                Box::new(MockTool("shell")),
                Box::new(MockTool("spawn_subagent")),
            ],
            None,
        )
        .await;
        assert!(
            names.iter().any(|n| n == "shell"),
            "unlisted tool kept: {names:?}"
        );
        assert!(
            !names.iter().any(|n| n == "spawn_subagent"),
            "excluded tool dropped: {names:?}"
        );
    }

    /// `deliver_file` emits a typed attachment only an ACP turn transports, so it
    /// is gated on `acp_delivery`: absent on every non-ACP assembly (where it would
    /// otherwise report a false success), present only when the ACP turn path opts in.
    async fn assemble_names_with_acp_delivery(acp_delivery: bool) -> Vec<String> {
        let config = Config::default();
        let security = Arc::new(SecurityPolicy::default());
        let out = ScopedToolRegistry::assemble(ScopedAssembly {
            config: &config,
            agent_alias: "default",
            security: &security,
            built: built_with(vec![
                Box::new(MockTool("shell")),
                Box::new(MockTool("deliver_file")),
            ]),
            skills: &[],
            runtime: Arc::new(crate::platform::NativeRuntime::new()),
            caller_allowed: None,
            connect_peripherals: false,
            exclude_memory: false,
            acp_delivery,
            emit_assembly_logs: false,
        })
        .await;
        out.registry.iter().map(|t| t.name().to_string()).collect()
    }

    #[tokio::test]
    async fn non_acp_assembly_omits_deliver_file() {
        let names = assemble_names_with_acp_delivery(false).await;
        assert!(
            names.iter().any(|n| n == "shell"),
            "unrelated tool kept: {names:?}"
        );
        assert!(
            !names.iter().any(|n| n == "deliver_file"),
            "deliver_file must be dropped on a non-ACP turn: {names:?}"
        );
    }

    #[tokio::test]
    async fn acp_assembly_keeps_deliver_file() {
        let names = assemble_names_with_acp_delivery(true).await;
        assert!(
            names.iter().any(|n| n == "deliver_file"),
            "deliver_file must survive on the ACP turn path: {names:?}"
        );
    }

    #[tokio::test]
    async fn assemble_threads_caller_allowed_narrowing() {
        // The documented per-run caller allowlist (run() path) narrows further, and is
        // honored through the seam like every other path that narrows.
        let allow = vec!["shell".to_string()];
        let names = assemble_names(
            Arc::new(SecurityPolicy::default()),
            vec![Box::new(MockTool("shell")), Box::new(MockTool("file_read"))],
            Some(&allow),
        )
        .await;
        assert_eq!(
            names,
            vec!["shell".to_string()],
            "caller_allowed narrows: {names:?}"
        );
    }

    fn assembled_with_sections(deferred: &str, pinned: &str) -> ScopedAssembled {
        ScopedAssembled {
            registry: ScopedToolRegistry(Vec::new()),
            delegate_handle: None,
            ask_user_handle: None,
            reaction_handle: Arc::new(parking_lot::RwLock::new(std::collections::HashMap::new())),
            poll_handle: None,
            escalate_handle: None,
            channel_room_handle: None,
            deferred_section: deferred.to_string(),
            pinned_section: pinned.to_string(),
            pinned_blocks: Vec::new(),
            activated_handle: None,
            mcp_tool_names: HashSet::new(),
        }
    }

    #[test]
    fn deferred_and_pinned_accessors_return_the_raw_unmerged_sections() {
        // The two-slot shape (`from_config`'s Agent) must get each section on its own,
        // NOT the combined block - this is what makes it safe for a caller with two
        // separate prompt-injection points to avoid duplicating pinned content.
        let assembled = assembled_with_sections("deferred-only", "pinned-only");
        assert_eq!(assembled.deferred_section(), "deferred-only");
        assert_eq!(assembled.pinned_section(), "pinned-only");
    }
}
