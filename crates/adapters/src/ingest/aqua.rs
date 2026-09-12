//! Builds the policy-authorized Aqua source plan shared by protocol fillers.

use std::sync::Arc;

use alloy::primitives::{keccak256, Address, B256, U256};
use alloy::sol_types::{SolType, SolValue};

use solvent_core::deps::execution::ExecutionAuthorizer;
use solvent_core::deps::ingest::FillBuilderError;
use solvent_core::primitives::execution::{ExecutionAuthorization, UserFillAuthorization};
use solvent_core::primitives::registry::{Snapshot, StrategyKey};
use solvent_core::primitives::routing::{RouteLeg, RoutePlan};

use crate::execution::filler::{uses_taker_credential, Authorization, Order, SourceSwap};

pub(crate) struct AquaSourcePlanBuilder {
    app: Address,
    taker_credential: Address,
    authorizer: Arc<dyn ExecutionAuthorizer>,
}

impl AquaSourcePlanBuilder {
    pub(crate) fn new(
        app: Address,
        taker_credential: Address,
        authorizer: Arc<dyn ExecutionAuthorizer>,
    ) -> Self {
        Self {
            app,
            taker_credential,
            authorizer,
        }
    }

    pub(crate) async fn build(
        &self,
        plan: &RoutePlan,
        context_hash: B256,
        snapshot: &Snapshot,
    ) -> Result<Vec<SourceSwap>, FillBuilderError> {
        if plan.legs.is_empty() {
            return Err(FillBuilderError::NoLegs);
        }
        let mut sources = Vec::with_capacity(plan.legs.len());
        for (index, leg) in plan.legs.iter().enumerate() {
            sources.push(self.source_for(leg, index, context_hash, snapshot).await?);
        }
        Ok(sources)
    }

    pub(crate) async fn source_for(
        &self,
        leg: &RouteLeg,
        index: usize,
        context_hash: B256,
        snapshot: &Snapshot,
    ) -> Result<SourceSwap, FillBuilderError> {
        let key = StrategyKey {
            maker: leg.maker,
            app: self.app,
            strategy_hash: leg.strategy_hash,
        };
        let strategy = snapshot
            .strategy(&key)
            .ok_or(FillBuilderError::MissingStrategy)?;
        let order = <Order as SolType>::abi_decode(&strategy.program)
            .map_err(|_| FillBuilderError::UndecodableProgram)?;
        if order.maker != leg.maker.0 {
            return Err(FillBuilderError::StrategyMakerMismatch);
        }
        if !uses_taker_credential(&order.data, self.taker_credential) {
            return Err(FillBuilderError::UnprotectedStrategy);
        }

        let authorization = ExecutionAuthorization::user_fill(UserFillAuthorization {
            nonce: user_fill_nonce(context_hash, leg, index),
            context_hash,
            strategy_hash: leg.strategy_hash,
            maker: leg.maker,
            token_in: leg.token_in,
            token_out: leg.token_out,
            amount_out: leg.amount_out,
            amount_in_limit: leg.amount_in,
        });
        let policy_signature = self
            .authorizer
            .authorize(&authorization)
            .await?
            .into_bytes();

        Ok(SourceSwap {
            order,
            authorization: Authorization::from(&authorization),
            policySignature: policy_signature,
        })
    }
}

fn user_fill_nonce(context_hash: B256, leg: &RouteLeg, index: usize) -> U256 {
    let digest = keccak256(
        (
            context_hash,
            leg.strategy_hash.0,
            leg.maker.0,
            leg.token_in,
            leg.token_out,
            leg.amount_in,
            leg.amount_out,
            U256::from(index),
        )
            .abi_encode(),
    );
    U256::from_be_slice(digest.as_slice())
}
