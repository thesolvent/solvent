//! `AquaSettledLegsReader` — the [`SettledLegsReader`] over Aqua fills: read a confirmed fill's
//! *actual* per-leg amounts from its own `Pushed` (what the maker received) and `Pulled` (what it
//! gave) events, overlaid onto the plan legs' identities. Reuses the shared Aqua decode.

use std::sync::Arc;

use alloy::primitives::{Address, B256, U256};
use async_trait::async_trait;
use solvent_core::deps::recapture::{SettledLegsError, SettledLegsReader};
use solvent_core::primitives::registry::AquaEvent;
use solvent_core::primitives::routing::RouteLeg;
use solvent_core::primitives::{MakerId, StrategyHash};

use crate::aqua::events_in_tx;
use crate::events::prelude::Provider;

/// Reads settled legs from a fill's Aqua events. `aqua` is the liquidity contract whose
/// `Pushed`/`Pulled` events record what each maker received and gave — the same contract the ledger
/// settlement reader reads.
pub struct AquaSettledLegsReader {
    provider: Arc<dyn Provider>,
    aqua: Address,
}

impl AquaSettledLegsReader {
    pub fn new(provider: Arc<dyn Provider>, aqua: Address) -> Self {
        Self { provider, aqua }
    }
}

#[async_trait]
impl SettledLegsReader for AquaSettledLegsReader {
    async fn actual_legs(
        &self,
        tx: B256,
        legs: &[RouteLeg],
    ) -> Result<Vec<RouteLeg>, SettledLegsError> {
        let events = events_in_tx(&self.provider, self.aqua, tx)
            .await
            .map_err(SettledLegsError::Read)?;
        Ok(legs
            .iter()
            .map(|leg| RouteLeg {
                amount_in: moved(&events, leg.maker, leg.strategy_hash, leg.token_in, Dir::In),
                amount_out: moved(
                    &events,
                    leg.maker,
                    leg.strategy_hash,
                    leg.token_out,
                    Dir::Out,
                ),
                ..leg.clone()
            })
            .collect())
    }
}

#[derive(Clone, Copy)]
enum Dir {
    In,
    Out,
}

/// Sum what a fill moved for one `(maker, strategy, token)` in one direction — `Pushed` for what the
/// maker received (`In`), `Pulled` for what it gave (`Out`). Zero if the fill never moved it.
fn moved(
    events: &[AquaEvent],
    maker: MakerId,
    strategy: StrategyHash,
    token: Address,
    dir: Dir,
) -> U256 {
    events
        .iter()
        .filter_map(|event| match (dir, event) {
            (
                Dir::In,
                AquaEvent::Pushed {
                    maker: m,
                    strategy_hash: s,
                    token: t,
                    amount,
                    ..
                },
            )
            | (
                Dir::Out,
                AquaEvent::Pulled {
                    maker: m,
                    strategy_hash: s,
                    token: t,
                    amount,
                    ..
                },
            ) if *m == maker && *s == strategy && *t == token => Some(*amount),
            _ => None,
        })
        .fold(U256::ZERO, |acc, amount| acc.saturating_add(amount))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(n: u8) -> Address {
        Address::from([n; 20])
    }
    fn maker(n: u8) -> MakerId {
        MakerId(addr(n))
    }
    fn strat(n: u8) -> StrategyHash {
        StrategyHash(B256::from([n; 32]))
    }
    fn t0() -> Address {
        addr(0xaa)
    }
    fn t1() -> Address {
        addr(0xbb)
    }

    /// A reverse-buy leg: the maker receives t0 (`token_in`) and gives t1 (`token_out`).
    fn leg(amount_in: u64, amount_out: u64) -> RouteLeg {
        RouteLeg {
            maker: maker(1),
            strategy_hash: strat(1),
            token_in: t0(),
            token_out: t1(),
            amount_in: U256::from(amount_in),
            amount_out: U256::from(amount_out),
        }
    }
    fn pushed(token: Address, amount: u64) -> AquaEvent {
        AquaEvent::Pushed {
            maker: maker(1),
            app: Address::ZERO,
            strategy_hash: strat(1),
            token,
            amount: U256::from(amount),
        }
    }
    fn pulled(token: Address, amount: u64) -> AquaEvent {
        AquaEvent::Pulled {
            maker: maker(1),
            app: Address::ZERO,
            strategy_hash: strat(1),
            token,
            amount: U256::from(amount),
        }
    }

    fn overlay(events: &[AquaEvent], legs: &[RouteLeg]) -> Vec<RouteLeg> {
        legs.iter()
            .map(|l| RouteLeg {
                amount_in: moved(events, l.maker, l.strategy_hash, l.token_in, Dir::In),
                amount_out: moved(events, l.maker, l.strategy_hash, l.token_out, Dir::Out),
                ..l.clone()
            })
            .collect()
    }

    // The overlay takes amounts from the events, not the plan: the maker actually received 1350 t0
    // and gave 500 t1, so the leg comes back re-amounted to those, regardless of the plan's numbers.
    #[test]
    fn overlays_actual_pushed_and_pulled() {
        let events = vec![pushed(t0(), 1350), pulled(t1(), 500)];
        let plan = [leg(2700, 1000)];
        let actual = overlay(&events, &plan);
        assert_eq!(actual[0].amount_in, U256::from(1350u64));
        assert_eq!(actual[0].amount_out, U256::from(500u64));
        // Identity is preserved from the plan.
        assert_eq!(actual[0].maker, maker(1));
        assert_eq!(actual[0].token_in, t0());
        assert_eq!(actual[0].token_out, t1());
    }

    // Multiple pulls of the same side sum; a leg the fill never touched comes back with zeros.
    #[test]
    fn sums_same_side_and_zeroes_untouched() {
        let events = vec![pushed(t0(), 1000), pushed(t0(), 350), pulled(t1(), 500)];
        assert_eq!(
            moved(&events, maker(1), strat(1), t0(), Dir::In),
            U256::from(1350u64)
        );
        // A different maker the fill never touched → zero both sides.
        let untouched = RouteLeg {
            maker: maker(2),
            ..leg(2700, 1000)
        };
        let actual = overlay(&events, std::slice::from_ref(&untouched));
        assert_eq!(actual[0].amount_in, U256::ZERO);
        assert_eq!(actual[0].amount_out, U256::ZERO);
    }

    // Direction is honored: a `Pushed` on the token never counts as `Pulled` (the wrong-direction
    // guard that keeps the taker's inflow from being booked as maker output, and vice versa).
    #[test]
    fn direction_is_not_conflated() {
        let events = vec![pushed(t1(), 999)];
        // Asking for what the maker *gave* of t1 finds nothing — only a Pushed exists.
        assert_eq!(
            moved(&events, maker(1), strat(1), t1(), Dir::Out),
            U256::ZERO
        );
        assert_eq!(
            moved(&events, maker(1), strat(1), t1(), Dir::In),
            U256::from(999u64)
        );
    }
}
