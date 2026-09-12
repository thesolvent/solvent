//! Our curve engine priced against the deployed SwapVM router's own `quote()`.
//!
//! The cases are real mainnet strategies, their real Aqua `safeBalances`, and the amounts the
//! deployed router returned for them on a fork — so the oracle here is the production contract
//! rather than a transcription of it. Regenerate the fixture by re-reading `Shipped` from Aqua and
//! re-quoting; the amounts are pinned to the block they were taken at.

use alloy_primitives::{Address, Bytes, B256, U256};
use serde::Deserialize;
use solvent_core::primitives::registry::{AquaEvent, CurveSpec, Snapshot, StrategyKey};
use solvent_core::primitives::{MakerId, StrategyHash};
use solvent_core::registry::{price, SharedSnapshot};

#[derive(Deserialize)]
struct Case {
    /// `CLEAN` for a program we decode today; `FEE` for one carrying the Aqua protocol fee.
    kind: String,
    hash: String,
    strategy: String,
    token0: String,
    token1: String,
    bal0: String,
    bal1: String,
    amount_in: String,
    router_amount_out: String,
}

#[derive(Deserialize)]
struct Fixture {
    cases: Vec<Case>,
}

fn addr(s: &str) -> Address {
    s.parse().expect("address")
}
fn amount(s: &str) -> U256 {
    U256::from_str_radix(s, 10).expect("u256")
}

/// What our engine makes of one case: the priced amount, or `None` when the decoder declines.
fn ours(case: &Case) -> Option<U256> {
    let maker = MakerId(Address::from([0x11; 20]));
    let app = Address::from([0xAA; 20]);
    let strategy_hash = StrategyHash(B256::from([7; 32]));
    let (t0, t1) = (addr(&case.token0), addr(&case.token1));

    let mut snap = Snapshot::default();
    snap.apply(AquaEvent::Shipped {
        maker,
        app,
        strategy_hash,
        strategy: Bytes::from(alloy_primitives::hex::decode(&case.strategy).expect("hex")),
    });
    for (token, balance) in [(t0, &case.bal0), (t1, &case.bal1)] {
        snap.apply(AquaEvent::Pushed {
            maker,
            app,
            strategy_hash,
            token,
            amount: amount(balance),
        });
    }
    let shared = SharedSnapshot::new(snap);
    let snapshot = shared.load();
    let strategy = snapshot
        .strategy(&StrategyKey {
            maker,
            app,
            strategy_hash,
        })
        .expect("strategy present");
    match strategy.curve {
        CurveSpec::Priceable { .. } => price(strategy, t0, t1, amount(&case.amount_in), true).ok(),
        _ => None,
    }
}

fn fixture() -> Fixture {
    serde_json::from_str(include_str!("fixtures/mainnet_fork_parity.json")).expect("fixture")
}

/// Every program we claim to price must price *exactly* as the deployed router does. A wei of
/// divergence is a fill that reverts after the makers have been bought, so there is no tolerance
/// to give here.
#[test]
fn we_price_decodable_strategies_exactly_as_the_router_does() {
    let fx = fixture();
    let clean: Vec<&Case> = fx
        .cases
        .iter()
        .filter(|c| c.kind == "CLEAN" || c.kind == "OURS")
        .collect();
    assert!(!clean.is_empty(), "fixture has decodable cases");
    for case in clean {
        assert_eq!(
            ours(case),
            Some(amount(&case.router_amount_out)),
            "strategy {}",
            case.hash
        );
    }
}

/// The Aqua protocol fee is not implemented, and until it is these must be declined rather than
/// priced. Skipping the instruction would over-quote — measured on this fixture at up to 0.025% —
/// and the router refuses an amount it cannot source.
#[test]
fn strategies_carrying_the_protocol_fee_are_declined_not_guessed() {
    let fx = fixture();
    let fee: Vec<&Case> = fx.cases.iter().filter(|c| c.kind == "FEE").collect();
    assert!(!fee.is_empty(), "fixture has protocol-fee cases");
    for case in fee {
        assert_eq!(ours(case), None, "strategy {}", case.hash);
    }
}
