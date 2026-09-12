use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use alloy_primitives::Address;

use crate::deps::crosschain::{
    RemoteProgress, StepMaterializer, StepStore, StepStoreError, StepValidator,
};
use crate::deps::execution::{Execution, SimGate};
use crate::primitives::crosschain::{
    ChainExecutionPlan, LegRole, Preparation, PreparationState, RemoteCommand, StepEvidence,
    StepValidationContext,
};
use crate::primitives::execution::{ExecHandle, ExecStatus, FillTx, SimVerdict};
use crate::primitives::{
    ChainId, CrossChainOrderId, CrossChainStepId, IntentId, ReservationId, SolventError,
};

/// Validates, stages, submits, and tracks named calls owned by one chain service.
pub struct LocalStepService {
    chain_id: ChainId,
    owner: Address,
    targets: BTreeMap<RemoteCommand, Address>,
    store: Arc<dyn StepStore>,
    execution: Arc<dyn Execution>,
    simulator: Arc<dyn SimGate>,
    validator: Option<Arc<dyn StepValidator>>,
    materializer: Option<Arc<dyn StepMaterializer>>,
}

impl LocalStepService {
    pub fn new(
        chain_id: ChainId,
        owner: Address,
        targets: BTreeMap<RemoteCommand, Address>,
        store: Arc<dyn StepStore>,
        execution: Arc<dyn Execution>,
        simulator: Arc<dyn SimGate>,
    ) -> Self {
        Self {
            chain_id,
            owner,
            targets,
            store,
            execution,
            simulator,
            validator: None,
            materializer: None,
        }
    }

    pub fn with_materializer(mut self, materializer: Arc<dyn StepMaterializer>) -> Self {
        self.materializer = Some(materializer);
        self
    }

    pub fn with_validator(mut self, validator: Arc<dyn StepValidator>) -> Self {
        self.validator = Some(validator);
        self
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all, fields(chain_id = %self.chain_id)))]
    pub async fn stage(
        &self,
        context: &StepValidationContext,
        plan: &ChainExecutionPlan,
    ) -> Result<(), SolventError> {
        if !initial_plan_shape(context, plan) {
            return Err(SolventError::InvalidCrossChain(
                "execution plan does not contain the complete initial command set".to_string(),
            ));
        }
        self.stage_validated(context, plan).await
    }

    pub async fn stage_cctp_completion(
        &self,
        context: &StepValidationContext,
        plan: &ChainExecutionPlan,
        preparation: &Preparation,
    ) -> Result<(), SolventError> {
        let [step] = plan.steps.as_slice() else {
            return Err(SolventError::InvalidCrossChain(
                "CCTP completion plan must contain only the close command".to_string(),
            ));
        };
        if context.quote.origin.route != crate::primitives::crosschain::CrossChainRoute::Cctp
            || context.role != LegRole::Destination
            || step.command != RemoteCommand::CloseDestination
            || preparation.quote_id != context.quote.id
            || preparation.chain_id != self.chain_id
            || preparation.role != LegRole::Destination
            || preparation.state != PreparationState::Executed
        {
            return Err(SolventError::InvalidCrossChain(
                "CCTP completion is not authorized by the executed destination preparation"
                    .to_string(),
            ));
        }
        self.stage_validated(context, plan).await
    }

    async fn stage_validated(
        &self,
        context: &StepValidationContext,
        plan: &ChainExecutionPlan,
    ) -> Result<(), SolventError> {
        if plan.aggregate_id != context.quote.id
            || plan.chain_id != self.chain_id
            || context.local_quote().local_chain != self.chain_id
        {
            return Err(SolventError::InvalidCrossChain(
                "execution plan does not match its aggregate validation context".to_string(),
            ));
        }
        let mut seen = BTreeSet::new();
        for step in &plan.steps {
            if !command_is_valid_for(context.role, context.quote.origin.route, step.command) {
                return Err(SolventError::InvalidCrossChain(
                    "execution command does not belong to this route and chain role".to_string(),
                ));
            }
            let target = self.targets.get(&step.command).ok_or_else(|| {
                SolventError::InvalidCrossChain(format!(
                    "command {:?} is not enabled on chain {}",
                    step.command, self.chain_id
                ))
            })?;
            if *target != step.target {
                return Err(SolventError::InvalidCrossChain(
                    "execution target is not allow-listed for its command".to_string(),
                ));
            }
            if !seen.insert(step.command) {
                return Err(SolventError::InvalidCrossChain(
                    "execution plan repeats a command".to_string(),
                ));
            }
        }
        if plan.steps.is_empty() {
            return Err(SolventError::InvalidCrossChain(
                "execution plan has no commands".to_string(),
            ));
        }
        self.validator
            .as_ref()
            .ok_or_else(|| {
                SolventError::InvalidCrossChain(
                    "cross-chain step validation is not configured".to_string(),
                )
            })?
            .validate_plan(context, plan)
            .await?;
        match self.store.stage(context.order_id, plan).await {
            Ok(()) => Ok(()),
            Err(StepStoreError::Conflict) => Err(SolventError::InvalidCrossChain(
                "order or aggregate quote conflicts with an existing staged plan".to_string(),
            )),
            Err(error) => Err(error.into()),
        }
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all, fields(chain_id = %self.chain_id, order_id = %order_id, command = ?command)))]
    pub async fn command(
        &self,
        order_id: CrossChainOrderId,
        command_id: CrossChainStepId,
        command: RemoteCommand,
        preparation: &Preparation,
    ) -> Result<RemoteProgress, SolventError> {
        let expected_id = crate::crosschain::proxy::step_id(order_id, self.chain_id, command);
        if expected_id != command_id {
            return Err(SolventError::InvalidCrossChain(
                "command id does not bind the order and step".to_string(),
            ));
        }
        let aggregate_id = self
            .store
            .aggregate_id(order_id, command)
            .await?
            .ok_or_else(|| {
                SolventError::InvalidCrossChain(
                    "command has no durable aggregate-quote binding".to_string(),
                )
            })?;
        let expected_role = match command {
            RemoteCommand::Deliver
            | RemoteCommand::DispatchFillProof
            | RemoteCommand::CloseDestination => LegRole::Destination,
            RemoteCommand::ClaimOrigin | RemoteCommand::DispatchRepayment => LegRole::Origin,
        };
        let state_authorizes = match command {
            RemoteCommand::Deliver | RemoteCommand::ClaimOrigin => matches!(
                preparation.state,
                PreparationState::Committed | PreparationState::Executed
            ),
            RemoteCommand::DispatchFillProof
            | RemoteCommand::DispatchRepayment
            | RemoteCommand::CloseDestination => preparation.state == PreparationState::Executed,
        };
        if preparation.quote_id != aggregate_id
            || preparation.chain_id != self.chain_id
            || preparation.role != expected_role
            || !state_authorizes
        {
            return Err(SolventError::InvalidCrossChain(
                "preparation does not authorize this order and command".to_string(),
            ));
        }
        let step =
            self.store.load(order_id, command).await?.ok_or_else(|| {
                SolventError::InvalidCrossChain("command was not staged".to_string())
            })?;
        let step = match &self.materializer {
            Some(materializer) => materializer.materialize(&step).await?,
            None => step,
        };
        let intent = IntentId(command_id.0);
        let fill = FillTx::new(
            intent,
            self.chain_id.0,
            self.owner,
            step.target,
            step.calldata,
        )
        .with_value(step.value);
        let handle = ExecHandle(command_id.0);
        if self.execution.status(handle).await?.is_none() {
            match self.simulator.simulate(&fill).await? {
                SimVerdict::Ok => {
                    self.execution
                        .submit(&fill, ReservationId(command_id.0))
                        .await?;
                }
                SimVerdict::Reject { reason } => return Ok(RemoteProgress::Failed { reason }),
            }
        }
        self.execution.tick().await?;
        match self.execution.status(handle).await? {
            None | Some(ExecStatus::Pending) => Ok(RemoteProgress::Pending),
            Some(ExecStatus::Confirmed { block, tx }) => Ok(RemoteProgress::Finalized {
                evidence: StepEvidence {
                    command_id,
                    transaction_hash: Some(tx),
                    block_number: Some(block),
                    message_id: None,
                },
            }),
            Some(ExecStatus::Failed { reason }) => Ok(RemoteProgress::Failed { reason }),
            Some(ExecStatus::Dropped) => Ok(RemoteProgress::Failed {
                reason: "transaction dropped before confirmation".to_string(),
            }),
        }
    }
}

