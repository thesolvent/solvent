use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use alloy::primitives::{keccak256, Address, Bytes, B256, U256};
use alloy::sol;
use alloy::sol_types::SolCall;
use async_trait::async_trait;
use reqwest::{Client, StatusCode, Url};
use serde::Deserialize;
use solvent_core::deps::crosschain::{
    CctpAttestation, CctpAttestationError, CctpAttestationStatus, CctpCompletion,
    CctpCompletionError, CctpFee, CctpPreparedStep, CctpQuoteTerms, CctpRouteCapacity,
};
use solvent_core::primitives::crosschain::{PreparedStep, RemoteCommand};
use solvent_core::primitives::CrossChainOrderId;

sol! {
    function completeRepayment(bytes32 orderId, bytes message, bytes attestation);
}

pub struct CircleCctpCompletion {
    iris: Arc<dyn CctpAttestation>,
    source_domain: u32,
    destination_domain: u32,
    finality_threshold: u32,
    destination_app: Address,
}

impl CircleCctpCompletion {
    pub fn new(
        iris: Arc<dyn CctpAttestation>,
        source_domain: u32,
        destination_domain: u32,
        finality_threshold: u32,
        destination_app: Address,
    ) -> Self {
        Self {
            iris,
            source_domain,
            destination_domain,
            finality_threshold,
            destination_app,
        }
    }
}

#[async_trait]
impl CctpCompletion for CircleCctpCompletion {
    async fn quote_terms(&self, repayment: U256) -> Result<CctpQuoteTerms, CctpCompletionError> {
        let capacity = self
            .iris
            .route_capacity(self.source_domain, self.destination_domain)
            .await
            .map_err(completion_error)?;
        let fee = capacity
            .fees
            .iter()
            .find(|fee| fee.finality_threshold == self.finality_threshold)
            .ok_or_else(|| {
                CctpCompletionError::Invalid(
                    "Circle returned no fee for the configured finality".to_string(),
                )
            })?;
        let denominator = U256::from(1_000_000u64);
        let numerator = repayment
            .checked_mul(U256::from(fee.minimum_fee_ppm))
            .and_then(|value| value.checked_add(denominator - U256::from(1)))
            .ok_or_else(|| CctpCompletionError::Invalid("CCTP fee overflow".to_string()))?;
        let max_fee = numerator / denominator;
        let burn_amount = repayment
            .checked_add(max_fee)
            .ok_or_else(|| CctpCompletionError::Invalid("CCTP burn overflow".to_string()))?;
        if capacity.fast_allowance_subunits < burn_amount {
            return Err(CctpCompletionError::Invalid(
                "Circle Fast Transfer allowance is below the requested repayment".to_string(),
            ));
        }
        Ok(CctpQuoteTerms {
            max_fee,
            finality_threshold: self.finality_threshold,
        })
    }

    async fn close_step(
        &self,
        order_id: CrossChainOrderId,
        origin_transaction: B256,
    ) -> Result<CctpPreparedStep, CctpCompletionError> {
        let status = self
            .iris
            .message(self.source_domain, origin_transaction)
            .await
            .map_err(completion_error)?;
        validate_message(
            &status.message,
            self.source_domain,
            self.destination_domain,
            self.destination_app,
            order_id,
        )?;
        let message_id = keccak256(&status.message);
        let calldata = completeRepaymentCall {
            orderId: order_id.0,
            message: status.message,
            attestation: status.attestation,
        }
        .abi_encode();
        Ok(CctpPreparedStep {
            step: PreparedStep {
                command: RemoteCommand::CloseDestination,
                target: self.destination_app,
                value: U256::ZERO,
                calldata: calldata.into(),
            },
            message_id,
        })
    }
}

fn completion_error(error: CctpAttestationError) -> CctpCompletionError {
    match error {
        CctpAttestationError::Pending => CctpCompletionError::Pending,
        CctpAttestationError::RateLimited { retry_after_secs } => {
            CctpCompletionError::RateLimited { retry_after_secs }
        }
        CctpAttestationError::Transport(message) => CctpCompletionError::Transport(message),
        CctpAttestationError::InvalidResponse(message) => CctpCompletionError::Invalid(message),
    }
}

