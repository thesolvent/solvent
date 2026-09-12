/**
 * Generate the server's runtime config from the devnet deploy manifest.
 *
 * The deploy script `new`-deploys the Core-6 tokens, so their addresses are whatever the chain
 * assigned — the checked-in `tokens.devnet.json` (mainnet addresses) does not describe a local
 * devnet. Both artifacts below are generated per-deploy and stay untracked.
 */
import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, relative, resolve } from "node:path";

import { readManifest, REPO_ROOT, type Manifest } from "../lib/manifest.ts";

const TOKENS_OUT = resolve(REPO_ROOT, "devnet/generated/tokens.json");
const CONFIG_OUT = resolve(REPO_ROOT, "solvent.toml");
const ENV_OUT = resolve(REPO_ROOT, "devnet/generated/env.sh");

// Anvil's deterministic dev accounts. Each transaction-producing role has its own nonce domain.
// Publicly known throwaway keys — devnet only, never a real network.
const DEPLOYER_KEY = "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
const COSIGNER_KEY = "0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d";
const FILLER_OWNER_KEY = "0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a";
const POLICY_SIGNER_KEY = "0x7c852118294e51e653712a81e05800f419141751be58f605c371e15141b007a6";

// Symbol -> token-list tag, mirroring the checked-in list's grouping. Tags are read by people in
// the asset picker, so they are cased as they should appear.
export const CORE_TOKEN_CATALOG = {
  WETH: {
    name: "Wrapped Ether",
    tag: "Majors",
    logoUri: "https://cdn.garden.finance/catalog/chain_images/ethereum.svg",
  },
  WBTC: {
    name: "Wrapped BTC",
    tag: "Majors",
    logoUri: "https://cdn.garden.finance/catalog/token-images/wbtc.svg",
  },
  USDC: {
    name: "USD Coin",
    tag: "Stables",
    logoUri: "https://cdn.garden.finance/catalog/token-images/usdc.svg",
  },
  USDT: {
    name: "Tether USD",
    tag: "Stables",
    logoUri: "https://cdn.garden.finance/catalog/token-images/usdt.svg",
  },
  DAI: {
    name: "Dai Stablecoin",
    tag: "Stables",
    logoUri:
      "https://raw.githubusercontent.com/trustwallet/assets/master/blockchains/ethereum/assets/0x6B175474E89094C44Da98b954EedeAC495271d0F/logo.png",
  },
  LINK: {
    name: "ChainLink Token",
    tag: "DeFi",
    logoUri:
      "https://raw.githubusercontent.com/trustwallet/assets/master/blockchains/ethereum/assets/0x514910771AF9Ca656af840dff83E8264EcF986CA/logo.png",
  },
} as const;

// Binance feed symbol -> the devnet token it prices.
const PRICE_FEEDS: Record<string, string> = {
  ETHUSDT: "WETH",
  BTCUSDT: "WBTC",
  USDCUSDT: "USDC",
  LINKUSDT: "LINK",
};

// Mainnet source identities and their independent price sources for the read-only UniswapX feed.
export type UniswapAsset = {
  sourceAddress: string;
  symbol: string;
  decimals: number;
  marketSymbol?: string;
  usdPeg?: boolean;
};

