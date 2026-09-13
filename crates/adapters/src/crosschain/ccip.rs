use alloy::providers::Provider;
use alloy::sol;
use alloy::sol_types::SolCall;
use async_trait::async_trait;
use solvent_core::deps::crosschain::{StepMaterializer, StepMaterializerError};
use solvent_core::primitives::crosschain::{PreparedStep, RemoteCommand};

sol! {
    #[sol(rpc)]
    interface ICcipProofOutbox {
        function fee(bytes calldata envelope) external view returns (uint256);
        function dispatch(bytes32 orderId, bytes calldata envelope) external payable returns (bytes32 messageId);
    }
}

pub struct CcipStepMaterializer<P> {
    provider: P,
}

impl<P> CcipStepMaterializer<P> {
    pub fn new(provider: P) -> Self {
        Self { provider }
    }
}

#[async_trait]
impl<P: Provider + Clone + 'static> StepMaterializer for CcipStepMaterializer<P> {
    async fn materialize(
        &self,
        step: &PreparedStep,
    ) -> Result<PreparedStep, StepMaterializerError> {
        if !matches!(
            step.command,
            RemoteCommand::DispatchFillProof | RemoteCommand::DispatchRepayment
        ) {
            return Ok(step.clone());
        }
        let call = ICcipProofOutbox::dispatchCall::abi_decode(&step.calldata)
            .map_err(|error| StepMaterializerError::Invalid(error.to_string()))?;
        let outbox = ICcipProofOutbox::new(step.target, self.provider.clone());
        let fee = outbox
            .fee(call.envelope)
            .call()
            .await
            .map_err(|error| StepMaterializerError::Backend(error.to_string()))?;
        let mut materialized = step.clone();
        materialized.value = fee;
        Ok(materialized)
    }
}
