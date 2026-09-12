//! Fast, explicitly non-executable UniswapX quote simulation.
//!
//! A prewarmed batch supplies random maker shapes before an order arrives. At quote time the
//! current market price binds those shapes to the order's pair, so the hot path performs only
//! bounded in-memory arithmetic.

use std::collections::VecDeque;

use alloy::primitives::{Address, U256, U512};
use parking_lot::Mutex;
use rand::{rngs::StdRng, Rng, SeedableRng};
use solvent_core::registry::{ConcentratePool, Pricing, XycPool};

const MAKERS_PER_BATCH: usize = 8;
const BPS: u64 = 10_000;
const Q18: u64 = 1_000_000_000_000_000_000;
const MIN_LIQUIDITY_MULTIPLIER: u64 = 50;
const MAX_LIQUIDITY_MULTIPLIER: u64 = 500;
const MAX_PRICE_BIAS_BPS: i16 = 12;
const MIN_CONCENTRATION_WIDTH_BPS: u16 = 500;
const MAX_CONCENTRATION_WIDTH_BPS: u16 = 2_000;

/// A queue of prebuilt simulated-maker shapes. Each claim returns a fresh batch and replaces it
/// before releasing the lock, leaving the next order with a ready batch.
pub struct SimulatedBatchPool {
    ready_batches: usize,
    state: Mutex<BatchState>,
}

struct BatchState {
    rng: StdRng,
    next_id: u64,
    ready: VecDeque<SimulatedBatch>,
}

impl SimulatedBatchPool {
    /// Production batches are random; their values are simulation inputs, never execution terms.
    pub fn random(ready_batches: usize) -> Self {
        Self::with_rng(StdRng::from_entropy(), ready_batches)
    }

    /// Initializes the pool from a supplied random seed.
    pub fn seeded(seed: u64, ready_batches: usize) -> Self {
        Self::with_rng(StdRng::seed_from_u64(seed), ready_batches)
    }

    fn with_rng(rng: StdRng, ready_batches: usize) -> Self {
        let pool = Self {
            ready_batches: ready_batches.max(1),
            state: Mutex::new(BatchState {
                rng,
                next_id: 0,
                ready: VecDeque::new(),
            }),
        };
        let mut state = pool.state.lock();
        refill(&mut state, pool.ready_batches);
        drop(state);
        pool
    }

    /// Claim one prewarmed eight-maker batch. The replacement is generated before returning so
    /// subsequent orders never wait for a market read or a background RPC.
    pub fn claim(&self) -> SimulatedBatch {
        let mut state = self.state.lock();
        let batch = state
            .ready
            .pop_front()
            .unwrap_or_else(|| next_batch(&mut state));
        refill(&mut state, self.ready_batches);
        batch
    }

    pub fn ready_batches(&self) -> usize {
        self.state.lock().ready.len()
    }
}

fn refill(state: &mut BatchState, target: usize) {
    while state.ready.len() < target {
        let batch = next_batch(state);
        state.ready.push_back(batch);
    }
}

fn next_batch(state: &mut BatchState) -> SimulatedBatch {
    let id = state.next_id;
    state.next_id = state.next_id.wrapping_add(1);
    let makers = (0..MAKERS_PER_BATCH)
        .map(|_| MakerShape {
            curve: match state.rng.gen_bool(0.5) {
                true => CurveShape::FullRange,
                false => CurveShape::Concentrated {
                    width_bps: state
                        .rng
                        .gen_range(MIN_CONCENTRATION_WIDTH_BPS..=MAX_CONCENTRATION_WIDTH_BPS),
                },
            },
            price_bias_bps: state
                .rng
                .gen_range(-MAX_PRICE_BIAS_BPS..=MAX_PRICE_BIAS_BPS),
            liquidity_multiplier: state
                .rng
                .gen_range(MIN_LIQUIDITY_MULTIPLIER..=MAX_LIQUIDITY_MULTIPLIER),
        })
        .collect();
    SimulatedBatch { id, makers }
}

/// One prewarmed virtual-maker batch. It has no token or price until an observed order claims it.
#[derive(Clone)]
pub struct SimulatedBatch {
    id: u64,
    makers: Vec<MakerShape>,
}

impl SimulatedBatch {
    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn maker_count(&self) -> usize {
        self.makers.len()
    }
}

#[derive(Clone)]
struct MakerShape {
    curve: CurveShape,
    price_bias_bps: i16,
    liquidity_multiplier: u64,
}

#[derive(Clone)]
enum CurveShape {
    FullRange,
    Concentrated { width_bps: u16 },
}

/// Input to a synchronous simulated quote. The rate is output whole-tokens per input whole-token,
/// scaled by 1e18 from the latest cached market price.
pub struct SimulatedQuoteRequest {
    pub token_in: Address,
    pub token_out: Address,
    pub amount_in: U256,
    pub input_decimals: u8,
    pub output_decimals: u8,
    pub market_out_per_in_q18: U256,
}

