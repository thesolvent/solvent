//! Decides *when* an order is worth filling, and fills it once.
//!
//! An order arriving from a feed is rarely fillable the instant it appears: its price slides toward
//! the filler's favour over a window, so the question is not "can we fill this" but "can we fill it
//! yet". This holds each intent and re-prices it every tick until the answer turns yes, its deadline
//! passes, or somebody else takes it.
//!
//! **Evaluate many times, act once.** Re-pricing is free — in-memory curve maths over a snapshot —
//! but committing is not, and the write path is built for a single attempt per order in three
//! separate places: the trade store dedups on the order hash, a reservation id is derived from
//! `(intent, plan)` and stays claimed even after it is voided, and submission is idempotent per
//! intent. Rather than unpick all three so a loop can retry, the loop only ever commits once. A
//! committed order leaves this service: the reconcile worker owns it from there.

use std::collections::BTreeMap;
use std::sync::Arc;

use alloy_primitives::Address;
use tokio::sync::mpsc;
use ulid::Ulid;

use crate::deps::ledger::Clock;
use crate::ledger::LedgerService;
use crate::obs::{debug, info, warn};
use crate::primitives::ingest::{Delivery, Intent};
use crate::primitives::routing::{RouteRequest, RoutingConfig};
use crate::primitives::trade::{TradeId, TradeStatus};
use crate::primitives::IntentId;
use crate::registry::SharedSnapshot;
use crate::routing::{route, LegCostResolver};
use crate::swap::{SwapService, TradePrices};
use crate::valuation::Valuation;

/// Knobs the loop needs beyond its collaborators.
#[derive(Clone, Debug)]
pub struct DecisionConfig {
    pub routing: RoutingConfig,
    /// The filler contract, matched against an order's exclusive filler to decide whether we owe
    /// the toll. This is the contract, not the operator account, because the settler compares it
    /// against its caller.
    pub filler: Address,
    /// Ceiling on intents held at once. The feed is public and its volume is not ours to control,
    /// so the loop is bounded by what it can price in a tick rather than by what arrives.
    pub max_tracked: usize,
}

/// The verdict on one intent at one instant.
enum Verdict {
    /// Not yet worth filling; hold it and try again.
    Wait,
    /// Worth filling now.
    Fill,
    /// Nothing further to do: expired, or unfillable by us at any point.
    Drop(&'static str),
}

pub struct DecisionService {
    registry: Arc<SharedSnapshot>,
    ledger: Arc<LedgerService>,
    swap: Arc<SwapService>,
    leg_cost: Arc<LegCostResolver>,
    valuation: Arc<Valuation>,
    clock: Arc<dyn Clock>,
    config: DecisionConfig,
}

impl DecisionService {
    pub fn new(
        registry: Arc<SharedSnapshot>,
        ledger: Arc<LedgerService>,
        swap: Arc<SwapService>,
        leg_cost: Arc<LegCostResolver>,
        valuation: Arc<Valuation>,
        clock: Arc<dyn Clock>,
        config: DecisionConfig,
    ) -> DecisionService {
        DecisionService {
            registry,
            ledger,
            swap,
            leg_cost,
            valuation,
            clock,
            config,
        }
    }

    /// Take arriving intents and re-price the held ones on every tick, until the channel closes.
    ///
    /// `ticks` paces the loop; one tick per block is the natural rate, since that is how often the
    /// chain state a decision rests on can change.
    pub async fn run<T>(&self, mut intents: mpsc::Receiver<Intent>, mut ticks: T)
    where
        T: futures::Stream<Item = ()> + Unpin,
    {
        use futures::StreamExt;
        let mut held: BTreeMap<IntentId, Intent> = BTreeMap::new();
        loop {
            tokio::select! {
                arrived = intents.recv() => match arrived {
                    Some(intent) => self.hold(&mut held, intent),
                    None => return,
                },
                tick = ticks.next() => match tick {
                    Some(()) => self.sweep(&mut held).await,
                    None => return,
                },
            }
        }
    }

    fn hold(&self, held: &mut BTreeMap<IntentId, Intent>, intent: Intent) {
        if held.len() >= self.config.max_tracked && !held.contains_key(&intent.id) {
            debug!("decision drop {}: tracking is full", intent.id);
            return;
        }
        held.insert(intent.id, intent);
    }

    /// One pass over every held intent.
    async fn sweep(&self, held: &mut BTreeMap<IntentId, Intent>) {
        let now = self.clock.now_unix();
        let mut done: Vec<IntentId> = Vec::new();
        for (id, intent) in held.iter() {
            match self.verdict(intent, now).await {
                Verdict::Wait => {}
                Verdict::Drop(reason) => {
                    debug!("decision drop {}: {}", id, reason);
                    done.push(*id);
                }
                Verdict::Fill => {
                    self.attempt(intent).await;
                    done.push(*id);
                }
            }
        }
        for id in done {
            held.remove(&id);
        }
    }

    /// Is this order worth filling at `now`? Read-only: it prices and routes against the live
    /// snapshot without reserving anything, so a tick that decides to wait costs nothing.
    async fn verdict(&self, intent: &Intent, now: u64) -> Verdict {
        let required = match standing(intent, self.config.filler, now) {
            Standing::Gone(reason) => return Verdict::Drop(reason),
            Standing::Barred => return Verdict::Wait,
            Standing::Fillable(required) => required,
        };
        let request = RouteRequest {
            intent: intent.id,
            token_in: intent.input.token,
            token_out: required.token,
            amount: required.amount,
            exact_in: false,
        };
        let per_leg_cost = self.leg_cost.for_request(&request).await;
        // The taker's input is the ceiling: a plan that cannot source the delivery within it, net of
        // gas, is not yet profitable. The router returning nothing is the whole test.
        let plan = route(
            &self.registry.load(),
            &self.ledger.snapshot(),
            &request,
            intent.input.curve.amount_at(now),
            &self.config.routing,
            per_leg_cost,
            None,
        );
        match plan {
            Some(_) => Verdict::Fill,
            None => Verdict::Wait,
        }
    }

