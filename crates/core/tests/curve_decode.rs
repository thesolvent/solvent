//! Decode real shipped Aqua strategies (built off-chain via `@1inch/swap-vm-sdk`)
//! into `CurveSpec`, and price one end-to-end through the shared snapshot.
//!
//! Fixture: `tests/fixtures/aqua_strategies.json` (regenerate with the study lab's
//! `swapvm-lab/test/gen-strategies.test.ts`).

use alloy_primitives::{Address, Bytes, B256, U256};
use serde::Deserialize;
use solvent_core::primitives::registry::{
    decode_strategy, AquaEvent, CurveSpec, EventCursor, PeggedParams, Snapshot, StrategyKey,
};
use solvent_core::primitives::{MakerId, StrategyHash};
use solvent_core::registry::{price, Pricing, SharedSnapshot, XycPool};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Strat {
    curve: String,
    strategy_hex: String,
    sqrt_price_min: Option<String>,
    sqrt_price_max: Option<String>,
    x0: Option<String>,
    y0: Option<String>,
    linear_width: Option<String>,
    rate_lt: Option<String>,
    rate_gt: Option<String>,
}

#[derive(Deserialize)]
struct Fixture {
    strategies: Vec<Strat>,
}

fn u(s: &str) -> U256 {
    U256::from_str_radix(s, 10).expect("decimal U256")
}
fn bytes(hex: &str) -> Vec<u8> {
    alloy_primitives::hex::decode(hex).expect("hex")
}
fn field(v: &Option<String>) -> U256 {
    u(v.as_deref().expect("missing param"))
}
fn e18(n: u128) -> U256 {
    U256::from(n) * U256::from(1_000_000_000_000_000_000u128)
}

#[test]
fn decodes_real_shipped_strategies() {
    let fixture: Fixture =
        serde_json::from_str(include_str!("fixtures/aqua_strategies.json")).expect("parse fixture");
    assert!(fixture.strategies.len() >= 5);

    for s in &fixture.strategies {
        let spec = decode_strategy(&bytes(&s.strategy_hex));
        match s.curve.as_str() {
            "xyc" => assert_eq!(spec, CurveSpec::Xyc),
            "concentrate" => assert_eq!(
                spec,
                CurveSpec::Concentrate {
                    sqrt_price_min: field(&s.sqrt_price_min),
                    sqrt_price_max: field(&s.sqrt_price_max),
                }
            ),
            "pegged" => assert_eq!(
                spec,
                CurveSpec::Pegged(PeggedParams {
                    x0: field(&s.x0),
                    y0: field(&s.y0),
                    linear_width: field(&s.linear_width),
                    rate_lt: field(&s.rate_lt),
                    rate_gt: field(&s.rate_gt),
                })
            ),
            "unsupported" => assert_eq!(spec, CurveSpec::Unsupported),
            other => panic!("unknown curve {other}"),
        }
    }
}

#[test]
fn prices_a_real_strategy_through_the_shared_snapshot() {
    let fixture: Fixture =
        serde_json::from_str(include_str!("fixtures/aqua_strategies.json")).expect("parse fixture");
    let xyc = fixture
        .strategies
        .iter()
        .find(|s| s.curve == "xyc")
        .unwrap();

    let maker = MakerId(Address::from([0x11; 20]));
    let app = Address::from([0xAA; 20]);
    let strategy_hash = StrategyHash(B256::from([7; 32]));
    let key = StrategyKey {
        maker,
        app,
        strategy_hash,
    };
    let (t_in, t_out) = (Address::from([0x01; 20]), Address::from([0x02; 20]));
    let cur = |log: u64| EventCursor {
        block_number: 1,
        log_index: log,
    };

    // Fold a real Shipped (decoded in-fold) + the two initial Pushed legs.
    let mut snap = Snapshot::default();
    snap.apply(
        cur(0),
        AquaEvent::Shipped {
            maker,
            app,
            strategy_hash,
            strategy: Bytes::from(bytes(&xyc.strategy_hex)),
        },
    );
    snap.apply(
        cur(1),
        AquaEvent::Pushed {
            maker,
            app,
            strategy_hash,
            token: t_in,
            amount: e18(1000),
        },
    );
    snap.apply(
        cur(2),
        AquaEvent::Pushed {
            maker,
            app,
            strategy_hash,
            token: t_out,
            amount: e18(1000),
        },
    );

    let shared = SharedSnapshot::new(snap);
    let snapshot = shared.load();
    let strategy = snapshot.strategy(&key).expect("strategy present");
    assert_eq!(strategy.curve, CurveSpec::Xyc);

    let got = price(strategy, t_in, t_out, e18(100), true).unwrap();
    let expected = XycPool::from_reserves(e18(1000), e18(1000))
        .quote_exact_in(e18(100))
        .unwrap();
    assert_eq!(got, expected);
}
