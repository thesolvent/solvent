//! `GET /v1/activity` — the Aqua on-chain event feed (ship / push / pull / dock), newest first,
//! from the durable event log. Filters (`kind`, `actor`, `token`) are applied to the decoded events;
//! a selective filter may return a sparse page that still carries a `next_cursor` (keep paging until
//! it is absent).

use alloy::primitives::Address;
use axum::extract::{Query, State};
use serde::{Deserialize, Serialize};
use solvent_core::asset::{AssetManager, Token};
use solvent_core::deps::registry::RecordedEvent;
use solvent_core::primitives::amount::{Amount, TokenAmount};
use solvent_core::primitives::registry::{AquaEvent, EventCursor, EventExt};
use solvent_core::primitives::{ChainId, MakerId};
use solvent_core::SolventError;

use crate::http::dto::{Cursor, List, Page};
use crate::http::primitives::{ApiResult, Response};
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
        .map(parse_addr)
        .transpose()?
        .map(MakerId);
    let token = query.token.as_deref().map(parse_addr).transpose()?;

    let rows = state
        .registry_store
        .recent(ChainId(state.config.chain_id), before, limit)
        .await
        .map_err(SolventError::from)?;
    // The cursor tracks the last *raw* row so paging advances even past filtered-out events.
    let next_cursor = (rows.len() as u32 == limit)
        .then(|| rows.last().and_then(|r| r.event.cursor()))
        .flatten()
        .map(|c| Cursor::encode(&(c.block_number, c.log_index)));
    let items = rows
        .iter()
        .filter_map(|r| shape(&state.assets, r, query.kind.as_deref(), actor, token))
        .collect();
    Ok(Response::ok(List::page(items, next_cursor, None)))
}

/// Apply the filters to one recorded event and shape it, or drop it (`None`).
fn shape(
    assets: &AssetManager,
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

    Some(ActivityEvent {
        kind: kind_name(event).to_string(),
        maker: key.maker.to_string(),
        strategy_hash: key.strategy_hash.to_string(),
        token: moved.map(|(addr, amount)| {
            let token = resolve_token(assets, addr);
            TokenAmount {
                amount: Amount::from_base_units(amount, token.decimals),
                token,
            }
        }),
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

/// The catalog token for `addr`, or a bare 18-decimal fallback for one not listed.
fn resolve_token(assets: &AssetManager, addr: Address) -> Token {
    assets.token(&addr).unwrap_or(Token {
        address: addr,
        chain_id: 0,
        symbol: String::new(),
        decimals: 18,
    })
}

fn parse_addr(s: &str) -> Result<Address, SolventError> {
    s.parse::<Address>().map_err(|e| SolventError::InvalidId {
        id_type: "address",
        reason: e.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::{B256, U256};
    use solvent_core::asset::{AssetManager, TokenList};
    use solvent_core::primitives::registry::EventExt;
    use solvent_core::primitives::StrategyHash;
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

    #[test]
    fn shapes_a_push_with_its_token_amount() {
        let dto = shape(&assets(), &pushed(5, 9, 1_000), None, None, None).unwrap();
        assert_eq!(dto.kind, "pushed");
        assert_eq!(dto.at, 1_000);
        assert_eq!(dto.token.unwrap().amount.raw, "1000");
    }

    #[test]
    fn filters_drop_non_matching_events() {
        let rec = pushed(5, 9, 1_000);
        // Wrong kind, wrong actor, and wrong token each drop it.
        assert!(shape(&assets(), &rec, Some("pulled"), None, None).is_none());
        assert!(shape(
            &assets(),
            &rec,
            None,
            Some(MakerId(Address::from([6; 20]))),
            None
        )
        .is_none());
        assert!(shape(&assets(), &rec, None, None, Some(Address::from([8; 20]))).is_none());
        // Matching filters keep it.
        assert!(shape(
            &assets(),
            &rec,
            Some("pushed"),
            None,
            Some(Address::from([9; 20]))
        )
        .is_some());
    }
}
