//! Shared live-E2E harness: a real anvil with a deployed Aqua, AquaSwapVMRouter,
//! and two ERC20s, plus helpers to ship/push/swap and read on-chain state. Used
//! by the pricing-matrix test (`e2e_registry`) and the watcher-lifecycle test
//! (`e2e_watcher`). Everything here is test-only, so `expect`/`unwrap` are fine.

#![allow(dead_code)] // each test binary uses a different subset of the harness.

use std::sync::Arc;

use alloy::{
    network::EthereumWallet,
    node_bindings::{Anvil, AnvilInstance},
    primitives::{keccak256, Address, Bytes, U256},
    providers::{DynProvider, Provider, ProviderBuilder},
    signers::local::PrivateKeySigner,
    sol,
    sol_types::SolValue,
};
use serde::Deserialize;
use solvent_adapters::registry::{AlloyChainSource, SqliteStore};
use solvent_core::{
    deps::registry::ChainSource,
    primitives::{
        registry::{MakerStrategy, Snapshot, StrategyKey},
        ChainConfig, ChainId, MakerId, StrategyHash,
    },
    registry::{price, RegistrySync, SharedSnapshot},
};
use sqlx::SqlitePool;
use tempfile::TempDir;

sol!(
    #[sol(rpc)]
    Aqua,
    "tests/fixtures/artifacts/Aqua.json"
);
sol!(
    #[sol(rpc)]
    AquaSwapVMRouter,
    "tests/fixtures/artifacts/AquaSwapVMRouter.json"
);
sol!(
    #[sol(rpc)]
    MockERC20,
    "tests/fixtures/artifacts/MockERC20.json"
);

/// The logical chain id the registry keys by (distinct from anvil's own).
pub const CHAIN: ChainId = ChainId(31337);

pub fn e18(n: u64) -> U256 {
    U256::from(n) * U256::from(10u64).pow(U256::from(18u64))
}

pub fn strategy_key(maker: Address, app: Address, strategy: &Bytes) -> StrategyKey {
    StrategyKey {
        maker: MakerId(maker),
        app,
        strategy_hash: StrategyHash(keccak256(strategy)),
    }
}

// ---- fixture ----------------------------------------------------------------

#[derive(Deserialize)]
pub struct Fixture {
    pub maker: Address,
    #[serde(rename = "takerTraits")]
    pub taker_traits: TakerTraits,
    pub strategies: Vec<StrategySpec>,
    #[serde(rename = "secondMaker")]
    pub second_maker: SecondMaker,
}

/// A bare XYC owned by a second maker (anvil #1), same token pair — multi-maker test.
#[derive(Deserialize)]
pub struct SecondMaker {
    pub maker: Address,
    #[serde(rename = "strategyHex")]
    pub strategy_hex: Bytes,
    #[serde(rename = "shipLo")]
    pub ship_lo: U256,
    #[serde(rename = "shipHi")]
    pub ship_hi: U256,
}

#[derive(Deserialize)]
pub struct TakerTraits {
    #[serde(rename = "exactIn")]
    pub exact_in: Bytes,
    #[serde(rename = "exactOut")]
    pub exact_out: Bytes,
    pub swap: Bytes,
}

#[derive(Deserialize, Clone)]
pub struct StrategySpec {
    pub label: String,
    pub curve: String,
    pub supported: bool,
    pub guarded: bool,
    #[serde(rename = "strategyHex")]
    pub strategy_hex: Bytes,
    #[serde(rename = "feesInBps")]
    pub fees_in_bps: Vec<u64>,
    #[serde(rename = "shipLo")]
    pub ship_lo: U256,
    #[serde(rename = "shipHi")]
    pub ship_hi: U256,
}

impl StrategySpec {
    pub fn key(&self, maker: Address, app: Address) -> StrategyKey {
        strategy_key(maker, app, &self.strategy_hex)
    }
    pub fn order(&self) -> ISwapVM::Order {
        ISwapVM::Order::abi_decode(self.strategy_hex.as_ref()).expect("decode order")
    }
    pub fn hash(&self) -> alloy::primitives::FixedBytes<32> {
        keccak256(&self.strategy_hex)
    }
}

pub fn load_fixture() -> Fixture {
    serde_json::from_str(include_str!("../fixtures/e2e_strategies.json")).expect("fixture parses")
}

