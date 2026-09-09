//! The canonical intent — the protocol-agnostic order every downstream slice (routing, ledger,
//! execution) consumes. Protocol-specific bytes ride along opaquely in `raw`, decoded only by the
//! protocol's own adapter, so adding a protocol is a new adapter with no change here.

use alloy_primitives::{Address, Bytes, U256};

use crate::primitives::ingest::curve::AmountCurve;
use crate::primitives::{ChainId, IntentId};

/// Basis-point denominator, as the settlement contracts use it.
const BPS: u64 = 10_000;

/// The source protocol of an intent; selects the normalizer that produced it and the fill builder
/// that will consume `raw`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum ProtocolId {
    UniswapXV2,
}

/// What the taker pays: the token and its amount over time.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct IntentInput {
    pub token: Address,
    pub curve: AmountCurve,
}

impl IntentInput {
    pub fn new(token: Address, curve: AmountCurve) -> IntentInput {
        IntentInput { token, curve }
    }
}

/// What the taker receives, and where: the token, its amount over time, and the recipient.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct IntentOutput {
    pub token: Address,
    pub curve: AmountCurve,
    pub recipient: Address,
}

impl IntentOutput {
    pub fn new(token: Address, curve: AmountCurve, recipient: Address) -> IntentOutput {
        IntentOutput {
            token,
            curve,
            recipient,
        }
    }
}

/// An exclusive-fill window. Until `ends_at`, `filler` fills at face value and anyone else pays
/// `override_bps` more on every output; `override_bps == 0` makes the window strict, and nobody
/// else can fill at all. `filler` is matched against the reactor's `msg.sender`, so it is the
/// resolver's filler *contract*, not the operator EOA.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Exclusivity {
    pub filler: Address,
    pub ends_at: u64,
    pub override_bps: u16,
}

impl Exclusivity {
    pub fn new(filler: Address, ends_at: u64, override_bps: u16) -> Exclusivity {
        Exclusivity {
            filler,
            ends_at,
            override_bps,
        }
    }

    /// Whether `filler` may fill at time `t` without paying the override — the reactor's
    /// `ExclusivityLib.hasFillingRights`, whose window comparison is strictly greater, so the
    /// window still holds *at* `ends_at`.
    pub fn grants_rights_to(&self, candidate: Address, t: u64) -> bool {
        t > self.ends_at || self.filler == candidate
    }
}

/// Everything an order's output legs come to at one instant: the token, and the total the settler
/// will demand across every leg.
///
/// An order routinely pays out more than once in the same token — the swapper plus an interface fee
/// recipient — and the settler collects the whole set. Sourcing only the first leg leaves the fill
/// short by the rest, which the filler discovers after it has already bought from every maker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Delivery {
    pub token: Address,
    pub amount: U256,
}

/// A normalized, protocol-agnostic order.
///
/// Built through [`Intent::new`] from an [`IntentParts`] literal rather than a positional argument
/// list: `swapper` and `settler` are both bare addresses, and transposing them is a class of bug no
/// test would catch.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Intent {
    /// The order hash.
    pub id: IntentId,
    pub protocol: ProtocolId,
    /// The account whose funds settle the input side — the reactor pulls from it via Permit2, and
    /// it is the trade's taker. Recovered by the protocol's adapter, since the canonical order does
    /// not otherwise carry it.
    pub swapper: Address,
    pub input: IntentInput,
    pub outputs: Vec<IntentOutput>,
    pub deadline: u64,
    pub exclusivity: Option<Exclusivity>,
    pub settler: Address,
    pub origin_chain: ChainId,
    /// Opaque encoded order, decoded only by the protocol's fill builder.
    pub raw: Bytes,
    /// The swapper's signature over `raw`, carried separately for the fill builder.
    pub signature: Bytes,
    /// Arrival time; routing prices the curves at this instant.
    pub observed_at: u64,
}

