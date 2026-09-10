//! Values shared by chain-local Solvent instances and the cross-chain coordinator.

use alloy_primitives::{Address, Bytes, B256, U256};
use serde::{Deserialize, Serialize};

use crate::primitives::ledger::ReservationSource;
use crate::primitives::{
    AggregateQuoteId, ChainId, CrossChainOrderId, CrossChainStepId, PrepareToken,
};

/// Repayment mechanism selected for an aggregate quote.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CrossChainRoute {
    Direct,
    Cctp,
}

/// Irreversible operations are named so a chain service can enforce a contract allow-list.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
    utoipa::ToSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum RemoteCommand {
    Deliver,
    DispatchFillProof,
    ClaimOrigin,
    DispatchRepayment,
    CloseDestination,
}

/// One pre-authorized call. Its debug representation deliberately omits calldata.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct PreparedStep {
    pub command: RemoteCommand,
    #[schema(value_type = String)]
    pub target: Address,
    #[schema(value_type = String)]
    pub value: U256,
    #[schema(value_type = String)]
    pub calldata: Bytes,
}

impl core::fmt::Debug for PreparedStep {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("PreparedStep")
            .field("command", &self.command)
            .field("target", &self.target)
            .field("value", &self.value)
            .field("calldata", &"[REDACTED]")
            .finish()
    }
}

/// Calls staged on exactly one chain before capital is committed.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ChainExecutionPlan {
    #[schema(value_type = String)]
    pub aggregate_id: AggregateQuoteId,
    #[schema(value_type = u64)]
    pub chain_id: ChainId,
    pub steps: Vec<PreparedStep>,
}

/// Immutable authority supplied when one chain stages its portion of an aggregate order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepValidationContext {
    pub order_id: CrossChainOrderId,
    pub quote: AggregateQuote,
    pub role: LegRole,
}

impl StepValidationContext {
    pub fn local_quote(&self) -> &LegQuote {
        match self.role {
            LegRole::Origin => &self.quote.origin,
            LegRole::Destination => &self.quote.destination,
        }
    }
}

impl core::fmt::Debug for ChainExecutionPlan {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("ChainExecutionPlan")
            .field("aggregate_id", &self.aggregate_id)
            .field("chain_id", &self.chain_id)
            .field("steps", &self.steps)
            .finish()
    }
}

/// The responsibility of a chain-local quote.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum LegRole {
    Origin,
    Destination,
}

/// A protocol-neutral request for one side of a cross-chain route.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LegQuoteRequest {
    pub request_id: B256,
    pub role: LegRole,
    pub local_chain: ChainId,
    pub remote_chain: ChainId,
    pub input_token: Address,
    pub output_token: Address,
    pub amount: U256,
    pub deadline_unix: u64,
    pub route: CrossChainRoute,
}

/// A chain-local promise used by the proxy to assemble an end-to-end quote.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct LegQuote {
    #[schema(value_type = String)]
    pub quote_id: B256,
    #[schema(value_type = String)]
    pub request_id: B256,
    pub role: LegRole,
    #[schema(value_type = u64)]
    pub local_chain: ChainId,
    #[schema(value_type = u64)]
    pub remote_chain: ChainId,
    #[schema(value_type = String)]
    pub input_token: Address,
    #[schema(value_type = String)]
    pub output_token: Address,
    #[schema(value_type = String)]
    pub amount_in: U256,
    #[schema(value_type = String)]
    pub amount_out: U256,
    pub route: CrossChainRoute,
    pub block_number: u64,
    pub expires_at_unix: u64,
    /// Capital held locally at admission; the chain service remains authoritative for these terms.
    pub sources: Vec<ReservationSource>,
}

/// The immutable quote accepted by the client and prepared on both chains.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct AggregateQuote {
    #[schema(value_type = String)]
    pub id: AggregateQuoteId,
    pub origin: LegQuote,
    pub destination: LegQuote,
    #[schema(value_type = String)]
    pub amount_in: U256,
    #[schema(value_type = String)]
    pub amount_out: U256,
    #[schema(value_type = String)]
    pub bridge_fee: U256,
    pub cctp_finality_threshold: Option<u32>,
    pub expires_at_unix: u64,
}

/// State of a chain-local capital hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreparationState {
    Prepared,
    Committed,
    Executed,
    Released,
}

/// A durable chain-local hold. The same aggregate quote can create at most one hold per role.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Preparation {
    pub token: PrepareToken,
    pub quote_id: AggregateQuoteId,
    pub role: LegRole,
    pub chain_id: ChainId,
    pub state: PreparationState,
    pub expires_at_unix: u64,
}

impl Preparation {
    pub fn commit(&mut self) -> Result<(), CrossChainInvariantError> {
        match self.state {
            PreparationState::Prepared | PreparationState::Committed => {
                self.state = PreparationState::Committed;
                Ok(())
            }
            state => Err(CrossChainInvariantError::PreparationTransition {
                from: state,
                to: PreparationState::Committed,
            }),
        }
    }

