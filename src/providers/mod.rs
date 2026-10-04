//! ModelProvider subsystem — re-exported from `kinetic-providers`.

pub use kinetic_providers::*;

// Keep traits.rs as a file module so its #[cfg(test)] block compiles.
#[path = "traits.rs"]
pub mod traits;
