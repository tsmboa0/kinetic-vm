//! Host implementation for tool and channel worlds that import durable state.

use crate::component::PluginState;
use crate::component::bindings;
use crate::services::PluginStateError;

// A macro cannot parameterize a generated module path through a Rust type, so
// keep the two exhaustive conversions adjacent to the two adapter impls.
macro_rules! state_error_mapper {
    ($name:ident, $world:ident) => {
        fn $name(error: PluginStateError) -> bindings::$world::kinetic::plugin::state::StateError {
            use bindings::$world::kinetic::plugin::state::StateError;
            match error {
                PluginStateError::InvalidKey => StateError::InvalidKey,
                PluginStateError::AccessDenied => StateError::AccessDenied,
                PluginStateError::NotFound => StateError::NotFound,
                PluginStateError::Conflict => StateError::Conflict,
                PluginStateError::QuotaExceeded => StateError::QuotaExceeded,
                PluginStateError::Unavailable => StateError::Unavailable,
            }
        }
    };
}

state_error_mapper!(map_tool_state_error, tool);
state_error_mapper!(map_channel_state_error, channel);

impl bindings::tool::kinetic::plugin::state::Host for PluginState {
    async fn get(
        &mut self,
        key: String,
    ) -> Result<
        Option<bindings::tool::kinetic::plugin::state::StateEntry>,
        bindings::tool::kinetic::plugin::state::StateError,
    > {
        self.state_get(key)
            .await
            .map(|entry| {
                entry.map(|entry| bindings::tool::kinetic::plugin::state::StateEntry {
                    value: entry.value().to_vec(),
                    revision: entry.revision(),
                })
            })
            .map_err(map_tool_state_error)
    }

    async fn put(
        &mut self,
        key: String,
        value: Vec<u8>,
        expected_revision: Option<u64>,
    ) -> Result<u64, bindings::tool::kinetic::plugin::state::StateError> {
        self.state_put(key, value, expected_revision)
            .await
            .map_err(map_tool_state_error)
    }

    async fn delete(
        &mut self,
        key: String,
        expected_revision: u64,
    ) -> Result<(), bindings::tool::kinetic::plugin::state::StateError> {
        self.state_delete(key, expected_revision)
            .await
            .map_err(map_tool_state_error)
    }
}

impl bindings::channel::kinetic::plugin::state::Host for PluginState {
    async fn get(
        &mut self,
        key: String,
    ) -> Result<
        Option<bindings::channel::kinetic::plugin::state::StateEntry>,
        bindings::channel::kinetic::plugin::state::StateError,
    > {
        self.state_get(key)
            .await
            .map(|entry| {
                entry.map(
                    |entry| bindings::channel::kinetic::plugin::state::StateEntry {
                        value: entry.value().to_vec(),
                        revision: entry.revision(),
                    },
                )
            })
            .map_err(map_channel_state_error)
    }

    async fn put(
        &mut self,
        key: String,
        value: Vec<u8>,
        expected_revision: Option<u64>,
    ) -> Result<u64, bindings::channel::kinetic::plugin::state::StateError> {
        self.state_put(key, value, expected_revision)
            .await
            .map_err(map_channel_state_error)
    }

    async fn delete(
        &mut self,
        key: String,
        expected_revision: u64,
    ) -> Result<(), bindings::channel::kinetic::plugin::state::StateError> {
        self.state_delete(key, expected_revision)
            .await
            .map_err(map_channel_state_error)
    }
}
