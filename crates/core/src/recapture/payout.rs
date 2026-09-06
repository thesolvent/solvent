//! The payout service: sweep the outstanding recapture credits and rebate each maker — one transfer
//! per (maker, token) summed across intents, settling a group only after its payment lands (a failed
//! payment is logged and left for the next sweep, so no credit is ever paid twice).

use std::collections::BTreeMap;
use std::sync::Arc;

use alloy_primitives::{Address, U256};

use crate::deps::recapture::{RebatePayer, RecaptureStore};
use crate::obs::warn;
use crate::primitives::recapture::AccruedCredit;
use crate::primitives::{MakerId, SolventError};

/// One maker+token's owed total and the accrued rows behind it.
struct Group {
    amount: U256,
    rows: Vec<AccruedCredit>,
}

/// Rebates outstanding recapture credits to makers.
pub struct PayoutService {
    store: Arc<dyn RecaptureStore>,
    payer: Arc<dyn RebatePayer>,
}

impl PayoutService {
    pub fn new(store: Arc<dyn RecaptureStore>, payer: Arc<dyn RebatePayer>) -> Self {
        Self { store, payer }
    }

    /// Pay every outstanding rebate, one transfer per `(maker, token)` summed across intents. A
    /// group is marked settled only after its payment succeeds; a failed payment is logged and left
    /// for the next sweep, so no credit is ever paid twice. Returns the number of groups paid.
    pub async fn settle_outstanding(&self) -> Result<usize, SolventError> {
        let mut groups: BTreeMap<(MakerId, Address), Group> = BTreeMap::new();
        for accrued in self.store.outstanding().await? {
            let group = groups
                .entry((accrued.credit.maker, accrued.credit.token))
                .or_insert_with(|| Group {
                    amount: U256::ZERO,
                    rows: Vec::new(),
                });
            group.amount = group.amount.saturating_add(accrued.credit.amount);
            group.rows.push(accrued);
        }

        let mut paid = 0;
        for ((maker, token), group) in groups {
            if self.payer.pay(maker, token, group.amount).await.is_ok() {
                self.store.mark_settled(&group.rows).await?;
                paid += 1;
            } else {
                warn!(maker = %maker.0, "rebate payment failed; leaving for the next sweep");
            }
        }
        Ok(paid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    use alloy_primitives::B256;
    use async_trait::async_trait;

    use crate::deps::recapture::{RebatePayerError, RecaptureStoreError};
    use crate::primitives::recapture::RecaptureCredit;
    use crate::primitives::IntentId;

    fn maker(n: u8) -> MakerId {
        MakerId(Address::from([n; 20]))
    }
    fn token(n: u8) -> Address {
        Address::from([n; 20])
    }
    fn intent(n: u8) -> IntentId {
        IntentId(B256::from([n; 32]))
    }
    fn credit(m: u8, t: u8, amount: u64) -> RecaptureCredit {
        RecaptureCredit::new(maker(m), token(t), U256::from(amount))
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

    #[derive(Default)]
    struct FakePayer {
        paid: Mutex<Vec<(MakerId, Address, U256)>>,
        fail_maker: Option<MakerId>,
    }
    #[async_trait]
    impl RebatePayer for FakePayer {
        async fn pay(
            &self,
            maker: MakerId,
            token: Address,
            amount: U256,
        ) -> Result<(), RebatePayerError> {
            if self.fail_maker == Some(maker) {
                return Err(RebatePayerError::Send("boom".into()));
            }
            self.paid.lock().expect("lock").push((maker, token, amount));
            Ok(())
        }
    }
    impl FakePayer {
        fn payments(&self) -> Vec<(MakerId, Address, U256)> {
            self.paid.lock().expect("lock").clone()
        }
    }

    #[tokio::test]
    async fn settles_outstanding_and_pays_each_maker() {
        let store = Arc::new(MemStore::default());
        store.accrue(intent(1), &[credit(5, 2, 240)]).await.unwrap();
        store.accrue(intent(2), &[credit(6, 2, 100)]).await.unwrap();
        let payer = Arc::new(FakePayer::default());
        let svc = PayoutService::new(store.clone(), payer.clone());

        assert_eq!(svc.settle_outstanding().await.unwrap(), 2);
        assert!(
            store.outstanding().await.unwrap().is_empty(),
            "every credit settled"
        );
        let mut payments = payer.payments();
        payments.sort_by_key(|(m, _, _)| m.0);
        assert_eq!(
            payments,
            vec![
                (maker(5), token(2), U256::from(240u64)),
                (maker(6), token(2), U256::from(100u64)),
            ]
        );
    }

    #[tokio::test]
    async fn aggregates_a_makers_credits_into_one_transfer() {
        let store = Arc::new(MemStore::default());
        // Same maker + token across two intents.
        store.accrue(intent(1), &[credit(5, 2, 240)]).await.unwrap();
        store.accrue(intent(2), &[credit(5, 2, 60)]).await.unwrap();
        let payer = Arc::new(FakePayer::default());
        let svc = PayoutService::new(store.clone(), payer.clone());

        assert_eq!(svc.settle_outstanding().await.unwrap(), 1, "one transfer");
        assert_eq!(
            payer.payments(),
            vec![(maker(5), token(2), U256::from(300u64))]
        );
        assert!(store.outstanding().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_failed_payment_is_left_for_the_next_sweep() {
        let store = Arc::new(MemStore::default());
        store.accrue(intent(1), &[credit(5, 2, 240)]).await.unwrap();
        store.accrue(intent(2), &[credit(6, 2, 100)]).await.unwrap();
        let payer = Arc::new(FakePayer {
            fail_maker: Some(maker(5)),
            ..Default::default()
        });
        let svc = PayoutService::new(store.clone(), payer.clone());

        assert_eq!(
            svc.settle_outstanding().await.unwrap(),
            1,
            "only maker 6 paid"
        );
        let out = store.outstanding().await.unwrap();
        assert_eq!(out.len(), 1, "maker 5's failed rebate stays outstanding");
        assert_eq!(out[0].credit.maker, maker(5));
    }
}
