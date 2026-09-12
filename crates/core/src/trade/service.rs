//! The trade read-surface: trade headers, full detail, and a maker's settlement feed (with per-fill
//! share and captured fee), assembled by composing the trade store with the asset catalog and USD
//! valuation. Handlers pass these DTOs straight through.

use std::sync::Arc;

use alloy_primitives::{Address, U256};

use crate::asset::AssetManager;
use crate::deps::trade::{MakerFill, Page, TradeFilter, TradeStats, TradeStore};
use crate::primitives::amount::{format_units, share_pct, TokenAmount};
use crate::primitives::trade::{
    MakerLeg, MakerTrade, Trade, TradeAction, TradeId, TradeInfo, TradeView,
};
use crate::valuation::Valuation;
use crate::SolventError;

pub struct TradeService {
    registry: Arc<crate::registry::SharedSnapshot>,
    trades: Arc<dyn TradeStore>,
    assets: Arc<AssetManager>,
    valuation: Arc<Valuation>,
}

impl TradeService {
    pub fn new(
        trades: Arc<dyn TradeStore>,
        assets: Arc<AssetManager>,
        valuation: Arc<Valuation>,
        registry: Arc<crate::registry::SharedSnapshot>,
    ) -> Self {
        Self {
            registry,
            trades,
            assets,
            valuation,
        }
    }

    /// Trade headers (heavy detail fields omitted), filtered and paginated.
    pub async fn list(
        &self,
        filter: &TradeFilter,
        page: &Page,
    ) -> Result<Vec<TradeView>, SolventError> {
        let rows = self.trades.list(filter, page).await?;
        let mut items = Vec::with_capacity(rows.len());
        for trade in &rows {
            items.push(self.summary(trade).await);
        }
        Ok(items)
    }

    /// One trade's full detail, or `None` if the id is unknown.
    pub async fn detail(&self, id: &TradeId) -> Result<Option<TradeView>, SolventError> {
        match self.trades.info(id).await? {
            Some(info) => Ok(Some(self.detail_view(&info).await)),
            None => Ok(None),
        }
    }

    /// A maker's settlements: its trades with per-trade share and captured fee.
    pub async fn maker_trades(
        &self,
        maker: Address,
        page: &Page,
        window: Option<std::ops::Range<u64>>,
    ) -> Result<Vec<MakerTrade>, SolventError> {
        let fills = self.trades.list_for_maker(maker, page, window).await?;
        let mut items = Vec::with_capacity(fills.len());
        for fill in &fills {
            let dec_in = self.assets.decimals(&fill.trade.token_in);
            let dec_out = self.assets.decimals(&fill.trade.token_out);
            items.push(MakerTrade {
                share_pct: maker_share_pct(fill),
                fee_usd: maker_fee_usd(fill, dec_in, dec_out),
                trade: self.summary(&fill.trade).await,
            });
        }
        Ok(items)
    }

    /// Aggregate settlement stats.
    pub async fn stats(&self) -> Result<TradeStats, SolventError> {
        Ok(self.trades.stats().await?)
    }

    /// A trade's header — the detail-only fields (`lifecycle`/`legs`/order coordinates) left empty.
    async fn summary(&self, trade: &Trade) -> TradeView {
        let token_in = self.assets.token_or_default(trade.token_in);
        let token_out = self.assets.token_or_default(trade.token_out);
        let delivered = trade.amount_out.unwrap_or(trade.min_amount_out);
        let input = self
            .valuation
            .amount(trade.amount_in, token_in.address, token_in.decimals)
            .await;
        let output = self
            .valuation
            .amount(delivered, token_out.address, token_out.decimals)
            .await;
        let surplus = match trade.surplus {
            Some(s) => Some(
                self.valuation
                    .amount(s, token_in.address, token_in.decimals)
                    .await,
            ),
            None => None,
        };
        let indicative_input = match trade.indicative_amount_in {
            Some(cost) => Some(
                self.valuation
                    .amount(cost, token_in.address, token_in.decimals)
                    .await,
            ),
            None => None,
        };
        TradeView {
            signature_present: None,
            id: trade.id.to_string(),
            status: trade.status.as_str().to_string(),
            taker: trade.taker,
            input: TokenAmount {
                amount: input,
                token: token_in,
            },
            output: TokenAmount {
                amount: output,
                token: token_out,
            },
            surplus,
            indicative_input,
            source: trade.source.as_str().to_string(),
            price_impact_pct: trade.price_impact_pct,
            tx_hash: trade.tx_hash.map(|h| h.to_string()),
            block_number: trade.block_number,
            created_at: trade.created_at,
            settled_at: trade.settled_at,
            lifecycle: None,
            legs: None,
            order_hash: None,
            deadline_block: None,
        }
    }

