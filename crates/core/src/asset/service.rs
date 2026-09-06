//! The asset manager: the single authority that answers everything about an asset by composing the
//! static token list with live protocol state (and, later, market data). Handlers go through this,
//! never raw metadata maps.

use std::collections::BTreeMap;
use std::sync::Arc;

use alloy_primitives::Address;

use crate::primitives::asset::{Asset, Token, TokenList, TokenMeta};
use crate::primitives::registry::{ActiveAsset, TokenPair};
use crate::registry::SharedSnapshot;

/// Answers asset queries from one place: the static token list joined with the live registry
/// snapshot (supported / count / pairs). Market fields stay `None` until the price feed is wired.
pub struct AssetManager {
    catalog: BTreeMap<Address, TokenMeta>,
    registry: Arc<SharedSnapshot>,
}

impl AssetManager {
    /// Index the list by address and hold the registry handle.
    pub fn new(list: TokenList, registry: Arc<SharedSnapshot>) -> Self {
        let catalog = list.tokens.into_iter().map(|t| (t.address, t)).collect();
        Self { catalog, registry }
    }

    /// Every catalog asset, or only those with active liquidity when `supported_only`.
    pub fn list(&self, supported_only: bool) -> Vec<Asset> {
        let snapshot = self.registry.load();
        let active = snapshot.active_assets();
        self.catalog
            .values()
            .map(|meta| self.assemble(meta, active.get(&meta.address)))
            .filter(|asset| !supported_only || asset.supported)
            .collect()
    }

    /// Compose one `Asset` from its metadata and (optional) live activity.
    fn assemble(&self, meta: &TokenMeta, active: Option<&ActiveAsset>) -> Asset {
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
            price_usd: None,
            change_24h_pct: None,
            supported: active.is_some(),
            active_strategy_count: active.map_or(0, |a| a.strategy_count as u64),
            pairs,
        }
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

    /// A token's decimals, defaulting to 18 for one not in the catalog.
    pub fn decimals(&self, address: &Address) -> u8 {
        self.catalog.get(address).map_or(18, |meta| meta.decimals)
    }

    /// The catalog token for `address`, or a bare 18-decimal fallback for one not listed.
    pub fn token_or_default(&self, address: Address) -> Token {
        self.token(&address).unwrap_or(Token {
            address,
            chain_id: 0,
            symbol: String::new(),
            decimals: 18,
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

    /// Whether `address` is tagged as a stablecoin in the catalog.
    pub fn is_stable(&self, address: &Address) -> bool {
        self.catalog
            .get(address)
            .is_some_and(|meta| meta.tags.iter().any(|tag| tag == "stables"))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::registry::{MakerStrategy, Snapshot, StrategyKey};
    use crate::primitives::{MakerId, StrategyHash};
    use alloy_primitives::{B256, U256};

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

    #[test]
    fn list_marks_supported_and_labels_pairs() {
        // WETH & USDC are quoted by one active strategy; DAI is listed but has no liquidity.
        let mgr = manager(
            vec![meta(1, "WETH"), meta(2, "USDC"), meta(3, "DAI")],
            vec![strategy(0, token(1), token(2))],
        );

        let all = mgr.list(false);
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

    #[test]
    fn supported_only_filters_out_idle_assets() {
        let mgr = manager(
            vec![meta(1, "WETH"), meta(2, "USDC"), meta(3, "DAI")],
            vec![strategy(0, token(1), token(2))],
        );
        let supported = mgr.list(true);
        assert_eq!(supported.len(), 2);
        assert!(supported.iter().all(|a| a.supported));
    }
}
