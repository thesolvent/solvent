//! Live E2E for the recapture payout leg: accrue a credit in the durable SQLite store, then run the
//! `PayoutService` over the real `AlloyRebatePayer`, which sends an on-chain ERC-20 transfer from the
//! treasury wallet to the maker. Asserts the maker's on-chain balance rises by the rebate and the
//! store settles. The fill→reconcile→accrue half is covered hermetically (core) and by
//! `e2e_execution`; this proves the parts that only exist on chain. Gated on anvil.

mod common;

use std::sync::Arc;

use alloy::primitives::{B256, U256};
use common::{balance_of, skip_without_anvil, Harness, MockERC20};
use solvent_adapters::recapture::{AlloyRebatePayer, SqliteRecaptureStore};
use solvent_core::{
    deps::recapture::RecaptureStore,
    primitives::{recapture::RecaptureCredit, IntentId, MakerId},
    recapture::PayoutService,
};
use sqlx::sqlite::SqlitePoolOptions;

#[tokio::test]
async fn e2e_rebate_pays_the_maker_on_chain_and_settles() {
    if skip_without_anvil() {
        return;
    }
    let h = Harness::setup().await;
    let token = h.t0;
    let recipient = h.maker; // the maker owed a rebate
    let amount = U256::from(1_000_000u64);

    // The treasury (the taker account) funds the rebate.
    MockERC20::new(token, h.taker_provider.clone())
        .mint(h.taker, amount)
        .send()
        .await
        .expect("mint")
        .watch()
        .await
        .expect("mint mined");
    let before = balance_of(&h, token, recipient).await;

    // A durable store with one outstanding credit for the recipient.
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("open in-memory sqlite");
    let store = Arc::new(SqliteRecaptureStore::new(pool));
    store.migrate().await.expect("migrate");
    store
        .accrue(
            IntentId(B256::from([1; 32])),
            &[RecaptureCredit::new(MakerId(recipient), token, amount)],
        )
        .await
        .expect("accrue");

    // Pay out via a real ERC-20 transfer from the treasury wallet.
    let payer = Arc::new(AlloyRebatePayer::new(h.taker_provider.clone()));
    let svc = PayoutService::new(store.clone(), payer);
    assert_eq!(svc.settle_outstanding().await.expect("settle"), 1);

    assert_eq!(
        balance_of(&h, token, recipient).await,
        before + amount,
        "the maker received the rebate on chain"
    );
    assert!(
        store.outstanding().await.expect("outstanding").is_empty(),
        "the paid credit is settled"
    );
}
