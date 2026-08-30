//! The chain-reading [`BudgetSource`]. It answers per `AccountKey`: a wallet's settleable budget is
//! the ERC-20 reality — `min(balanceOf(maker), allowance(maker → Aqua))`, read just-in-time as the
//! "firm quote" confirmation — and a strategy virtual's is its balance in the registry snapshot. The
//! service asks; this adapter owns the chain-vs-registry split.

use std::sync::Arc;

use alloy::{
    primitives::{Address, U256},
    providers::Provider,
    sol,
};
use async_trait::async_trait;
use solvent_core::{
    deps::ledger::{BudgetSource, BudgetSourceError},
    primitives::{ledger::AccountKey, registry::StrategyKey},
    registry::SharedSnapshot,
};

sol! {
    #[sol(rpc)]
    interface IERC20 {
        function balanceOf(address account) external view returns (uint256);
        function allowance(address owner, address spender) external view returns (uint256);
    }
}

pub struct AlloyBudgetSource<P> {
    provider: P,
    aqua: Address,
    app: Address,
    registry: Arc<SharedSnapshot>,
}

impl<P: Provider> AlloyBudgetSource<P> {
    /// `aqua` is the allowance spender; `app` is the SwapVM router that keys strategies in the
    /// snapshot; `registry` is the live liquidity snapshot the strategy-virtual budget reads.
    pub fn new(provider: P, aqua: Address, app: Address, registry: Arc<SharedSnapshot>) -> Self {
        Self {
            provider,
            aqua,
            app,
            registry,
        }
    }
}

fn chain(e: impl std::fmt::Display) -> BudgetSourceError {
    BudgetSourceError::Chain(e.to_string())
}

#[async_trait]
impl<P: Provider + Clone + 'static> BudgetSource for AlloyBudgetSource<P> {
    async fn budget(&self, account: &AccountKey) -> Result<U256, BudgetSourceError> {
        match account {
            AccountKey::WalletBudget { maker, token } => {
                // One atomic, same-block read via Multicall3 — a firm "last look" wants
                // balanceOf and allowance to agree on one block, not straddle two.
                let erc20 = IERC20::new(*token, self.provider.clone());
                let (balance, allowance) = self
                    .provider
                    .multicall()
                    .add(erc20.balanceOf(maker.0))
                    .add(erc20.allowance(maker.0, self.aqua))
                    .aggregate()
                    .await
                    .map_err(chain)?;
                Ok(balance.min(allowance))
            }
            AccountKey::StrategyVirtual {
                maker,
                strategy_hash,
                token,
            } => {
                let key = StrategyKey {
                    maker: *maker,
                    app: self.app,
                    strategy_hash: *strategy_hash,
                };
                // A docked strategy is a tombstone that keeps its old balance but can no longer be
                // pulled — treat it as zero budget, or we would promise capital that reverts on-chain.
                Ok(self
                    .registry
                    .load()
                    .strategy(&key)
                    .filter(|strategy| strategy.active)
                    .map(|strategy| strategy.balance(token))
                    .unwrap_or(U256::ZERO))
            }
        }
    }
}
