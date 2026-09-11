use std::sync::Arc;

use alloy_primitives::keccak256;

use crate::deps::crosschain::{LegQuoteStore, LegQuoter, PreparationStore};
use crate::deps::ledger::Clock;
use crate::ledger::LedgerService;
use crate::primitives::crosschain::{
    LegQuote, LegQuoteRequest, Preparation, PreparationState, StepValidationContext,
};
use crate::primitives::ledger::{LedgerError, ReservationState};
use crate::primitives::{
    AggregateQuoteId, ChainId, IntentId, PrepareToken, ReservationId, SolventError,
};

/// Chain-local quote and capital preparation. It rejects foreign-chain work before touching state.
pub struct LocalCrossChainService {
    chain_id: ChainId,
    quoter: Arc<dyn LegQuoter>,
    quotes: Arc<dyn LegQuoteStore>,
    preparations: Arc<dyn PreparationStore>,
    ledger: Arc<LedgerService>,
    clock: Arc<dyn Clock>,
}

impl LocalCrossChainService {
    pub fn new(
        chain_id: ChainId,
        quoter: Arc<dyn LegQuoter>,
        quotes: Arc<dyn LegQuoteStore>,
        preparations: Arc<dyn PreparationStore>,
        ledger: Arc<LedgerService>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            chain_id,
            quoter,
            quotes,
            preparations,
            ledger,
            clock,
        }
    }

    pub async fn quote(&self, request: &LegQuoteRequest) -> Result<LegQuote, SolventError> {
        if request.local_chain != self.chain_id {
            return Err(SolventError::InvalidCrossChain(format!(
                "request chain {} does not match service chain {}",
                request.local_chain, self.chain_id
            )));
        }
        if request.deadline_unix <= self.clock.now_unix() {
            return Err(SolventError::InvalidCrossChain(
                "quote deadline has expired".to_string(),
            ));
        }
        let quote = self.quoter.quote(request).await?;
        validate_leg(request, &quote, self.clock.now_unix())?;
        Ok(self.quotes.insert(&quote).await?)
    }

    pub async fn prepare(
        &self,
        aggregate_id: AggregateQuoteId,
        quote: &LegQuote,
    ) -> Result<Preparation, SolventError> {
        if quote.local_chain != self.chain_id {
            return Err(SolventError::InvalidCrossChain(
                "cannot prepare a quote owned by another chain".to_string(),
            ));
        }
        if quote.expires_at_unix <= self.clock.now_unix() {
            return Err(SolventError::InvalidCrossChain(
                "cannot prepare an expired quote".to_string(),
            ));
        }
        if quote.quote_id != crate::crosschain::leg_quote_id(quote) {
            return Err(SolventError::InvalidCrossChain(
                "chain-local quote fingerprint is invalid".to_string(),
            ));
        }
        let issued = self.quotes.load(quote.quote_id).await?.ok_or_else(|| {
            SolventError::InvalidCrossChain(
                "chain-local quote was not issued by this service".to_string(),
            )
        })?;
        if issued != *quote {
            return Err(SolventError::InvalidCrossChain(
                "chain-local quote terms differ from the issued quote".to_string(),
            ));
        }
        if let Some(existing) = self
            .preparations
            .load_for_quote(aggregate_id, quote.role)
            .await?
        {
            return Ok(existing);
        }

        let preparation = Preparation {
            token: preparation_token(aggregate_id, self.chain_id, quote.role),
            quote_id: aggregate_id,
            role: quote.role,
            chain_id: self.chain_id,
            state: PreparationState::Prepared,
            expires_at_unix: quote.expires_at_unix,
        };
        let reservation = ReservationId(preparation.token.0);
        let ttl = quote.expires_at_unix.saturating_sub(self.clock.now_unix());
        match self
            .ledger
            .reserve(
                reservation,
                IntentId(aggregate_id.0),
                quote.sources.clone(),
                ttl,
            )
            .await
        {
            Ok(()) | Err(SolventError::Ledger(LedgerError::Duplicate(_))) => {}
            Err(error) => return Err(error),
        }
        Ok(self.preparations.insert(&preparation).await?)
    }

    pub async fn validate_stage_context(
        &self,
        context: &StepValidationContext,
    ) -> Result<(), SolventError> {
        crate::crosschain::proxy::validate_aggregate_binding(
            &context.quote,
            self.clock.now_unix(),
        )?;
        let quote = context.local_quote();
        if quote.local_chain != self.chain_id
            || quote.quote_id != crate::crosschain::leg_quote_id(quote)
        {
            return Err(SolventError::InvalidCrossChain(
                "stage context does not contain this service's quote".to_string(),
            ));
        }
        let issued = self.quotes.load(quote.quote_id).await?.ok_or_else(|| {
            SolventError::InvalidCrossChain(
                "stage context quote was not issued by this service".to_string(),
            )
        })?;
        if issued != *quote {
            return Err(SolventError::InvalidCrossChain(
                "stage context differs from the locally issued quote".to_string(),
            ));
        }
        Ok(())
    }

    pub async fn commit(&self, token: PrepareToken) -> Result<Preparation, SolventError> {
        let mut preparation = self.load_owned(token).await?;
        if preparation.expires_at_unix <= self.clock.now_unix() {
            return Err(SolventError::InvalidCrossChain(
                "preparation expired before commit".to_string(),
            ));
        }
        preparation
            .commit()
            .map_err(|error| SolventError::InvalidCrossChain(error.to_string()))?;
        self.ledger.commit(ReservationId(token.0)).await?;
        self.preparations.update(&preparation).await?;
        Ok(preparation)
    }

    pub async fn inspect(&self, token: PrepareToken) -> Result<Preparation, SolventError> {
        self.load_owned(token).await
    }

    pub async fn release(&self, token: PrepareToken) -> Result<Preparation, SolventError> {
        let mut preparation = self.load_owned(token).await?;
        let already_released = preparation.state == PreparationState::Released;
        preparation
            .release()
            .map_err(|error| SolventError::InvalidCrossChain(error.to_string()))?;
        if !already_released {
            match self.ledger.void(ReservationId(token.0)).await {
                Ok(())
                | Err(SolventError::Ledger(LedgerError::WrongState {
                    found: ReservationState::Voided,
                    ..
                })) => {}
                Err(error) => return Err(error),
            }
        }
        self.preparations.update(&preparation).await?;
        Ok(preparation)
    }

    pub async fn mark_executed(&self, token: PrepareToken) -> Result<Preparation, SolventError> {
        let mut preparation = self.load_owned(token).await?;
        let already_executed = preparation.state == PreparationState::Executed;
        preparation
            .execute()
            .map_err(|error| SolventError::InvalidCrossChain(error.to_string()))?;
        if !already_executed {
            let sources = self
                .ledger
                .settlement_reservation_sources(ReservationId(token.0))
                .await
                .ok_or_else(|| {
                    SolventError::InvalidCrossChain(
                        "preparation has no capital reservation".to_string(),
                    )
                })?;
            let filled = sources
                .iter()
                .map(|source| source.amount)
                .collect::<Vec<_>>();
            match self.ledger.post(ReservationId(token.0), &filled).await {
                Ok(())
                | Err(SolventError::Ledger(LedgerError::WrongState {
                    found: ReservationState::Posted,
                    ..
                })) => {}
                Err(error) => return Err(error),
            }
        }
        self.preparations.update(&preparation).await?;
        Ok(preparation)
    }

    async fn load_owned(&self, token: PrepareToken) -> Result<Preparation, SolventError> {
        let preparation = self.preparations.load(token).await?.ok_or_else(|| {
            SolventError::InvalidCrossChain("unknown preparation token".to_string())
        })?;
        if preparation.chain_id != self.chain_id {
            return Err(SolventError::InvalidCrossChain(
                "preparation belongs to another chain".to_string(),
            ));
        }
        Ok(preparation)
    }
}

