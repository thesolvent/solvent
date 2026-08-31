//! Candidate selection: reduce an intent's pair to a bounded, deterministic set of
//! priceable maker venues for the solver to optimize over.
//!
//! Like a production router (Uniswap's `get-candidate-pools`), this ranks by a cheap
//! depth proxy and keeps a top-`k` before any exact quoting — here the proxy is
//! `cap_out`, the maker's frozen deliverable. The pruning only bites when a pair has
//! more than `k` makers; below that it is a no-op ordering. Caps are frozen by reading
//! `caps` once; the ledger re-checks the firm figure at reservation time.

use alloy_primitives::{Address, U256};

use crate::ledger::AvailableSnapshot;
use crate::primitives::ledger::AccountKey;
use crate::primitives::registry::{CurveSpec, MakerStrategy, Snapshot, StrategyKey, TokenPair};
use crate::registry::CurvePool;

/// A frozen, priceable maker venue for one `token_in -> token_out` direction. Carries
/// the oriented curve and its flat fees so the solver can price it net-of-fee.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Candidate {
    pub key: StrategyKey,
    pub token_in: Address,
    pub token_out: Address,
    /// Max deliverable output — the frozen `min(wallet budget, strategy virtual)`.
    pub cap_out: U256,
    pub pool: CurvePool,
    pub fees_in_bps: Vec<u32>,
}

/// The most promising venues for `token_in -> token_out`, deterministically ordered and
/// capped at `k`. Ranks by deliverable depth (`cap_out`) — the analogue of a liquidity
/// sort — then by `strategy_hash` for a stable order independent of snapshot iteration.
pub fn select(
    snapshot: &Snapshot,
    caps: &AvailableSnapshot,
    token_in: Address,
    token_out: Address,
    k: usize,
) -> Vec<Candidate> {
    let pair = TokenPair::new(token_in, token_out);
    let mut out: Vec<Candidate> = snapshot
        .active_strategies_for_pair(pair)
        .filter_map(|strategy| build_candidate(strategy, caps, token_in, token_out))
        .collect();
    out.sort_by(|a, b| {
        b.cap_out
            .cmp(&a.cap_out)
            .then_with(|| a.key.strategy_hash.cmp(&b.key.strategy_hash))
    });
    out.truncate(k);
    out
}

/// Build one eligible candidate, or `None` when the strategy can't source this pair:
/// an unsupported curve, an empty reserve side, or no deliverable room left.
fn build_candidate(
    strategy: &MakerStrategy,
    caps: &AvailableSnapshot,
    token_in: Address,
    token_out: Address,
) -> Option<Candidate> {
    let CurveSpec::Priceable { curve, fees_in_bps } = &strategy.curve else {
        return None;
    };
    let balance_in = strategy.balance(&token_in);
    let balance_out = strategy.balance(&token_out);
    if balance_in.is_zero() || balance_out.is_zero() {
        return None;
    }
    let cap_out = frozen_cap(strategy, caps, token_out);
    if cap_out.is_zero() {
        return None;
    }
    Some(Candidate {
        key: strategy.key,
        token_in,
        token_out,
        cap_out,
        pool: CurvePool::from_curve(curve, token_in, token_out, balance_in, balance_out),
        fees_in_bps: fees_in_bps.clone(),
    })
}

