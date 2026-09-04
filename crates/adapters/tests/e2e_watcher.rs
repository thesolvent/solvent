//! Live E2E — watcher *over time*: drives real on-chain state changes (ship,
//! maker push, third-party push, taker swaps, dock) and, after each, syncs the
//! genuine pipeline and asserts the snapshot still matches chain — balances ==
//! `safeBalances()` and `price()` == `quote()`. Also covers duplicate-log
//! handling (overlap re-scan, full replay), restart recovery of the accumulated
//! state, on-chain immutability, app-filter scoping, and multi-maker on one pair.
//!
//! Gated on `TEST_DATABASE_URL` + a spawnable `anvil`. Run:
//!   scripts/with-postgres.sh cargo test -p solvent-adapters --test e2e_watcher

mod common;

use std::sync::Arc;

use alloy::primitives::{Address, U256};
use common::{pipeline, strategy_key, Harness, StrategySpec};
use solvent_adapters::registry::PgStore;
use solvent_core::{
    primitives::{registry::TokenPair, ChainConfig, ChainId},
    registry::{RegistrySync, SharedSnapshot},
};

fn priceable(h: &Harness) -> Vec<&StrategySpec> {
    h.fx.strategies
        .iter()
        .filter(|s| s.supported && !s.guarded)
        .collect()
}

/// Every priceable strategy: snapshot balances == chain, and price() == quote().
async fn assert_matches_chain(h: &Harness, snapshot: &SharedSnapshot) {
    let snap = snapshot.load();
    for spec in priceable(h) {
        let strat = snap
            .strategy(&spec.key(h.maker, h.app))
            .unwrap_or_else(|| panic!("{} present", spec.label));
        assert!(strat.active, "{} active", spec.label);
        h.assert_balances(&snap, spec).await;
        h.assert_price_parity(strat, spec).await;
    }
}

#[tokio::test]
async fn e2e_watcher_lifecycle() {
    let Some(db_url) = common::db_url_or_skip() else {
        return;
    };

    let h = Harness::setup().await;
    h.ship_all().await;
    let (sync, snapshot, pool) = pipeline(&h, &db_url, ChainId(41337)).await;

    // S0 — ship: backfill matches chain.
    sync.sync_once(h.latest_block().await).await.expect("s0");
    assert_matches_chain(&h, &snapshot).await;

    let specs: Vec<StrategySpec> = priceable(&h).into_iter().cloned().collect();

    // S1 — maker tops up t0 on every strategy (permissionless push).
    for s in &specs {
        h.push(
            s,
            h.t0,
            s.ship_lo / U256::from(10),
            h.maker,
            &h.maker_provider,
        )
        .await;
    }
    sync.sync_once(h.latest_block().await).await.expect("s1");
    assert_matches_chain(&h, &snapshot).await;

    // S2 — a THIRD PARTY (taker) pushes t1 into the first strategy: balances move
    // from an arbitrary source, and the watcher still tracks it exactly.
    h.push(
        &specs[0],
        h.t1,
        specs[0].ship_hi / U256::from(10),
        h.taker,
        &h.taker_provider,
    )
    .await;
    sync.sync_once(h.latest_block().await).await.expect("s2");
    assert_matches_chain(&h, &snapshot).await;

    // S3 — taker swaps t1 -> t0 on every strategy (Pushed + Pulled; price moves).
    for s in &specs {
        h.swap(s, h.t1, h.t0, s.ship_hi / U256::from(20)).await;
    }
    sync.sync_once(h.latest_block().await).await.expect("s3");
    assert_matches_chain(&h, &snapshot).await;

    // S4 — taker swaps the other way, larger.
    for s in &specs {
        h.swap(s, h.t0, h.t1, s.ship_lo / U256::from(10)).await;
    }
    sync.sync_once(h.latest_block().await).await.expect("s4");
    assert_matches_chain(&h, &snapshot).await;

    // D — duplicate handling. An overlap re-scan changes nothing…
    let settled = snapshot.load();
    sync.sync_once(h.latest_block().await)
        .await
        .expect("overlap");
    assert_eq!(
        &*snapshot.load(),
        &*settled,
        "overlap re-scan changed state"
    );
    // …and a FULL re-delivery (reset the cursor so the next scan re-fetches from
    // block 0) is deduped by the store's unique key — the snapshot is unchanged.
    sqlx::query("DELETE FROM registry_cursor WHERE chain = $1")
        .bind(41337i64)
        .execute(&pool)
        .await
        .expect("reset cursor");
    sync.sync_once(h.latest_block().await)
        .await
        .expect("full re-scan");
    assert_eq!(&*snapshot.load(), &*settled, "full re-scan double-counted");

    // R — restart recovery rebuilds the accumulated snapshot from the store alone.
    {
        let store = PgStore::new(pool.clone());
        let recovered = Arc::new(SharedSnapshot::default());
        let config = ChainConfig::new(ChainId(41337), 0, 25, 15);
        let restart = RegistrySync::new(
            &config,
            h.chain_source(),
            Arc::new(store),
            recovered.clone(),
        );
        restart.recover().await.expect("recover");
        assert_eq!(&*recovered.load(), &*settled, "recovery diverged");
    }

    // X — on-chain immutability: re-shipping an existing strategy reverts, so the
    // watcher never sees a re-ship; docking then re-shipping is impossible.
    h.dock(&specs[0]).await;
    sync.sync_once(h.latest_block().await).await.expect("dock");
    assert!(
        !snapshot
            .load()
            .strategy(&specs[0].key(h.maker, h.app))
            .unwrap()
            .active,
        "docked strategy inactive"
    );
    assert!(
        h.try_ship(&specs[0].strategy_hex, specs[0].ship_lo, specs[0].ship_hi)
            .await
            .is_err(),
        "re-shipping a docked strategy must revert on-chain"
    );
}

