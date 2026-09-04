//! The wall-clock `Clock` — the live source of "now" for TTL expiry.

use solvent_core::deps::ledger::Clock;

pub struct SystemClock;

impl Clock for SystemClock {
    fn now_unix(&self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_secs())
            .unwrap_or(0)
    }
}
