//! The asset manager: the single authority that answers everything about an asset by composing the
//! static token list with live protocol state and market data. Handlers go through this, never raw
//! metadata maps.

use std::collections::BTreeMap;
use std::sync::Arc;

use alloy_primitives::Address;
use itertools::Itertools;

use crate::primitives::amount::TokenBalance;
use crate::primitives::asset::{
    Asset, PairInfo, PairKind, PairWallet, Token, TokenList, TokenMeta,
};
use crate::primitives::registry::{ActiveAsset, TokenPair};
use crate::primitives::UsdPrice;
use crate::registry::SharedSnapshot;
use crate::valuation::Valuation;

/// Decimals assumed for a token that isn't in the catalog.
const DEFAULT_DECIMALS: u8 = 18;

/// Answers asset queries from one place: the static token list joined with the live registry
/// snapshot (supported / count / pairs), with market fields valued on `list`.
pub struct AssetManager {
    catalog: BTreeMap<Address, TokenMeta>,
    registry: Arc<SharedSnapshot>,
}

/// Catalog tag that marks a token as holding a dollar peg.
const STABLE_TAG: &str = "stables";

impl AssetManager {
    /// Index the list by address and hold the registry handle.
    pub fn new(list: TokenList, registry: Arc<SharedSnapshot>) -> Self {
        let catalog = list.tokens.into_iter().map(|t| (t.address, t)).collect();
        Self { catalog, registry }
    }

    /// Every catalog asset with its market fields valued, or only those with active liquidity when
    /// `supported_only`.
    pub async fn list(&self, supported_only: bool, valuation: &Valuation) -> Vec<Asset> {
        let snapshot = self.registry.load();
        let active = snapshot.active_assets();
        let mut assets = Vec::with_capacity(self.catalog.len());
        for meta in self.catalog.values() {
            let asset = self
                .assemble(meta, active.get(&meta.address), valuation)
                .await;
            if !supported_only || asset.supported {
                assets.push(asset);
            }
        }
        assets
    }

    /// Compose one `Asset` from its metadata, (optional) live activity, and market data.
    async fn assemble(
        &self,
        meta: &TokenMeta,
        active: Option<&ActiveAsset>,
        valuation: &Valuation,
    ) -> Asset {
        let pairs = active
            .map(|a| a.pairs.iter().map(|p| self.pair_label(p)).collect())
            .unwrap_or_default();
        Asset {
            address: meta.address,
            chain_id: meta.chain_id,
            symbol: meta.symbol.clone(),
            name: meta.name.clone(),
            decimals: meta.decimals,
            tags: meta.tags.clone(),
            logo_uri: meta.logo_uri.clone(),
            price_usd: valuation.price(meta.address).await.map(UsdPrice::to_f64),
            change_24h_pct: valuation.change_24h(meta.address).await,
            supported: active.is_some(),
            active_strategy_count: active.map_or(0, |a| a.strategy_count as u64),
            pairs,
        }
    }

    /// The tradeable pairs for the Create wizard: every catalog asset quoted against a stablecoin,
    /// plus stable/stable, each with mid price and the suggested defaults. `search` filters by
    /// symbol or address on either side; `wallet` (when given) adds the maker's per-side balance.
    pub async fn pairs(
        &self,
        valuation: &Valuation,
        wallet: Option<&[TokenBalance]>,
        search: Option<&str>,
        default_fee_bps: u32,
        default_band_pct: f64,
    ) -> Vec<PairInfo> {
        let tokens = self.catalog_tokens();
        let mut out = Vec::new();
        for (a, b) in tokens
            .iter()
            .cloned()
            .tuple_combinations::<(Token, Token)>()
        {
            let (a_stable, b_stable) = (self.is_stable(&a.address), self.is_stable(&b.address));
            // A pair is offered only when at least one side is a stablecoin (preset + stable).
            if !a_stable && !b_stable {
                continue;
            }
            // Reuse the one base/quote ordering rule (stablecoin is the quote).
            let (base_addr, quote_addr) = self.base_quote(&TokenPair::new(a.address, b.address));
            let (Some(base), Some(quote)) = (self.token(&base_addr), self.token(&quote_addr))
            else {
                continue;
            };
            if search.is_some_and(|q| !matches_search(&base, &quote, q)) {
                continue;
            }
            out.push(PairInfo {
                kind: if a_stable && b_stable {
                    PairKind::Stable
                } else {
                    PairKind::Volatile
                },
                mid: mid_price(valuation, base.address, quote.address).await,
                default_fee_bps,
                default_band_pct,
                wallet: wallet.map(|bals| PairWallet {
                    base: wallet_balance(bals, &base.address),
                    quote: wallet_balance(bals, &quote.address),
                }),
                base,
                quote,
            });
        }
        out
    }