export const UNISWAP_ASSETS: readonly UniswapAsset[] = [
  { sourceAddress: "0x0000000000000000000000000000000000000000", symbol: "ETH", decimals: 18, marketSymbol: "ETHUSDT" },
  { sourceAddress: "0x000006c2A22ff4A44ff1f5d0F2ed65F781F55555", symbol: "ZKC", decimals: 18, marketSymbol: "ZKCUSDT" },
  { sourceAddress: "0x00c83aeCC790e8a4453e5dD3B0B4b3680501a7A7", symbol: "SKL", decimals: 18, marketSymbol: "SKLUSDT" },
  { sourceAddress: "0x01791F726B4103694969820be083196cC7c045fF", symbol: "YB", decimals: 18, marketSymbol: "YBUSDT" },
  { sourceAddress: "0x04Fa0d235C4abf4BcF4787aF4CF447DE572eF828", symbol: "UMA", decimals: 18, marketSymbol: "UMAUSDT" },
  { sourceAddress: "0x090185f2135308BaD17527004364eBcC2D37e5F6", symbol: "SPELL", decimals: 18, marketSymbol: "SPELLUSDT" },
  { sourceAddress: "0x0b38210ea11411557c13457D4dA7dC6ea731B88a", symbol: "API3", decimals: 18, marketSymbol: "API3USDT" },
  { sourceAddress: "0x0bc529c00C6401aEF6D220BE8C6Ea1667F6Ad93e", symbol: "YFI", decimals: 18, marketSymbol: "YFIUSDT" },
  { sourceAddress: "0x0D8775F648430679A709E98d2b0Cb6250d2887EF", symbol: "BAT", decimals: 18, marketSymbol: "BATUSDT" },
  { sourceAddress: "0x0f2D719407FdBeFF09D87557AbB7232601FD9F29", symbol: "SYN", decimals: 18, marketSymbol: "SYNUSDT" },
  { sourceAddress: "0x0F5D2fB29fb7d3CFeE444a200298f468908cC942", symbol: "MANA", decimals: 18, marketSymbol: "MANAUSDT" },
  { sourceAddress: "0x0F81001eF0A83ecCE5ccebf63EB302c70a39a654", symbol: "DOLO", decimals: 18, marketSymbol: "DOLOUSDT" },
  { sourceAddress: "0x0fc2a55d5BD13033f1ee0cdd11f60F7eFe66f467", symbol: "LA", decimals: 18, marketSymbol: "LAUSDT" },
  { sourceAddress: "0x111111111117dC0aa78b770fA6A738034120C302", symbol: "1INCH", decimals: 18, marketSymbol: "1INCHUSDT" },
  { sourceAddress: "0x152649eA73beAb28c5b49B26eb48f7EAD6d4c898", symbol: "CAKE", decimals: 18, marketSymbol: "CAKEUSDT" },
  { sourceAddress: "0x1776e1F26f98b1A5dF9cD347953a26dd3Cb46671", symbol: "NMR", decimals: 18, marketSymbol: "NMRUSDT" },
  { sourceAddress: "0x1789e0043623282D5DCc7F213d703C6D8BAfBB04", symbol: "LINEA", decimals: 18, marketSymbol: "LINEAUSDT" },
  { sourceAddress: "0x1F573D6Fb3F13d689FF844B4cE37794d79a7FF1C", symbol: "BNT", decimals: 18, marketSymbol: "BNTUSDT" },
  { sourceAddress: "0x1f9840a85d5aF5bf1D1762F925BDADdC4201F984", symbol: "UNI", decimals: 18, marketSymbol: "UNIUSDT" },
  { sourceAddress: "0x2260FAC5E5542a773Aa44fBCfeDf7C193bc2C599", symbol: "WBTC", decimals: 8, marketSymbol: "WBTCUSDT" },
  { sourceAddress: "0x25f8087EAD173b73D6e8B84329989A8eEA16CF73", symbol: "YGG", decimals: 18, marketSymbol: "YGGUSDT" },
  { sourceAddress: "0x3073f7aAA4DB83f95e9FFf17424F71D4751a3073", symbol: "MOVE", decimals: 8, marketSymbol: "MOVEUSDT" },
  { sourceAddress: "0x320623b8E4fF03373931769A31Fc52A4E78B5d70", symbol: "RSR", decimals: 18, marketSymbol: "RSRUSDT" },
  { sourceAddress: "0x3506424F91fD33084466F402d5D97f05F8e3b4AF", symbol: "CHZ", decimals: 18, marketSymbol: "CHZUSDT" },
  { sourceAddress: "0x3845badAde8e6dFF049820680d1F14bD3903a5d0", symbol: "SAND", decimals: 18, marketSymbol: "SANDUSDT" },
  { sourceAddress: "0x3B50805453023a91a8bf641e279401a0b23FA6F9", symbol: "REZ", decimals: 18, marketSymbol: "REZUSDT" },
  { sourceAddress: "0x44ff8620b8cA30902395A7bD3F2407e1A091BF73", symbol: "VIRTUAL", decimals: 18, marketSymbol: "VIRTUALUSDT" },
  { sourceAddress: "0x455e53CBB86018Ac2B8092FdCd39d8444aFFC3F6", symbol: "POL", decimals: 18, marketSymbol: "POLUSDT" },
  { sourceAddress: "0x45804880De22913dAFE09f4980848ECE6EcbAf78", symbol: "PAXG", decimals: 18, marketSymbol: "PAXGUSDT" },
  { sourceAddress: "0x467719aD09025FcC6cF6F8311755809d45a5E5f3", symbol: "AXL", decimals: 6, marketSymbol: "AXLUSDT" },
  { sourceAddress: "0x4691937a7508860F876c9c0a2a617E7d9E945D4B", symbol: "WOO", decimals: 18, marketSymbol: "WOOUSDT" },
  { sourceAddress: "0x4a220E6096B25EADb88358cb44068A3248254675", symbol: "QNT", decimals: 18, marketSymbol: "QNTUSDT" },
  { sourceAddress: "0x4C1746A800D224393fE2470C70A35717eD4eA5F1", symbol: "PLUME", decimals: 18, marketSymbol: "PLUMEUSDT" },
  { sourceAddress: "0x4d224452801ACEd8B2F0aebE155379bb5D594381", symbol: "APE", decimals: 18, marketSymbol: "APEUSDT" },
  { sourceAddress: "0x4d7078DDd6cCFED2F85dB5B7D3Ff16828d378d48", symbol: "AI", decimals: 18, marketSymbol: "AIUSDT" },
  { sourceAddress: "0x4e3FBD56CD56c3e72c1403e103b45Db9da5B9D2B", symbol: "CVX", decimals: 18, marketSymbol: "CVXUSDT" },
  { sourceAddress: "0x4F9254C83EB525f9FCf346490bbb3ed28a81C667", symbol: "CELR", decimals: 18, marketSymbol: "CELRUSDT" },
  { sourceAddress: "0x514910771AF9Ca656af840dff83E8264EcF986CA", symbol: "LINK", decimals: 18, marketSymbol: "LINKUSDT" },
  { sourceAddress: "0x5283D291DBCF85356A21bA090E6db59121208b44", symbol: "BLUR", decimals: 18, marketSymbol: "BLURUSDT" },
  { sourceAddress: "0x56072C95FAA701256059aa122697B133aDEd9279", symbol: "SKY", decimals: 18, marketSymbol: "SKYUSDT" },
  { sourceAddress: "0x57e114B691Db790C35207b2e685D4A43181e6061", symbol: "ENA", decimals: 18, marketSymbol: "ENAUSDT" },
  { sourceAddress: "0x58b6A8A3302369DAEc383334672404Ee733aB239", symbol: "LPT", decimals: 18, marketSymbol: "LPTUSDT" },
  { sourceAddress: "0x58D97B57BB95320F9a05dC918Aef65434969c2B2", symbol: "MORPHO", decimals: 18, marketSymbol: "MORPHOUSDT" },
  { sourceAddress: "0x595832F8FC6BF59c85C527fEC3740A1b7a361269", symbol: "POWR", decimals: 6, marketSymbol: "POWRUSDT" },
  { sourceAddress: "0x5A98FcBEA516Cf06857215779Fd812CA3beF1B32", symbol: "LDO", decimals: 18, marketSymbol: "LDOUSDT" },
  { sourceAddress: "0x6033F7f88332B8db6ad452B7C6D5bB643990aE3f", symbol: "LSK", decimals: 18, marketSymbol: "LSKUSDT" },
  { sourceAddress: "0x607F4C5BB672230e8672085532f7e901544a7375", symbol: "RLC", decimals: 9, marketSymbol: "RLCUSDT" },
  { sourceAddress: "0x643C4E15d7d62Ad0aBeC4a9BD4b001aA3Ef52d66", symbol: "SYRUP", decimals: 18, marketSymbol: "SYRUPUSDT" },
  { sourceAddress: "0x64Bc2cA1Be492bE7185FAA2c8835d9b824c8a194", symbol: "BIGTIME", decimals: 18, marketSymbol: "BIGTIMEUSDT" },
  { sourceAddress: "0x6810e776880C02933D47DB1b9fc05908e5386b96", symbol: "GNO", decimals: 18, marketSymbol: "GNOUSDT" },
  { sourceAddress: "0x68749665FF8D2d112Fa859AA293F07A622782F38", symbol: "XAUT", decimals: 6, marketSymbol: "XAUTUSDT" },
  { sourceAddress: "0x6982508145454Ce325dDbE47a25d4ec3d2311933", symbol: "PEPE", decimals: 18, marketSymbol: "PEPEUSDT" },
  { sourceAddress: "0x6985884C4392D348587B19cb9eAAf157F13271cd", symbol: "ZRO", decimals: 18, marketSymbol: "ZROUSDT" },
  { sourceAddress: "0x6B175474E89094C44Da98b954EedeAC495271d0F", symbol: "DAI", decimals: 18, usdPeg: true },
  { sourceAddress: "0x6B3595068778DD592e39A122f4f5a5cF09C90fE2", symbol: "SUSHI", decimals: 18, marketSymbol: "SUSHIUSDT" },
  { sourceAddress: "0x6DEA81C8171D0bA574754EF6F8b412F2Ed88c54D", symbol: "LQTY", decimals: 18, marketSymbol: "LQTYUSDT" },
  { sourceAddress: "0x6E2a43be0B1d33b726f0CA3b8de60b3482b8b050", symbol: "ARKM", decimals: 18, marketSymbol: "ARKMUSDT" },
  { sourceAddress: "0x7420B4b9a0110cdC71fB720908340C03F9Bc03EC", symbol: "JASMY", decimals: 18, marketSymbol: "JASMYUSDT" },
  { sourceAddress: "0x767FE9EDC9E0dF98E07454847909b5E959D7ca0E", symbol: "ILV", decimals: 18, marketSymbol: "ILVUSDT" },
  { sourceAddress: "0x77146784315Ba81904d654466968e3a7c196d1f3", symbol: "TREE", decimals: 18, marketSymbol: "TREEUSDT" },
  { sourceAddress: "0x7DD9c5Cba05E151C895FDe1CF355C9A1D5DA6429", symbol: "GLM", decimals: 18, marketSymbol: "GLMUSDT" },
  { sourceAddress: "0x7Fc66500c84A76Ad7e9c93437bFc5Ac33E2DDaE9", symbol: "AAVE", decimals: 18, marketSymbol: "AAVEUSDT" },
  { sourceAddress: "0x808507121B80c02388fAd14726482e061B8da827", symbol: "PENDLE", decimals: 18, marketSymbol: "PENDLEUSDT" },
  { sourceAddress: "0x812Ba41e071C7b7fA4EBcFB62dF5F45f6fA853Ee", symbol: "Neiro", decimals: 9, marketSymbol: "NEIROUSDT" },
  { sourceAddress: "0x8207c1FfC5B6804F6024322CcF34F29c3541Ae26", symbol: "OGN", decimals: 18, marketSymbol: "OGNUSDT" },
  { sourceAddress: "0x8292Bb45bf1Ee4d140127049757C2E0fF06317eD", symbol: "RLUSD", decimals: 18, marketSymbol: "RLUSDUSDT" },
  { sourceAddress: "0x84cA8bc7997272c7CfB4D0Cd3D55cd942B3c9419", symbol: "DIA", decimals: 18, marketSymbol: "DIAUSDT" },
  { sourceAddress: "0x853d955aCEf822Db058eb8505911ED77F175b99e", symbol: "FRAX", decimals: 18, marketSymbol: "FRAXUSDT" },
  { sourceAddress: "0x868FCEd65edBF0056c4163515dD840e9f287A4c3", symbol: "SIGN", decimals: 18, marketSymbol: "SIGNUSDT" },
  { sourceAddress: "0x88dF592F8eb5D7Bd38bFeF7dEb0fBc02cf3778a0", symbol: "TRB", decimals: 18, marketSymbol: "TRBUSDT" },
  { sourceAddress: "0x8d0D000Ee44948FC98c9B98A4FA4921476f08B0d", symbol: "USD1", decimals: 18, marketSymbol: "USD1USDT" },
  { sourceAddress: "0x8E870D67F660D95d5be530380D0eC0bd388289E1", symbol: "USDP", decimals: 18, marketSymbol: "USDPUSDT" },
  { sourceAddress: "0x8f8221aFbB33998d8584A2B05749bA73c37a938a", symbol: "REQ", decimals: 18, marketSymbol: "REQUSDT" },
  { sourceAddress: "0x92D6C1e31e14520e676a687F0a93788B716BEff5", symbol: "DYDX", decimals: 18, marketSymbol: "DYDXUSDT" },
  { sourceAddress: "0x95aD61b0a150d79219dCF64E1E6Cc01f0B64C4cE", symbol: "SHIB", decimals: 18, marketSymbol: "SHIBUSDT" },
  { sourceAddress: "0x9C7BEBa8F6eF6643aBd725e45a4E8387eF260649", symbol: "G", decimals: 18, marketSymbol: "GUSDT" },
  { sourceAddress: "0x9E32b13ce7f2E80A01932B42553652E053D6ed8e", symbol: "METIS", decimals: 18, marketSymbol: "METISUSDT" },
  { sourceAddress: "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48", symbol: "USDC", decimals: 6, marketSymbol: "USDCUSDT" },
  { sourceAddress: "0xA12CC123ba206d4031D1c7f6223D1C2Ec249f4f3", symbol: "ZAMA", decimals: 18, marketSymbol: "ZAMAUSDT" },
  { sourceAddress: "0xA35923162C49cF95e6BF26623385eb431ad920D3", symbol: "TURBO", decimals: 18, marketSymbol: "TURBOUSDT" },
  { sourceAddress: "0xa6C0c097741D55ECd9a3A7DeF3A8253fD022ceB9", symbol: "AVA", decimals: 18, marketSymbol: "AVAUSDT" },
  { sourceAddress: "0xaea46A60368A7bD060eec7DF8CBa43b7EF41Ad85", symbol: "FET", decimals: 18, marketSymbol: "FETUSDT" },
  { sourceAddress: "0xAf5191B0De278C7286d6C7CC6ab6BB8A73bA2Cd6", symbol: "STG", decimals: 18, marketSymbol: "STGUSDT" },
  { sourceAddress: "0xb131f4A55907B10d1F0A50d8ab8FA09EC342cd74", symbol: "MEME", decimals: 18, marketSymbol: "MEMEUSDT" },
  { sourceAddress: "0xB50721BCf8d664c30412Cfbc6cf7a15145234ad1", symbol: "ARB", decimals: 18, marketSymbol: "ARBUSDT" },
  { sourceAddress: "0xB528edBef013aff855ac3c50b381f253aF13b997", symbol: "AEVO", decimals: 18, marketSymbol: "AEVOUSDT" },
  { sourceAddress: "0xBA11D00c5f74255f56a5E366F4F77f5A186d7f55", symbol: "BAND", decimals: 18, marketSymbol: "BANDUSDT" },
  { sourceAddress: "0xba5BDe662c17e2aDFF1075610382B9B691296350", symbol: "RARE", decimals: 18, marketSymbol: "RAREUSDT" },
  { sourceAddress: "0xBB0E17EF65F82Ab018d8EDd776e8DD940327B28b", symbol: "AXS", decimals: 18, marketSymbol: "AXSUSDT" },
  { sourceAddress: "0xc00e94Cb662C3520282E6f5717214004A7f26888", symbol: "COMP", decimals: 18, marketSymbol: "COMPUSDT" },
  { sourceAddress: "0xC011a73ee8576Fb46F5E1c5751cA3B9Fe0af2a6F", symbol: "SNX", decimals: 18, marketSymbol: "SNXUSDT" },
  { sourceAddress: "0xC02aaA39b223FE8D0A0E5C4F27eAD9083C756Cc2", symbol: "WETH", decimals: 18, marketSymbol: "ETHUSDT" },
  { sourceAddress: "0xC18360217D8F7Ab5e7c516566761Ea12Ce7F9D72", symbol: "ENS", decimals: 18, marketSymbol: "ENSUSDT" },
  { sourceAddress: "0xc20059e0317DE91738d13af027DfC4a50781b066", symbol: "SPK", decimals: 18, marketSymbol: "SPKUSDT" },
  { sourceAddress: "0xc43C6bfeDA065fE2c4c11765Bf838789bd0BB5dE", symbol: "RED", decimals: 18, marketSymbol: "REDUSDT" },
  { sourceAddress: "0xC4441c2BE5d8fA8126822B9929CA0b81Ea0DE38E", symbol: "USUAL", decimals: 18, marketSymbol: "USUALUSDT" },
  { sourceAddress: "0xc944E90C64B2c07662A292be6244BDf05Cda44a7", symbol: "GRT", decimals: 18, marketSymbol: "GRTUSDT" },
  { sourceAddress: "0xCa14007Eff0dB1f8135f4C25B34De49AB0d42766", symbol: "STRK", decimals: 18, marketSymbol: "STRKUSDT" },
  { sourceAddress: "0xcb1592591996765Ec0eFc1f92599A19767ee5ffA", symbol: "BIO", decimals: 18, marketSymbol: "BIOUSDT" },
  { sourceAddress: "0xcccCCCcCCC33D538DBC2EE4fEab0a7A1FF4e8A94", symbol: "CFG", decimals: 18, marketSymbol: "CFGUSDT" },
  { sourceAddress: "0xCdF7028ceAB81fA0C6971208e83fa7872994beE5", symbol: "T", decimals: 18, marketSymbol: "TUSDT" },
  { sourceAddress: "0xD0eC028a3D21533Fdd200838F39c85B03679285D", symbol: "NEWT", decimals: 18, marketSymbol: "NEWTUSDT" },
  { sourceAddress: "0xd1d2Eb1B1e90B638588728b4130137D262C87cae", symbol: "GALA", decimals: 8, marketSymbol: "GALAUSDT" },
  { sourceAddress: "0xD31a59c85aE9D8edEFeC411D448f90841571b89c", symbol: "SOL", decimals: 9, marketSymbol: "SOLUSDT" },
  { sourceAddress: "0xD33526068D116cE69F19A9ee46F0bd304F21A51f", symbol: "RPL", decimals: 18, marketSymbol: "RPLUSDT" },
  { sourceAddress: "0xD533a949740bb3306d119CC777fa900bA034cd52", symbol: "CRV", decimals: 18, marketSymbol: "CRVUSDT" },
  { sourceAddress: "0xd9Fcd98c322942075A5C3860693e9f4f03AAE07b", symbol: "EUL", decimals: 18, marketSymbol: "EULUSDT" },
  { sourceAddress: "0xdA5e1988097297dCdc1f90D4dFE7909e847CBeF6", symbol: "WLFI", decimals: 18, marketSymbol: "WLFIUSDT" },
  { sourceAddress: "0xdAC17F958D2ee523a2206206994597C13D831ec7", symbol: "USDT", decimals: 6, usdPeg: true },
  { sourceAddress: "0xdC035D45d973E3EC169d2276DDab16f1e407384F", symbol: "USDS", decimals: 18, marketSymbol: "USDSUSDT" },
  { sourceAddress: "0xDDB3422497E61e13543BeA06989C0789117555c5", symbol: "COTI", decimals: 18, marketSymbol: "COTIUSDT" },
  { sourceAddress: "0xDEf1CA1fb7FBcDC777520aa7f396b4E015F497aB", symbol: "COW", decimals: 18, marketSymbol: "COWUSDT" },
  { sourceAddress: "0xe28b3B32B6c345A34Ff64674606124Dd5Aceca30", symbol: "INJ", decimals: 18, marketSymbol: "INJUSDT" },
  { sourceAddress: "0xE2AD0BF751834f2fbdC62A41014f84d67cA1de2A", symbol: "ERA", decimals: 18, marketSymbol: "ERAUSDT" },
  { sourceAddress: "0xE41d2489571d322189246DaFA5ebDe1F4699F498", symbol: "ZRX", decimals: 18, marketSymbol: "ZRXUSDT" },
  { sourceAddress: "0xe53EC727dbDEB9E2d5456c3be40cFF031AB40A55", symbol: "SUPER", decimals: 18, marketSymbol: "SUPERUSDT" },
  { sourceAddress: "0xE6Bfd33F52d82Ccb5b37E16D3dD81f9FFDAbB195", symbol: "SXT", decimals: 18, marketSymbol: "SXTUSDT" },
  { sourceAddress: "0xec53bF9167f50cDEB3Ae105f56099aaaB9061F83", symbol: "EIGEN", decimals: 18, marketSymbol: "EIGENUSDT" },
  { sourceAddress: "0xEd04915c23f00A313a544955524EB7DBD823143d", symbol: "ACH", decimals: 8, marketSymbol: "ACHUSDT" },
  { sourceAddress: "0xeF4461891DfB3AC8572cCf7C794664A8DD927945", symbol: "WCT", decimals: 18, marketSymbol: "WCTUSDT" },
  { sourceAddress: "0xf0DB65D17e30a966C2ae6A21f6BBA71cea6e9754", symbol: "BARD", decimals: 18, marketSymbol: "BARDUSDT" },
  { sourceAddress: "0xF17e65822b568B3903685a7c9F496CF7656Cc6C2", symbol: "BICO", decimals: 18, marketSymbol: "BICOUSDT" },
  { sourceAddress: "0xF57e7e7C23978C3cAEC3C3548E3D615c346e79fF", symbol: "IMX", decimals: 18, marketSymbol: "IMXUSDT" },
  { sourceAddress: "0xF629cBd94d3791C9250152BD8dfBDF380E2a3B9c", symbol: "ENJ", decimals: 18, marketSymbol: "ENJUSDT" },
  { sourceAddress: "0xfAbA6f8e4a5E8Ab82F62fe7C39859FA577269BE3", symbol: "ONDO", decimals: 18, marketSymbol: "ONDOUSDT" },
  { sourceAddress: "0xFe0c30065B384F05761f15d0CC899D4F9F9Cc0eB", symbol: "ETHFI", decimals: 18, marketSymbol: "ETHFIUSDT" },
  { sourceAddress: "0xfF20817765cB7f73d4bde2e66e067E58D11095C2", symbol: "AMP", decimals: 18, marketSymbol: "AMPUSDT" },
];