    pub fn execute(&mut self) -> Result<(), CrossChainInvariantError> {
        match self.state {
            PreparationState::Committed | PreparationState::Executed => {
                self.state = PreparationState::Executed;
                Ok(())
            }
            state => Err(CrossChainInvariantError::PreparationTransition {
                from: state,
                to: PreparationState::Executed,
            }),
        }
    }

    pub fn release(&mut self) -> Result<(), CrossChainInvariantError> {
        match self.state {
            PreparationState::Prepared
            | PreparationState::Committed
            | PreparationState::Released => {
                self.state = PreparationState::Released;
                Ok(())
            }
            state => Err(CrossChainInvariantError::PreparationTransition {
                from: state,
                to: PreparationState::Released,
            }),
        }
    }
}

/// Durable proxy state. The linear states make the next safe action explicit after a restart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SagaState {
    Quoted,
    Preparing,
    Prepared,
    DestinationPending,
    DestinationFinalized,
    FillProofPending,
    OriginPending,
    OriginFinalized,
    RepaymentPending,
    Complete,
    FailedBeforeDelivery,
    NeedsReconcile,
}

impl SagaState {
    pub fn permits(self, next: Self) -> bool {
        use SagaState::*;
        matches!(
            (self, next),
            (Quoted, Preparing)
                | (Preparing, Prepared | FailedBeforeDelivery)
                | (Prepared, DestinationPending | FailedBeforeDelivery)
                | (
                    DestinationPending,
                    DestinationFinalized | FailedBeforeDelivery | NeedsReconcile
                )
                | (DestinationFinalized, FillProofPending | NeedsReconcile)
                | (FillProofPending, OriginPending | NeedsReconcile)
                | (OriginPending, OriginFinalized | NeedsReconcile)
                | (
                    OriginFinalized,
                    RepaymentPending | Complete | NeedsReconcile
                )
                | (RepaymentPending, Complete | NeedsReconcile)
                | (
                    NeedsReconcile,
                    DestinationPending
                        | FillProofPending
                        | OriginPending
                        | RepaymentPending
                        | Complete
                )
        ) || self == next
    }
}

/// Evidence for an idempotent remote command. Payload bytes and signatures remain chain-local.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct StepEvidence {
    #[schema(value_type = String)]
    pub command_id: CrossChainStepId,
    #[schema(value_type = Option<String>)]
    pub transaction_hash: Option<B256>,
    pub block_number: Option<u64>,
    #[schema(value_type = Option<String>)]
    pub message_id: Option<B256>,
}

/// The proxy's recoverable record for one end-to-end order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct CrossChainSaga {
    #[schema(value_type = String)]
    pub order_id: CrossChainOrderId,
    pub quote: AggregateQuote,
    pub state: SagaState,
    #[schema(value_type = Option<String>)]
    pub origin_prepare: Option<PrepareToken>,
    #[schema(value_type = Option<String>)]
    pub destination_prepare: Option<PrepareToken>,
    pub destination: Option<StepEvidence>,
    pub fill_proof: Option<StepEvidence>,
    pub origin: Option<StepEvidence>,
    pub repayment: Option<StepEvidence>,
    pub failure: Option<String>,
}

impl CrossChainSaga {
    pub fn transition(&mut self, next: SagaState) -> Result<(), CrossChainInvariantError> {
        if !self.state.permits(next) {
            return Err(CrossChainInvariantError::SagaTransition {
                from: self.state,
                to: next,
            });
        }
        self.state = next;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CrossChainInvariantError {
    #[error("invalid preparation transition from {from:?} to {to:?}")]
    PreparationTransition {
        from: PreparationState,
        to: PreparationState,
    },
    #[error("invalid saga transition from {from:?} to {to:?}")]
    SagaTransition { from: SagaState, to: SagaState },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn committed_capital_can_be_compensated_until_execution() {
        let mut preparation = Preparation {
            token: PrepareToken(B256::ZERO),
            quote_id: AggregateQuoteId(B256::ZERO),
            role: LegRole::Destination,
            chain_id: ChainId(1),
            state: PreparationState::Prepared,
            expires_at_unix: 10,
        };
        preparation.commit().unwrap();
        preparation.release().unwrap();
        assert_eq!(preparation.state, PreparationState::Released);
    }

    #[test]
    fn post_delivery_failure_must_enter_reconciliation() {
        assert!(SagaState::DestinationPending.permits(SagaState::NeedsReconcile));
        assert!(SagaState::DestinationPending.permits(SagaState::FailedBeforeDelivery));
        assert!(!SagaState::DestinationFinalized.permits(SagaState::FailedBeforeDelivery));
    }
}
