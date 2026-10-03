//! ModelProvider family predicates used by config validation.
//! These live in the config crate to break the circular dependency between
//! config and model_providers.

/// Families whose runtime provider reads `wire_api` to choose between the
/// chat-completions and responses wires.
pub fn family_honors_wire_api(family: &str) -> bool {
    matches!(family, "openai" | "custom")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_api_is_honored_only_by_openai_wire_families() {
        for family in ["openai", "custom"] {
            assert!(family_honors_wire_api(family), "{family} honors wire_api");
        }
        for family in ["anthropic", "gemini", "openrouter", "ollama", ""] {
            assert!(!family_honors_wire_api(family), "{family} ignores wire_api");
        }
    }
}