#[tokio::test]
async fn e2e_watcher_app_filter() {
    let Some(db_url) = common::db_url_or_skip() else {
        return;
    };

    let h = Harness::setup().await;
    h.ship_all().await;

    // A strategy shipped on a SECOND router (a foreign `app`) must be ignored.
    let foreign = common::AquaSwapVMRouter::deploy(
        h.maker_provider.clone(),
        *h.aqua.address(),
        Address::ZERO,
        h.maker,
        "SwapVM".to_string(),
        "1.0.0".to_string(),
    )
    .await
    .expect("deploy foreign router");
    let spec = &h.fx.strategies[0];
    common::MockERC20::new(h.t0, h.maker_provider.clone())
        .mint(h.maker, spec.ship_lo)
        .send()
        .await
        .expect("mint")
        .watch()
        .await
        .expect("mint mined");
    common::MockERC20::new(h.t1, h.maker_provider.clone())
        .mint(h.maker, spec.ship_hi)
        .send()
        .await
        .expect("mint")
        .watch()
        .await
        .expect("mint mined");
    h.aqua
        .ship(
            *foreign.address(),
            spec.strategy_hex.clone(),
            vec![h.t0, h.t1],
            vec![spec.ship_lo, spec.ship_hi],
        )
        .send()
        .await
        .expect("ship foreign")
        .watch()
        .await
        .expect("ship foreign mined");

    let (sync, snapshot, _pool) = pipeline(&h, &db_url, ChainId(41338)).await;
    sync.sync_once(h.latest_block().await).await.expect("sync");

    let snap = snapshot.load();
    assert_eq!(
        snap.len(),
        h.fx.strategies.len(),
        "only our app's strategies"
    );
    let foreign_key = strategy_key(h.maker, *foreign.address(), &spec.strategy_hex);
    assert!(
        snap.strategy(&foreign_key).is_none(),
        "foreign-app strategy must be filtered out"
    );
}

#[tokio::test]
async fn e2e_watcher_multi_maker() {
    let Some(db_url) = common::db_url_or_skip() else {
        return;
    };

    let h = Harness::setup().await;
    // Two makers ship an XYC on the same pair: maker #0 (fixture[0]) and maker #1.
    h.ship(&h.fx.strategies[0]).await;
    h.ship_second_maker().await;

    let (sync, snapshot, _pool) = pipeline(&h, &db_url, ChainId(41339)).await;
    sync.sync_once(h.latest_block().await).await.expect("sync");

    let snap = snapshot.load();
    let key0 = h.fx.strategies[0].key(h.maker, h.app);
    let key1 = strategy_key(
        h.fx.second_maker.maker,
        h.app,
        &h.fx.second_maker.strategy_hex,
    );
    assert!(
        snap.strategy(&key0).is_some_and(|s| s.active),
        "maker #0 present"
    );
    assert!(
        snap.strategy(&key1).is_some_and(|s| s.active),
        "maker #1 present"
    );

    // Both distinct makers are routable on the same pair.
    let pair = TokenPair::new(h.t0, h.t1);
    let routable: Vec<_> = snap
        .active_strategies_for_pair(pair)
        .map(|s| s.key)
        .collect();
    assert!(routable.contains(&key0), "maker #0 routable on the pair");
    assert!(routable.contains(&key1), "maker #1 routable on the pair");
}
