//! Solvent adapters — port implementations (RPC, store, order feed, signer) and inbound handlers.
//!
//! `events` is the vendored garden-rs log indexer (`event_provider!` macro + the
//! `EventExt` bridge); `registry` is the Aqua `ChainSource` adapter that composes it.

pub mod aqua;
pub mod balances;
pub mod chain;
pub mod crosschain;
pub mod erc20;
pub mod events;
pub mod execution;
pub mod http;
pub mod ingest;
pub mod ledger;
pub mod metrics;
pub mod rebate;
pub mod registry;
pub mod routing;
pub mod trade;
