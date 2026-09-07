//! A batched `eth_getLogs` indexer, copied from garden-rs
//! (`crates/evm/src/events/events.rs`). Only three things differ from the
//! original: `eyre::Result`/`eyre::eyre!` become the typed [`IndexerError`],
//! `utils::retry_with_backoff` is inlined below (we don't take garden as a dep),
//! and `EventExt::from((log, event))` becomes `event_ext(log, event)` because
//! the type now lives in `solvent-core` (orphan rule bars its `From<Log>` here).

use std::fmt::Display;
use std::future::Future;

use tokio::time::{sleep, Duration};

/// A failure while querying logs. Replaces garden's `eyre` errors.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum IndexerError {
    #[error("invalid block range: `to_block` must be >= `from_block`")]
    InvalidRange,
    #[error("failed to fetch logs from block {from} to {to}: {reason}")]
    GetLogs { from: u64, to: u64, reason: String },
}

/// Retries an async operation with exponential backoff. Copied from garden-rs
/// (`utils::retry_with_backoff`).
pub async fn retry_with_backoff<F, Fut, T, E>(
    mut operation: F,
    max_retries: usize,
    initial_delay_ms: u64,
) -> Result<T, E>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
    E: Display,
{
    for attempt in 0..max_retries - 1 {
        match operation().await {
            Ok(result) => return Ok(result),
            Err(e) => {
                tracing::error!("{}", e);
                let delay = initial_delay_ms * (2_u64.pow(attempt as u32));
                sleep(Duration::from_millis(delay)).await;
            }
        }
    }
    operation().await
}

/// Import this before using the `event_provider!` macro — it re-exports the types and traits the
/// macro's expansion needs.
pub mod prelude {

    pub use crate::count_types;
    pub use crate::events::event_ext;
    pub use alloy::{
        primitives::Address, providers::Provider, rpc::types::Filter, sol_types::SolEventInterface,
    };
    pub use futures::{stream::FuturesUnordered, StreamExt};
    pub use solvent_core::primitives::registry::EventExt;
    pub use std::collections::HashMap;
    pub use std::sync::Arc;

    pub use super::{retry_with_backoff, IndexerError};