    /// The full detail — the header plus the stage timeline, maker legs, and order coordinates.
    async fn detail_view(&self, info: &TradeInfo) -> TradeView {
        let token_in = self.assets.token_or_default(info.trade.token_in);
        let token_out = self.assets.token_or_default(info.trade.token_out);
        let mut legs = Vec::with_capacity(info.legs.len());
        for leg in &info.legs {
            let curve = self
                .registry
                .load()
                .strategy_by_hash(leg.strategy_hash)
                .and_then(|s| match &s.curve {
                    crate::primitives::registry::CurveSpec::Priceable { curve, .. } => {
                        Some(crate::primitives::registry::curve_label(curve).to_string())
                    }
                    _ => None,
                });
            legs.push(MakerLeg {
                curve,
                maker: leg.maker.to_string(),
                strategy_hash: leg.strategy_hash.to_string(),
                amount_in: self
                    .valuation
                    .amount(leg.amount_in, token_in.address, token_in.decimals)
                    .await,
                amount_out: self
                    .valuation
                    .amount(leg.amount_out, token_out.address, token_out.decimals)
                    .await,
            });
        }
        TradeView {
            signature_present: Some(info.trade.signature.is_some()),
            lifecycle: Some(
                info.attempts
                    .iter()
                    .map(|a| TradeAction {
                        status: a.status.as_str().to_string(),
                        at: a.at,
                    })
                    .collect(),
            ),
            legs: Some(legs),
            order_hash: Some(info.trade.order_hash.to_string()),
            deadline_block: Some(info.trade.deadline_block),
            ..self.summary(&info.trade).await
        }
    }
}

/// The maker's slice of the trade — its delivered amount over the trade's total. Settled trades only
/// (an unsettled trade has no total yet).
fn maker_share_pct(fill: &MakerFill) -> Option<f64> {
    Some(share_pct(fill.amount_out, fill.trade.amount_out?))
}

/// The maker's captured fee: value received minus value delivered, at the trade-time prices stored
/// on the trade. Settled + priced trades only.
fn maker_fee_usd(fill: &MakerFill, dec_in: u8, dec_out: u8) -> Option<f64> {
    fill.trade.amount_out?;
    let price_in = fill.trade.token_in_price_usd?;
    let price_out = fill.trade.token_out_price_usd?;
    let received_usd = human(fill.amount_in, dec_in) * price_in;
    let delivered_usd = human(fill.amount_out, dec_out) * price_out;
    Some(received_usd - delivered_usd)
}