// ---- harness ----------------------------------------------------------------

/// A deployed stack on a live anvil: maker (account #0) + taker (account #1)
/// providers, the Aqua + router instances, and two canonically-ordered tokens.
pub struct Harness {
    _anvil: AnvilInstance,
    pub maker: Address,
    pub taker: Address,
    pub taker_signer: PrivateKeySigner,
    pub maker_provider: DynProvider,
    pub taker_provider: DynProvider,
    pub aqua: Aqua::AquaInstance<DynProvider>,
    pub router: AquaSwapVMRouter::AquaSwapVMRouterInstance<DynProvider>,
    pub app: Address,
    pub t0: Address,
    pub t1: Address,
    pub fx: Fixture,
}

impl Harness {
    pub async fn setup() -> Self {
        let anvil = Anvil::new()
            .arg("--disable-code-size-limit")
            .try_spawn()
            .expect("spawn anvil (is it on PATH?)");
        let fx = load_fixture();

        let maker_signer: PrivateKeySigner = anvil.keys()[0].clone().into();
        let taker_signer: PrivateKeySigner = anvil.keys()[1].clone().into();
        let maker = maker_signer.address();
        let taker = taker_signer.address();
        assert_eq!(maker, fx.maker, "harness maker must match the fixture's");

        let maker_provider = ProviderBuilder::new()
            .wallet(EthereumWallet::from(maker_signer))
            .connect_http(anvil.endpoint_url())
            .erased();
        let taker_provider = ProviderBuilder::new()
            .wallet(EthereumWallet::from(taker_signer.clone()))
            .connect_http(anvil.endpoint_url())
            .erased();

        let aqua = Aqua::deploy(maker_provider.clone())
            .await
            .expect("deploy Aqua");
        let router = AquaSwapVMRouter::deploy(
            maker_provider.clone(),
            *aqua.address(),
            Address::ZERO,
            maker,
            "SwapVM".to_string(),
            "1.0.0".to_string(),
        )
        .await
        .expect("deploy router");
        let app = *router.address();

        let ta = MockERC20::deploy(maker_provider.clone(), "Token A".into(), "TKA".into())
            .await
            .expect("deploy token A");
        let tb = MockERC20::deploy(maker_provider.clone(), "Token B".into(), "TKB".into())
            .await
            .expect("deploy token B");
        let (t0, t1) = if ta.address() < tb.address() {
            (*ta.address(), *tb.address())
        } else {
            (*tb.address(), *ta.address())
        };

        // Maker + taker both approve Aqua and the router to pull either token.
        for provider in [&maker_provider, &taker_provider] {
            for token in [t0, t1] {
                MockERC20::new(token, provider.clone())
                    .approve(*aqua.address(), U256::MAX)
                    .send()
                    .await
                    .expect("approve aqua")
                    .watch()
                    .await
                    .expect("approve aqua mined");
                MockERC20::new(token, provider.clone())
                    .approve(app, U256::MAX)
                    .send()
                    .await
                    .expect("approve router")
                    .watch()
                    .await
                    .expect("approve router mined");
            }
        }

        Self {
            _anvil: anvil,
            maker,
            taker,
            taker_signer,
            maker_provider,
            taker_provider,
            aqua,
            router,
            app,
            t0,
            t1,
            fx,
        }
    }

    pub async fn latest_block(&self) -> u64 {
        self.maker_provider
            .get_block_number()
            .await
            .expect("block number")
    }

    async fn mint(&self, token: Address, to: Address, amount: U256, provider: &DynProvider) {
        MockERC20::new(token, provider.clone())
            .mint(to, amount)
            .send()
            .await
            .expect("mint")
            .watch()
            .await
            .expect("mint mined");
    }

    /// Ship a strategy with its fixture balances (maker funds it).
    pub async fn ship(&self, spec: &StrategySpec) {
        self.mint(self.t0, self.maker, spec.ship_lo, &self.maker_provider)
            .await;
        self.mint(self.t1, self.maker, spec.ship_hi, &self.maker_provider)
            .await;
        self.aqua
            .ship(
                self.app,
                spec.strategy_hex.clone(),
                vec![self.t0, self.t1],
                vec![spec.ship_lo, spec.ship_hi],
            )
            .send()
            .await
            .expect("ship")
            .watch()
            .await
            .expect("ship mined");
    }