/** Held at $1 instead of tracking a feed: USDT is the quote unit itself, and Binance's DAIUSDT
 *  is a dead pair that reports a zero price. */
const PEGGED = ["USDT", "DAI"];

function tokenList(manifest: Manifest) {
  return {
    name: "Solvent Devnet",
    version: { major: 1, minor: 0, patch: 0 },
    tokens: Object.entries(manifest.tokens).map(([symbol, token]) => ({
      chainId: manifest.chain_id,
      address: token.address,
      symbol,
      name: CORE_TOKEN_CATALOG[symbol as keyof typeof CORE_TOKEN_CATALOG]?.name ?? symbol,
      decimals: token.decimals,
      logoURI: CORE_TOKEN_CATALOG[symbol as keyof typeof CORE_TOKEN_CATALOG]?.logoUri,
      tags: [CORE_TOKEN_CATALOG[symbol as keyof typeof CORE_TOKEN_CATALOG]?.tag ?? "other"],
    })),
  };
}

function address(manifest: Manifest, symbol: string): string {
  const token = manifest.tokens[symbol];
  if (!token) throw new Error(`manifest has no ${symbol} token`);
  return token.address;
}

function present(manifest: Manifest, symbols: string[]): string[] {
  return symbols.filter((symbol) => manifest.tokens[symbol]);
}

