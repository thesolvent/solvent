//! Read-only system configuration: built once at startup (by an adapter, from env/file) and only
//! read in the domain. `#[non_exhaustive]` so fields grow per consumer without breaking callers.

use std::time::Duration;

use super::ids::ChainId;

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

/// Per-chain watcher settings. `overlap_blocks` is re-scanned each cycle so an
/// event an RPC omitted and returns later is still picked up; `block_time_secs`
/// converts that block window into the wall-clock TTL of the dedup cache.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ChainConfig {
    chain: ChainId,
    start_block: u64,
    overlap_blocks: u64,
    block_time_secs: u64,
}

impl ChainConfig {
    pub fn new(
        chain: ChainId,
        start_block: u64,
        overlap_blocks: u64,
        block_time_secs: u64,
    ) -> Self {
        Self {
            chain,
            start_block,
            overlap_blocks,
            block_time_secs,
        }
    }

    /// Ethereum mainnet defaults: re-scan 25 blocks at a 15s block time.
    pub fn ethereum(start_block: u64) -> Self {
        Self::new(ChainId(1), start_block, 25, 15)
    }

    pub fn chain(&self) -> ChainId {
        self.chain
    }

    pub fn start_block(&self) -> u64 {
        self.start_block
    }

    pub fn overlap_blocks(&self) -> u64 {
        self.overlap_blocks
    }

    /// How long a processed event stays in the dedup cache: twice the re-scan
    /// window in wall-clock time, so any event re-fetched within the overlap is
    /// still a cache hit.
    pub fn dedup_ttl(&self) -> Duration {
        Duration::from_secs(self.overlap_blocks * self.block_time_secs * 2)
    }
}
