use std::sync::Arc;

use alloy_primitives::keccak256;

use crate::deps::crosschain::{
    CctpCompletion, CctpCompletionError, RemoteProgress, RemoteSolvent, RemoteSolventError,
    SagaStore,
};
use crate::primitives::crosschain::{
    AggregateQuote, ChainExecutionPlan, CrossChainSaga, LegQuote, LegQuoteRequest, LegRole,
    RemoteCommand, SagaState,
};
use crate::primitives::{AggregateQuoteId, CrossChainOrderId, CrossChainStepId, SolventError};

use super::leg_quote_id;

/// Coordinates two remote single-chain Solvent services without owning an RPC or signer.
pub struct CrossChainProxy {
    origin: Arc<dyn RemoteSolvent>,
    destination: Arc<dyn RemoteSolvent>,
    sagas: Arc<dyn SagaStore>,
    cctp: Option<Arc<dyn CctpCompletion>>,
}

impl CrossChainProxy {
    pub fn new(
        origin: Arc<dyn RemoteSolvent>,
        destination: Arc<dyn RemoteSolvent>,
        sagas: Arc<dyn SagaStore>,
    ) -> Self {
        Self {
            origin,
            destination,
            sagas,
            cctp: None,
        }
    }

    pub fn with_cctp(mut self, cctp: Arc<dyn CctpCompletion>) -> Self {
        self.cctp = Some(cctp);
        self
    }

    pub async fn quote(
        &self,
        origin_request: &LegQuoteRequest,
        destination_request: &LegQuoteRequest,
        now_unix: u64,
    ) -> Result<AggregateQuote, SolventError> {
        let (origin, destination) = tokio::try_join!(
            self.origin.quote(origin_request),
            self.destination.quote(destination_request)
        )?;
        let mut aggregate = aggregate_quote(origin, destination, now_unix)?;
        if aggregate.origin.route == crate::primitives::crosschain::CrossChainRoute::Cctp {
            let cctp = self.cctp.as_ref().ok_or_else(|| {
                SolventError::InvalidCrossChain("CCTP route is not configured".to_string())
            })?;
            let terms = cctp.quote_terms(aggregate.destination.amount_in).await?;
            let required = aggregate
                .destination
                .amount_in
                .checked_add(terms.max_fee)
                .ok_or_else(|| {
                    SolventError::InvalidCrossChain(
                        "CCTP repayment plus fee exceeds uint256".to_string(),
                    )
                })?;
            if aggregate.origin.amount_out < required {
                return Err(SolventError::InvalidCrossChain(
                    "origin quote does not cover destination repayment and CCTP fee".to_string(),
                ));
            }
            aggregate.bridge_fee = terms.max_fee;
            aggregate.cctp_finality_threshold = Some(terms.finality_threshold);
            aggregate.id = aggregate_id(
                &aggregate.origin,
                &aggregate.destination,
                terms.max_fee,
                Some(terms.finality_threshold),
            );
        }
        Ok(aggregate)
    }

    pub async fn status(
        &self,
        order_id: CrossChainOrderId,
    ) -> Result<CrossChainSaga, SolventError> {
        self.sagas
            .load(order_id)
            .await?
            .ok_or_else(|| SolventError::InvalidCrossChain("unknown cross-chain order".to_string()))
    }

    pub async fn recoverable(&self) -> Result<Vec<CrossChainSaga>, SolventError> {
        Ok(self.sagas.recoverable().await?)
    }

    pub async fn start(
        &self,
        order_id: CrossChainOrderId,
        quote: AggregateQuote,
        origin_plan: &ChainExecutionPlan,
        destination_plan: &ChainExecutionPlan,
        now_unix: u64,
    ) -> Result<CrossChainSaga, SolventError> {
        if let Some(existing) = self.sagas.load(order_id).await? {
            if existing.quote == quote {
                return Ok(existing);
            }
            return Err(SolventError::InvalidCrossChain(
                "order id already belongs to a different quote".to_string(),
            ));
        }

        validate_aggregate_binding(&quote, now_unix)?;
        match quote.origin.route {
            crate::primitives::crosschain::CrossChainRoute::Direct
                if !quote.bridge_fee.is_zero() || quote.cctp_finality_threshold.is_some() =>
            {
                return Err(SolventError::InvalidCrossChain(
                    "direct quote contains CCTP terms".to_string(),
                ));
            }
            crate::primitives::crosschain::CrossChainRoute::Cctp => {
                let cctp = self.cctp.as_ref().ok_or_else(|| {
                    SolventError::InvalidCrossChain("CCTP route is not configured".to_string())
                })?;
                let current = cctp.quote_terms(quote.destination.amount_in).await?;
                if quote.bridge_fee != current.max_fee
                    || quote.cctp_finality_threshold != Some(current.finality_threshold)
                {
                    return Err(SolventError::InvalidCrossChain(
                        "CCTP capacity or fee changed; request a fresh quote".to_string(),
                    ));
                }
            }
            _ => {}
        }

        validate_plan(&quote, origin_plan, LegRole::Origin)?;
        validate_plan(&quote, destination_plan, LegRole::Destination)?;
        let origin_context = crate::primitives::crosschain::StepValidationContext {
            order_id,
            quote: quote.clone(),
            role: LegRole::Origin,
        };
        let destination_context = crate::primitives::crosschain::StepValidationContext {
            order_id,
            quote: quote.clone(),
            role: LegRole::Destination,
        };
        self.origin.stage(&origin_context, origin_plan).await?;
        self.destination
            .stage(&destination_context, destination_plan)
            .await?;

        let mut saga = CrossChainSaga {
            order_id,
            quote,
            state: SagaState::Quoted,
            origin_prepare: None,
            destination_prepare: None,
            destination: None,
            fill_proof: None,
            origin: None,
            repayment: None,
            failure: None,
        };
        saga = self.sagas.insert(&saga).await?;
        saga.transition(SagaState::Preparing).map_err(invariant)?;
        self.sagas.update(&saga).await?;

        self.continue_preparation(&mut saga).await?;
        Ok(saga)
    }