    /// A human pair label as `base/quote` (quote = the stablecoin side when one token is a stable),
    /// falling back to a short address for tokens not in the catalog.
    pub fn pair_label(&self, pair: &TokenPair) -> String {
        let (base, quote) = self.base_quote(pair);
        format!("{}/{}", self.symbol(&base), self.symbol(&quote))
    }

    /// Order a pair as `(base, quote)`: the stablecoin is the quote when exactly one side is a
    /// stable, else canonical address order.
    pub fn base_quote(&self, pair: &TokenPair) -> (Address, Address) {
        match (self.is_stable(&pair.lo), self.is_stable(&pair.hi)) {
            (true, false) => (pair.hi, pair.lo),
            _ => (pair.lo, pair.hi),
        }
    }

    /// A token's identity + display essentials, if it is in the catalog.
    pub fn token(&self, address: &Address) -> Option<Token> {
        self.catalog.get(address).map(Self::token_of)
    }

    /// A token's decimals, defaulting to [`DEFAULT_DECIMALS`] for one not in the catalog.
    pub fn decimals(&self, address: &Address) -> u8 {
        self.catalog
            .get(address)
            .map_or(DEFAULT_DECIMALS, |meta| meta.decimals)
    }

    /// The catalog token for `address`, or a bare fallback for one not listed.
    pub fn token_or_default(&self, address: Address) -> Token {
        self.token(&address).unwrap_or(Token {
            address,
            chain_id: 0,
            symbol: String::new(),
            decimals: DEFAULT_DECIMALS,
        })
    }

    /// Every catalog token as its lightweight identity — no snapshot load, no `Asset` assembly. For
    /// callers that need the token set but not the full market picture (e.g. wallet balances).
    pub fn catalog_tokens(&self) -> Vec<Token> {
        self.catalog.values().map(Self::token_of).collect()
    }

    fn token_of(meta: &TokenMeta) -> Token {
        Token {
            address: meta.address,
            chain_id: meta.chain_id,
            symbol: meta.symbol.clone(),
            decimals: meta.decimals,
        }
    }

    /// Whether `address` is tagged as a stablecoin in the catalog. Tags are read by people as
    /// well as matched on, so how the list cased one must not change what it classifies.
    pub fn is_stable(&self, address: &Address) -> bool {
        self.catalog.get(address).is_some_and(|meta| {
            meta.tags
                .iter()
                .any(|tag| tag.eq_ignore_ascii_case(STABLE_TAG))
        })
    }

    fn symbol(&self, address: &Address) -> String {
        self.catalog
            .get(address)
            .map(|meta| meta.symbol.clone())
            .unwrap_or_else(|| short(address))
    }
}

/// `0x1234…abcd` for a token that isn't in the catalog.
fn short(address: &Address) -> String {
    let s = address.to_string();
    format!("{}…{}", &s[..6], &s[s.len() - 4..])
}

/// Mid price as quote per 1 base, from the two USD prices; `None` if either is missing.
async fn mid_price(valuation: &Valuation, base: Address, quote: Address) -> Option<f64> {
    let base_price_usd = valuation.price(base).await?.to_f64();
    let quote_price_usd = valuation.price(quote).await?.to_f64();
    (quote_price_usd != 0.0).then_some(base_price_usd / quote_price_usd)
}

/// The maker's whole-token balance of `addr`, or `0` when the wallet doesn't hold it.
fn wallet_balance(balances: &[TokenBalance], addr: &Address) -> f64 {
    balances
        .iter()
        .find(|b| b.token.address == *addr)
        .and_then(|b| b.balance.display.parse().ok())
        .unwrap_or(0.0)
}