function configToml(manifest: Manifest, tokenListPath: string): string {
  const pegs = present(manifest, PEGGED)
    .map((symbol) => `"${address(manifest, symbol)}"`)
    .join(", ");

  // Every scalar key must precede the first [[price_symbols]] header: in TOML a key written after
  // an array-of-tables header belongs to that table, which silently drops it from the root.
  const feeds = Object.entries(PRICE_FEEDS)
    .filter(([, symbol]) => manifest.tokens[symbol])
    .map(
      ([feed, symbol]) =>
        `[[price_symbols]]\nsymbol = "${feed}"\ntokens = ["${address(manifest, symbol)}"]  # ${symbol}\n`,
    )
    .join("\n");
  const uniswapAssets = UNISWAP_ASSETS.map((asset) => {
    const market = asset.marketSymbol ? `market_symbol = "${asset.marketSymbol}"\n` : "";
    const peg = asset.usdPeg ? "usd_peg = true\n" : "";
    return `[[uniswap_assets]]
source_address = "${asset.sourceAddress}"
symbol = "${asset.symbol}"
decimals = ${asset.decimals}
logo_uri = "${assetLogoUri(asset)}"
${market}${peg}`;
  }).join("\n");

  return `# Generated by @solvent/scripts bootstrap — regenerate after every devnet deploy.
# Addresses come from contracts/deployments/solvent-devnet.json.

bind_addr = "0.0.0.0:8080"
rpc_url = "http://127.0.0.1:8545"
chain_id = ${manifest.chain_id}

# Required to run the live server, despite being optional in the config type.
database_url = "sqlite:devnet/generated/solvent.db?mode=rwc"

aqua_address = "${manifest.aqua}"
app_address = "${manifest.router}"

default_fee_bps = 5
block_explorer_url = "http://localhost:5100"
networks = ["Ethereum"]
network_logo_uri = "https://cdn.garden.finance/catalog/chain_images/ethereum.svg"
faucet = true

token_list = "${tokenListPath}"

native_token = "${address(manifest, "WETH")}"
gas_units_per_leg = 150000
binance_ws_url = "wss://stream.binance.com:9443"
usd_stable_pegs = [${pegs}]

filler = "${manifest.filler}"
erc7683_settler = "${manifest.erc7683_settler}"
erc7683_filler = "${manifest.erc7683_filler}"
erc7683_resolver = "${manifest.erc7683_resolver}"
erc7683_executor_fee_bps = 5
reactor = "${manifest.reactor}"
permit2 = "${manifest.permit2}"
confirmations = 1
reservation_ttl_secs = 120
decay_window_secs = 60

wallet_state_db = "devnet/generated/filler-walletkit.redb"

[rebate]
authorization_ttl_blocks = 90

${feeds}
${uniswapAssets}`;
}