    /// Advance by at most one remote command. Repeating this method is always safe.
    pub async fn advance(
        &self,
        order_id: CrossChainOrderId,
    ) -> Result<CrossChainSaga, SolventError> {
        let mut saga = self.sagas.load(order_id).await?.ok_or_else(|| {
            SolventError::InvalidCrossChain("unknown cross-chain order".to_string())
        })?;

        if saga.state == SagaState::NeedsReconcile {
            saga.state = if saga.repayment.is_some() {
                SagaState::RepaymentPending
            } else if saga.origin.is_some() {
                SagaState::OriginFinalized
            } else if saga.fill_proof.is_some() {
                SagaState::OriginPending
            } else if saga.destination.is_some() {
                SagaState::DestinationFinalized
            } else {
                return Ok(saga);
            };
            self.sagas.update(&saga).await?;
        }

        match saga.state {
            SagaState::Preparing => {
                self.continue_preparation(&mut saga).await?;
                return Ok(saga);
            }
            SagaState::Prepared | SagaState::DestinationPending => {
                let (origin, destination) = tokio::try_join!(
                    self.origin.inspect(required_token(saga.origin_prepare)?),
                    self.destination
                        .inspect(required_token(saga.destination_prepare)?)
                )?;
                validate_committed(&saga, &origin, LegRole::Origin)?;
                validate_committed(&saga, &destination, LegRole::Destination)?;
                let progress = self
                    .destination
                    .command(
                        order_id,
                        step_id(
                            order_id,
                            saga.quote.destination.local_chain,
                            RemoteCommand::Deliver,
                        ),
                        RemoteCommand::Deliver,
                        required_token(saga.destination_prepare)?,
                    )
                    .await?;
                match progress {
                    RemoteProgress::Pending => saga.state = SagaState::DestinationPending,
                    RemoteProgress::Finalized { evidence } => {
                        saga.destination = Some(evidence);
                        saga.state = SagaState::DestinationFinalized;
                    }
                    RemoteProgress::Failed { reason } => {
                        self.compensate(&mut saga, reason).await?;
                        return Ok(saga);
                    }
                }
            }
            SagaState::DestinationFinalized | SagaState::FillProofPending => {
                let progress = self
                    .destination
                    .command(
                        order_id,
                        step_id(
                            order_id,
                            saga.quote.destination.local_chain,
                            RemoteCommand::DispatchFillProof,
                        ),
                        RemoteCommand::DispatchFillProof,
                        required_token(saga.destination_prepare)?,
                    )
                    .await?;
                apply_post_delivery(
                    &mut saga,
                    progress,
                    SagaState::FillProofPending,
                    SagaState::OriginPending,
                    |saga, evidence| saga.fill_proof = Some(evidence),
                );
            }
            SagaState::OriginPending => {
                let progress = self
                    .origin
                    .command(
                        order_id,
                        step_id(
                            order_id,
                            saga.quote.origin.local_chain,
                            RemoteCommand::ClaimOrigin,
                        ),
                        RemoteCommand::ClaimOrigin,
                        required_token(saga.origin_prepare)?,
                    )
                    .await?;
                apply_post_delivery(
                    &mut saga,
                    progress,
                    SagaState::OriginPending,
                    SagaState::OriginFinalized,
                    |saga, evidence| saga.origin = Some(evidence),
                );
            }
            SagaState::OriginFinalized => match saga.quote.origin.route {
                crate::primitives::crosschain::CrossChainRoute::Direct => {
                    let progress = self
                        .origin
                        .command(
                            order_id,
                            step_id(
                                order_id,
                                saga.quote.origin.local_chain,
                                RemoteCommand::DispatchRepayment,
                            ),
                            RemoteCommand::DispatchRepayment,
                            required_token(saga.origin_prepare)?,
                        )
                        .await?;
                    apply_post_delivery(
                        &mut saga,
                        progress,
                        SagaState::OriginFinalized,
                        SagaState::RepaymentPending,
                        |saga, evidence| saga.repayment = Some(evidence),
                    );
                }
                crate::primitives::crosschain::CrossChainRoute::Cctp => {
                    let origin_transaction = saga
                        .origin
                        .as_ref()
                        .and_then(|evidence| evidence.transaction_hash)
                        .ok_or_else(|| {
                            SolventError::InvalidCrossChain(
                                "origin burn finalized without a transaction hash".to_string(),
                            )
                        })?;
                    let cctp = self.cctp.as_ref().ok_or_else(|| {
                        SolventError::InvalidCrossChain(
                            "CCTP completion is not configured".to_string(),
                        )
                    })?;
                    match cctp.close_step(order_id, origin_transaction).await {
                        Ok(prepared) => {
                            self.destination
                                .stage_cctp_completion(
                                    &crate::primitives::crosschain::StepValidationContext {
                                        order_id,
                                        quote: saga.quote.clone(),
                                        role: LegRole::Destination,
                                    },
                                    &ChainExecutionPlan {
                                        aggregate_id: saga.quote.id,
                                        chain_id: saga.quote.destination.local_chain,
                                        steps: vec![prepared.step],
                                    },
                                    required_token(saga.destination_prepare)?,
                                )
                                .await?;
                            saga.repayment = Some(crate::primitives::crosschain::StepEvidence {
                                command_id: step_id(
                                    order_id,
                                    saga.quote.destination.local_chain,
                                    RemoteCommand::CloseDestination,
                                ),
                                transaction_hash: Some(origin_transaction),
                                block_number: saga
                                    .origin
                                    .as_ref()
                                    .and_then(|evidence| evidence.block_number),
                                message_id: Some(prepared.message_id),
                            });
                            saga.state = SagaState::RepaymentPending;
                        }
                        Err(
                            CctpCompletionError::Pending | CctpCompletionError::RateLimited { .. },
                        ) => return Ok(saga),
                        Err(CctpCompletionError::Transport(message)) => {
                            return Err(CctpCompletionError::Transport(message).into());
                        }
                        Err(CctpCompletionError::Invalid(message)) => {
                            saga.failure = Some(message);
                            saga.state = SagaState::NeedsReconcile;
                        }
                    }
                }
            },
            SagaState::RepaymentPending => {
                let progress = self
                    .destination
                    .command(
                        order_id,
                        step_id(
                            order_id,
                            saga.quote.destination.local_chain,
                            RemoteCommand::CloseDestination,
                        ),
                        RemoteCommand::CloseDestination,
                        required_token(saga.destination_prepare)?,
                    )
                    .await?;
                apply_post_delivery(
                    &mut saga,
                    progress,
                    SagaState::RepaymentPending,
                    SagaState::Complete,
                    |_, _| {},
                );
            }
            SagaState::Quoted
            | SagaState::Complete
            | SagaState::FailedBeforeDelivery
            | SagaState::NeedsReconcile => return Ok(saga),
        }
        self.sagas.update(&saga).await?;
        Ok(saga)
    }