/// The two-ceiling cap for the maker's payout token: `min(wallet budget, strategy virtual)`.
fn frozen_cap(strategy: &MakerStrategy, caps: &AvailableSnapshot, token_out: Address) -> U256 {
    let wallet = caps.available(&AccountKey::WalletBudget {
        maker: strategy.key.maker,
        token: token_out,
    });
    let strategy_virtual = caps.available(&AccountKey::StrategyVirtual {
        maker: strategy.key.maker,
        strategy_hash: strategy.key.strategy_hash,
        token: token_out,
    });
    wallet.min(strategy_virtual)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::registry::Curve;
    use crate::primitives::{MakerId, StrategyHash};
    use alloy_primitives::B256;
    use std::collections::BTreeMap;

    fn tok(n: u8) -> Address {
        Address::from([n; 20])
    }
    fn maker(n: u8) -> MakerId {
        MakerId(Address::from([n; 20]))
    }
    fn hash(n: u8) -> StrategyHash {
        StrategyHash(B256::from([n; 32]))
    }
    fn wallet(m: MakerId, token: Address) -> AccountKey {
        AccountKey::WalletBudget { maker: m, token }
    }
    fn virt(m: MakerId, h: StrategyHash, token: Address) -> AccountKey {
        AccountKey::StrategyVirtual {
            maker: m,
            strategy_hash: h,
            token,
        }
    }

    /// An XYC strategy over `(in_tok, out_tok)` with equal reserves `bal`.
    fn xyc(
        m: MakerId,
        h: StrategyHash,
        in_tok: Address,
        out_tok: Address,
        bal: u64,
    ) -> MakerStrategy {
        let mut balances = BTreeMap::new();
        balances.insert(in_tok, U256::from(bal));
        balances.insert(out_tok, U256::from(bal));
        MakerStrategy {
            key: StrategyKey {
                maker: m,
                app: Address::ZERO,
                strategy_hash: h,
            },
            curve: CurveSpec::Priceable {
                curve: Curve::Xyc,
                fees_in_bps: vec![],
            },
            balances,
            active: true,
        }
    }

    fn caps(entries: &[(AccountKey, u64)]) -> AvailableSnapshot {
        AvailableSnapshot(entries.iter().map(|(k, v)| (*k, U256::from(*v))).collect())
    }

    #[test]
    fn freezes_cap_as_min_of_the_two_ceilings() {
        let (m, h) = (maker(1), hash(1));
        let (a, b) = (tok(1), tok(2));
        let snap = Snapshot::from_strategies([xyc(m, h, a, b, 1000)]);
        // Payout token is b; wallet 100, strategy-virtual 60 ⇒ cap is the tighter 60.
        let c = caps(&[(wallet(m, b), 100), (virt(m, h, b), 60)]);
        let out = select(&snap, &c, a, b, 64);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].cap_out, U256::from(60u64));
        assert_eq!(out[0].token_out, b);
    }

    #[test]
    fn drops_unsupported_zero_cap_and_empty_reserve() {
        let (a, b) = (tok(1), tok(2));
        let mut unsupported = xyc(maker(1), hash(1), a, b, 1000);
        unsupported.curve = CurveSpec::Unsupported;
        let zero_cap = xyc(maker(2), hash(2), a, b, 1000);
        let mut empty = xyc(maker(3), hash(3), a, b, 1000);
        empty.balances.insert(b, U256::ZERO);
        let snap = Snapshot::from_strategies([unsupported, zero_cap, empty]);
        // Ample room everywhere except maker(2)'s virtual ceiling (0) — so only the
        // eligibility rules, not the caps, decide who survives.
        let c = caps(&[
            (wallet(maker(1), b), 100),
            (virt(maker(1), hash(1), b), 100),
            (wallet(maker(2), b), 100),
            (virt(maker(2), hash(2), b), 0),
            (wallet(maker(3), b), 100),
            (virt(maker(3), hash(3), b), 100),
        ]);
        assert!(select(&snap, &c, a, b, 64).is_empty());
    }

    #[test]
    fn ranks_by_cap_then_hash() {
        let (a, b) = (tok(1), tok(2));
        // maker order (1,2,3) deliberately disagrees with cap order (30,90,90).
        let snap = Snapshot::from_strategies([
            xyc(maker(1), hash(10), a, b, 1000),
            xyc(maker(2), hash(5), a, b, 1000),
            xyc(maker(3), hash(9), a, b, 1000),
        ]);
        let c = caps(&[
            (wallet(maker(1), b), 30),
            (virt(maker(1), hash(10), b), 30),
            (wallet(maker(2), b), 90),
            (virt(maker(2), hash(5), b), 90),
            (wallet(maker(3), b), 90),
            (virt(maker(3), hash(9), b), 90),
        ]);
        let order: Vec<_> = select(&snap, &c, a, b, 64)
            .iter()
            .map(|c| c.key.strategy_hash)
            .collect();
        // cap desc puts the two 90s first; the tie breaks on strategy_hash asc.
        assert_eq!(order, vec![hash(5), hash(9), hash(10)]);
    }

    #[test]
    fn truncates_to_k_and_returns_all_below_k() {
        let (a, b) = (tok(1), tok(2));
        let snap = Snapshot::from_strategies((1u8..=5).map(|n| xyc(maker(n), hash(n), a, b, 1000)));
        let mut entries = Vec::new();
        for n in 1u8..=5 {
            entries.push((wallet(maker(n), b), u64::from(n) * 10));
            entries.push((virt(maker(n), hash(n), b), u64::from(n) * 10));
        }
        let c = caps(&entries);
        let top2 = select(&snap, &c, a, b, 2);
        assert_eq!(top2.len(), 2);
        assert_eq!(top2[0].cap_out, U256::from(50u64));
        assert_eq!(top2[1].cap_out, U256::from(40u64));
        assert_eq!(select(&snap, &c, a, b, 64).len(), 5);
    }
}
