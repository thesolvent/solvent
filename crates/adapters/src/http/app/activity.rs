//! `GET /v1/activity` — the Aqua on-chain event feed (ship / push / pull / dock), newest first,
//! from the durable event log. Filters (`kind`, `actor`, `token`) are applied to the decoded events;
//! a selective filter may return a sparse page that still carries a `next_cursor` (keep paging until
//! it is absent).

use alloy::primitives::Address;
use axum::extract::{Query, State};
use serde::{Deserialize, Serialize};
use solvent_core::asset::AssetManager;
use solvent_core::deps::registry::RecordedEvent;
use solvent_core::primitives::amount::TokenAmount;
use solvent_core::primitives::registry::{AquaEvent, EventCursor, EventExt};
use solvent_core::primitives::{ChainId, MakerId};
use solvent_core::valuation::Valuation;
use solvent_core::SolventError;

use crate::http::dto::{Cursor, List, Page};
use crate::http::primitives::{parse_addr, ApiResult, Response};
use crate::http::state::AppState;

/// One Aqua event on the feed. `token` is present only for the balance-moving kinds (push / pull).
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct ActivityEvent {
    pub kind: String,
    pub maker: String,
    pub strategy_hash: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<TokenAmount>,
    /// Unix seconds the event was recorded.
    pub at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub block_number: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tx_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log_index: Option<u64>,
}

/// Feed query: pagination (`limit`/`cursor`) and filters (`kind`, `actor` maker, `token`).
#[derive(Debug, Deserialize)]
pub struct ActivityQuery {
    limit: Option<u32>,
    cursor: Option<String>,
    kind: Option<String>,
    actor: Option<String>,
    token: Option<String>,
}

#[utoipa::path(
    get,
    path = "/v1/activity",
    params(
        ("limit" = Option<u32>, Query, description = "Page size (default 50, max 200)"),
        ("cursor" = Option<String>, Query, description = "Opaque next-page cursor"),
        ("kind" = Option<String>, Query, description = "shipped|pushed|pulled|docked"),
        ("actor" = Option<String>, Query, description = "Maker address filter"),
        ("token" = Option<String>, Query, description = "Token address filter (push/pull only)"),
    ),
    responses((status = 200, body = Response<List<ActivityEvent>>))
)]
pub async fn activity(
    State(state): State<AppState>,
    Query(query): Query<ActivityQuery>,
) -> ApiResult<List<ActivityEvent>> {
    let page = Page {
        limit: query.limit,
        cursor: query.cursor.clone(),
    };
    let before = page
        .cursor::<(u64, u64)>()?
        .map(|(block_number, log_index)| EventCursor {
            block_number,
            log_index,
        });
    let limit = page.limit();
    let actor = query
        .actor
        .as_deref()
        .map(|s| parse_addr("address", s))
        .transpose()?
        .map(MakerId);
    let token = query
        .token
        .as_deref()
        .map(|s| parse_addr("address", s))
        .transpose()?;

    let rows = state
        .registry_store
        .recent(ChainId(state.config.chain_id), before, limit)
        .await
        .map_err(SolventError::from)?;
    // The cursor tracks the last *raw* row that carries one, so paging advances even when the
    // final rows lack a cursor rather than halting early.
    let next_cursor = (rows.len() as u32 == limit)
        .then(|| rows.iter().rev().find_map(|r| r.event.cursor()))
        .flatten()
        .map(|c| Cursor::encode(&(c.block_number, c.log_index)));
    let mut items = Vec::new();
    for row in &rows {
        if let Some(event) = shape(
            &state.assets,
            &state.valuation,
            row,
            query.kind.as_deref(),
            actor,
            token,
        )
        .await
        {
            items.push(event);
        }
    }
    Ok(Response::ok(List::page(items, next_cursor, None)))
}