    async fn compensate(
        &self,
        saga: &mut CrossChainSaga,
        reason: String,
    ) -> Result<(), SolventError> {
        let mut release_error = None;
        if let Some(token) = saga.origin_prepare {
            if let Err(error) = self.origin.release(token).await {
                release_error = Some(error);
            }
        }
        if let Some(token) = saga.destination_prepare {
            if let Err(error) = self.destination.release(token).await {
                if release_error.is_none() {
                    release_error = Some(error);
                }
            }
        }
        if let Some(error) = release_error {
            saga.failure = Some(format!("{reason}; compensation pending: {error}"));
            self.sagas.update(saga).await?;
            return Err(error.into());
        }
        saga.failure = Some(reason);
        saga.transition(SagaState::FailedBeforeDelivery)
            .map_err(invariant)?;
        self.sagas.update(saga).await?;
        Ok(())
    }

    async fn continue_preparation(&self, saga: &mut CrossChainSaga) -> Result<(), SolventError> {
        let origin = match saga.origin_prepare {
            Some(token) => self.origin.inspect(token).await,
            None => self.origin.prepare(saga.quote.id, &saga.quote.origin).await,
        };
        let origin = match origin {
            Ok(preparation) => preparation,
            Err(RemoteSolventError::Rejected(reason)) => {
                self.compensate(saga, reason).await?;
                return Ok(());
            }
            Err(error) => return Err(error.into()),
        };
        if saga.origin_prepare.is_none() {
            saga.origin_prepare = Some(origin.token);
            self.sagas.update(saga).await?;
        }

        let destination = match saga.destination_prepare {
            Some(token) => self.destination.inspect(token).await,
            None => {
                self.destination
                    .prepare(saga.quote.id, &saga.quote.destination)
                    .await
            }
        };
        let destination = match destination {
            Ok(preparation) => preparation,
            Err(RemoteSolventError::Rejected(reason)) => {
                self.compensate(saga, reason).await?;
                return Ok(());
            }
            Err(error) => return Err(error.into()),
        };
        if saga.destination_prepare.is_none() {
            saga.destination_prepare = Some(destination.token);
            self.sagas.update(saga).await?;
        }

        let origin_committed = match origin.state {
            crate::primitives::crosschain::PreparationState::Prepared => {
                match self.origin.commit(origin.token).await {
                    Ok(preparation) => preparation,
                    Err(RemoteSolventError::Rejected(reason)) => {
                        self.compensate(saga, reason).await?;
                        return Ok(());
                    }
                    Err(error) => return Err(error.into()),
                }
            }
            crate::primitives::crosschain::PreparationState::Committed => origin,
            _ => {
                self.compensate(saga, "origin preparation is no longer usable".to_string())
                    .await?;
                return Ok(());
            }
        };
        if let Err(error) = validate_committed(saga, &origin_committed, LegRole::Origin) {
            self.compensate(saga, error.to_string()).await?;
            return Ok(());
        }

        let destination_committed = match destination.state {
            crate::primitives::crosschain::PreparationState::Prepared => {
                match self.destination.commit(destination.token).await {
                    Ok(preparation) => preparation,
                    Err(RemoteSolventError::Rejected(reason)) => {
                        self.compensate(saga, reason).await?;
                        return Ok(());
                    }
                    Err(error) => return Err(error.into()),
                }
            }
            crate::primitives::crosschain::PreparationState::Committed => destination,
            _ => {
                self.compensate(
                    saga,
                    "destination preparation is no longer usable".to_string(),
                )
                .await?;
                return Ok(());
            }
        };
        if let Err(error) = validate_committed(saga, &destination_committed, LegRole::Destination) {
            self.compensate(saga, error.to_string()).await?;
            return Ok(());
        }

        saga.transition(SagaState::Prepared).map_err(invariant)?;
        self.sagas.update(saga).await?;
        Ok(())
    }
}

