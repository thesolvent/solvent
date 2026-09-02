//! Solvent adapters — port implementations (RPC, store, order feed, signer) and inbound handlers.
//!
//! `events` is the vendored garden-rs log indexer (`event_provider!` macro + the
//! `EventExt` bridge); `registry` is the Aqua `ChainSource` adapter that composes it.

pub mod events;
pub mod execution;
pub mod ingest;
pub mod ledger;
pub mod registry;
pub mod routing;
