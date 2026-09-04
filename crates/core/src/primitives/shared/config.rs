//! Read-only system configuration: built once at startup (by an adapter, from env/file) and only
//! read in the domain. `#[non_exhaustive]` so fields grow per consumer without breaking callers.

/// Read-only runtime configuration.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct SystemConfig {
    chain_id: u64,
}

impl SystemConfig {
    pub fn new(chain_id: u64) -> Self {
        Self { chain_id }
    }

    /// The EVM chain this instance operates on.
    pub fn chain_id(&self) -> u64 {
        self.chain_id
    }
}