fn validate_plan(
    quote: &AggregateQuote,
    plan: &ChainExecutionPlan,
    role: LegRole,
) -> Result<(), SolventError> {
    let chain = match role {
        LegRole::Origin => quote.origin.local_chain,
        LegRole::Destination => quote.destination.local_chain,
    };
    if plan.aggregate_id != quote.id || plan.chain_id != chain || plan.steps.is_empty() {
        return Err(SolventError::InvalidCrossChain(
            "execution plan does not match the aggregate quote".to_string(),
        ));
    }
    let expected: &[RemoteCommand] = match (quote.origin.route, role) {
        (crate::primitives::crosschain::CrossChainRoute::Direct, LegRole::Origin) => {
            &[RemoteCommand::ClaimOrigin, RemoteCommand::DispatchRepayment]
        }
        (crate::primitives::crosschain::CrossChainRoute::Direct, LegRole::Destination) => &[
            RemoteCommand::Deliver,
            RemoteCommand::DispatchFillProof,
            RemoteCommand::CloseDestination,
        ],
        (crate::primitives::crosschain::CrossChainRoute::Cctp, LegRole::Origin) => {
            &[RemoteCommand::ClaimOrigin]
        }
        (crate::primitives::crosschain::CrossChainRoute::Cctp, LegRole::Destination) => {
            &[RemoteCommand::Deliver, RemoteCommand::DispatchFillProof]
        }
    };
    let actual = plan
        .steps
        .iter()
        .map(|step| step.command)
        .collect::<std::collections::BTreeSet<_>>();
    let expected = expected
        .iter()
        .copied()
        .collect::<std::collections::BTreeSet<_>>();
    if actual != expected || actual.len() != plan.steps.len() {
        return Err(SolventError::InvalidCrossChain(
            "execution plan has the wrong commands for its route and chain role".to_string(),
        ));
    }
    Ok(())
}

pub fn aggregate_quote(
    origin: LegQuote,
    destination: LegQuote,
    now_unix: u64,
) -> Result<AggregateQuote, SolventError> {
    let compatible = origin.quote_id == leg_quote_id(&origin)
        && destination.quote_id == leg_quote_id(&destination)
        && origin.role == LegRole::Origin
        && destination.role == LegRole::Destination
        && origin.local_chain != destination.local_chain
        && origin.local_chain == destination.remote_chain
        && origin.remote_chain == destination.local_chain
        && origin.request_id == destination.request_id
        && origin.route == destination.route
        && !origin.amount_in.is_zero()
        && !origin.amount_out.is_zero()
        && !destination.amount_in.is_zero()
        && !destination.amount_out.is_zero()
        && (origin.route != crate::primitives::crosschain::CrossChainRoute::Cctp
            || origin.amount_out >= destination.amount_in);
    if !compatible {
        return Err(SolventError::InvalidCrossChain(
            "origin and destination quotes are incompatible".to_string(),
        ));
    }
    let expires_at_unix = origin.expires_at_unix.min(destination.expires_at_unix);
    if expires_at_unix <= now_unix {
        return Err(SolventError::InvalidCrossChain(
            "aggregate quote has expired".to_string(),
        ));
    }
    let id = aggregate_id(&origin, &destination, alloy_primitives::U256::ZERO, None);
    let amount_in = origin.amount_in;
    let amount_out = destination.amount_out;
    Ok(AggregateQuote {
        id,
        origin,
        destination,
        amount_in,
        amount_out,
        bridge_fee: alloy_primitives::U256::ZERO,
        cctp_finality_threshold: None,
        expires_at_unix,
    })
}