    pub async fn ship_all(&self) {
        for spec in &self.fx.strategies {
            self.ship(spec).await;
        }
    }

    /// Ship the second-maker XYC from anvil #1 (keyed under that maker on chain).
    pub async fn ship_second_maker(&self) {
        let sm = &self.fx.second_maker;
        self.mint(self.t0, self.taker, sm.ship_lo, &self.taker_provider)
            .await;
        self.mint(self.t1, self.taker, sm.ship_hi, &self.taker_provider)
            .await;
        Aqua::new(*self.aqua.address(), self.taker_provider.clone())
            .ship(
                self.app,
                sm.strategy_hex.clone(),
                vec![self.t0, self.t1],
                vec![sm.ship_lo, sm.ship_hi],
            )
            .send()
            .await
            .expect("ship second maker")
            .watch()
            .await
            .expect("ship second maker mined");
    }

    /// Push `amount` of `token` into a strategy from `pusher` (maker or taker) —
    /// permissionless top-up, funded by the caller.
    pub async fn push(
        &self,
        spec: &StrategySpec,
        token: Address,
        amount: U256,
        from: Address,
        pusher: &DynProvider,
    ) {
        self.mint(token, from, amount, pusher).await;
        Aqua::new(*self.aqua.address(), pusher.clone())
            .push(self.maker, self.app, spec.hash(), token, amount)
            .send()
            .await
            .expect("push")
            .watch()
            .await
            .expect("push mined");
    }

    /// Execute a real taker swap against a strategy (emits Pushed + Pulled).
    pub async fn swap(
        &self,
        spec: &StrategySpec,
        token_in: Address,
        token_out: Address,
        amount: U256,
    ) {
        self.mint(token_in, self.taker, amount, &self.taker_provider)
            .await;
        AquaSwapVMRouter::new(self.app, self.taker_provider.clone())
            .swap(
                spec.order(),
                token_in,
                token_out,
                amount,
                self.fx.taker_traits.swap.clone(),
            )
            .send()
            .await
            .expect("swap")
            .watch()
            .await
            .expect("swap mined");
    }

    /// Attempt to ship a strategy, returning the error text on revert. On-chain
    /// immutability makes re-shipping an existing (live or docked) strategy fail.
    pub async fn try_ship(&self, strategy_hex: &Bytes, lo: U256, hi: U256) -> Result<(), String> {
        self.mint(self.t0, self.maker, lo, &self.maker_provider)
            .await;
        self.mint(self.t1, self.maker, hi, &self.maker_provider)
            .await;
        match self
            .aqua
            .ship(
                self.app,
                strategy_hex.clone(),
                vec![self.t0, self.t1],
                vec![lo, hi],
            )
            .send()
            .await
        {
            Ok(pending) => pending.watch().await.map(|_| ()).map_err(|e| e.to_string()),
            Err(e) => Err(e.to_string()),
        }
    }

    pub async fn dock(&self, spec: &StrategySpec) {
        self.aqua
            .dock(self.app, spec.hash(), vec![self.t0, self.t1])
            .send()
            .await
            .expect("dock")
            .watch()
            .await
            .expect("dock mined");
    }

    /// Set the maker's Aqua allowance for `token` to `amount`, overwriting the setup's `MAX` — used
    /// to make the allowance (not the balance) the binding side of the wallet budget.
    pub async fn approve_aqua(&self, token: Address, amount: U256) {
        MockERC20::new(token, self.maker_provider.clone())
            .approve(*self.aqua.address(), amount)
            .send()
            .await
            .expect("approve")
            .watch()
            .await
            .expect("approve mined");
    }

    /// On-chain virtual balances `(t0, t1)` for a strategy.
    pub async fn on_chain_balances(&self, spec: &StrategySpec) -> (U256, U256) {
        let r = self
            .aqua
            .safeBalances(self.maker, self.app, spec.hash(), self.t0, self.t1)
            .call()
            .await
            .expect("safeBalances");
        (r.balance0, r.balance1)
    }

    pub fn chain_source(&self) -> Arc<dyn ChainSource> {
        Arc::new(AlloyChainSource::new(
            Arc::new(self.maker_provider.clone()),
            *self.aqua.address(),
            self.app,
            None,
        ))
    }