fn validate_leg(
    request: &LegQuoteRequest,
    quote: &LegQuote,
    now_unix: u64,
) -> Result<(), SolventError> {
    let matches = quote.request_id == request.request_id
        && quote.role == request.role
        && quote.local_chain == request.local_chain
        && quote.remote_chain == request.remote_chain
        && quote.input_token == request.input_token
        && quote.output_token == request.output_token
        && quote.route == request.route
        && quote.amount_in == request.amount;
    if !matches {
        return Err(SolventError::InvalidCrossChain(
            "chain-local quote does not match its request".to_string(),
        ));
    }
    if quote.expires_at_unix <= now_unix || quote.expires_at_unix > request.deadline_unix {
        return Err(SolventError::InvalidCrossChain(
            "chain-local quote has an invalid expiry".to_string(),
        ));
    }
    Ok(())
}

fn preparation_token(
    quote_id: AggregateQuoteId,
    chain_id: ChainId,
    role: crate::primitives::crosschain::LegRole,
) -> PrepareToken {
    let mut bytes = Vec::with_capacity(41);
    bytes.extend_from_slice(quote_id.0.as_slice());
    bytes.extend_from_slice(&chain_id.0.to_be_bytes());
    bytes.push(role as u8);
    PrepareToken(keccak256(bytes))
}
