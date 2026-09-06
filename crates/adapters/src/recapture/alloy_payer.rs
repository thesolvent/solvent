//! The alloy `RebatePayer`: rebates a maker as an ERC-20 `transfer` from the operator's treasury —
//! the signer behind the injected provider. One transfer per (maker, token); the payout service
//! sums across intents before calling, so this stays a single send.

use alloy::primitives::{Address, U256};
use alloy::providers::DynProvider;
use alloy::sol;
use async_trait::async_trait;
use solvent_core::{
    deps::recapture::{RebatePayer, RebatePayerError},
    primitives::MakerId,
};

sol! {
    #[sol(rpc)]
    interface IERC20 {
        function transfer(address to, uint256 amount) external returns (bool);
    }
}

pub struct AlloyRebatePayer {
    provider: DynProvider,
}

impl AlloyRebatePayer {
    pub fn new(provider: DynProvider) -> Self {
        Self { provider }
    }
}

fn send(e: impl std::fmt::Display) -> RebatePayerError {
    RebatePayerError::Send(e.to_string())
}

#[async_trait]
impl RebatePayer for AlloyRebatePayer {
    async fn pay(
        &self,
        maker: MakerId,
        token: Address,
        amount: U256,
    ) -> Result<(), RebatePayerError> {
        IERC20::new(token, self.provider.clone())
            .transfer(maker.0, amount)
            .send()
            .await
            .map_err(send)?
            .watch()
            .await
            .map_err(send)?;
        Ok(())
    }
}