    /// Assert the snapshot's balances for a strategy match on-chain exactly.
    pub async fn assert_balances(&self, snap: &Snapshot, spec: &StrategySpec) {
        let (b0, b1) = self.on_chain_balances(spec).await;
        let strat = snap
            .strategy(&spec.key(self.maker, self.app))
            .unwrap_or_else(|| panic!("{} present", spec.label));
        assert_eq!(
            strat.balance(&self.t0),
            b0,
            "{} t0 balance vs chain",
            spec.label
        );
        assert_eq!(
            strat.balance(&self.t1),
            b1,
            "{} t1 balance vs chain",
            spec.label
        );
    }

    /// Assert our `price()` equals the router's on-chain `quote()`, both
    /// directions × exact-in/out, at amounts scaled to the strategy's reserves.
    pub async fn assert_price_parity(&self, strat: &MakerStrategy, spec: &StrategySpec) {
        let order = spec.order();
        for (t_in, t_out) in [(self.t1, self.t0), (self.t0, self.t1)] {
            let bal_in = if t_in == self.t0 {
                spec.ship_lo
            } else {
                spec.ship_hi
            };
            let bal_out = if t_out == self.t0 {
                spec.ship_lo
            } else {
                spec.ship_hi
            };
            for div in [1000u64, 100, 10] {
                self.assert_quote_matches(
                    strat,
                    &order,
                    t_in,
                    t_out,
                    bal_in / U256::from(div),
                    true,
                    &spec.label,
                )
                .await;
            }
            for div in [1000u64, 100] {
                self.assert_quote_matches(
                    strat,
                    &order,
                    t_in,
                    t_out,
                    bal_out / U256::from(div),
                    false,
                    &spec.label,
                )
                .await;
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn assert_quote_matches(
        &self,
        strat: &MakerStrategy,
        order: &ISwapVM::Order,
        token_in: Address,
        token_out: Address,
        amount: U256,
        exact_in: bool,
        label: &str,
    ) {
        if amount.is_zero() {
            return;
        }
        let taker = if exact_in {
            &self.fx.taker_traits.exact_in
        } else {
            &self.fx.taker_traits.exact_out
        };
        let quoted = self
            .router
            .quote(order.clone(), token_in, token_out, amount, taker.clone())
            .call()
            .await
            .unwrap_or_else(|e| panic!("{label} quote ({}): {e}", direction(exact_in)));
        let (on_chain, ours) = if exact_in {
            (
                quoted.amountOut,
                price(strat, token_in, token_out, amount, true),
            )
        } else {
            (
                quoted.amountIn,
                price(strat, token_in, token_out, amount, false),
            )
        };
        assert_eq!(
            Ok(on_chain),
            ours,
            "{label} parity ({}, {token_in} -> {token_out}, amount {amount})",
            direction(exact_in)
        );
    }
}

pub fn direction(exact_in: bool) -> &'static str {
    if exact_in {
        "exact-in"
    } else {
        "exact-out"
    }
}

/// True (printing a skip notice) when `anvil` cannot be spawned — the live E2E
/// tests need a chain, so the default `cargo test` skips them when foundry is not
/// on `PATH`. The SQLite store is always available, so nothing else gates them.
pub fn skip_without_anvil() -> bool {
    match Anvil::new().try_spawn() {
        Ok(_) => false,
        Err(_) => {
            eprintln!("anvil not spawnable (foundry on PATH?) — skipping live E2E");
            true
        }
    }
}

/// Wire the real pipeline (`AlloyChainSource` → `RegistrySync` → `SqliteStore`)
/// over a fresh temp-file SQLite database. The returned `TempDir` owns that file
/// and must outlive the pool, so the caller keeps it for the test's duration.
pub async fn pipeline(
    h: &Harness,
    chain: ChainId,
) -> (RegistrySync, Arc<SharedSnapshot>, SqlitePool, TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let url = format!(
        "sqlite://{}?mode=rwc",
        dir.path().join("registry.db").display()
    );
    let pool = SqlitePool::connect(&url).await.expect("open sqlite");
    let store = SqliteStore::new(pool.clone());
    store.migrate().await.expect("migrate");
    let config = ChainConfig::new(chain, 0, 25, 15);
    let snapshot = Arc::new(SharedSnapshot::default());
    let sync = RegistrySync::new(&config, h.chain_source(), Arc::new(store), snapshot.clone());
    (sync, snapshot, pool, dir)
}
