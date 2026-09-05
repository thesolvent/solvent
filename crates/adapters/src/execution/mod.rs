//! Execution adapters: the walletkit-backed tx engine that submits and tracks fills.

pub mod executor;

pub use executor::WalletkitExecutor;