/// Base units as a human decimal (for USD math), `0.0` if the decimals are nonsensical.
fn human(amount: U256, decimals: u8) -> f64 {
    format_units(amount, decimals).parse().unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::ingest::OrderSource;
    use alloy_primitives::B256;
    use async_trait::async_trait;
    use rust_decimal::Decimal;
    use ulid::Ulid;

    use crate::deps::routing::{PriceOracle, PriceOracleError};
    use crate::deps::trade::{CreateResult, Settlement, TradeStoreError};
    use crate::primitives::asset::{TokenList, TokenMeta};
    use crate::primitives::trade::{TradeAttempt, TradeLeg, TradeStatus};
    use crate::primitives::{IntentId, MakerId, StrategyHash, UsdPrice};
    use crate::registry::SharedSnapshot;

    struct Prices;
    #[async_trait]
    impl PriceOracle for Prices {
        async fn price(&self, token: Address) -> Result<UsdPrice, PriceOracleError> {
            match token {
                t if t == Address::from([1; 20]) => Ok(UsdPrice(Decimal::from(2_000))),
                t if t == Address::from([2; 20]) => Ok(UsdPrice(Decimal::ONE)),
                _ => Err(PriceOracleError::NotFound(token)),
            }
        }
    }

    /// Returns canned reads; the writes are unused by the read-surface.
    struct FakeStore {
        info: Option<TradeInfo>,
        maker_fill: Option<MakerFill>,
    }
    #[async_trait]
    impl TradeStore for FakeStore {
        async fn create(
            &self,
            _: &Trade,
            _: &[TradeLeg],
            _: &[TradeAttempt],
        ) -> Result<CreateResult, TradeStoreError> {
            unreachable!("read-surface never creates")
        }
        async fn advance(
            &self,
            _: &TradeId,
            _: TradeStatus,
            _: u64,
        ) -> Result<(), TradeStoreError> {
            Ok(())
        }
        async fn settle(&self, _: &TradeId, _: &Settlement) -> Result<(), TradeStoreError> {
            Ok(())
        }
        async fn info(&self, _: &TradeId) -> Result<Option<TradeInfo>, TradeStoreError> {
            Ok(self.info.clone())
        }
        async fn find_by_order(&self, _: &IntentId) -> Result<Option<Trade>, TradeStoreError> {
            Ok(None)
        }
        async fn list(&self, _: &TradeFilter, _: &Page) -> Result<Vec<Trade>, TradeStoreError> {
            Ok(self.info.iter().map(|i| i.trade.clone()).collect())
        }
        async fn list_for_maker(
            &self,
            _: Address,
            _: &Page,
            _: Option<std::ops::Range<u64>>,
        ) -> Result<Vec<MakerFill>, TradeStoreError> {
            Ok(self
                .maker_fill
                .iter()
                .map(|fill| MakerFill::new(fill.trade.clone(), fill.amount_in, fill.amount_out))
                .collect())
        }
        async fn stats(&self) -> Result<TradeStats, TradeStoreError> {
            Ok(TradeStats {
                settled: 0,
                confirmed: 0,
                failed: 0,
                median_impact_pct: None,
            })
        }
    }

    fn service(info: Option<TradeInfo>) -> TradeService {
        service_with_store(FakeStore {
            info,
            maker_fill: None,
        })
    }

    fn service_with_store(store: FakeStore) -> TradeService {
        let list = TokenList {
            name: "t".into(),
            tokens: vec![
                TokenMeta {
                    chain_id: 31337,
                    address: Address::from([1; 20]),
                    symbol: "WETH".into(),
                    name: "Wrapped Ether".into(),
                    decimals: 18,
                    logo_uri: None,
                    tags: vec![],
                },
                TokenMeta {
                    chain_id: 31337,
                    address: Address::from([2; 20]),
                    symbol: "USDC".into(),
                    name: "USD Coin".into(),
                    decimals: 6,
                    logo_uri: None,
                    tags: vec![],
                },
            ],
        };
        let assets = Arc::new(AssetManager::new(list, Arc::new(SharedSnapshot::default())));
        let valuation = Arc::new(Valuation::new(Arc::new(Prices)));
        TradeService::new(
            Arc::new(store),
            assets,
            valuation,
            Arc::new(SharedSnapshot::default()),
        )
    }

    fn a_trade() -> Trade {
        Trade {
            indicative_amount_in: None,
            source: OrderSource::UniswapX,
            id: TradeId(Ulid::from_parts(1, 2)),
            order_hash: IntentId(B256::from([7; 32])),
            taker: Address::from([9; 20]),
            token_in: Address::from([1; 20]),
            token_out: Address::from([2; 20]),
            amount_in: U256::from(1_000u64),
            min_amount_out: U256::from(500u64),
            amount_out: None,
            status: TradeStatus::Reserved,
            deadline_block: 123,
            signature: None,
            price_impact_pct: None,
            surplus: Some(U256::from(3u64)),
            tx_hash: None,
            block_number: None,
            created_at: 100,
            settled_at: None,
            token_in_price_usd: None,
            token_out_price_usd: None,
        }
    }

    fn leg(amount_in: u128, amount_out: u128) -> TradeLeg {
        TradeLeg {
            maker: MakerId(Address::from([3; 20])),
            strategy_hash: StrategyHash(B256::from([3; 32])),
            amount_in: U256::from(amount_in),
            amount_out: U256::from(amount_out),
        }
    }

    fn settled(amount_out: Option<u128>, price_in: Option<f64>, price_out: Option<f64>) -> Trade {
        Trade {
            token_in: Address::from([2; 20]),
            token_out: Address::from([3; 20]),
            amount_in: U256::from(2_010_000_000u64),
            min_amount_out: U256::ZERO,
            amount_out: amount_out.map(U256::from),
            status: TradeStatus::Confirmed,
            settled_at: Some(1),
            token_in_price_usd: price_in,
            token_out_price_usd: price_out,
            ..a_trade()
        }
    }

    #[tokio::test]
    async fn surplus_uses_the_input_tokens_decimals_and_price() {
        let svc = service(None);
        // Exact-out profit is retained input: 1.25 WETH at $2,000, or 1.25 USDC at $1.
        for (input, output, raw, usd) in [
            (1, 2, 1_250_000_000_000_000_000u64, 2_500.0),
            (2, 1, 1_250_000u64, 1.25),
        ] {
            let trade = Trade {
                token_in: Address::from([input; 20]),
                token_out: Address::from([output; 20]),
                surplus: Some(U256::from(raw)),
                ..a_trade()
            };
            let surplus = svc.summary(&trade).await.surplus.unwrap();
            assert_eq!(surplus.raw, raw.to_string());
            assert_eq!(surplus.display, "1.25");
            assert_eq!(surplus.usd, Some(usd));
        }
    }

    #[tokio::test]
    async fn list_omits_the_heavy_detail_fields() {
        let svc = service(Some(TradeInfo {
            trade: a_trade(),
            attempts: Vec::new(),
            legs: Vec::new(),
        }));
        let filter = TradeFilter::default();
        let page = Page {
            limit: 50,
            cursor: None,
        };
        let items = svc.list(&filter, &page).await.unwrap();
        assert_eq!(items.len(), 1);
        let dto = &items[0];
        assert!(dto.lifecycle.is_none());
        assert!(dto.legs.is_none());
        assert!(dto.order_hash.is_none());
        // A catalog token renders its decimals; the unfilled output shows the signed floor.
        assert_eq!(dto.input.token.symbol, "WETH");
        assert_eq!(dto.output.amount.raw, "500");
    }

    #[tokio::test]
    async fn detail_includes_lifecycle_legs_and_order() {
        let info = TradeInfo {
            trade: a_trade(),
            attempts: vec![TradeAttempt {
                status: TradeStatus::Reserved,
                at: 100,
            }],
            legs: vec![leg(1_000, 500)],
        };
        let id = info.trade.id;
        let want_order = info.trade.order_hash.to_string();
        let dto = service(Some(info)).detail(&id).await.unwrap().unwrap();
        assert_eq!(dto.lifecycle.as_deref().unwrap().len(), 1);
        assert_eq!(dto.legs.as_deref().unwrap().len(), 1);
        assert_eq!(dto.order_hash.unwrap(), want_order);
        assert_eq!(dto.deadline_block, Some(123));
    }

    #[tokio::test]
    async fn maker_feed_values_combined_amounts_at_trade_time_prices() {
        // Two strategies together received 3010 USDC and delivered 2 WETH of a 4-WETH trade.
        // The stored WETH price ($1500) differs from today's oracle price ($2000).
        let trade = Trade {
            token_out: Address::from([1; 20]),
            ..settled(Some(4_000_000_000_000_000_000), Some(1.0), Some(1500.0))
        };
        let svc = service_with_store(FakeStore {
            info: None,
            maker_fill: Some(MakerFill::new(
                trade,
                U256::from(3_010_000_000u64),
                U256::from(2_000_000_000_000_000_000u64),
            )),
        });
        let fills = svc
            .maker_trades(
                Address::from([3; 20]),
                &Page {
                    limit: 50,
                    cursor: None,
                },
                None,
            )
            .await
            .unwrap();
        assert_eq!(fills.len(), 1);
        assert_eq!(fills[0].fee_usd, Some(10.0));
        assert_eq!(fills[0].share_pct, Some(50.0));
    }

    #[test]
    fn unsettled_or_unpriced_has_no_fee() {
        let leg = leg(2_010_000_000, 1_000_000_000_000_000_000);
        // Unsettled (no delivered total): no share, no fee.
        let pending = MakerFill::new(
            settled(None, Some(1.0), Some(2000.0)),
            leg.amount_in,
            leg.amount_out,
        );
        assert_eq!(maker_fee_usd(&pending, 6, 18), None);
        assert_eq!(maker_share_pct(&pending), None);
        // Settled but unpriced: share available, fee not.
        let unpriced = MakerFill::new(
            settled(Some(2_000_000_000_000_000_000), None, None),
            leg.amount_in,
            leg.amount_out,
        );
        assert_eq!(maker_fee_usd(&unpriced, 6, 18), None);
        assert_eq!(maker_share_pct(&unpriced), Some(50.0));
    }
}
