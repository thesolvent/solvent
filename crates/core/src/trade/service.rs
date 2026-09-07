//! The trade read-surface: trade headers, full detail, and a maker's settlement feed (with per-fill
//! share and captured fee), assembled by composing the trade store with the asset catalog and USD
//! valuation. Handlers pass these DTOs straight through.

use std::sync::Arc;

use alloy_primitives::{Address, U256};

use crate::asset::AssetManager;
use crate::deps::trade::{Page, TradeFilter, TradeStats, TradeStore};
use crate::primitives::amount::{format_units, share_pct, TokenAmount};
use crate::primitives::trade::{
    MakerLeg, MakerTrade, Trade, TradeAction, TradeId, TradeInfo, TradeLeg, TradeView,
};
use crate::valuation::Valuation;
use crate::SolventError;

pub struct TradeService {
    trades: Arc<dyn TradeStore>,
    assets: Arc<AssetManager>,
    valuation: Arc<Valuation>,
}

impl TradeService {
    pub fn new(
        trades: Arc<dyn TradeStore>,
        assets: Arc<AssetManager>,
        valuation: Arc<Valuation>,
    ) -> Self {
        Self {
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
    ) -> Result<Vec<MakerTrade>, SolventError> {
        let fills = self.trades.list_for_maker(maker, page).await?;
        let mut items = Vec::with_capacity(fills.len());
        for fill in &fills {
            let dec_in = self.assets.decimals(&fill.trade.token_in);
            let dec_out = self.assets.decimals(&fill.trade.token_out);
            items.push(MakerTrade {
                share_pct: maker_share_pct(&fill.trade, &fill.leg),
                fee_usd: maker_fee_usd(&fill.trade, &fill.leg, dec_in, dec_out),
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
                    .amount(s, token_out.address, token_out.decimals)
                    .await,
            ),
            None => None,
        };
        TradeView {
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
            legs.push(MakerLeg {
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
fn maker_share_pct(trade: &Trade, leg: &TradeLeg) -> Option<f64> {
    Some(share_pct(leg.amount_out, trade.amount_out?))
}

/// The maker's captured fee: value received minus value delivered, at the trade-time prices stored
/// on the trade. Settled + priced trades only.
fn maker_fee_usd(trade: &Trade, leg: &TradeLeg, dec_in: u8, dec_out: u8) -> Option<f64> {
    trade.amount_out?;
    let price_in = trade.token_in_price_usd?;
    let price_out = trade.token_out_price_usd?;
    let received_usd = human(leg.amount_in, dec_in) * price_in;
    let delivered_usd = human(leg.amount_out, dec_out) * price_out;
    Some(received_usd - delivered_usd)
}

/// Base units as a human decimal (for USD math), `0.0` if the decimals are nonsensical.
fn human(amount: U256, decimals: u8) -> f64 {
    format_units(amount, decimals).parse().unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::B256;
    use async_trait::async_trait;
    use ulid::Ulid;

    use crate::deps::routing::{PriceOracle, PriceOracleError};
    use crate::deps::trade::{CreateResult, MakerFill, Settlement, TradeStoreError};
    use crate::primitives::asset::{TokenList, TokenMeta};
    use crate::primitives::trade::{TradeAttempt, TradeStatus};
    use crate::primitives::{IntentId, MakerId, StrategyHash, UsdPrice};
    use crate::registry::SharedSnapshot;

    struct NoPrices;
    #[async_trait]
    impl PriceOracle for NoPrices {
        async fn price(&self, token: Address) -> Result<UsdPrice, PriceOracleError> {
            Err(PriceOracleError::NotFound(token))
        }
    }

    /// Returns canned reads; the writes are unused by the read-surface.
    struct FakeStore {
        info: Option<TradeInfo>,
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
        ) -> Result<Vec<MakerFill>, TradeStoreError> {
            Ok(Vec::new())
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
        let list = TokenList {
            name: "t".into(),
            tokens: vec![TokenMeta {
                chain_id: 31337,
                address: Address::from([1; 20]),
                symbol: "WETH".into(),
                name: "Wrapped Ether".into(),
                decimals: 18,
                logo_uri: None,
                tags: vec![],
            }],
        };
        let assets = Arc::new(AssetManager::new(list, Arc::new(SharedSnapshot::default())));
        let valuation = Arc::new(Valuation::new(Arc::new(NoPrices)));
        TradeService::new(Arc::new(FakeStore { info }), assets, valuation)
    }

    fn a_trade() -> Trade {
        Trade {
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
    async fn list_omits_the_heavy_detail_fields() {
        let svc = service(Some(TradeInfo {
            trade: a_trade(),
            attempts: Vec::new(),
            legs: Vec::new(),
        }));
        let filter = TradeFilter {
            status: None,
            taker: None,
            pair: None,
        };
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

    #[test]
    fn fee_and_share_use_trade_time_prices() {
        // Maker received 2010 USDC (6dp), delivered 1 WETH (18dp) of a 2-WETH trade.
        let leg = leg(2_010_000_000, 1_000_000_000_000_000_000);
        let t = settled(Some(2_000_000_000_000_000_000), Some(1.0), Some(2000.0));
        // fee = 2010·$1 − 1·$2000 = $10 at the stored prices.
        assert_eq!(maker_fee_usd(&t, &leg, 6, 18), Some(10.0));
        // share = 1 WETH of the 2-WETH trade = 50%.
        assert_eq!(maker_share_pct(&t, &leg), Some(50.0));
    }

    #[test]
    fn unsettled_or_unpriced_has_no_fee() {
        let leg = leg(2_010_000_000, 1_000_000_000_000_000_000);
        // Unsettled (no delivered total): no share, no fee.
        let pending = settled(None, Some(1.0), Some(2000.0));
        assert_eq!(maker_fee_usd(&pending, &leg, 6, 18), None);
        assert_eq!(maker_share_pct(&pending, &leg), None);
        // Settled but unpriced: share available, fee not.
        let unpriced = settled(Some(2_000_000_000_000_000_000), None, None);
        assert_eq!(maker_fee_usd(&unpriced, &leg, 6, 18), None);
        assert_eq!(maker_share_pct(&unpriced, &leg), Some(50.0));
    }
}
