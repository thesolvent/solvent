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

/// This module contains the prelude for the events crate.
/// It is essential to import this prelude for the `event_provider!` macro to work correctly,
/// as it brings in all necessary types and traits required by the macro and event querying.
///
/// # Important
/// You must import the `prelude` module from this crate before using the `event_provider!` macro:
/// ```ignore
/// use prelude::*;
/// ```
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

    /// Decodes logs for a specific event type, filtering by provided addresses.
    ///
    /// # Arguments
    /// - `logs`: List of raw logs to process.
    /// - `addresses`: List of contract addresses to filter logs.
    ///
    /// # Returns
    /// A vector of decoded events wrapped in `EventExt`.
    pub fn process_logs<T: SolEventInterface>(
        logs: &[alloy::rpc::types::Log],
        addresses: &[Address],
    ) -> Vec<EventExt<T>> {
        // Group logs by address
        let mut addr_to_log_map = HashMap::new();
        logs.iter().for_each(|log| {
            if addresses.contains(&log.address()) {
                addr_to_log_map
                    .entry(log.address())
                    .or_insert_with(Vec::new)
                    .push(log.clone());
            }
        });

        // Decode logs for each address
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

    /// Generates block ranges for batched queries to limit the number of blocks per query.
    ///
    /// # Arguments
    /// - `from_block`: Starting block number.
    /// - `to_block`: Ending block number.
    /// - `max_block_span`: Maximum number of blocks per range.
    ///
    /// # Returns
    /// A vector of tuples representing block ranges `(start, end)`.
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
/// Macro to generate a batched event provider for querying multiple Ethereum contract types events in parallel.
///
/// # Usage
/// Before using the `event_provider!` macro, import the `prelude` module from this crate:
/// ```ignore
/// use crate::events::prelude::*;
/// ```
///
/// This macro generates a provider struct and associated methods to efficiently query and decode logs for the specified event types
/// from an Ethereum node. It supports batching queries over block ranges and limits concurrency to avoid overloading the node.
///
/// # Parameters
/// - `$mod_name`: The module name for the generated provider.
/// - `($alias, $event_type)`: One or more pairs specifying an alias and the event type to query. Each event type must implement the required traits.
///
/// # Generated Items
/// - A module `$mod_name` containing:
///   - A struct `EventProvider` over a shared `Arc<dyn Provider>`.
///   - An `AddressFilter` struct for specifying addresses per event type.
///   - A `new` constructor to initialize the provider with configuration options.
///   - An async `query` method to fetch and decode events in batches for a block range and addresses.
///
/// # Errors
/// - Returns an error if log queries fail after all retry attempts.
macro_rules! event_provider {
    ($mod_name:ident, $(($alias:ident, $event_type:ty)),+ $(,)?) => {
        pub mod $mod_name {
            use super::*;

            // Configuration constants for retry and concurrency
            const DEFAULT_RETRY_DELAY_MS: u64 = 500;
            const MAX_RETRY_ATTEMPTS: usize = 5;
            const DEFAULT_CONCURRENT_TASKS_LIMIT: usize = 5;
            const DEFAULT_MAX_BLOCK_SPAN: u64 = 10000;

            /// Struct to hold address filters for each event type.
            #[derive(Clone, Debug, Default)]
            pub struct AddressFilter {
                $(
                    $alias: Vec<Address>,
                )+
            }

            impl AddressFilter {
                /// Creates a new `AddressFilter` with empty address vectors.
                pub fn new() -> Self {
                    Self {
                        $(
                            $alias: vec![],
                        )+
                    }
                }

                $(
                    /// Sets addresses for the `$alias` event type.
                    pub fn $alias(mut self, addresses: Vec<Address>) -> Self {
                        self.$alias = addresses;
                        self
                    }
                )+

                /// Builds an array of address vectors for all event types.
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
                /// Initializes a new event provider with the specified configuration.
                ///
                /// # Arguments
                /// - `provider`: Ethereum provider for querying logs.
                /// - `max_block_span`: Maximum block range per query (defaults to `DEFAULT_MAX_BLOCK_SPAN`).
                /// - `concurrent_tasks_limit`: Maximum concurrent tasks (defaults to `DEFAULT_CONCURRENT_TASKS_LIMIT`).
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
                /// Queries events for the specified block range and address filter in batches.
                ///
                /// # Arguments
                /// - `from_block`: Starting block number.
                /// - `to_block`: Ending block number.
                /// - `filter`: Address filter for each event type.
                ///
                /// # Returns
                /// A `Result` containing a tuple of event vectors for each event type.
                pub async fn query(
                    &self,
                    from_block: u64,
                    to_block: u64,
                    filter: &AddressFilter,
                ) -> Result<($(Vec<EventExt<$event_type>>,)+), IndexerError> {
                    // Validate block range
                    if to_block < from_block {
                        return Err(IndexerError::InvalidRange);
                    }
                    let addresses = filter.build();
                    // Combine all addresses into a single vector for filtering
                    let all_addresses: Vec<Address> = addresses.iter().flatten().cloned().collect();
                    let block_ranges = get_block_ranges(from_block, to_block, self.max_block_span);
                    let mut logs = Vec::new();

                    // Process block ranges in chunks to respect concurrency limit
                    for chunk in block_ranges.chunks(self.concurrent_tasks_limit) {
                        let mut tasks = FuturesUnordered::new();

                        // Create tasks for each block range in the chunk
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

                        // Collect logs from all tasks in the chunk
                        while let Some(result) = tasks.next().await {
                            logs.extend(result?);
                        }
                    }

                    // Decode logs for each event type
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