/// Apply the filters to one recorded event and shape it, or drop it (`None`).
async fn shape(
    assets: &AssetManager,
    valuation: &Valuation,
    rec: &RecordedEvent,
    kind: Option<&str>,
    actor: Option<MakerId>,
    token: Option<Address>,
) -> Option<ActivityEvent> {
    let ext: &EventExt<AquaEvent> = &rec.event;
    let event = &ext.event;

    if kind.is_some_and(|k| !kind_name(event).eq_ignore_ascii_case(k)) {
        return None;
    }
    let key = event.key();
    if actor.is_some_and(|a| key.maker != a) {
        return None;
    }
    let moved = token_moved(event);
    if let Some(want) = token {
        if moved.map(|(t, _)| t) != Some(want) {
            return None;
        }
    }

    let moved_amount = match moved {
        Some((addr, amount)) => {
            let token = assets.token_or_default(addr);
            Some(TokenAmount {
                amount: valuation
                    .amount(amount, token.address, token.decimals)
                    .await,
                token,
            })
        }
        None => None,
    };
    Some(ActivityEvent {
        kind: kind_name(event).to_string(),
        maker: key.maker.to_string(),
        strategy_hash: key.strategy_hash.to_string(),
        token: moved_amount,
        at: rec.at,
        block_number: ext.block_number,
        tx_hash: ext.transaction_hash.map(|h| h.to_string()),
        log_index: ext.log_index,
    })
}

/// The token and amount a balance-moving event carried, if any.
fn token_moved(event: &AquaEvent) -> Option<(Address, alloy::primitives::U256)> {
    match *event {
        AquaEvent::Pushed { token, amount, .. } | AquaEvent::Pulled { token, amount, .. } => {
            Some((token, amount))
        }
        _ => None,
    }
}

fn kind_name(event: &AquaEvent) -> &'static str {
    match event {
        AquaEvent::Shipped { .. } => "shipped",
        AquaEvent::Pushed { .. } => "pushed",
        AquaEvent::Pulled { .. } => "pulled",
        AquaEvent::Docked { .. } => "docked",
        _ => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::{B256, U256};
    use async_trait::async_trait;
    use solvent_core::asset::{AssetManager, TokenList};
    use solvent_core::deps::routing::{PriceOracle, PriceOracleError};
    use solvent_core::primitives::registry::EventExt;
    use solvent_core::primitives::{StrategyHash, UsdPrice};
    use solvent_core::registry::SharedSnapshot;
    use std::sync::Arc;

    fn assets() -> AssetManager {
        AssetManager::new(
            TokenList {
                name: "t".into(),
                tokens: vec![],
            },
            Arc::new(SharedSnapshot::default()),
        )
    }

    struct NoPrices;
    #[async_trait]
    impl PriceOracle for NoPrices {
        async fn price(&self, token: Address) -> Result<UsdPrice, PriceOracleError> {
            Err(PriceOracleError::NotFound(token))
        }
    }
    fn valuation() -> Valuation {
        Valuation::new(Arc::new(NoPrices))
    }

    fn pushed(maker: u8, token: u8, amount: u64) -> RecordedEvent {
        RecordedEvent {
            at: 1_000,
            event: EventExt {
                event: AquaEvent::Pushed {
                    maker: MakerId(Address::from([maker; 20])),
                    app: Address::ZERO,
                    strategy_hash: StrategyHash(B256::from([1; 32])),
                    token: Address::from([token; 20]),
                    amount: U256::from(amount),
                },
                address: Address::ZERO,
                block_hash: None,
                block_number: Some(10),
                transaction_hash: None,
                transaction_index: None,
                log_index: Some(2),
                removed: false,
            },
        }
    }

    #[tokio::test]
    async fn shapes_a_push_with_its_token_amount() {
        let v = valuation();
        let dto = shape(&assets(), &v, &pushed(5, 9, 1_000), None, None, None)
            .await
            .unwrap();
        assert_eq!(dto.kind, "pushed");
        assert_eq!(dto.at, 1_000);
        assert_eq!(dto.token.unwrap().amount.raw, "1000");
    }

    #[tokio::test]
    async fn filters_drop_non_matching_events() {
        let v = valuation();
        let rec = pushed(5, 9, 1_000);
        // Wrong kind, wrong actor, and wrong token each drop it.
        assert!(shape(&assets(), &v, &rec, Some("pulled"), None, None)
            .await
            .is_none());
        assert!(shape(
            &assets(),
            &v,
            &rec,
            None,
            Some(MakerId(Address::from([6; 20]))),
            None
        )
        .await
        .is_none());
        assert!(shape(
            &assets(),
            &v,
            &rec,
            None,
            None,
            Some(Address::from([8; 20]))
        )
        .await
        .is_none());
        // Matching filters keep it.
        assert!(shape(
            &assets(),
            &v,
            &rec,
            Some("pushed"),
            None,
            Some(Address::from([9; 20]))
        )
        .await
        .is_some());
    }
}