/// The fields of an [`Intent`], named at the construction site. Deliberately exhaustive: a new
/// required field should break every builder rather than default silently.
#[derive(Clone, Debug)]
pub struct IntentParts {
    pub id: IntentId,
    pub protocol: ProtocolId,
    pub swapper: Address,
    pub input: IntentInput,
    pub outputs: Vec<IntentOutput>,
    pub deadline: u64,
    pub exclusivity: Option<Exclusivity>,
    pub settler: Address,
    pub origin_chain: ChainId,
    pub raw: Bytes,
    pub signature: Bytes,
    pub observed_at: u64,
}

impl IntentParts {
    /// The fields every intent needs; the rest are set on the returned literal.
    pub fn new(
        id: IntentId,
        protocol: ProtocolId,
        swapper: Address,
        input: IntentInput,
        outputs: Vec<IntentOutput>,
        origin_chain: ChainId,
    ) -> IntentParts {
        IntentParts {
            id,
            protocol,
            swapper,
            input,
            outputs,
            deadline: 0,
            exclusivity: None,
            settler: Address::ZERO,
            origin_chain,
            raw: Bytes::new(),
            signature: Bytes::new(),
            observed_at: 0,
        }
    }
}

impl Intent {
    /// What this order's outputs come to at `at`: the delivered token and the sum across every leg.
    ///
    /// `None` when there is nothing to deliver, or when the legs span more than one token. The
    /// second case is not a shape we settle: every live mainnet order pays out in a single token,
    /// and sourcing several would need a route and a reservation per token that either all succeed
    /// or all unwind. Declining is honest; sourcing one of them and discovering the rest on chain
    /// is not.
    pub fn delivery(&self, at: u64) -> Option<Delivery> {
        let token = self.outputs.first()?.token;
        self.outputs
            .iter()
            .try_fold(U256::ZERO, |total, output| {
                (output.token == token).then(|| total.saturating_add(output.curve.amount_at(at)))
            })
            .map(|amount| Delivery { token, amount })
    }

    /// What `filler` must actually deliver at `at`: the delivery, raised by the exclusivity toll when
    /// the window belongs to someone else.
    ///
    /// Mirrors `ExclusivityLib._handleExclusiveOverride`. Rounding is up, as the contract's
    /// `mulDivUp` is — a wei short is an approval the reactor refuses. `None` when there is no single
    /// delivery, or when the window is strict (`override_bps == 0`) and not ours, since then nobody
    /// else may fill at any price.
    pub fn required_output(&self, filler: Address, at: u64) -> Option<Delivery> {
        let delivery = self.delivery(at)?;
        let Some(exclusivity) = self.exclusivity else {
            return Some(delivery);
        };
        if exclusivity.grants_rights_to(filler, at) {
            return Some(delivery);
        }
        if exclusivity.override_bps == 0 {
            return None;
        }
        let scale = U256::from(BPS + u64::from(exclusivity.override_bps));
        let amount = delivery
            .amount
            .checked_mul(scale)?
            .div_ceil(U256::from(BPS));
        Some(Delivery { amount, ..delivery })
    }

