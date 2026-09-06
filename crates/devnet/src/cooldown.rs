use std::collections::HashMap;
use std::time::{Duration, Instant};

use alloy::primitives::Address;
use parking_lot::Mutex;

/// Per-address rate limiter: at most one drip per `window`. `check` is atomic — it records the hit
/// only when the caller is allowed, so two concurrent requests can't both pass.
pub struct Cooldown {
    window: Duration,
    last: Mutex<HashMap<Address, Instant>>,
}

impl Cooldown {
    pub fn new(window: Duration) -> Self {
        Self {
            window,
            last: Mutex::new(HashMap::new()),
        }
    }

    /// Allow `who` at `now` (recording the hit), or return the time still remaining if cooling down.
    pub fn check(&self, who: Address, now: Instant) -> Result<(), Duration> {
        let mut last = self.last.lock();
        if let Some(&prev) = last.get(&who) {
            let elapsed = now.saturating_duration_since(prev);
            if elapsed < self.window {
                return Err(self.window - elapsed);
            }
        }
        last.insert(who, now);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_first_hit_then_blocks_until_the_window_elapses() {
        let cd = Cooldown::new(Duration::from_secs(60));
        let a = Address::from([1u8; 20]);
        let t0 = Instant::now();

        assert!(cd.check(a, t0).is_ok());
        match cd.check(a, t0 + Duration::from_secs(10)) {
            Err(remaining) => assert_eq!(remaining, Duration::from_secs(50)),
            Ok(()) => panic!("should still be cooling down"),
        }
        assert!(cd.check(a, t0 + Duration::from_secs(60)).is_ok());
    }

    #[test]
    fn tracks_addresses_independently() {
        let cd = Cooldown::new(Duration::from_secs(60));
        let (a, b) = (Address::from([1u8; 20]), Address::from([2u8; 20]));
        let t0 = Instant::now();

        assert!(cd.check(a, t0).is_ok());
        assert!(cd.check(b, t0).is_ok(), "b is not limited by a's hit");
    }
}