fn initial_plan_shape(context: &StepValidationContext, plan: &ChainExecutionPlan) -> bool {
    use crate::primitives::crosschain::CrossChainRoute;
    let actual = plan
        .steps
        .iter()
        .map(|step| step.command)
        .collect::<BTreeSet<_>>();
    let expected: BTreeSet<_> = match (context.quote.origin.route, context.role) {
        (CrossChainRoute::Direct, LegRole::Origin) => {
            [RemoteCommand::ClaimOrigin, RemoteCommand::DispatchRepayment]
                .into_iter()
                .collect()
        }
        (CrossChainRoute::Direct, LegRole::Destination) => [
            RemoteCommand::Deliver,
            RemoteCommand::DispatchFillProof,
            RemoteCommand::CloseDestination,
        ]
        .into_iter()
        .collect(),
        (CrossChainRoute::Cctp, LegRole::Origin) => {
            [RemoteCommand::ClaimOrigin].into_iter().collect()
        }
        (CrossChainRoute::Cctp, LegRole::Destination) => {
            [RemoteCommand::Deliver, RemoteCommand::DispatchFillProof]
                .into_iter()
                .collect()
        }
    };
    actual == expected && actual.len() == plan.steps.len()
}

fn command_is_valid_for(
    role: LegRole,
    route: crate::primitives::crosschain::CrossChainRoute,
    command: RemoteCommand,
) -> bool {
    use crate::primitives::crosschain::CrossChainRoute;
    matches!(
        (route, role, command),
        (
            CrossChainRoute::Direct,
            LegRole::Origin,
            RemoteCommand::ClaimOrigin
        ) | (
            CrossChainRoute::Direct,
            LegRole::Origin,
            RemoteCommand::DispatchRepayment
        ) | (
            CrossChainRoute::Direct,
            LegRole::Destination,
            RemoteCommand::Deliver
        ) | (
            CrossChainRoute::Direct,
            LegRole::Destination,
            RemoteCommand::DispatchFillProof
        ) | (
            CrossChainRoute::Direct,
            LegRole::Destination,
            RemoteCommand::CloseDestination
        ) | (
            CrossChainRoute::Cctp,
            LegRole::Origin,
            RemoteCommand::ClaimOrigin
        ) | (
            CrossChainRoute::Cctp,
            LegRole::Destination,
            RemoteCommand::Deliver
        ) | (
            CrossChainRoute::Cctp,
            LegRole::Destination,
            RemoteCommand::DispatchFillProof
        ) | (
            CrossChainRoute::Cctp,
            LegRole::Destination,
            RemoteCommand::CloseDestination
        )
    )
}