    pub fn new(parts: IntentParts) -> Intent {
        Intent {
            id: parts.id,
            protocol: parts.protocol,
            swapper: parts.swapper,
            input: parts.input,
            outputs: parts.outputs,
            deadline: parts.deadline,
            exclusivity: parts.exclusivity,
            settler: parts.settler,
            origin_chain: parts.origin_chain,
            raw: parts.raw,
            signature: parts.signature,
            observed_at: parts.observed_at,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::IntentId;
    use alloy_primitives::B256;

    fn addr(n: u8) -> Address {
        Address::repeat_byte(n)
    }

    fn intent(outputs: Vec<IntentOutput>) -> Intent {
        Intent::new(IntentParts::new(
            IntentId(B256::ZERO),
            ProtocolId::UniswapXV2,
            addr(9),
            IntentInput::new(addr(1), AmountCurve::scalar(U256::from(100u64))),
            outputs,
            ChainId(1),
        ))
    }

    fn output(token: u8, amount: u64) -> IntentOutput {
        IntentOutput::new(
            addr(token),
            AmountCurve::scalar(U256::from(amount)),
            addr(3),
        )
    }

    #[test]
    fn a_single_output_delivers_itself() {
        let delivery = intent(vec![output(2, 1_000)])
            .delivery(0)
            .expect("delivers");
        assert_eq!(delivery.token, addr(2));
        assert_eq!(delivery.amount, U256::from(1_000u64));
    }

    /// The shape every multi-output mainnet order takes: the swapper's leg plus an interface fee,
    /// same token, two recipients. Sourcing only the first is short by the fee.
    #[test]
    fn legs_in_one_token_sum() {
        let delivery = intent(vec![output(2, 1_000), output(2, 25)])
            .delivery(0)
            .expect("delivers");
        assert_eq!(delivery.token, addr(2));
        assert_eq!(delivery.amount, U256::from(1_025u64));
    }

    #[test]
    fn legs_in_different_tokens_have_no_single_delivery() {
        assert!(intent(vec![output(2, 1_000), output(4, 25)])
            .delivery(0)
            .is_none());
    }

    #[test]
    fn an_order_with_no_outputs_delivers_nothing() {
        assert!(intent(Vec::new()).delivery(0).is_none());
    }

    /// Each leg decays on its own curve, so the total is summed at the instant asked for, never
    /// scaled from one leg.
    fn reserved_for(filler: u8, bps: u16) -> Intent {
        let mut order = intent(vec![output(2, 1_000)]);
        order.exclusivity = Some(Exclusivity::new(addr(filler), 100, bps));
        order
    }

    const US: u8 = 7;

    #[test]
    fn our_own_window_costs_face_value() {
        let order = reserved_for(US, 100);
        let d = order.required_output(addr(US), 50).expect("fillable");
        assert_eq!(d.amount, U256::from(1_000u64));
    }

    /// Someone else's window: the toll applies until it ends, and the window is closed *at* its end
    /// instant — the reactor compares strictly greater.
    #[test]
    fn another_filler_pays_the_toll_until_the_window_ends() {
        let order = reserved_for(3, 100);
        let toll = |t| order.required_output(addr(US), t).expect("fillable").amount;
        assert_eq!(toll(50), U256::from(1_010u64), "inside the window");
        assert_eq!(toll(100), U256::from(1_010u64), "at the end instant");
        assert_eq!(toll(101), U256::from(1_000u64), "past it");
    }

    /// The contract rounds the scaled amount up; a wei short is an approval it refuses.
    #[test]
    fn the_toll_rounds_up() {
        let mut order = intent(vec![output(2, 1)]);
        order.exclusivity = Some(Exclusivity::new(addr(3), 100, 100));
        // 1 * 10100 / 10000 = 1.01 -> 2
        assert_eq!(
            order
                .required_output(addr(US), 50)
                .expect("fillable")
                .amount,
            U256::from(2u64)
        );
    }

    /// A strict window bars everyone but its holder, at any price.
    #[test]
    fn a_strict_window_is_unfillable_by_anyone_else() {
        let order = reserved_for(3, 0);
        assert!(order.required_output(addr(US), 50).is_none());
        assert!(order.required_output(addr(3), 50).is_some());
        assert!(
            order.required_output(addr(US), 101).is_some(),
            "once it lapses"
        );
    }

    #[test]
    fn an_open_order_never_pays_a_toll() {
        let order = intent(vec![output(2, 1_000)]);
        assert_eq!(
            order
                .required_output(addr(US), 50)
                .expect("fillable")
                .amount,
            U256::from(1_000u64)
        );
    }

    #[test]
    fn each_leg_is_priced_at_the_same_instant() {
        let falling = IntentOutput::new(
            addr(2),
            AmountCurve::dutch(U256::from(1_000u64), U256::from(900u64), 100, 200),
            addr(3),
        );
        let flat = output(2, 50);
        let order = intent(vec![falling, flat]);
        assert_eq!(
            order.delivery(100).expect("delivers").amount,
            U256::from(1_050u64)
        );
        assert_eq!(
            order.delivery(200).expect("delivers").amount,
            U256::from(950u64)
        );
    }
}
