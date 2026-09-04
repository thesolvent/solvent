//! Live E2E — strategy *breadth*: ships every Tier-0 variant and asserts the
//! real pipeline (`AlloyChainSource` → `RegistrySync` → `PgStore` →
//! `SharedSnapshot` → `price`) backfills, prices each curve bit-exactly against
//! on-chain `quote()`, dedupes re-scans, and recovers — all at the initial state.
//! State-change-over-time is covered by `e2e_watcher`.
//!
//! Gated on `TEST_DATABASE_URL` + a spawnable `anvil`. Run:
//!   scripts/with-postgres.sh cargo test -p solvent-adapters --test e2e_registry

mod common;

use std::sync::Arc;

use common::{pipeline, strategy_key, Harness, StrategySpec, CHAIN};
use solvent_adapters::registry::PgStore;
use solvent_core::{
    primitives::{registry::Snapshot, ChainConfig},
    registry::{price, PriceError, RegistrySync, SharedSnapshot},
};

#[tokio::test]
async fn e2e_registry_full_pipeline() {
    let Some(db_url) = common::db_url_or_skip() else {
        return;
    };

    let h = Harness::setup().await;
    h.ship_all().await;
    let (sync, snapshot, pool) = pipeline(&h, &db_url, CHAIN).await;
    let to_block = h.latest_block().await;
    sync.sync_once(to_block).await.expect("sync");

    // E1 — cold-start backfill: every strategy folded with on-chain balances.
    {
        let snap = snapshot.load();
        assert_eq!(snap.len(), h.fx.strategies.len(), "every strategy folded");
        for spec in &h.fx.strategies {
            let strat = snap
                .strategy(&spec.key(h.maker, h.app))
                .unwrap_or_else(|| panic!("{} present", spec.label));
            assert!(strat.active, "{} active", spec.label);
            h.assert_balances(&snap, spec).await;
        }
    }

    // Pricing parity + guard neutrality + unsupported.
    for spec in &h.fx.strategies {
        let snap = snapshot.load();
        let strat = snap.strategy(&spec.key(h.maker, h.app)).expect("present");
        if !spec.supported {
            assert_eq!(
                price(strat, h.t1, h.t0, common::e18(1), true),
                Err(PriceError::Unsupported),
                "{} must be unsupported",
                spec.label
            );
        } else if spec.guarded {
            assert_guard_is_neutral(&snap, &h.fx.strategies, spec, &h);
        } else {
            h.assert_price_parity(strat, spec).await;
        }
    }

    // E3 — effectively-once: re-scanning the same range changes nothing.
    let before = snapshot.load();
    sync.sync_once(to_block).await.expect("re-sync");
    assert_eq!(&*snapshot.load(), &*before, "re-scan changed the snapshot");

    // E2 — restart recovery rebuilds an identical snapshot from the store.
    {
        let config = ChainConfig::new(CHAIN, 0, 25, 15);
        let store = PgStore::new(pool.clone());
        let recovered = Arc::new(SharedSnapshot::default());
        let restart = RegistrySync::new(
            &config,
            h.chain_source(),
            Arc::new(store),
            recovered.clone(),
        );
        restart.recover().await.expect("recover");
        assert_eq!(&*recovered.load(), &*before, "recovery is not identical");
    }

    // E5 — dock tombstones + de-indexes.
    let docked = &h.fx.strategies[0];
    h.dock(docked).await;
    let to_block = h.latest_block().await;
    sync.sync_once(to_block).await.expect("sync after dock");
    {
        let snap = snapshot.load();
        let key = docked.key(h.maker, h.app);
        let strat = snap.strategy(&key).expect("docked strategy retained");
        assert!(!strat.active, "docked strategy inactive");
        let routable = strat
            .pair()
            .map(|pair| snap.active_strategies_for_pair(pair).any(|s| s.key == key))
            .unwrap_or(false);
        assert!(!routable, "docked strategy must not be routable");
    }
}

/// A guarded strategy prices identically to its non-guarded twin (same curve +
/// fees): the tx-origin gate is price-neutral. Quoting it on-chain would revert
/// without the gate token, so parity rides on the twin (asserted separately).
fn assert_guard_is_neutral(
    snap: &Snapshot,
    strategies: &[StrategySpec],
    guarded: &StrategySpec,
    h: &Harness,
) {
    let twin = strategies
        .iter()
        .find(|s| {
            !s.guarded
                && s.supported
                && s.curve == guarded.curve
                && s.fees_in_bps == guarded.fees_in_bps
        })
        .unwrap_or_else(|| panic!("{} has a non-guarded twin", guarded.label));
    let guarded_strat = snap
        .strategy(&strategy_key(h.maker, h.app, &guarded.strategy_hex))
        .expect("guarded present");
    let twin_strat = snap
        .strategy(&strategy_key(h.maker, h.app, &twin.strategy_hex))
        .expect("twin present");
    for (t_in, t_out) in [(h.t1, h.t0), (h.t0, h.t1)] {
        let bal_in = if t_in == h.t0 {
            guarded.ship_lo
        } else {
            guarded.ship_hi
        };
        for div in [1000u64, 100, 10] {
            let amount = bal_in / alloy::primitives::U256::from(div);
            assert_eq!(
                price(guarded_strat, t_in, t_out, amount, true),
                price(twin_strat, t_in, t_out, amount, true),
                "{} must price like its twin {}",
                guarded.label,
                twin.label
            );
        }
    }
}
