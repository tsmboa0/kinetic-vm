//! Allow/deny policy shared by tools that execute other tools.

#[derive(Clone, Default)]
pub struct ToolAccessPolicy {
    pub allowed: Option<Vec<String>>,
    pub caller_allowed: Option<Vec<String>>,
    pub denied: Option<Vec<String>>,
}

impl ToolAccessPolicy {
    pub fn from_security(
        allowed_tools: Option<&[String]>,
        excluded_tools: Option<&[String]>,
        caller_allowed: Option<&[String]>,
    ) -> Option<Self> {
        let mut policy = Self::default();
        if let Some(list) = allowed_tools {
            policy.allowed = Some(list.to_vec());
        }
        if let Some(caller) = caller_allowed {
            policy.caller_allowed = Some(caller.to_vec());
        }
        if let Some(list) = excluded_tools {
            policy.denied = Some(list.to_vec());
        }
        if policy.allowed.is_some() || policy.caller_allowed.is_some() || policy.denied.is_some() {
            Some(policy)
        } else {
            None
        }
    }

    pub fn is_tool_allowed(&self, name: &str) -> bool {
        let in_deny = self
            .denied
            .as_ref()
            .is_some_and(|list| list.iter().any(|t| t == name));
        if in_deny {
            return false;
        }

        let risk_ok = match self.allowed.as_ref() {
            None => true,
            Some(list) => !list.is_empty() && list.iter().any(|t| t == name),
        };
        if !risk_ok {
            return false;
        }

        match self.caller_allowed.as_ref() {
            None => true,
            Some(list) => list.iter().any(|t| t == name),
        }
    }
}
