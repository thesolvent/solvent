//! The payout worker: drives [`PayoutService`](super::PayoutService) on a cadence, sweeping the
//! outstanding recapture credits and rebating makers. A sweep that fails to read the store is logged
//! and swallowed so the loop survives to retry on the next tick; a failed *payment* is already left
//! outstanding by the payout service, so nothing is ever paid twice.

use std::time::Duration;

use tokio::sync::watch;
use tokio::time::{interval, MissedTickBehavior};

use crate::obs::{info, warn};
use crate::recapture::PayoutService;

/// Rebates outstanding recapture credits to makers on a fixed cadence. The cadence provides the
/// batching window — credits accrued between ticks settle in one transfer per maker.
pub struct PayoutWorker {
    payout: PayoutService,
}

impl PayoutWorker {
    pub fn new(payout: PayoutService) -> Self {
        Self { payout }
    }

    /// One sweep: rebate every outstanding credit, one transfer per `(maker, token)` summed across
    /// intents. Returns the number of groups paid. A store-read error is logged and reported as zero
    /// rather than propagated, so a driving loop never dies on a transient failure.
    pub async fn sweep(&self) -> usize {
        match self.payout.settle_outstanding().await {
            Ok(paid) => {
                info!(groups = %paid, "recapture payout swept");
                paid
            }
            Err(_) => {
                warn!("recapture payout sweep failed; retrying next tick");
                0
            }
        }
    }

    /// Sweep every `period` until `shutdown` flips to `true`. The flag is read once per tick, so the
    /// worker stops within one `period` of the signal. A missed tick is skipped, not caught up, so a
    /// slow sweep never triggers a burst of back-to-back sweeps.
    pub async fn run(self, period: Duration, shutdown: watch::Receiver<bool>) {
        let mut ticker = interval(period);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            if *shutdown.borrow() {
                break;
            }
            self.sweep().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    use alloy_primitives::{Address, B256, U256};
    use async_trait::async_trait;

    use crate::deps::recapture::{
        RebatePayer, RebatePayerError, RecaptureStore, RecaptureStoreError,
    };
    use crate::primitives::recapture::{AccruedCredit, RecaptureCredit};
    use crate::primitives::{IntentId, MakerId};

    fn maker(n: u8) -> MakerId {
        MakerId(Address::from([n; 20]))
    }
    fn credit(m: u8, amount: u64) -> RecaptureCredit {
        RecaptureCredit::new(maker(m), Address::from([9; 20]), U256::from(amount))
    }
    fn intent(n: u8) -> IntentId {
        IntentId(B256::from([n; 32]))
    }

    #[derive(Default)]
    struct MemStore(Mutex<Vec<AccruedCredit>>);
    #[async_trait]
    impl RecaptureStore for MemStore {
        async fn accrue(
            &self,
            id: IntentId,
            credits: &[RecaptureCredit],
        ) -> Result<(), RecaptureStoreError> {
            let mut rows = self.0.lock().expect("lock");
            for c in credits {
                rows.push(AccruedCredit::new(id, c.clone()));
            }
            Ok(())
        }
        async fn outstanding(&self) -> Result<Vec<AccruedCredit>, RecaptureStoreError> {
            Ok(self.0.lock().expect("lock").clone())
        }
        async fn mark_settled(&self, credits: &[AccruedCredit]) -> Result<(), RecaptureStoreError> {
            self.0
                .lock()
                .expect("lock")
                .retain(|a| !credits.contains(a));
            Ok(())
        }
    }

    /// A store whose reads always fail — exercises the worker's resilience.
    struct ErrStore;
    #[async_trait]
    impl RecaptureStore for ErrStore {
        async fn accrue(
            &self,
            _: IntentId,
            _: &[RecaptureCredit],
        ) -> Result<(), RecaptureStoreError> {
            Ok(())
        }
        async fn outstanding(&self) -> Result<Vec<AccruedCredit>, RecaptureStoreError> {
            Err(RecaptureStoreError::Write("store down".into()))
        }
        async fn mark_settled(&self, _: &[AccruedCredit]) -> Result<(), RecaptureStoreError> {
            Ok(())
        }
    }

    #[derive(Default)]
    struct FakePayer(Mutex<Vec<(MakerId, Address, U256)>>);
    #[async_trait]
    impl RebatePayer for FakePayer {
        async fn pay(
            &self,
            maker: MakerId,
            token: Address,
            amount: U256,
        ) -> Result<(), RebatePayerError> {
            self.0.lock().expect("lock").push((maker, token, amount));
            Ok(())
        }
    }
    impl FakePayer {
        fn count(&self) -> usize {
            self.0.lock().expect("lock").len()
        }
    }

    fn worker(store: Arc<dyn RecaptureStore>, payer: Arc<dyn RebatePayer>) -> PayoutWorker {
        PayoutWorker::new(PayoutService::new(store, payer))
    }

    // A sweep pays every outstanding group (one transfer per maker), empties the store, and reports
    // how many it paid.
    #[tokio::test]
    async fn sweep_pays_and_returns_the_count() {
        let store = Arc::new(MemStore::default());
        store
            .accrue(intent(1), &[credit(5, 240), credit(6, 100)])
            .await
            .unwrap();
        let payer = Arc::new(FakePayer::default());
        let w = worker(store.clone(), payer.clone());

        assert_eq!(w.sweep().await, 2);
        assert!(store.outstanding().await.unwrap().is_empty());
        assert_eq!(payer.count(), 2);
    }

    // A store-read error is swallowed: the sweep reports zero and never panics or propagates, so a
    // driving loop survives it.
    #[tokio::test]
    async fn sweep_swallows_a_store_error() {
        let payer = Arc::new(FakePayer::default());
        let w = worker(Arc::new(ErrStore), payer.clone());

        assert_eq!(w.sweep().await, 0);
        assert_eq!(payer.count(), 0, "nothing paid on a failed read");
    }

    // The run loop sweeps on its cadence and stops on the shutdown signal.
    #[tokio::test]
    async fn run_sweeps_until_shutdown() {
        let store = Arc::new(MemStore::default());
        store.accrue(intent(1), &[credit(5, 240)]).await.unwrap();
        let payer = Arc::new(FakePayer::default());
        let w = worker(store.clone(), payer.clone());

        let (tx, rx) = watch::channel(false);
        let handle = tokio::spawn(w.run(Duration::from_millis(2), rx));
        tokio::time::sleep(Duration::from_millis(25)).await;
        tx.send(true).unwrap();
        handle.await.unwrap();

        assert!(
            store.outstanding().await.unwrap().is_empty(),
            "the seeded credit was paid on a tick"
        );
        assert!(payer.count() >= 1);
    }
}
