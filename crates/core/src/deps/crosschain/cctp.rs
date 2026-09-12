use alloy_primitives::{Bytes, B256, U256};
use async_trait::async_trait;
use thiserror::Error;

/// Signed Circle material. Debug output never exposes message or attestation bytes.
#[derive(Clone, PartialEq, Eq)]
pub struct CctpAttestationStatus {
    pub message: Bytes,
    pub attestation: Bytes,
    pub nonce: String,
}

impl core::fmt::Debug for CctpAttestationStatus {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("CctpAttestationStatus")
            .field("message", &"[REDACTED]")
            .field("attestation", &"[REDACTED]")
            .field("nonce", &self.nonce)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CctpFee {
    pub finality_threshold: u32,
    /// Parts per million, preserving fractional basis points without floating point.
    pub minimum_fee_ppm: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CctpRouteCapacity {
    pub fees: Vec<CctpFee>,
    pub fast_allowance_subunits: U256,
}

#[derive(Debug, Error)]
pub enum CctpAttestationError {
    #[error("Circle attestation is not ready")]
    Pending,
    #[error("Circle rate limit; retry after {retry_after_secs:?} seconds")]
    RateLimited { retry_after_secs: Option<u64> },
    #[error("Circle API transport: {0}")]
    Transport(String),
    #[error("invalid Circle response: {0}")]
    InvalidResponse(String),
}

#[async_trait]
pub trait CctpAttestation: Send + Sync {
    async fn message(
        &self,
        source_domain: u32,
        transaction_hash: B256,
    ) -> Result<CctpAttestationStatus, CctpAttestationError>;
    async fn reattest(&self, nonce: &str) -> Result<(), CctpAttestationError>;
    async fn route_capacity(
        &self,
        source_domain: u32,
        destination_domain: u32,
    ) -> Result<CctpRouteCapacity, CctpAttestationError>;
}
