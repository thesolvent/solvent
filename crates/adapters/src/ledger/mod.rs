//! Ledger adapters: the SQLite [`LedgerStore`] and the system [`Clock`].

mod sqlite_store;
mod system_clock;

pub use sqlite_store::SqliteLedgerStore;
pub use system_clock::SystemClock;