fn validate_message(
    message: &[u8],
    source_domain: u32,
    destination_domain: u32,
    destination_app: Address,
    order_id: CrossChainOrderId,
) -> Result<(), CctpCompletionError> {
    const MESSAGE_LENGTH: usize = 440;
    if message.len() != MESSAGE_LENGTH {
        return Err(CctpCompletionError::Invalid(format!(
            "message length {} does not match {MESSAGE_LENGTH}",
            message.len()
        )));
    }
    if u32_at(message, 4)? != source_domain || u32_at(message, 8)? != destination_domain {
        return Err(CctpCompletionError::Invalid(
            "message domains do not match the configured route".to_string(),
        ));
    }
    let expected_app = padded_address(destination_app);
    if message[108..140] != expected_app || message[184..216] != expected_app {
        return Err(CctpCompletionError::Invalid(
            "message destination caller or mint recipient is not the destination app".to_string(),
        ));
    }
    if message[376..408] != U256::from(1).to_be_bytes::<32>() || message[408..440] != order_id.0[..]
    {
        return Err(CctpCompletionError::Invalid(
            "message repayment hook does not bind the order".to_string(),
        ));
    }
    Ok(())
}

fn u32_at(message: &[u8], offset: usize) -> Result<u32, CctpCompletionError> {
    let bytes: [u8; 4] = message
        .get(offset..offset + 4)
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or_else(|| CctpCompletionError::Invalid("truncated CCTP message".to_string()))?;
    Ok(u32::from_be_bytes(bytes))
}

fn padded_address(address: Address) -> [u8; 32] {
    let mut value = [0; 32];
    value[12..].copy_from_slice(address.as_slice());
    value
}

pub struct CircleIrisClient {
    client: Client,
    base_url: Url,
}

impl CircleIrisClient {
    pub fn new(base_url: Url) -> Result<Self, CctpAttestationError> {
        let client = Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|error| CctpAttestationError::InvalidResponse(error.to_string()))?;
        Ok(Self { client, base_url })
    }

    pub fn from_base_url(base_url: &str) -> Result<Self, CctpAttestationError> {
        let base_url = Url::parse(base_url)
            .map_err(|error| CctpAttestationError::InvalidResponse(error.to_string()))?;
        Self::new(base_url)
    }

    async fn get(&self, path: &str) -> Result<reqwest::Response, CctpAttestationError> {
        let url = self
            .base_url
            .join(path)
            .map_err(|error| CctpAttestationError::InvalidResponse(error.to_string()))?;
        self.client
            .get(url)
            .send()
            .await
            .map_err(|error| CctpAttestationError::Transport(error.to_string()))
    }
}

#[derive(Deserialize)]
struct MessagesResponse {
    messages: Vec<CircleMessage>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CircleMessage {
    message: String,
    attestation: String,
    event_nonce: String,
    status: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FeeResponse {
    finality_threshold: u32,
    minimum_fee: serde_json::Number,
}

#[derive(Deserialize)]
struct AllowanceResponse {
    allowance: serde_json::Number,
}

#[async_trait]
impl CctpAttestation for CircleIrisClient {
    async fn message(
        &self,
        source_domain: u32,
        transaction_hash: B256,
    ) -> Result<CctpAttestationStatus, CctpAttestationError> {
        let response = self
            .get(&format!(
                "v2/messages/{source_domain}?transactionHash={transaction_hash:#x}"
            ))
            .await?;
        if response.status() == StatusCode::NOT_FOUND {
            return Err(CctpAttestationError::Pending);
        }
        if response.status() == StatusCode::TOO_MANY_REQUESTS {
            return Err(rate_limited(&response));
        }
        if !response.status().is_success() {
            return Err(CctpAttestationError::Transport(format!(
                "Circle returned HTTP {}",
                response.status()
            )));
        }
        let body: MessagesResponse = response
            .json()
            .await
            .map_err(|error| CctpAttestationError::InvalidResponse(error.to_string()))?;
        let message = body
            .messages
            .into_iter()
            .find(|message| message.status == "complete")
            .ok_or(CctpAttestationError::Pending)?;
        Ok(CctpAttestationStatus {
            message: Bytes::from_str(&message.message)
                .map_err(|error| CctpAttestationError::InvalidResponse(error.to_string()))?,
            attestation: Bytes::from_str(&message.attestation)
                .map_err(|error| CctpAttestationError::InvalidResponse(error.to_string()))?,
            nonce: message.event_nonce,
        })
    }

    async fn reattest(&self, nonce: &str) -> Result<(), CctpAttestationError> {
        if nonce.is_empty() || !nonce.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(CctpAttestationError::InvalidResponse(
                "CCTP nonce must be decimal digits".to_string(),
            ));
        }
        let url = self
            .base_url
            .join(&format!("v2/reattest/{nonce}"))
            .map_err(|error| CctpAttestationError::InvalidResponse(error.to_string()))?;
        let response = self
            .client
            .post(url)
            .send()
            .await
            .map_err(|error| CctpAttestationError::Transport(error.to_string()))?;
        if response.status() == StatusCode::TOO_MANY_REQUESTS {
            return Err(rate_limited(&response));
        }
        if !response.status().is_success() {
            return Err(CctpAttestationError::Transport(format!(
                "Circle returned HTTP {}",
                response.status()
            )));
        }
        Ok(())
    }

    async fn route_capacity(
        &self,
        source_domain: u32,
        destination_domain: u32,
    ) -> Result<CctpRouteCapacity, CctpAttestationError> {
        let fees_response = self
            .get(&format!(
                "v2/burn/USDC/fees/{source_domain}/{destination_domain}"
            ))
            .await?;
        ensure_success(&fees_response)?;
        let fees: Vec<FeeResponse> = fees_response
            .json()
            .await
            .map_err(|error| CctpAttestationError::InvalidResponse(error.to_string()))?;
        let fees = fees
            .into_iter()
            .map(|fee| {
                Ok(CctpFee {
                    finality_threshold: fee.finality_threshold,
                    minimum_fee_ppm: decimal_scaled_u32(&fee.minimum_fee.to_string(), 2)?,
                })
            })
            .collect::<Result<Vec<_>, CctpAttestationError>>()?;

        let allowance_response = self.get("v2/fastBurn/USDC/allowance").await?;
        ensure_success(&allowance_response)?;
        let allowance: AllowanceResponse = allowance_response
            .json()
            .await
            .map_err(|error| CctpAttestationError::InvalidResponse(error.to_string()))?;
        Ok(CctpRouteCapacity {
            fees,
            fast_allowance_subunits: decimal_usdc(&allowance.allowance.to_string())?,
        })
    }
}

fn ensure_success(response: &reqwest::Response) -> Result<(), CctpAttestationError> {
    if response.status() == StatusCode::TOO_MANY_REQUESTS {
        return Err(rate_limited(response));
    }
    if !response.status().is_success() {
        return Err(CctpAttestationError::Transport(format!(
            "Circle returned HTTP {}",
            response.status()
        )));
    }
    Ok(())
}

fn rate_limited(response: &reqwest::Response) -> CctpAttestationError {
    let retry_after_secs = response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse().ok());
    CctpAttestationError::RateLimited { retry_after_secs }
}