pub(crate) fn aggregate_id(
    origin: &LegQuote,
    destination: &LegQuote,
    bridge_fee: alloy_primitives::U256,
    cctp_finality_threshold: Option<u32>,
) -> AggregateQuoteId {
    let mut bytes = Vec::with_capacity(100);
    bytes.extend_from_slice(origin.quote_id.as_slice());
    bytes.extend_from_slice(destination.quote_id.as_slice());
    bytes.extend_from_slice(&bridge_fee.to_be_bytes::<32>());
    bytes.extend_from_slice(&cctp_finality_threshold.unwrap_or_default().to_be_bytes());
    AggregateQuoteId(keccak256(bytes))
}

pub(crate) fn validate_aggregate_binding(
    quote: &AggregateQuote,
    now_unix: u64,
) -> Result<(), SolventError> {
    let compatible = quote.id
        == aggregate_id(
            &quote.origin,
            &quote.destination,
            quote.bridge_fee,
            quote.cctp_finality_threshold,
        )
        && quote.origin.quote_id == leg_quote_id(&quote.origin)
        && quote.destination.quote_id == leg_quote_id(&quote.destination)
        && quote.origin.role == LegRole::Origin
        && quote.destination.role == LegRole::Destination
        && quote.origin.local_chain != quote.destination.local_chain
        && quote.origin.local_chain == quote.destination.remote_chain
        && quote.origin.remote_chain == quote.destination.local_chain
        && quote.origin.request_id == quote.destination.request_id
        && quote.origin.route == quote.destination.route
        && quote.amount_in == quote.origin.amount_in
        && quote.amount_out == quote.destination.amount_out
        && quote.expires_at_unix
            == quote
                .origin
                .expires_at_unix
                .min(quote.destination.expires_at_unix)
        && !quote.amount_in.is_zero()
        && !quote.amount_out.is_zero();
    if !compatible {
        return Err(SolventError::InvalidCrossChain(
            "aggregate quote context is inconsistent".to_string(),
        ));
    }
    match quote.origin.route {
        crate::primitives::crosschain::CrossChainRoute::Direct
            if !quote.bridge_fee.is_zero() || quote.cctp_finality_threshold.is_some() =>
        {
            return Err(SolventError::InvalidCrossChain(
                "direct quote contains CCTP terms".to_string(),
            ));
        }
        crate::primitives::crosschain::CrossChainRoute::Cctp => {
            let required = quote
                .destination
                .amount_in
                .checked_add(quote.bridge_fee)
                .ok_or_else(|| {
                    SolventError::InvalidCrossChain(
                        "CCTP repayment plus fee exceeds uint256".to_string(),
                    )
                })?;
            if quote.cctp_finality_threshold.is_none() || quote.origin.amount_out < required {
                return Err(SolventError::InvalidCrossChain(
                    "CCTP aggregate terms are inconsistent".to_string(),
                ));
            }
        }
        _ => {}
    }
    if quote.expires_at_unix <= now_unix {
        return Err(SolventError::InvalidCrossChain(
            "aggregate quote expired before order admission".to_string(),
        ));
    }
    Ok(())
}

pub(crate) fn step_id(
    order_id: CrossChainOrderId,
    chain_id: crate::primitives::ChainId,
    command: RemoteCommand,
) -> CrossChainStepId {
    let mut bytes = Vec::with_capacity(41);
    bytes.extend_from_slice(order_id.0.as_slice());
    bytes.extend_from_slice(&chain_id.0.to_be_bytes());
    bytes.push(command as u8);
    CrossChainStepId(keccak256(bytes))
}

fn apply_post_delivery(
    saga: &mut CrossChainSaga,
    progress: RemoteProgress,
    pending: SagaState,
    finalized: SagaState,
    record: impl FnOnce(&mut CrossChainSaga, crate::primitives::crosschain::StepEvidence),
) {
    match progress {
        RemoteProgress::Pending => saga.state = pending,
        RemoteProgress::Finalized { evidence } => {
            record(saga, evidence);
            saga.state = finalized;
        }
        RemoteProgress::Failed { reason } => {
            saga.failure = Some(reason);
            saga.state = SagaState::NeedsReconcile;
        }
    }
}

fn invariant(error: crate::primitives::crosschain::CrossChainInvariantError) -> SolventError {
    SolventError::InvalidCrossChain(error.to_string())
}

fn required_token(
    token: Option<crate::primitives::PrepareToken>,
) -> Result<crate::primitives::PrepareToken, SolventError> {
    token.ok_or_else(|| {
        SolventError::InvalidCrossChain("saga is missing a committed preparation".to_string())
    })
}

