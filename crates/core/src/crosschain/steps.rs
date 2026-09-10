use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use alloy_primitives::Address;

use crate::deps::crosschain::{RemoteProgress, StepMaterializer, StepStore};
use crate::deps::execution::{Execution, SimGate};
use crate::primitives::crosschain::{ChainExecutionPlan, RemoteCommand, StepEvidence};
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
            materializer: None,
        }
    }

    pub fn with_materializer(mut self, materializer: Arc<dyn StepMaterializer>) -> Self {
        self.materializer = Some(materializer);
        self
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all, fields(chain_id = %self.chain_id)))]
    pub async fn stage(
        &self,
        order_id: CrossChainOrderId,
        plan: &ChainExecutionPlan,
    ) -> Result<(), SolventError> {
        if plan.chain_id != self.chain_id {
            return Err(SolventError::InvalidCrossChain(
                "execution plan belongs to another chain".to_string(),
            ));
        }
        let mut seen = BTreeSet::new();
        for step in &plan.steps {
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
        self.store.stage(order_id, plan).await?;
        Ok(())
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all, fields(chain_id = %self.chain_id, order_id = %order_id, command = ?command)))]
    pub async fn command(
        &self,
        order_id: CrossChainOrderId,
        command_id: CrossChainStepId,
        command: RemoteCommand,
    ) -> Result<RemoteProgress, SolventError> {
        let expected_id = crate::crosschain::proxy::step_id(order_id, self.chain_id, command);
        if expected_id != command_id {
            return Err(SolventError::InvalidCrossChain(
                "command id does not bind the order and step".to_string(),
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