function assetLogoUri(asset: UniswapAsset): string {
  return asset.symbol === "ETH"
    ? "https://cdn.garden.finance/catalog/chain_images/ethereum.svg"
    : `https://raw.githubusercontent.com/trustwallet/assets/master/blockchains/ethereum/assets/${asset.sourceAddress}/logo.png`;
}

function main(): void {
  const manifest = readManifest();
  const tokenListPath = relative(REPO_ROOT, TOKENS_OUT);

  mkdirSync(dirname(TOKENS_OUT), { recursive: true });
  writeFileSync(TOKENS_OUT, `${JSON.stringify(tokenList(manifest), null, 2)}\n`);
  writeFileSync(CONFIG_OUT, configToml(manifest, tokenListPath));
  writeFileSync(
    ENV_OUT,
      `# Signing keys the server reads from the environment. Anvil dev accounts — devnet only.\n` +
      `export SOLVENT_SIGNER_KEY=${FILLER_OWNER_KEY}\n` +
      `export SOLVENT_POLICY_SIGNER_KEY=${POLICY_SIGNER_KEY}\n` +
      `export SOLVENT_COSIGNER_KEY=${COSIGNER_KEY}\n`,
  );

  console.log(`bootstrap: aqua=${manifest.aqua} app=${manifest.router}`);
  console.log(`bootstrap: wrote ${tokenListPath} (${Object.keys(manifest.tokens).length} tokens)`);
  console.log(`bootstrap: wrote ${relative(REPO_ROOT, CONFIG_OUT)}`);
  console.log(`bootstrap: wrote ${relative(REPO_ROOT, ENV_OUT)}`);
}

if (import.meta.main) {
  main();
}
