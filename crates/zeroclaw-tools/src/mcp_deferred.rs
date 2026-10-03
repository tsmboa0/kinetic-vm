//! Activated-tool bag the turn loop consults after the static registry.
//!
//! MCP no longer fills this set. It stays empty unless a caller activates a
//! tool by name, which production does not do.

use std::collections::HashMap;
use std::sync::Arc;

use zeroclaw_api::tool::Tool;
use zeroclaw_api::tool::ToolSpec;

/// Tools activated for the current conversation.
///
/// The agent loop includes [`Self::tool_specs`] in later iterations and
/// resolves calls through [`Self::get_resolved`].
pub struct ActivatedToolSet {
    tools: HashMap<String, Arc<dyn Tool>>,
}

impl ActivatedToolSet {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    pub fn activate(&mut self, name: String, tool: Arc<dyn Tool>) {
        self.tools.insert(name, tool);
    }

    pub fn is_activated(&self, name: &str) -> bool {
        self.tools.contains_key(name)
    }

    /// Clone the `Arc` so the caller can drop the mutex guard before awaiting.
    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }

    /// Resolve `name` exactly, or as the unique `__` suffix of one activated tool.
    ///
    /// A name that already contains `__` is exact-match only. Two tools sharing
    /// the same suffix do not resolve.
    pub fn get_resolved(&self, name: &str) -> Option<Arc<dyn Tool>> {
        if let Some(tool) = self.get(name) {
            return Some(tool);
        }
        if name.contains("__") {
            return None;
        }

        let mut resolved = None;
        for (tool_name, tool) in &self.tools {
            let Some((_, suffix)) = tool_name.split_once("__") else {
                continue;
            };
            if suffix != name {
                continue;
            }
            if resolved.is_some() {
                return None;
            }
            resolved = Some(Arc::clone(tool));
        }

        resolved
    }

    pub fn tool_specs(&self) -> Vec<ToolSpec> {
        self.tools.values().map(|t| t.spec()).collect()
    }

    pub fn tool_names(&self) -> Vec<&str> {
        self.tools.keys().map(|s| s.as_str()).collect()
    }

    /// Drop activated tools a newly narrowed principal may no longer invoke.
    pub fn retain_allowed(&mut self, allowed: &[String]) {
        self.tools
            .retain(|name, _| allowed.iter().any(|allowed_name| allowed_name == name));
    }
}

impl Default for ActivatedToolSet {
    fn default() -> Self {
        Self::new()
    }
}