    /// Commit: hand the intent to the write path, which routes it again, reserves, simulates and
    /// submits. Called at most once per intent — see the module note.
    async fn attempt(&self, intent: &Intent) {
        let prices = TradePrices {
            token_in_usd: self.usd(intent.input.token).await,
            token_out_usd: match intent.outputs.first() {
                Some(output) => self.usd(output.token).await,
                None => None,
            },
        };
        match self
            .swap
            .submit(intent.clone(), intent.swapper, TradeId(Ulid::new()), prices)
            .await
        {
            Ok(outcome) => match outcome.status {
                TradeStatus::Submitted => {
                    info!(
                        "intent {} submitted as trade {}",
                        intent.id, outcome.trade_id
                    )
                }
                other => {
                    debug!("intent {} not submitted: {:?}", intent.id, other)
                }
            },
            // Infrastructure, not a verdict on the order. The intent is dropped either way: another
            // attempt would collide with the trade, reservation and submission this one may already
            // have created.
            Err(e) => warn!("intent {} fill attempt failed: {}", intent.id, e),
        }
    }

    async fn usd(&self, token: Address) -> Option<f64> {
        self.valuation
            .price(token)
            .await
            .map(|price| price.to_f64())
    }
}

/// Where an order stands at one instant, before any market state is consulted.
#[derive(Debug, PartialEq, Eq)]
enum Standing {
    /// Past saving: it will never be fillable again.
    Gone(&'static str),
    /// Not fillable *yet*, but might be later — a window reserved for someone else that bars all
    /// comers. Distinct from `Gone`, because it lapses.
    Barred,
    /// Fillable now, if the price is right. Carries what the settler will demand.
    Fillable(Delivery),
}

/// The part of the decision that needs no market state: has the order expired, and may this filler
/// buy into it at all?
fn standing(intent: &Intent, filler: Address, now: u64) -> Standing {
    if intent.deadline <= now {
        return Standing::Gone("deadline passed");
    }
    if intent.input.curve.amount_at(now).is_zero() {
        return Standing::Gone("no input to receive");
    }
    match intent.required_output(filler, now) {
        Some(required) if !required.amount.is_zero() => Standing::Fillable(required),
        // Either the legs span several tokens — permanent, but the deadline ends the hold — or a
        // strict window bars us until it lapses.
        _ => Standing::Barred,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::ingest::{
        AmountCurve, Exclusivity, IntentInput, IntentOutput, IntentParts, ProtocolId,
    };
    use crate::primitives::{ChainId, IntentId};
    use alloy_primitives::{B256, U256};

    const US: u8 = 7;
    const THEM: u8 = 3;

    fn addr(n: u8) -> Address {
        Address::repeat_byte(n)
    }

    fn order(deadline: u64, exclusivity: Option<Exclusivity>) -> Intent {
        Intent::new(IntentParts {
            deadline,
            exclusivity,
            ..IntentParts::new(
                IntentId(B256::ZERO),
                ProtocolId::UniswapXV2,
                addr(9),
                IntentInput::new(addr(1), AmountCurve::scalar(U256::from(1_000u64))),
                vec![IntentOutput::new(
                    addr(2),
                    AmountCurve::scalar(U256::from(500u64)),
                    addr(9),
                )],
                ChainId(1),
            )
        })
    }

    #[test]
    fn an_expired_order_is_gone() {
        let o = order(100, None);
        assert_eq!(
            standing(&o, addr(US), 100),
            Standing::Gone("deadline passed")
        );
        assert_eq!(
            standing(&o, addr(US), 101),
            Standing::Gone("deadline passed")
        );
        assert!(matches!(standing(&o, addr(US), 99), Standing::Fillable(_)));
    }

    /// A window reserved for someone else with a toll is fillable — at a price. That is the whole
    /// point of the override, and treating it as unfillable would forfeit the only orders a
    /// non-quoter ever sees early.
    #[test]
    fn another_fillers_window_is_fillable_at_the_toll() {
        let o = order(1_000, Some(Exclusivity::new(addr(THEM), 500, 100)));
        match standing(&o, addr(US), 200) {
            Standing::Fillable(required) => assert_eq!(required.amount, U256::from(505u64)),
            other => panic!("expected a priced window, got {other:?}"),
        }
    }

    /// A strict window bars everyone else outright — but only until it lapses, so the order is held
    /// rather than dropped.
    #[test]
    fn a_strict_window_bars_us_until_it_lapses() {
        let o = order(1_000, Some(Exclusivity::new(addr(THEM), 500, 0)));
        assert_eq!(standing(&o, addr(US), 200), Standing::Barred);
        assert!(matches!(standing(&o, addr(US), 501), Standing::Fillable(_)));
    }

    #[test]
    fn our_own_window_costs_face_value() {
        let o = order(1_000, Some(Exclusivity::new(addr(US), 500, 100)));
        match standing(&o, addr(US), 200) {
            Standing::Fillable(required) => assert_eq!(required.amount, U256::from(500u64)),
            other => panic!("expected face value, got {other:?}"),
        }
    }

    /// Nothing to receive means nothing to earn, whatever the market does.
    #[test]
    fn an_order_paying_us_nothing_is_gone() {
        let mut o = order(1_000, None);
        o.input = IntentInput::new(addr(1), AmountCurve::scalar(U256::ZERO));
        assert_eq!(
            standing(&o, addr(US), 10),
            Standing::Gone("no input to receive")
        );
    }
}