    /// Decodes the logs from `addresses` into `EventExt<T>`, dropping any that don't parse as `T`.
    pub fn process_logs<T: SolEventInterface>(
        logs: &[alloy::rpc::types::Log],
        addresses: &[Address],
    ) -> Vec<EventExt<T>> {
        let mut addr_to_log_map = HashMap::new();
        logs.iter().for_each(|log| {
            if addresses.contains(&log.address()) {
                addr_to_log_map
                    .entry(log.address())
                    .or_insert_with(Vec::new)
                    .push(log.clone());
            }
        });

        addresses
            .iter()
            .flat_map(|addr| {
                addr_to_log_map
                    .get(addr)
                    .unwrap_or(&Vec::new())
                    .iter()
                    .filter_map(|log| {
                        T::decode_raw_log(log.topics(), &log.data().data)
                            .ok()
                            .map(|event| event_ext(log, event))
                    })
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    /// Split `[from_block, to_block]` into `(start, end)` chunks of at most `max_block_span` blocks.
    pub fn get_block_ranges(
        from_block: u64,
        to_block: u64,
        max_block_span: u64,
    ) -> Vec<(u64, u64)> {
        (from_block..=to_block)
            .step_by(max_block_span as usize + 1)
            .map(|start| {
                let end = (start + max_block_span).min(to_block);
                (start, end)
            })
            .collect()
    }
}

#[macro_export]
/// Generates a batched, parallel event provider for the given contract event types. Import the
/// `prelude` first; the generated `$mod_name::EventProvider::query` fetches and decodes logs over a
/// block range, batching by block span and capping concurrency.
macro_rules! event_provider {
    ($mod_name:ident, $(($alias:ident, $event_type:ty)),+ $(,)?) => {
        pub mod $mod_name {
            use super::*;

            const DEFAULT_RETRY_DELAY_MS: u64 = 500;
            const MAX_RETRY_ATTEMPTS: usize = 5;
            const DEFAULT_CONCURRENT_TASKS_LIMIT: usize = 5;
            const DEFAULT_MAX_BLOCK_SPAN: u64 = 10000;

            #[derive(Clone, Debug, Default)]
            pub struct AddressFilter {
                $(
                    $alias: Vec<Address>,
                )+
            }

            impl AddressFilter {
                pub fn new() -> Self {
                    Self {
                        $(
                            $alias: vec![],
                        )+
                    }
                }

                $(
                    pub fn $alias(mut self, addresses: Vec<Address>) -> Self {
                        self.$alias = addresses;
                        self
                    }
                )+

                pub fn build(&self) -> [Vec<Address>; count_types!($($event_type),+)] {
                    [
                        $(
                            self.$alias.clone(),
                        )+
                    ]
                }
            }

            /// Batched event provider for querying contract events in parallel.
            pub struct EventProvider{
                provider: Arc<dyn Provider>,
                max_block_span: u64,
                concurrent_tasks_limit: usize,
            }

            impl EventProvider
            {
                /// Wraps `provider`, defaulting the block span and concurrency when unset.
                pub fn new(
                    provider: Arc<dyn Provider>,
                    max_block_span: Option<u64>,
                    concurrent_tasks_limit: Option<usize>,
                ) -> Self {
                    Self {
                        provider,
                        max_block_span: max_block_span.unwrap_or(DEFAULT_MAX_BLOCK_SPAN),
                        concurrent_tasks_limit: concurrent_tasks_limit.unwrap_or(DEFAULT_CONCURRENT_TASKS_LIMIT),
                    }
                }

                #[allow(unused_assignments)]
                /// Fetches and decodes every event type over `[from_block, to_block]` for the
                /// address filter, batched and concurrency-limited.
                pub async fn query(
                    &self,
                    from_block: u64,
                    to_block: u64,
                    filter: &AddressFilter,
                ) -> Result<($(Vec<EventExt<$event_type>>,)+), IndexerError> {
                    if to_block < from_block {
                        return Err(IndexerError::InvalidRange);
                    }
                    let addresses = filter.build();
                    let all_addresses: Vec<Address> = addresses.iter().flatten().cloned().collect();
                    let block_ranges = get_block_ranges(from_block, to_block, self.max_block_span);
                    let mut logs = Vec::new();

                    for chunk in block_ranges.chunks(self.concurrent_tasks_limit) {
                        let mut tasks = FuturesUnordered::new();

                        for &(chunk_from, chunk_to) in chunk {
                            let provider = self.provider.clone();
                            let all_addresses = all_addresses.clone();
                            let provider_name = stringify!($mod_name);
                            tasks.push(async move {
                                retry_with_backoff(
                                    || async {
                                        tracing::info!("Querying logs in {} from block {} to {}", provider_name, chunk_from, chunk_to);
                                        let filter = Filter::new()
                                            .address(all_addresses.clone())
                                            .from_block(chunk_from)
                                            .to_block(chunk_to);

                                        provider.get_logs(&filter).await.map_err(|e| {
                                            IndexerError::GetLogs {
                                                from: chunk_from,
                                                to: chunk_to,
                                                reason: e.to_string(),
                                            }
                                        })
                                    },
                                    MAX_RETRY_ATTEMPTS,
                                    DEFAULT_RETRY_DELAY_MS,
                                )
                                .await
                            });
                        }

                        while let Some(result) = tasks.next().await {
                            logs.extend(result?);
                        }
                    }

                    let mut index = 0;
                    Ok((
                        $(
                            {
                                let events = process_logs::<$event_type>(&logs, &addresses[index]);
                                index += 1;
                                events
                            },
                        )+
                    ))
                }
            }
        }
    };
}

#[macro_export]
/// Counts the number of event types at compile time for array sizing.
macro_rules! count_types {
    () => (0usize);
    ($head:ty $(, $tail:ty)*) => (1usize + $crate::count_types!($($tail),*));
}
