//! The one public error type. Each port defines its own `{Trait}Error` that maps in via `From`;
//! a variant is added only when a consumer needs it — and no classification (`kind()`) until a
//! caller actually branches on it.

use thiserror::Error;

use crate::deps::asset::PairPriceHistorySourceError;
use crate::deps::balances::BalancesOracleError;
use crate::deps::crosschain::{
    CctpAttestationError, CctpCompletionError, DirectPlanAuthorError, LegQuoteStoreError,
    LegQuoterError, PreparationStoreError, RemoteSolventError, SagaStoreError,
    StepMaterializerError, StepStoreError, StepValidatorError,
};
use crate::deps::execution::{ExecutionAuthorizerError, ExecutionError, SettlementError, SimError};
use crate::deps::ingest::{FillBuilderError, NormalizeError};
use crate::deps::ledger::{BudgetSourceError, LedgerStoreError};
use crate::deps::maker_metrics::MakerMetricsError;
use crate::deps::rebate::{
    RebateAccrualSourceError, RebateCallBuilderError, RebateChainSourceError,
    RebateMarketBookError, RebateStoreError,
};
use crate::deps::registry::{BlockTimesError, ChainSourceError, StoreError};
use crate::deps::routing::GasPriceError;
use crate::deps::trade::TradeStoreError;
use crate::primitives::ledger::LedgerError;
use crate::rebate::RebateError;

/// The error every fallible Solvent API returns.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum SolventError {
    #[error("cross-chain quote: {0}")]
    LegQuote(#[from] LegQuoterError),
    #[error("cross-chain quote store: {0}")]
    LegQuoteStore(#[from] LegQuoteStoreError),
    #[error("cross-chain preparation store: {0}")]
    PreparationStore(#[from] PreparationStoreError),
    #[error("cross-chain saga store: {0}")]
    SagaStore(#[from] SagaStoreError),
    #[error("cross-chain step store: {0}")]
    StepStore(#[from] StepStoreError),
    #[error("cross-chain step materializer: {0}")]
    StepMaterializer(#[from] StepMaterializerError),
    #[error("cross-chain step validation: {0}")]
    StepValidator(#[from] StepValidatorError),
    #[error("direct plan author: {0}")]
    DirectPlanAuthor(#[from] DirectPlanAuthorError),
    #[error("CCTP attestation: {0}")]
    CctpAttestation(#[from] CctpAttestationError),
    #[error("CCTP completion: {0}")]
    CctpCompletion(#[from] CctpCompletionError),
    #[error("remote Solvent: {0}")]
    RemoteSolvent(#[from] RemoteSolventError),
    #[error("invalid cross-chain request: {0}")]
    InvalidCrossChain(String),
    /// Reading historical market prices failed.
    #[error("pair price history: {0}")]
    PairPriceHistory(#[from] PairPriceHistorySourceError),
    #[error("block times: {0}")]
    BlockTimes(#[from] BlockTimesError),
    /// A typed identifier could not be parsed from its input.
    #[error("invalid {id_type} id: {reason}")]
    InvalidId {
        id_type: &'static str,
        reason: String,
    },
    /// Reading Aqua events from the chain failed.
    #[error("chain source: {0}")]
    ChainSource(#[from] ChainSourceError),
    /// The registry store failed.
    #[error("store: {0}")]
    Store(#[from] StoreError),
    /// The ledger rejected a command (over-commitment, wrong state, bad fill).
    #[error("ledger: {0}")]
    Ledger(#[from] LedgerError),
    /// The ledger store failed.
    #[error("ledger store: {0}")]
    LedgerStore(#[from] LedgerStoreError),
    /// Rebate accrual or policy evaluation failed.
    #[error("rebate: {0}")]
    Rebate(#[from] RebateError),
    /// The durable rebate batch store failed.
    #[error("rebate store: {0}")]
    RebateStore(#[from] RebateStoreError),
    /// Reading confirmed fills awaiting rebate accrual failed.
    #[error("rebate accrual source: {0}")]
    RebateAccrualSource(#[from] RebateAccrualSourceError),
    /// Reading mined rebate execution events failed.
    #[error("rebate chain source: {0}")]
    RebateChainSource(#[from] RebateChainSourceError),
    /// A fresh executable market book was unavailable.
    #[error("rebate market: {0}")]
    RebateMarketBook(#[from] RebateMarketBookError),
    /// Building signed public rebate calldata failed.
    #[error("rebate call builder: {0}")]
    RebateCallBuilder(#[from] RebateCallBuilderError),
    /// Reading the current gas price for a rebate failed.
    #[error("gas price: {0}")]
    GasPrice(#[from] GasPriceError),
    /// Reading a settleable budget failed.
    #[error("budget source: {0}")]
    BudgetSource(#[from] BudgetSourceError),
    /// Reading a wallet's on-chain balances failed.
    #[error("balances oracle: {0}")]
    BalancesOracle(#[from] BalancesOracleError),
    /// The tx engine failed to submit or track a fill.
    #[error("execution: {0}")]
    Execution(#[from] ExecutionError),
    /// Signing a maker-execution policy failed.
    #[error("execution authorizer: {0}")]
    ExecutionAuthorizer(#[from] ExecutionAuthorizerError),
    /// The simulation engine failed to evaluate a fill.
    #[error("simulation: {0}")]
    Sim(#[from] SimError),
    /// Reading a confirmed fill's on-chain settlement failed.
    #[error("settlement: {0}")]
    Settlement(#[from] SettlementError),
    /// The trade store failed.
    #[error("trade store: {0}")]
    TradeStore(#[from] TradeStoreError),
    /// Normalizing a protocol order into an intent failed — a malformed order (client input).
    #[error("normalize: {0}")]
    Normalize(#[from] NormalizeError),
    /// Building a protocol fill's calldata failed.
    #[error("fill builder: {0}")]
    FillBuilder(#[from] FillBuilderError),
    /// Reading maker metrics failed.
    #[error("maker metrics: {0}")]
    MakerMetrics(#[from] MakerMetricsError),
}
