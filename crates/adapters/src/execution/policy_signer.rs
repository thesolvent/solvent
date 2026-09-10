//! Local EIP-712 signer for policy-authorized maker executions.

use alloy::primitives::{Address, Bytes};
use alloy::signers::{local::PrivateKeySigner, SignerSync};
use alloy::sol_types::{eip712_domain, SolStruct};
use async_trait::async_trait;

use solvent_core::deps::execution::{ExecutionAuthorizer, ExecutionAuthorizerError};
use solvent_core::primitives::execution::{ExecutionAuthorization, PolicySignature};

use super::filler::Authorization;

/// Signs the filler contract's authorization schema with a local key. The key is never exposed via
/// `Debug`, errors, or tracing fields.
pub struct LocalPolicySigner {
    chain_id: u64,
    filler: Address,
    signer: PrivateKeySigner,
}

impl LocalPolicySigner {
    pub fn new(chain_id: u64, filler: Address, signer: PrivateKeySigner) -> Self {
        Self {
            chain_id,
            filler,
            signer,
        }
    }
}

#[async_trait]
impl ExecutionAuthorizer for LocalPolicySigner {
    #[tracing::instrument(skip_all, fields(chain_id = self.chain_id, filler = %self.filler))]
    async fn authorize(
        &self,
        authorization: &ExecutionAuthorization,
    ) -> Result<PolicySignature, ExecutionAuthorizerError> {
        let authorization = Authorization::from(authorization);
        let domain = eip712_domain! {
            name: "Solvent Aqua Filler",
            version: "1",
            chain_id: self.chain_id,
            verifying_contract: self.filler,
        };
        let digest = authorization.eip712_signing_hash(&domain);
        self.signer
            .sign_hash_sync(&digest)
            .map(|signature| PolicySignature::new(Bytes::from(signature.as_bytes())))
            .map_err(|_| ExecutionAuthorizerError::Signing)
    }
}

#[cfg(test)]
mod tests {
    use alloy::primitives::{address, Address, B256, U256};
    use alloy::signers::local::PrivateKeySigner;
    use alloy::sol_types::{eip712_domain, SolStruct};

    use solvent_core::deps::execution::ExecutionAuthorizer;
    use solvent_core::primitives::execution::{ExecutionAuthorization, UserFillAuthorization};
    use solvent_core::primitives::{MakerId, StrategyHash};

    use super::{Authorization, LocalPolicySigner};

    #[tokio::test]
    async fn signature_recovers_for_the_contract_eip712_schema() {
        let key = B256::from([0x30; 32]);
        let signer = PrivateKeySigner::from_bytes(&key).expect("valid test key");
        let filler = address!("1111111111111111111111111111111111111111");
        let policy = LocalPolicySigner::new(31337, filler, signer.clone());
        let authorization = ExecutionAuthorization::user_fill(UserFillAuthorization {
            nonce: U256::from(7),
            context_hash: B256::from([0x11; 32]),
            strategy_hash: StrategyHash(B256::from([0x22; 32])),
            maker: MakerId(Address::from([0x33; 20])),
            token_in: Address::from([0x44; 20]),
            token_out: Address::from([0x55; 20]),
            amount_out: U256::from(100),
            amount_in_limit: U256::from(200),
        });

        let signature = policy
            .authorize(&authorization)
            .await
            .expect("local signing succeeds")
            .into_bytes();
        let parsed = alloy::primitives::Signature::from_raw(&signature).expect("65-byte signature");
        let domain = eip712_domain! {
            name: "Solvent Aqua Filler",
            version: "1",
            chain_id: 31337,
            verifying_contract: filler,
        };
        let digest = Authorization::from(&authorization).eip712_signing_hash(&domain);

        assert_eq!(
            Authorization::eip712_root_type(),
            "Authorization(uint8 kind,uint256 nonce,bytes32 contextHash,bytes32 strategyHash,address maker,address tokenIn,address tokenOut,uint256 amountOut,uint256 amountInLimit,uint256 rebateAmount,uint64 deadlineBlock)"
        );
        assert_eq!(
            parsed
                .recover_address_from_prehash(&digest)
                .expect("recovers"),
            signer.address()
        );
    }
}
