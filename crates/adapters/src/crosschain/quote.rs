use std::sync::Arc;

use alloy::primitives::{B256, U256};
use async_trait::async_trait;
use solvent_core::deps::crosschain::{LegQuoter, LegQuoterError};
use solvent_core::deps::ledger::Clock;
use solvent_core::primitives::crosschain::{CrossChainRoute, LegQuote, LegQuoteRequest, LegRole};
use solvent_core::primitives::ledger::ReservationSource;
use solvent_core::primitives::{ChainId, MakerId, StrategyHash};
use solvent_core::quote::QuoteService;

use crate::chain::ChainHead;

/// Reuses the normal chain-local routing book; the proxy only joins the resulting promises.
pub struct ServiceLegQuoter {
    chain_id: ChainId,
    quote: Arc<QuoteService>,
    head: ChainHead,
    clock: Arc<dyn Clock>,
    ttl_secs: u64,
}

impl ServiceLegQuoter {
    pub fn new(
        chain_id: ChainId,
        quote: Arc<QuoteService>,
        head: ChainHead,
        clock: Arc<dyn Clock>,
        ttl_secs: u64,
    ) -> Self {
        Self {
            chain_id,
            quote,
            head,
            clock,
            ttl_secs,
        }
    }
}

#[async_trait]
impl LegQuoter for ServiceLegQuoter {
    async fn quote(&self, request: &LegQuoteRequest) -> Result<LegQuote, LegQuoterError> {
        if request.local_chain != self.chain_id {
            return Err(LegQuoterError::Backend(format!(
                "configured for chain {}, received {}",
                self.chain_id, request.local_chain
            )));
        }
        let routed = if request.role == LegRole::Origin && request.route == CrossChainRoute::Direct
        {
            None
        } else {
            Some(
                self.quote
                    .quote(request.input_token, request.output_token, request.amount)
                    .await
                    .ok_or(LegQuoterError::NoRoute)?,
            )
        };
        let amount_out = match &routed {
            Some(quote) => quote
                .amount_out
                .raw
                .parse::<U256>()
                .map_err(|error| LegQuoterError::Backend(error.to_string()))?,
            None => request.amount,
        };
        let block_number = self.head.latest();
        let expires_at_unix = request
            .deadline_unix
            .min(self.clock.now_unix().saturating_add(self.ttl_secs));
        let sources = routed
            .iter()
            .flat_map(|quote| quote.legs.iter())
            .map(|leg| {
                let strategy_hash = leg
                    .strategy_hash
                    .parse::<StrategyHash>()
                    .map_err(|error| LegQuoterError::Backend(error.to_string()))?;
                let amount = leg
                    .amount_out
                    .raw
                    .parse::<U256>()
                    .map_err(|error| LegQuoterError::Backend(error.to_string()))?;
                Ok(ReservationSource {
                    maker: MakerId(leg.maker),
                    strategy_hash,
                    token: leg.token_out.address,
                    amount,
                })
            })
            .collect::<Result<Vec<_>, LegQuoterError>>()?;
        let mut quote = LegQuote {
            quote_id: B256::ZERO,
            request_id: request.request_id,
            role: request.role,
            local_chain: request.local_chain,
            remote_chain: request.remote_chain,
            input_token: request.input_token,
            output_token: request.output_token,
            amount_in: request.amount,
            amount_out,
            route: request.route,
            block_number,
            expires_at_unix,
            sources,
        };
        quote.quote_id = solvent_core::crosschain::leg_quote_id(&quote);
        Ok(quote)
    }
}