/// Whether `search` (case-insensitive) matches either token's symbol or address.
fn matches_search(base: &Token, quote: &Token, search: &str) -> bool {
    let needle = search.to_lowercase();
    let hit = |t: &Token| {
        t.symbol.to_lowercase().contains(&needle)
            || t.address.to_string().to_lowercase().contains(&needle)
    };
    hit(base) || hit(quote)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deps::routing::{PriceOracle, PriceOracleError};
    use crate::primitives::amount::Amount;
    use crate::primitives::registry::{MakerStrategy, Snapshot, StrategyKey};
    use crate::primitives::{MakerId, StrategyHash};
    use alloy_primitives::{B256, U256};
    use async_trait::async_trait;
    use rust_decimal::Decimal;
    use std::collections::HashMap;

    #[derive(Default)]
    struct FakeMarket {
        prices: HashMap<Address, UsdPrice>,
        changes: HashMap<Address, f64>,
    }
    #[async_trait]
    impl PriceOracle for FakeMarket {
        async fn price(&self, token: Address) -> Result<UsdPrice, PriceOracleError> {
            self.prices
                .get(&token)
                .copied()
                .ok_or(PriceOracleError::NotFound(token))
        }
        async fn change_24h(&self, token: Address) -> Option<f64> {
            self.changes.get(&token).copied()
        }
    }
    fn valuation(market: FakeMarket) -> Valuation {
        Valuation::new(Arc::new(market))
    }

    fn token(n: u8) -> Address {
        Address::from([n; 20])
    }

    fn meta(n: u8, symbol: &str) -> TokenMeta {
        TokenMeta {
            chain_id: 31337,
            address: token(n),
            symbol: symbol.to_string(),
            name: symbol.to_string(),
            decimals: 18,
            logo_uri: None,
            tags: vec![],
        }
    }

    fn stable_meta(n: u8, symbol: &str) -> TokenMeta {
        TokenMeta {
            tags: vec!["stables".to_string()],
            ..meta(n, symbol)
        }
    }

    fn balance(mgr: &AssetManager, n: u8, display: &str) -> TokenBalance {
        let amount = Amount {
            raw: "0".to_string(),
            display: display.to_string(),
            usd: None,
        };
        TokenBalance {
            token: mgr.token_or_default(token(n)),
            balance: amount.clone(),
            pullable: amount,
        }
    }

    /// An active strategy quoting the pair `(a, b)`.
    fn strategy(s: u8, a: Address, b: Address) -> MakerStrategy {
        let key = StrategyKey {
            maker: MakerId(Address::from([s; 20])),
            app: Address::from([0xAA; 20]),
            strategy_hash: StrategyHash(B256::from([s; 32])),
        };
        let mut st = MakerStrategy::new(key, &[s]);
        st.balances.insert(a, U256::from(1u8));
        st.balances.insert(b, U256::from(1u8));
        st
    }

    fn manager(tokens: Vec<TokenMeta>, strategies: Vec<MakerStrategy>) -> AssetManager {
        let snapshot = Snapshot::from_strategies(strategies);
        let registry = Arc::new(SharedSnapshot::new(snapshot));
        AssetManager::new(
            TokenList {
                name: "test".into(),
                tokens,
            },
            registry,
        )
    }

    #[tokio::test]
    async fn list_marks_supported_and_labels_pairs() {
        // WETH & USDC are quoted by one active strategy; DAI is listed but has no liquidity.
        let mgr = manager(
            vec![meta(1, "WETH"), meta(2, "USDC"), meta(3, "DAI")],
            vec![strategy(0, token(1), token(2))],
        );

        let all = mgr.list(false, &valuation(FakeMarket::default())).await;
        assert_eq!(all.len(), 3);

        let weth = all.iter().find(|a| a.symbol == "WETH").unwrap();
        assert!(weth.supported);
        assert_eq!(weth.active_strategy_count, 1);
        assert_eq!(weth.pairs, vec!["WETH/USDC".to_string()]);

        let dai = all.iter().find(|a| a.symbol == "DAI").unwrap();
        assert!(!dai.supported);
        assert_eq!(dai.active_strategy_count, 0);
        assert!(dai.pairs.is_empty());
    }

    #[tokio::test]
    async fn supported_only_filters_out_idle_assets() {
        let mgr = manager(
            vec![meta(1, "WETH"), meta(2, "USDC"), meta(3, "DAI")],
            vec![strategy(0, token(1), token(2))],
        );
        let supported = mgr.list(true, &valuation(FakeMarket::default())).await;
        assert_eq!(supported.len(), 2);
        assert!(supported.iter().all(|a| a.supported));
    }

    #[tokio::test]
    async fn list_values_price_and_change() {
        let mgr = manager(vec![meta(1, "WETH"), meta(2, "USDC")], vec![]);
        let market = FakeMarket {
            prices: HashMap::from([(token(1), UsdPrice(Decimal::from(2000)))]),
            changes: HashMap::from([(token(1), 1.5)]),
        };
        let all = mgr.list(false, &valuation(market)).await;

        let weth = all.iter().find(|a| a.symbol == "WETH").unwrap();
        assert_eq!(weth.price_usd, Some(2000.0));
        assert_eq!(weth.change_24h_pct, Some(1.5));
        let usdc = all.iter().find(|a| a.symbol == "USDC").unwrap();
        assert_eq!(usdc.price_usd, None); // unpriced → no value
        assert_eq!(usdc.change_24h_pct, None);
    }

    #[tokio::test]
    async fn pairs_lists_asset_vs_stable_and_stable_vs_stable() {
        // WETH is volatile; USDC & DAI are stable → WETH/USDC, WETH/DAI (volatile), USDC/DAI (stable).
        let mgr = manager(
            vec![
                meta(1, "WETH"),
                stable_meta(2, "USDC"),
                stable_meta(3, "DAI"),
            ],
            vec![],
        );
        let market = FakeMarket {
            prices: HashMap::from([
                (token(1), UsdPrice(Decimal::from(2000))),
                (token(2), UsdPrice(Decimal::from(1))),
                (token(3), UsdPrice(Decimal::from(1))),
            ]),
            changes: HashMap::new(),
        };
        let pairs = mgr.pairs(&valuation(market), None, None, 30, 5.0).await;
        assert_eq!(pairs.len(), 3);

        let weth_usdc = pairs
            .iter()
            .find(|p| p.base.symbol == "WETH" && p.quote.symbol == "USDC")
            .unwrap();
        assert!(matches!(weth_usdc.kind, PairKind::Volatile));
        assert_eq!(weth_usdc.mid, Some(2000.0));
        assert_eq!(weth_usdc.default_fee_bps, 30);
        assert_eq!(weth_usdc.default_band_pct, 5.0);
        assert!(weth_usdc.wallet.is_none());

        let usdc_dai = pairs
            .iter()
            .find(|p| p.base.symbol == "USDC" && p.quote.symbol == "DAI")
            .unwrap();
        assert!(matches!(usdc_dai.kind, PairKind::Stable));
        assert_eq!(usdc_dai.mid, Some(1.0));
    }

    #[tokio::test]
    async fn pairs_search_filters_by_symbol() {
        let mgr = manager(
            vec![
                meta(1, "WETH"),
                stable_meta(2, "USDC"),
                stable_meta(3, "DAI"),
            ],
            vec![],
        );
        let pairs = mgr
            .pairs(
                &valuation(FakeMarket::default()),
                None,
                Some("weth"),
                30,
                5.0,
            )
            .await;
        assert_eq!(pairs.len(), 2);
        assert!(pairs.iter().all(|p| p.base.symbol == "WETH"));
    }

    #[tokio::test]
    async fn pairs_include_wallet_balances_when_supplied() {
        let mgr = manager(vec![meta(1, "WETH"), stable_meta(2, "USDC")], vec![]);
        let wallet = vec![balance(&mgr, 1, "1.5"), balance(&mgr, 2, "500")];
        let pairs = mgr
            .pairs(
                &valuation(FakeMarket::default()),
                Some(&wallet),
                None,
                30,
                5.0,
            )
            .await;

        let weth_usdc = pairs
            .iter()
            .find(|p| p.base.symbol == "WETH" && p.quote.symbol == "USDC")
            .unwrap();
        let w = weth_usdc.wallet.as_ref().unwrap();
        assert_eq!(w.base, 1.5); // WETH
        assert_eq!(w.quote, 500.0); // USDC
    }
}
