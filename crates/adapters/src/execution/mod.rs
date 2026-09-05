//! Execution adapters: the walletkit-backed tx engine that submits and tracks fills.

pub mod executor;
pub mod settlement;

pub use executor::WalletkitExecutor;
pub use settlement::AquaSettlementReader;