/// The best output from a fixed simulation batch. It is indicative only and cannot authorize a
/// fill, reserve maker capital, or enter the execution path.
#[non_exhaustive]
pub struct SimulatedQuote {
    pub batch_id: u64,
    pub amount_out: U256,
    pub makers_evaluated: usize,
}

pub struct SimulatedQuoteEngine;

impl SimulatedQuoteEngine {
    pub fn new() -> Self {
        Self
    }

    /// Price an order against the batch's eight virtual makers. There is no I/O: callers supply a
    /// market rate already read from the Binance-backed cache.
    pub fn quote(
        &self,
        batch: &SimulatedBatch,
        request: SimulatedQuoteRequest,
    ) -> Option<SimulatedQuote> {
        if request.token_in == request.token_out
            || request.amount_in.is_zero()
            || request.market_out_per_in_q18.is_zero()
        {
            return None;
        }
        let raw_market_rate_q18 = raw_market_rate_q18(&request)?;
        let amount_out = batch.makers.iter().try_fold(U256::ZERO, |best, maker| {
            quote_maker(maker, &request, raw_market_rate_q18).map(|amount_out| best.max(amount_out))
        })?;
        if amount_out.is_zero() {
            return None;
        }
        Some(SimulatedQuote {
            batch_id: batch.id,
            amount_out,
            makers_evaluated: batch.makers.len(),
        })
    }
}

impl Default for SimulatedQuoteEngine {
    fn default() -> Self {
        Self::new()
    }
}

fn quote_maker(
    maker: &MakerShape,
    request: &SimulatedQuoteRequest,
    raw_market_rate_q18: U256,
) -> Option<U256> {
    let rate_q18 = biased_rate(raw_market_rate_q18, maker.price_bias_bps)?;
    let reserve_in = request
        .amount_in
        .checked_mul(U256::from(maker.liquidity_multiplier))?;
    let reserve_out = mul_div(reserve_in, rate_q18, U256::from(Q18))?;
    if reserve_out.is_zero() {
        return None;
    }
    match maker.curve {
        CurveShape::FullRange => XycPool::from_reserves(reserve_in, reserve_out)
            .quote_exact_in(request.amount_in)
            .ok(),
        CurveShape::Concentrated { width_bps } => {
            let (min, max) = concentrated_bounds(request, rate_q18, width_bps)?;
            ConcentratePool::from_reserves_and_bounds(
                request.token_in,
                request.token_out,
                reserve_in,
                reserve_out,
                min,
                max,
            )
            .quote_exact_in(request.amount_in)
            .ok()
        }
    }
}

/// The output-raw per input-raw rate, preserved in Q18 form so sub-unit raw rates remain
/// representable for mixed-decimal pairs.
fn raw_market_rate_q18(request: &SimulatedQuoteRequest) -> Option<U256> {
    let output_scale = pow10(request.output_decimals)?;
    let input_scale = pow10(request.input_decimals)?;
    mul_div(request.market_out_per_in_q18, output_scale, input_scale)
}

fn concentrated_bounds(
    request: &SimulatedQuoteRequest,
    raw_rate_q18: U256,
    width_bps: u16,
) -> Option<(U256, U256)> {
    let canonical_rate_q18 = match request.token_in < request.token_out {
        true => raw_rate_q18,
        false => mul_div(U256::from(Q18), U256::from(Q18), raw_rate_q18)?,
    };
    let anchor = canonical_rate_q18.checked_mul(U256::from(Q18))?.root(2);
    let width = U256::from(BPS.checked_add(u64::from(width_bps))?);
    let min = mul_div(anchor, U256::from(BPS), width)?;
    let max = mul_div(anchor, width, U256::from(BPS))?;
    (!min.is_zero() && min < max).then_some((min, max))
}

fn pow10(decimals: u8) -> Option<U256> {
    (0..decimals).try_fold(U256::from(1u8), |scale, _| {
        scale.checked_mul(U256::from(10u8))
    })
}

fn biased_rate(rate: U256, bias_bps: i16) -> Option<U256> {
    let factor = match bias_bps.is_negative() {
        true => BPS.checked_sub(u64::from(bias_bps.unsigned_abs()))?,
        false => BPS.checked_add(u64::from(bias_bps as u16))?,
    };
    mul_div(rate, U256::from(factor), U256::from(BPS))
}

fn mul_div(a: U256, b: U256, denominator: U256) -> Option<U256> {
    if denominator.is_zero() {
        return None;
    }
    let quotient = (U512::from(a) * U512::from(b)) / U512::from(denominator);
    if quotient > U512::from(U256::MAX) {
        return None;
    }
    let limbs = quotient.as_limbs();
    Some(U256::from_limbs([limbs[0], limbs[1], limbs[2], limbs[3]]))
}