fn validate_committed(
    saga: &CrossChainSaga,
    preparation: &crate::primitives::crosschain::Preparation,
    role: LegRole,
) -> Result<(), SolventError> {
    let expected_token = match role {
        LegRole::Origin => saga.origin_prepare,
        LegRole::Destination => saga.destination_prepare,
    };
    let expected_chain = match role {
        LegRole::Origin => saga.quote.origin.local_chain,
        LegRole::Destination => saga.quote.destination.local_chain,
    };
    if Some(preparation.token) != expected_token
        || preparation.quote_id != saga.quote.id
        || preparation.role != role
        || preparation.chain_id != expected_chain
        || preparation.state != crate::primitives::crosschain::PreparationState::Committed
    {
        return Err(SolventError::InvalidCrossChain(
            "remote service did not confirm the expected committed preparation".to_string(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deps::crosschain::{RemoteSolventError, SagaStoreError};
    use crate::primitives::crosschain::{
        CrossChainRoute, Preparation, PreparationState, PreparedStep, StepEvidence,
    };
    use crate::primitives::PrepareToken;
    use alloy_primitives::{Address, Bytes, B256, U256};
    use async_trait::async_trait;
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use tokio::sync::Mutex;

    fn leg(role: LegRole, expires: u64) -> LegQuote {
        let (local, remote) = match role {
            LegRole::Origin => (1, 10),
            LegRole::Destination => (10, 1),
        };
        let mut quote = LegQuote {
            quote_id: B256::ZERO,
            request_id: B256::from([9; 32]),
            role,
            local_chain: crate::primitives::ChainId(local),
            remote_chain: crate::primitives::ChainId(remote),
            input_token: Address::from([1; 20]),
            output_token: Address::from([2; 20]),
            amount_in: U256::from(100),
            amount_out: U256::from(90),
            route: CrossChainRoute::Direct,
            block_number: 10,
            expires_at_unix: expires,
            sources: Vec::new(),
        };
        quote.quote_id = leg_quote_id(&quote);
        quote
    }

    #[test]
    fn aggregate_uses_the_earliest_expiry() {
        let quote =
            aggregate_quote(leg(LegRole::Origin, 50), leg(LegRole::Destination, 40), 20).unwrap();
        assert_eq!(quote.expires_at_unix, 40);
    }

    #[test]
    fn aggregate_rejects_wrong_chain_identity() {
        let origin = leg(LegRole::Origin, 50);
        let mut destination = leg(LegRole::Destination, 50);
        destination.remote_chain = crate::primitives::ChainId(2);
        destination.quote_id = leg_quote_id(&destination);
        assert!(aggregate_quote(origin, destination, 20).is_err());
    }

    #[test]
    fn aggregate_rejects_cctp_origin_shortfall() {
        let mut origin = leg(LegRole::Origin, 50);
        let mut destination = leg(LegRole::Destination, 50);
        origin.route = CrossChainRoute::Cctp;
        destination.route = CrossChainRoute::Cctp;
        origin.amount_out = U256::from(99);
        destination.amount_in = U256::from(100);
        origin.quote_id = leg_quote_id(&origin);
        destination.quote_id = leg_quote_id(&destination);
        assert!(aggregate_quote(origin, destination, 20).is_err());
    }

    struct MemorySagas(Mutex<BTreeMap<CrossChainOrderId, CrossChainSaga>>);

    #[async_trait]
    impl SagaStore for MemorySagas {
        async fn insert(&self, saga: &CrossChainSaga) -> Result<CrossChainSaga, SagaStoreError> {
            let mut sagas = self.0.lock().await;
            if let Some(existing) = sagas.get(&saga.order_id) {
                return if existing == saga {
                    Ok(existing.clone())
                } else {
                    Err(SagaStoreError::Conflict)
                };
            }
            sagas.insert(saga.order_id, saga.clone());
            Ok(saga.clone())
        }

        async fn load(
            &self,
            order_id: CrossChainOrderId,
        ) -> Result<Option<CrossChainSaga>, SagaStoreError> {
            Ok(self.0.lock().await.get(&order_id).cloned())
        }

        async fn update(&self, saga: &CrossChainSaga) -> Result<(), SagaStoreError> {
            self.0.lock().await.insert(saga.order_id, saga.clone());
            Ok(())
        }

        async fn recoverable(&self) -> Result<Vec<CrossChainSaga>, SagaStoreError> {
            Ok(self
                .0
                .lock()
                .await
                .values()
                .filter(|saga| {
                    !matches!(
                        saga.state,
                        SagaState::Complete | SagaState::FailedBeforeDelivery
                    )
                })
                .cloned()
                .collect())
        }
    }

    struct FakeRemote {
        quote: LegQuote,
        fail_prepare: AtomicBool,
        fail_commit_once: AtomicBool,
        releases: AtomicUsize,
        preparation: Mutex<Option<Preparation>>,
    }

    impl FakeRemote {
        fn new(quote: LegQuote) -> Self {
            Self {
                quote,
                fail_prepare: AtomicBool::new(false),
                fail_commit_once: AtomicBool::new(false),
                releases: AtomicUsize::new(0),
                preparation: Mutex::new(None),
            }
        }
    }

    #[async_trait]
    impl RemoteSolvent for FakeRemote {
        async fn quote(&self, _: &LegQuoteRequest) -> Result<LegQuote, RemoteSolventError> {
            Ok(self.quote.clone())
        }

        async fn stage(
            &self,
            _: &crate::primitives::crosschain::StepValidationContext,
            _: &ChainExecutionPlan,
        ) -> Result<(), RemoteSolventError> {
            Ok(())
        }

        async fn stage_cctp_completion(
            &self,
            context: &crate::primitives::crosschain::StepValidationContext,
            plan: &ChainExecutionPlan,
            _: PrepareToken,
        ) -> Result<(), RemoteSolventError> {
            self.stage(context, plan).await
        }

        async fn prepare(
            &self,
            aggregate_id: AggregateQuoteId,
            quote: &LegQuote,
        ) -> Result<Preparation, RemoteSolventError> {
            if self.fail_prepare.load(Ordering::Relaxed) {
                return Err(RemoteSolventError::Rejected("no capacity".to_string()));
            }
            let preparation = Preparation {
                token: PrepareToken(B256::from([quote.local_chain.0 as u8; 32])),
                quote_id: aggregate_id,
                role: quote.role,
                chain_id: quote.local_chain,
                state: PreparationState::Prepared,
                expires_at_unix: quote.expires_at_unix,
            };
            *self.preparation.lock().await = Some(preparation.clone());
            Ok(preparation)
        }

        async fn commit(&self, token: PrepareToken) -> Result<Preparation, RemoteSolventError> {
            if self.fail_commit_once.swap(false, Ordering::Relaxed) {
                return Err(RemoteSolventError::Unavailable(
                    "temporary timeout".to_string(),
                ));
            }
            let mut preparation = self
                .preparation
                .lock()
                .await
                .clone()
                .ok_or_else(|| RemoteSolventError::Rejected("not prepared".to_string()))?;
            preparation.token = token;
            preparation.state = PreparationState::Committed;
            *self.preparation.lock().await = Some(preparation.clone());
            Ok(preparation)
        }

        async fn inspect(&self, token: PrepareToken) -> Result<Preparation, RemoteSolventError> {
            self.preparation
                .lock()
                .await
                .clone()
                .filter(|preparation| preparation.token == token)
                .ok_or_else(|| RemoteSolventError::Rejected("unknown preparation".to_string()))
        }

        async fn release(&self, token: PrepareToken) -> Result<Preparation, RemoteSolventError> {
            self.releases.fetch_add(1, Ordering::Relaxed);
            let mut preparation = self
                .preparation
                .lock()
                .await
                .clone()
                .ok_or_else(|| RemoteSolventError::Rejected("not prepared".to_string()))?;
            preparation.token = token;
            preparation.state = PreparationState::Released;
            *self.preparation.lock().await = Some(preparation.clone());
            Ok(preparation)
        }

        async fn command(
            &self,
            _: CrossChainOrderId,
            command_id: CrossChainStepId,
            _: RemoteCommand,
            _: PrepareToken,
        ) -> Result<RemoteProgress, RemoteSolventError> {
            Ok(RemoteProgress::Finalized {
                evidence: StepEvidence {
                    command_id,
                    transaction_hash: Some(command_id.0),
                    block_number: Some(1),
                    message_id: None,
                },
            })
        }
    }

    struct FakeCctp;

    #[async_trait]
    impl crate::deps::crosschain::CctpCompletion for FakeCctp {
        async fn quote_terms(
            &self,
            _: U256,
        ) -> Result<crate::deps::crosschain::CctpQuoteTerms, CctpCompletionError> {
            Ok(crate::deps::crosschain::CctpQuoteTerms {
                max_fee: U256::ZERO,
                finality_threshold: 1_000,
            })
        }

        async fn close_step(
            &self,
            _: CrossChainOrderId,
            _: B256,
        ) -> Result<crate::deps::crosschain::CctpPreparedStep, CctpCompletionError> {
            Ok(crate::deps::crosschain::CctpPreparedStep {
                step: PreparedStep {
                    command: RemoteCommand::CloseDestination,
                    target: Address::from([1; 20]),
                    value: U256::ZERO,
                    calldata: Bytes::from_static(b"circle"),
                },
                message_id: B256::from([6; 32]),
            })
        }
    }

    fn plan(quote: &AggregateQuote, role: LegRole) -> ChainExecutionPlan {
        let chain_id = match role {
            LegRole::Origin => quote.origin.local_chain,
            LegRole::Destination => quote.destination.local_chain,
        };
        let commands = match role {
            LegRole::Origin => vec![RemoteCommand::ClaimOrigin, RemoteCommand::DispatchRepayment],
            LegRole::Destination => vec![
                RemoteCommand::Deliver,
                RemoteCommand::DispatchFillProof,
                RemoteCommand::CloseDestination,
            ],
        };
        ChainExecutionPlan {
            aggregate_id: quote.id,
            chain_id,
            steps: commands
                .into_iter()
                .map(|command| PreparedStep {
                    command,
                    target: Address::from([1; 20]),
                    value: U256::ZERO,
                    calldata: Bytes::new(),
                })
                .collect(),
        }
    }

    fn cctp_plan(quote: &AggregateQuote, role: LegRole) -> ChainExecutionPlan {
        let chain_id = match role {
            LegRole::Origin => quote.origin.local_chain,
            LegRole::Destination => quote.destination.local_chain,
        };
        let commands = match role {
            LegRole::Origin => vec![RemoteCommand::ClaimOrigin],
            LegRole::Destination => {
                vec![RemoteCommand::Deliver, RemoteCommand::DispatchFillProof]
            }
        };
        ChainExecutionPlan {
            aggregate_id: quote.id,
            chain_id,
            steps: commands
                .into_iter()
                .map(|command| PreparedStep {
                    command,
                    target: Address::from([1; 20]),
                    value: U256::ZERO,
                    calldata: Bytes::new(),
                })
                .collect(),
        }
    }

    #[tokio::test]
    async fn second_service_prepare_failure_compensates_the_first() {
        let quote =
            aggregate_quote(leg(LegRole::Origin, 50), leg(LegRole::Destination, 50), 10).unwrap();
        let origin = Arc::new(FakeRemote::new(quote.origin.clone()));
        let destination = Arc::new(FakeRemote::new(quote.destination.clone()));
        destination.fail_prepare.store(true, Ordering::Relaxed);
        let store = Arc::new(MemorySagas(Mutex::new(BTreeMap::new())));
        let proxy = CrossChainProxy::new(origin.clone(), destination, store);
        let saga = proxy
            .start(
                CrossChainOrderId(B256::from([7; 32])),
                quote.clone(),
                &plan(&quote, LegRole::Origin),
                &plan(&quote, LegRole::Destination),
                10,
            )
            .await
            .unwrap();
        assert_eq!(saga.state, SagaState::FailedBeforeDelivery);
        assert_eq!(origin.releases.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn preparation_resumes_after_a_transient_commit_failure() {
        let quote =
            aggregate_quote(leg(LegRole::Origin, 50), leg(LegRole::Destination, 50), 10).unwrap();
        let origin = Arc::new(FakeRemote::new(quote.origin.clone()));
        let destination = Arc::new(FakeRemote::new(quote.destination.clone()));
        destination.fail_commit_once.store(true, Ordering::Relaxed);
        let store = Arc::new(MemorySagas(Mutex::new(BTreeMap::new())));
        let proxy = CrossChainProxy::new(origin, destination, store);
        let order_id = CrossChainOrderId(B256::from([4; 32]));

        let error = proxy
            .start(
                order_id,
                quote.clone(),
                &plan(&quote, LegRole::Origin),
                &plan(&quote, LegRole::Destination),
                10,
            )
            .await
            .unwrap_err();
        assert!(matches!(error, SolventError::RemoteSolvent(_)));
        assert_eq!(
            proxy.status(order_id).await.unwrap().state,
            SagaState::Preparing
        );

        let saga = proxy.advance(order_id).await.unwrap();
        assert_eq!(saga.state, SagaState::Prepared);
    }

    #[tokio::test]
    async fn coordinator_reaches_complete_with_idempotent_named_steps() {
        let quote =
            aggregate_quote(leg(LegRole::Origin, 50), leg(LegRole::Destination, 50), 10).unwrap();
        let origin = Arc::new(FakeRemote::new(quote.origin.clone()));
        let destination = Arc::new(FakeRemote::new(quote.destination.clone()));
        let store = Arc::new(MemorySagas(Mutex::new(BTreeMap::new())));
        let proxy = CrossChainProxy::new(origin, destination, store);
        let order_id = CrossChainOrderId(B256::from([8; 32]));
        let mut saga = proxy
            .start(
                order_id,
                quote.clone(),
                &plan(&quote, LegRole::Origin),
                &plan(&quote, LegRole::Destination),
                10,
            )
            .await
            .unwrap();
        for _ in 0..5 {
            saga = proxy.advance(order_id).await.unwrap();
        }
        assert_eq!(saga.state, SagaState::Complete);
        assert!(saga.destination.is_some());
        assert!(saga.origin.is_some());
    }

    #[tokio::test]
    async fn cctp_completion_is_staged_only_after_the_origin_burn() {
        let mut origin_leg = leg(LegRole::Origin, 50);
        let mut destination_leg = leg(LegRole::Destination, 50);
        origin_leg.route = CrossChainRoute::Cctp;
        destination_leg.route = CrossChainRoute::Cctp;
        origin_leg.amount_out = U256::from(100);
        destination_leg.amount_in = U256::from(100);
        origin_leg.quote_id = leg_quote_id(&origin_leg);
        destination_leg.quote_id = leg_quote_id(&destination_leg);
        let mut quote = aggregate_quote(origin_leg, destination_leg, 10).unwrap();
        quote.cctp_finality_threshold = Some(1_000);
        quote.id = aggregate_id(
            &quote.origin,
            &quote.destination,
            quote.bridge_fee,
            quote.cctp_finality_threshold,
        );
        let origin = Arc::new(FakeRemote::new(quote.origin.clone()));
        let destination = Arc::new(FakeRemote::new(quote.destination.clone()));
        let store = Arc::new(MemorySagas(Mutex::new(BTreeMap::new())));
        let proxy = CrossChainProxy::new(origin, destination, store).with_cctp(Arc::new(FakeCctp));
        let order_id = CrossChainOrderId(B256::from([5; 32]));
        let mut saga = proxy
            .start(
                order_id,
                quote.clone(),
                &cctp_plan(&quote, LegRole::Origin),
                &cctp_plan(&quote, LegRole::Destination),
                10,
            )
            .await
            .unwrap();
        for _ in 0..5 {
            saga = proxy.advance(order_id).await.unwrap();
        }
        assert_eq!(saga.state, SagaState::Complete);
        assert_eq!(
            saga.repayment.and_then(|evidence| evidence.message_id),
            Some(B256::from([6; 32]))
        );
    }
}