fn decimal_usdc(value: &str) -> Result<U256, CctpAttestationError> {
    let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
    if whole.starts_with('-') || fraction.len() > 6 {
        return Err(CctpAttestationError::InvalidResponse(
            "allowance is not a non-negative six-decimal USDC amount".to_string(),
        ));
    }
    let whole = U256::from_str(whole)
        .map_err(|error| CctpAttestationError::InvalidResponse(error.to_string()))?;
    let mut padded = fraction.to_string();
    padded.extend(std::iter::repeat_n('0', 6 - fraction.len()));
    let fraction = if padded.is_empty() {
        U256::ZERO
    } else {
        U256::from_str(&padded)
            .map_err(|error| CctpAttestationError::InvalidResponse(error.to_string()))?
    };
    Ok(whole * U256::from(1_000_000u64) + fraction)
}

fn decimal_scaled_u32(value: &str, scale: usize) -> Result<u32, CctpAttestationError> {
    let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
    if whole.starts_with('-') || fraction.len() > scale {
        return Err(CctpAttestationError::InvalidResponse(
            "numeric value has unsupported precision".to_string(),
        ));
    }
    let mut digits = whole.to_string();
    digits.push_str(fraction);
    digits.extend(std::iter::repeat_n('0', scale - fraction.len()));
    digits
        .parse::<u32>()
        .map_err(|error| CctpAttestationError::InvalidResponse(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowance_decimal_never_uses_floating_point() {
        assert_eq!(decimal_usdc("123.45").unwrap(), U256::from(123_450_000u64));
        assert!(decimal_usdc("1.0000001").is_err());
        assert!(decimal_usdc("-1").is_err());
    }

    #[test]
    fn fractional_basis_points_become_parts_per_million() {
        assert_eq!(decimal_scaled_u32("1.3", 2).unwrap(), 130);
    }

    #[test]
    fn completion_rejects_a_message_for_another_order() {
        let app = Address::from([7; 20]);
        let order = CrossChainOrderId(B256::from([8; 32]));
        let mut message = vec![0; 440];
        message[4..8].copy_from_slice(&1u32.to_be_bytes());
        message[8..12].copy_from_slice(&2u32.to_be_bytes());
        message[108..140].copy_from_slice(&padded_address(app));
        message[184..216].copy_from_slice(&padded_address(app));
        message[376..408].copy_from_slice(&U256::from(1).to_be_bytes::<32>());
        message[408..440].copy_from_slice(B256::from([9; 32]).as_slice());
        assert!(validate_message(&message, 1, 2, app, order).is_err());
        message[408..440].copy_from_slice(order.0.as_slice());
        assert!(validate_message(&message, 1, 2, app, order).is_ok());
    }
}
