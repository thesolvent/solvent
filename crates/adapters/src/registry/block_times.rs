//! Immutable block headers shared by repeated history reads.

use alloy::{primitives::B256, providers::Provider};
use async_trait::async_trait;
use futures::{stream, StreamExt, TryStreamExt};
use moka::future::Cache;
use solvent_core::deps::registry::{BlockTimes, BlockTimesError};
use std::{collections::BTreeMap, sync::Arc};

pub struct AlloyBlockTimes {
    provider: Arc<dyn Provider>,
    timestamps: Cache<B256, u64>,
}

impl AlloyBlockTimes {
    pub fn new(provider: Arc<dyn Provider>) -> Self {
        Self {
            provider,
            timestamps: Cache::new(10_000),
        }
    }

    async fn timestamp(&self, hash: B256) -> Result<u64, BlockTimesError> {
        self.timestamps
            .try_get_with(hash, async {
                let block = self
                    .provider
                    .get_block_by_hash(hash)
                    .await
                    .map_err(|error| BlockTimesError::Rpc(error.to_string()))?
                    .ok_or(BlockTimesError::Missing(hash))?;
                Ok(block.header.timestamp)
            })
            .await
            .map_err(|error: Arc<BlockTimesError>| (*error).clone())
    }
}

#[async_trait]
impl BlockTimes for AlloyBlockTimes {
    async fn timestamps(&self, hashes: &[B256]) -> Result<BTreeMap<B256, u64>, BlockTimesError> {
        stream::iter(hashes.iter().copied())
            .map(|hash| async move { self.timestamp(hash).await.map(|time| (hash, time)) })
            .buffer_unordered(8)
            .try_collect()
            .await
    }
}
